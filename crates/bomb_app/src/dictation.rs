//! Dictation through `bomb-dictate`, the Swift helper next to this binary (see build.rs): it
//! runs Apple's on-device SpeechAnalyzer and prints what it hears as JSON lines.

use std::io::{BufRead, BufReader};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::OnceLock;
use std::time::Duration;

/// What the helper reports while it listens.
pub enum Heard {
    /// `settled` only grows; `volatile` is the words still being worked out, replaced each time.
    Words { settled: String, volatile: String },
    Done,
    Failed(String),
}

fn helper() -> Option<PathBuf> {
    let path = std::env::current_exe().ok()?.parent()?.join("bomb-dictate");
    path.is_file().then_some(path)
}

/// Whether this Mac can dictate (macOS 26+, a supported Mac, and a build with the helper).
/// The first call runs the helper, so make it off the main thread.
pub fn available() -> bool {
    static AVAILABLE: OnceLock<bool> = OnceLock::new();
    *AVAILABLE.get_or_init(|| {
        helper()
            .and_then(|h| Command::new(h).arg("--check").stderr(Stdio::null()).output().ok())
            .is_some_and(|o| String::from_utf8_lossy(&o.stdout).trim() == "ok")
    })
}

/// A running dictation. Dropping it stops listening.
pub struct Listening {
    child: Option<Child>,
}

impl Listening {
    pub fn start() -> Result<(Self, async_channel::Receiver<Heard>), String> {
        let helper = helper().ok_or("Dictation isn't part of this build.")?;
        let mut child = Command::new(helper)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| format!("Couldn't start dictation: {e}"))?;
        let stdout = child.stdout.take().ok_or("Couldn't start dictation.")?;
        let (tx, rx) = async_channel::unbounded();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                let Ok(v) = serde_json::from_str::<serde_json::Value>(&line) else { continue };
                let heard = if let Some(error) = v["error"].as_str() {
                    Heard::Failed(error.to_string())
                } else if v["done"] == true {
                    Heard::Done
                } else if let Some(settled) = v["final"].as_str() {
                    Heard::Words { settled: settled.to_string(), volatile: v["volatile"].as_str().unwrap_or_default().to_string() }
                } else {
                    continue;
                };
                if tx.send_blocking(heard).is_err() {
                    return;
                }
            }
            let _ = tx.send_blocking(Heard::Done);
        });
        Ok((Self { child: Some(child) }, rx))
    }

    /// Stop listening: the last words still arrive, then `Done`.
    pub fn stop(&mut self) {
        if let Some(child) = &mut self.child {
            drop(child.stdin.take());
        }
    }
}

impl Drop for Listening {
    fn drop(&mut self) {
        self.stop();
        if let Some(mut child) = self.child.take() {
            // Give it a moment to settle the last words, then make sure it's gone.
            std::thread::spawn(move || {
                for _ in 0..50 {
                    if matches!(child.try_wait(), Ok(Some(_))) {
                        return;
                    }
                    std::thread::sleep(Duration::from_millis(200));
                }
                let _ = child.kill();
                let _ = child.wait();
            });
        }
    }
}
