//! Read-aloud on the Mac, with Fish Audio voices. `bomb-speak` (next to this binary, built by
//! build.rs from the same Swift engine the iPhone app uses) fetches the speech and plays it; this
//! module drives it as a `SpeechEngine`, the only thing the read-aloud model and views talk to.

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

/// A Fish Audio voice.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Voice {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub author: String,
    #[serde(default)]
    pub languages: Vec<String>,
    /// A recording of the voice, for a preview.
    #[serde(default)]
    pub sample: Option<String>,
    #[serde(default)]
    pub likes: u64,
}

/// Which voices the picker shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum VoiceList {
    /// Fish Audio's own, in the person's language.
    #[default]
    Recommended,
    /// Everyone's, most used first.
    Popular,
    /// The person's own (cloned) voices.
    Mine,
}

impl VoiceList {
    pub fn key(self) -> &'static str {
        match self { Self::Recommended => "recommended", Self::Popular => "popular", Self::Mine => "mine" }
    }
}

/// "Sarah" (Fish Official): read with until another voice is chosen.
pub const DEFAULT_VOICE: &str = "933563129e564b19a115bedd57b7406a";
pub const DEFAULT_VOICE_NAME: &str = "Sarah";

pub enum SpeechEvent {
    Status(SpeechStatus),
    /// A voice list, and the search it answers.
    Voices { voices: Vec<Voice>, list: String, query: String },
    VoicesFailed(String),
    /// The voice being previewed; None once the preview ends.
    Previewing(Option<String>),
    /// Whether Fish Audio accepted the key sent with `check_key`, and why not.
    KeyChecked(Result<(), String>),
}

/// Render, play, pause, seek, speed, and voices: everything read-aloud needs from the platform.
pub trait SpeechEngine {
    /// Read `sentences` from `from` seconds; `key` names the cached rendering.
    fn play(&mut self, key: &str, sentences: &[String], voice: &str, api_key: &str, rate: f32, from: f64);
    fn pause(&mut self);
    fn resume(&mut self);
    fn seek(&mut self, seconds: f64);
    fn skip(&mut self, seconds: f64);
    fn set_rate(&mut self, rate: f32);
    fn stop(&mut self);
    fn request_voices(&mut self, api_key: &str, list: VoiceList, query: &str);
    fn preview(&mut self, voice: &Voice, api_key: &str);
    fn stop_preview(&mut self);
    fn check_key(&mut self, api_key: &str);
}

fn helper() -> Option<PathBuf> {
    let path = std::env::current_exe().ok()?.parent()?.join("bomb-speak");
    path.is_file().then_some(path)
}

/// Whether this Mac can read aloud (the helper is part of this build and runs). The first call
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
                        list: v["list"].as_str().unwrap_or_default().to_string(),
                        query: v["query"].as_str().unwrap_or_default().to_string(),
                    })
                } else if let Some(check) = v.get("keyCheck") {
                    Some(SpeechEvent::KeyChecked(if check["ok"].as_bool() == Some(true) {
                        Ok(())
                    } else {
                        Err(check["error"].as_str().unwrap_or("Fish Audio didn't accept this API key.").to_string())
                    }))
                } else if let Some(previewing) = v.get("previewing") {
                    Some(SpeechEvent::Previewing(previewing.as_str().map(String::from)))
                } else {
                    v.get("voicesError").and_then(Value::as_str).map(|e| SpeechEvent::VoicesFailed(e.to_string()))
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
    fn play(&mut self, key: &str, sentences: &[String], voice: &str, api_key: &str, rate: f32, from: f64) {
        self.send(json!({ "cmd": "play", "key": key, "sentences": sentences, "voice": voice, "apiKey": api_key, "rate": rate, "from": from }));
    }
    fn pause(&mut self) { self.send(json!({ "cmd": "pause" })); }
    fn resume(&mut self) { self.send(json!({ "cmd": "resume" })); }
    fn seek(&mut self, seconds: f64) { self.send(json!({ "cmd": "seek", "to": seconds })); }
    fn skip(&mut self, seconds: f64) { self.send(json!({ "cmd": "skip", "by": seconds })); }
    fn set_rate(&mut self, rate: f32) { self.send(json!({ "cmd": "rate", "rate": rate })); }
    fn stop(&mut self) { self.send(json!({ "cmd": "stop" })); }
    fn request_voices(&mut self, api_key: &str, list: VoiceList, query: &str) {
        self.send(json!({ "cmd": "voices", "apiKey": api_key, "list": list.key(), "query": query }));
    }
    fn preview(&mut self, voice: &Voice, api_key: &str) {
        self.send(json!({ "cmd": "preview", "voice": voice.id, "sample": voice.sample, "apiKey": api_key }));
    }
    fn stop_preview(&mut self) { self.send(json!({ "cmd": "stop_preview" })); }
    fn check_key(&mut self, api_key: &str) { self.send(json!({ "cmd": "check_key", "apiKey": api_key })); }
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
