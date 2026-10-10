//! Notifications on the Mac. macOS only lets an app bundle post them, so build.rs wraps a small
//! Swift helper as "Bomb Code Notifier.app" next to this binary (inside the .app, under Helpers);
//! this module runs it and speaks its line protocol (see tools/bomb-notify/main.swift).

use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::{Child, ChildStdin, Command, Stdio};

use serde_json::{json, Value};

pub enum NotifyEvent {
    /// "notDetermined", "denied", "authorized" or "provisional".
    Status(String),
    /// A notification was clicked: the thread it's about.
    Clicked(String),
}

fn helper() -> Option<PathBuf> {
    let dir = std::env::current_exe().ok()?.parent()?.to_path_buf();
    let inner = "Bomb Code Notifier.app/Contents/MacOS/bomb-notify";
    [dir.join(inner), dir.join("../Helpers").join(inner)].into_iter().find(|p| p.is_file())
}

/// Whether this build has the notifier (it needs Xcode's Swift compiler to build).
pub fn available() -> bool {
    helper().is_some()
}

pub struct Notifier {
    child: Child,
    stdin: ChildStdin,
}

impl Notifier {
    pub fn start() -> Result<(Self, async_channel::Receiver<NotifyEvent>), String> {
        let path = helper().ok_or("Notifications aren't part of this build.")?;
        let mut child = Command::new(path)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| format!("Couldn't start notifications: {e}"))?;
        let stdin = child.stdin.take().ok_or("Couldn't start notifications.")?;
        let stdout = child.stdout.take().ok_or("Couldn't start notifications.")?;
        let (tx, rx) = async_channel::unbounded();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                let Ok(v) = serde_json::from_str::<Value>(&line) else { continue };
                let event = if let Some(status) = v["status"].as_str() {
                    NotifyEvent::Status(status.to_string())
                } else if let Some(thread) = v["clicked"].as_str() {
                    NotifyEvent::Clicked(thread.to_string())
                } else {
                    continue;
                };
                if tx.send_blocking(event).is_err() { return; }
            }
        });
        Ok((Self { child, stdin }, rx))
    }

    fn send(&mut self, command: Value) {
        let _ = writeln!(self.stdin, "{command}");
        let _ = self.stdin.flush();
    }

    pub fn status(&mut self) { self.send(json!({ "cmd": "status" })); }
    pub fn authorize(&mut self) { self.send(json!({ "cmd": "authorize" })); }
    pub fn notify(&mut self, id: &str, title: &str, body: &str, thread: &str) {
        self.send(json!({ "cmd": "notify", "id": id, "title": title, "body": body, "thread": thread }));
    }
    pub fn withdraw(&mut self, thread: &str) { self.send(json!({ "cmd": "withdraw", "thread": thread })); }
}

impl Drop for Notifier {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
