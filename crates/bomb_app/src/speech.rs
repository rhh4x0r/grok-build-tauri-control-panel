//! Read-aloud on the Mac. Apple's speech is Swift-only, so `bomb-speak` (next to this binary, built
//! by build.rs from the same engine the iPhone app uses) renders and plays; this module drives it
//! as a `SpeechEngine`, the only thing the read-aloud model and views talk to.

use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::OnceLock;

use serde::Deserialize;
use serde_json::{json, Value};

/// Where playback stands, as the engine reports it.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
pub struct SpeechStatus {
    pub key: String,
    pub playing: bool,
    /// Seconds into the audio.
    pub position: f64,
    /// Seconds rendered so far; the total once `complete`.
    pub duration: f64,
    pub complete: bool,
    /// The sentence being spoken.
    pub sentence: usize,
    /// Sentences rendered so far, and in all.
    #[serde(default)]
    pub rendered: usize,
    #[serde(default)]
    pub total: usize,
    pub failed: Option<String>,
}

impl SpeechStatus {
    /// The whole length: exact once rendered, estimated from the sentences so far before.
    pub fn estimated_total(&self) -> f64 {
        if self.complete || self.rendered == 0 { self.duration } else { self.duration * self.total as f64 / self.rendered as f64 }
    }
}

/// An installed voice.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Voice {
    pub id: String,
    pub name: String,
    pub language: String,
    /// "premium", "enhanced" or "default".
    pub quality: String,
    pub personal: bool,
}

pub enum SpeechEvent {
    Status(SpeechStatus),
    /// The installed voices, whether a better one is worth downloading, and the best one's id.
    Voices { voices: Vec<Voice>, needs_better: bool, best: String },
    PersonalVoice(bool),
}

/// Render, play, pause, seek, speed, and voices: everything read-aloud needs from the platform.
pub trait SpeechEngine {
    /// Read `sentences` from `from` seconds; `key` names the cached rendering.
    fn play(&mut self, key: &str, sentences: &[String], voice: Option<&str>, rate: f32, from: f64);
    fn pause(&mut self);
    fn resume(&mut self);
    fn seek(&mut self, seconds: f64);
    fn skip(&mut self, seconds: f64);
    fn set_rate(&mut self, rate: f32);
    fn stop(&mut self);
    fn request_voices(&mut self);
    fn preview(&mut self, voice: &str);
    fn request_personal_voice(&mut self);
}

fn helper() -> Option<PathBuf> {
    let path = std::env::current_exe().ok()?.parent()?.join("bomb-speak");
    path.is_file().then_some(path)
}

/// Whether this Mac can read aloud (the helper is here and voices are installed). The first call
/// runs the helper, so make it off the main thread.
pub fn available() -> bool {
    static AVAILABLE: OnceLock<bool> = OnceLock::new();
    *AVAILABLE.get_or_init(|| {
        helper()
            .and_then(|h| Command::new(h).arg("--check").stderr(Stdio::null()).output().ok())
            .is_some_and(|o| String::from_utf8_lossy(&o.stdout).trim() == "ok")
    })
}

/// `bomb-speak`, started once and kept running; its reports arrive on the returned channel.
pub struct HelperEngine {
    child: Child,
    stdin: ChildStdin,
}

impl HelperEngine {
    pub fn start() -> Result<(Self, async_channel::Receiver<SpeechEvent>), String> {
        let path = helper().ok_or("Read-aloud isn't part of this build.")?;
        let mut child = Command::new(path)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| format!("Couldn't start read-aloud: {e}"))?;
        let stdin = child.stdin.take().ok_or("Couldn't start read-aloud.")?;
        let stdout = child.stdout.take().ok_or("Couldn't start read-aloud.")?;
        let (tx, rx) = async_channel::unbounded();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                let Ok(v) = serde_json::from_str::<Value>(&line) else { continue };
                let event = if let Some(status) = v.get("status") {
                    serde_json::from_value(status.clone()).ok().map(SpeechEvent::Status)
                } else if let Some(voices) = v.get("voices") {
                    Some(SpeechEvent::Voices {
                        voices: serde_json::from_value(voices.clone()).unwrap_or_default(),
                        needs_better: v["needsBetterVoice"].as_bool().unwrap_or(false),
                        best: v["best"].as_str().unwrap_or_default().to_string(),
                    })
                } else {
                    v.get("personalVoice").and_then(Value::as_bool).map(SpeechEvent::PersonalVoice)
                };
                if let Some(event) = event {
                    if tx.send_blocking(event).is_err() {
                        return;
                    }
                }
            }
        });
        Ok((Self { child, stdin }, rx))
    }

    fn send(&mut self, command: Value) {
        let _ = writeln!(self.stdin, "{command}");
        let _ = self.stdin.flush();
    }
}

impl Drop for HelperEngine {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl SpeechEngine for HelperEngine {
    fn play(&mut self, key: &str, sentences: &[String], voice: Option<&str>, rate: f32, from: f64) {
        self.send(json!({ "cmd": "play", "key": key, "sentences": sentences, "voice": voice, "rate": rate, "from": from }));
    }
    fn pause(&mut self) { self.send(json!({ "cmd": "pause" })); }
    fn resume(&mut self) { self.send(json!({ "cmd": "resume" })); }
    fn seek(&mut self, seconds: f64) { self.send(json!({ "cmd": "seek", "to": seconds })); }
    fn skip(&mut self, seconds: f64) { self.send(json!({ "cmd": "skip", "by": seconds })); }
    fn set_rate(&mut self, rate: f32) { self.send(json!({ "cmd": "rate", "rate": rate })); }
    fn stop(&mut self) { self.send(json!({ "cmd": "stop" })); }
    fn request_voices(&mut self) { self.send(json!({ "cmd": "voices" })); }
    fn preview(&mut self, voice: &str) { self.send(json!({ "cmd": "preview", "voice": voice })); }
    fn request_personal_voice(&mut self) { self.send(json!({ "cmd": "personal_voice" })); }
}

/// A cache name for a message read in a voice: stable across launches (FNV-1a of the text).
pub fn cache_key(message: &str, voice: &str, sentences: &[String]) -> String {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in voice.bytes().chain([0]).chain(sentences.join("\n").bytes()) {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    let safe: String = message.chars().filter(|c| c.is_ascii_alphanumeric() || *c == '-').take(48).collect();
    format!("{safe}-{hash:x}")
}

/// "1:05" from seconds.
pub fn clock(seconds: f64) -> String {
    let s = seconds.max(0.0) as u64;
    format!("{}:{:02}", s / 60, s % 60)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cache_keys_change_with_voice_or_text_and_clocks_read_naturally() {
        let a = cache_key("thread-12", "Ava", &["Hi.".into()]);
        assert_eq!(a, cache_key("thread-12", "Ava", &["Hi.".into()]));
        assert_ne!(a, cache_key("thread-12", "Tom", &["Hi.".into()]));
        assert_ne!(a, cache_key("thread-12", "Ava", &["Hello.".into()]));
        assert_eq!(clock(65.4), "1:05");
        assert_eq!(clock(0.0), "0:00");
    }
}
