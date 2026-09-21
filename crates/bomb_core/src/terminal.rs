//! User-operated PTY sessions. Independent of agent approval and tool terminals.
use portable_pty::{ChildKiller, CommandBuilder, MasterPty, NativePtySystem, PtySize, PtySystem};
use std::{
    io::{Read, Write},
    path::Path,
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc, Mutex,
    },
};

/// What a terminal on a paired server needs from its connection.
pub enum RemoteOp {
    Input(Vec<u8>),
    Resize(u16, u16),
    Close,
}

enum Backend {
    /// A shell on this machine.
    Local {
        master: Mutex<Box<dyn MasterPty + Send>>,
        writer: Mutex<Box<dyn Write + Send>>,
        killer: Mutex<Box<dyn ChildKiller + Send + Sync>>,
    },
    /// A shell on a paired server: bytes go out through `send`, output arrives through [`TerminalSession::feed`].
    Remote { send: Box<dyn Fn(RemoteOp) + Send + Sync> },
}

/// Raw shell output, for a server relaying it to a client. `None` means the shell exited.
pub type OutputTap = Box<dyn Fn(Option<&[u8]>) + Send>;

pub struct TerminalSession {
    backend: Backend,
    parser: Arc<Mutex<vt100::Parser>>,
    exited: Arc<AtomicBool>,
    revision: Arc<AtomicU64>,
}
impl TerminalSession {
    pub fn spawn(cwd: &Path) -> Result<Arc<Self>, String> {
        Self::spawn_shell(cwd, &Self::login_shell(), true, None)
    }
    /// A shell whose raw output is also handed to `tap` (a server relaying it to a client).
    pub fn spawn_tapped(cwd: &Path, tap: OutputTap) -> Result<Arc<Self>, String> {
        Self::spawn_shell(cwd, &Self::login_shell(), true, Some(tap))
    }
    /// The screen for a shell running on a paired server.
    pub fn remote(send: Box<dyn Fn(RemoteOp) + Send + Sync>) -> Arc<Self> {
        Arc::new(Self {
            backend: Backend::Remote { send },
            parser: Arc::new(Mutex::new(vt100::Parser::new(14, 100, 5000))),
            exited: Arc::new(AtomicBool::new(false)),
            revision: Arc::new(AtomicU64::new(0)),
        })
    }
    /// Output from the server's shell; `None` when it exited.
    pub fn feed(&self, bytes: Option<&[u8]>) {
        match bytes {
            Some(bytes) => self.parser.lock().unwrap_or_else(|e| e.into_inner()).process(bytes),
            None => self.exited.store(true, Ordering::Relaxed),
        }
        self.revision.fetch_add(1, Ordering::Relaxed);
    }
    fn login_shell() -> String {
        std::env::var("SHELL")
            .ok()
            .filter(|s| Path::new(s).is_absolute() && Path::new(s).is_file())
            .unwrap_or_else(|| "/bin/sh".into())
    }
    fn spawn_shell(cwd: &Path, shell: &str, login: bool, tap: Option<OutputTap>) -> Result<Arc<Self>, String> {
        if !cwd.is_absolute() || !cwd.is_dir() {
            return Err("Terminal needs an existing thread folder".into());
        }
        let pair = NativePtySystem::default()
            .openpty(PtySize {
                rows: 14,
                cols: 100,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|e| e.to_string())?;
        let mut command = CommandBuilder::new(shell);
        if login {
            command.arg("-l");
        }
        command.cwd(cwd);
        command.env("TERM", "xterm-256color");
        command.env("COLORTERM", "truecolor");
        let mut reader = pair.master.try_clone_reader().map_err(|e| e.to_string())?;
        let writer = pair.master.take_writer().map_err(|e| e.to_string())?;
        let mut child = pair
            .slave
            .spawn_command(command)
            .map_err(|e| e.to_string())?;
        drop(pair.slave);
        let killer = child.clone_killer();
        let parser = Arc::new(Mutex::new(vt100::Parser::new(14, 100, 5000)));
        let exited = Arc::new(AtomicBool::new(false));
        let revision = Arc::new(AtomicU64::new(0));
        let session = Arc::new(Self {
            backend: Backend::Local { master: Mutex::new(pair.master), writer: Mutex::new(writer), killer: Mutex::new(killer) },
            parser: parser.clone(),
            exited: exited.clone(),
            revision: revision.clone(),
        });
        let output_revision = revision.clone();
        std::thread::spawn(move || {
            let mut bytes = [0_u8; 8192];
            while let Ok(n) = reader.read(&mut bytes) {
                if n == 0 {
                    break;
                }
                parser
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .process(&bytes[..n]);
                output_revision.fetch_add(1, Ordering::Relaxed);
                if let Some(tap) = &tap { tap(Some(&bytes[..n])); }
            }
            if let Some(tap) = &tap { tap(None); }
        });
        std::thread::spawn(move || {
            let _ = child.wait();
            exited.store(true, Ordering::Relaxed);
            revision.fetch_add(1, Ordering::Relaxed);
        });
        Ok(session)
    }
    pub fn write(&self, bytes: &[u8]) -> Result<(), String> {
        if self.exited() {
            return Err("This terminal has exited. Open a new terminal.".into());
        }
        self.parser
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .screen_mut()
            .set_scrollback(0);
        match &self.backend {
            Backend::Local { writer, .. } => {
                let mut writer = writer.lock().unwrap_or_else(|e| e.into_inner());
                writer.write_all(bytes).and_then(|_| writer.flush()).map_err(|e| e.to_string())
            }
            Backend::Remote { send } => { send(RemoteOp::Input(bytes.to_vec())); Ok(()) }
        }
    }
    pub fn resize(&self, rows: u16, cols: u16) {
        let (rows, cols) = (rows.clamp(2, 100), cols.clamp(10, 300));
        let mut parser = self.parser.lock().unwrap_or_else(|e| e.into_inner());
        if parser.screen().size() == (rows, cols) {
            return;
        }
        let resized = match &self.backend {
            Backend::Local { master, .. } => master.lock().unwrap_or_else(|e| e.into_inner()).resize(PtySize { rows, cols, pixel_width: 0, pixel_height: 0 }).is_ok(),
            Backend::Remote { send } => { send(RemoteOp::Resize(rows, cols)); true }
        };
        if resized {
            parser.screen_mut().set_size(rows, cols);
            self.revision.fetch_add(1, Ordering::Relaxed);
        }
    }
    pub fn scroll(&self, lines: isize) {
        let mut parser = self.parser.lock().unwrap_or_else(|e| e.into_inner());
        let offset = parser.screen().scrollback().saturating_add_signed(lines);
        parser.screen_mut().set_scrollback(offset);
        self.revision.fetch_add(1, Ordering::Relaxed);
    }
    pub fn screen(&self) -> vt100::Screen {
        self.parser
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .screen()
            .clone()
    }
    pub fn revision(&self) -> u64 {
        self.revision.load(Ordering::Relaxed)
    }
    pub fn exited(&self) -> bool {
        self.exited.load(Ordering::Relaxed)
    }
}
impl Drop for TerminalSession {
    fn drop(&mut self) {
        match &self.backend {
            Backend::Local { killer, .. } => { let _ = killer.lock().unwrap_or_else(|e| e.into_inner()).kill(); }
            Backend::Remote { send } => send(RemoteOp::Close),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn terminal_runs_in_requested_directory_and_accepts_input() {
        let dir = tempfile::tempdir().unwrap();
        let terminal = TerminalSession::spawn_shell(dir.path(), "/bin/sh", false, None).unwrap();
        terminal
            .write(b"printf '\\nBOMB_%s_OK\\n' TERMINAL; pwd\r")
            .unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            let output = terminal.screen().contents();
            if output.contains("BOMB_TERMINAL_OK")
                && output.contains(dir.path().file_name().unwrap().to_str().unwrap())
            {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "terminal did not produce expected output"
            );
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        terminal.resize(20, 80);
        assert_eq!(terminal.screen().size(), (20, 80));
        terminal.write(b"exit\r").unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while !terminal.exited() {
            assert!(std::time::Instant::now() < deadline, "shell did not exit");
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
    }
}
