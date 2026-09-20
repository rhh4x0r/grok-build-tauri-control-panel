//! ACP terminal/* host: create, output, wait_for_exit, kill, release.
//!
//! Spec: https://agentclientprotocol.com/protocol/v1/terminals
//! Without this, Grok `run_terminal_command` hangs on `terminal/create`.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;

use serde_json::{json, Value};
use tokio::io::{AsyncReadExt, BufReader};
use tokio::process::Command;
use tokio::sync::{watch, Mutex};
use tracing::{debug, warn};
use uuid::Uuid;

use crate::error::{AcpError, Result};

const DEFAULT_OUTPUT_LIMIT: usize = 1_048_576; // 1 MiB

#[derive(Debug)]
struct TerminalState {
    output: String,
    truncated: bool,
    output_limit: usize,
    exit_code: Option<i32>,
    signal: Option<String>,
    finished: bool,
}

impl TerminalState {
    fn push_bytes(&mut self, chunk: &[u8]) {
        if chunk.is_empty() {
            return;
        }
        let s = String::from_utf8_lossy(chunk);
        self.output.push_str(&s);
        if self.output.len() > self.output_limit {
            // Truncate from the beginning at a char boundary.
            let excess = self.output.len() - self.output_limit;
            let mut cut = excess.min(self.output.len());
            while cut < self.output.len() && !self.output.is_char_boundary(cut) {
                cut += 1;
            }
            self.output = self.output[cut..].to_string();
            self.truncated = true;
        }
    }

    fn to_output_result(&self) -> Value {
        let mut out = json!({
            "output": self.output,
            "truncated": self.truncated,
        });
        if self.finished {
            out["exitStatus"] = json!({
                "exitCode": self.exit_code,
                "signal": self.signal,
            });
        }
        out
    }

    fn to_wait_result(&self) -> Value {
        json!({
            "exitCode": self.exit_code,
            "signal": self.signal,
        })
    }
}

struct ManagedTerminal {
    state: Arc<Mutex<TerminalState>>,
    /// `true` once the process has exited (watch avoids lost-wakeup races).
    finished_rx: watch::Receiver<bool>,
    /// The process owner listens while waiting for exit; cancellation never
    /// needs a lock held by `Child::wait`.
    cancel_tx: watch::Sender<bool>,
}

/// Each Unix terminal owns a separate process group. Stopping a shell must
/// also stop npm/dev-server descendants, without signalling the app's group.
#[cfg(unix)]
struct ProcessGroup(Option<u32>);

#[cfg(unix)]
impl ProcessGroup {
    fn kill(&mut self) {
        if let Some(pid) = self.0.take().and_then(|id| libc::pid_t::try_from(id).ok()) {
            if pid > 0 {
                // SAFETY: `pid` is the process-group leader we just spawned
                // with process_group(0). No pointers are passed to kill(2).
                unsafe {
                    libc::kill(-pid, libc::SIGKILL);
                }
            }
        }
    }
}

#[cfg(unix)]
impl Drop for ProcessGroup {
    fn drop(&mut self) {
        self.kill();
    }
}

async fn wait_finished(mut finished: watch::Receiver<bool>) {
    while !*finished.borrow() {
        if finished.changed().await.is_err() {
            break;
        }
    }
}

/// In-memory terminal registry for one ACP client connection.
pub struct TerminalRegistry {
    terminals: Mutex<HashMap<String, ManagedTerminal>>,
    default_cwd: PathBuf,
}

impl TerminalRegistry {
    pub fn new(default_cwd: PathBuf) -> Self {
        Self {
            terminals: Mutex::new(HashMap::new()),
            default_cwd,
        }
    }

    pub async fn handle(&self, method: &str, params: &Option<Value>) -> Result<Value> {
        match method {
            "terminal/create" => self.create(params).await,
            "terminal/output" => self.output(params).await,
            "terminal/wait_for_exit" | "terminal/waitForExit" => self.wait_for_exit(params).await,
            "terminal/kill" => self.kill(params).await,
            "terminal/release" => self.release(params).await,
            other => Err(AcpError::Protocol(format!(
                "unknown terminal method: {other}"
            ))),
        }
    }

    async fn create(&self, params: &Option<Value>) -> Result<Value> {
        let p = params
            .as_ref()
            .ok_or_else(|| AcpError::Protocol("terminal/create missing params".into()))?;

        let command = p
            .get("command")
            .and_then(|v| v.as_str())
            .ok_or_else(|| AcpError::Protocol("terminal/create missing command".into()))?
            .to_string();

        let args: Vec<String> = p
            .get("args")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default();

        let cwd = p
            .get("cwd")
            .and_then(|v| v.as_str())
            .map(PathBuf::from)
            .unwrap_or_else(|| self.default_cwd.clone());
        // Terminals must stay inside the workspace — an agent-supplied cwd
        // outside it defeats the fs sandbox entirely.
        let workspace = self
            .default_cwd
            .canonicalize()
            .unwrap_or_else(|_| self.default_cwd.clone());
        let cwd_canon = cwd.canonicalize().unwrap_or_else(|_| cwd.clone());
        if cwd_canon != workspace && !cwd_canon.starts_with(&workspace) {
            return Err(AcpError::Protocol(format!(
                "terminal cwd outside workspace: {}",
                cwd.display()
            )));
        }
        let cwd = cwd_canon;

        let output_limit = p
            .get("outputByteLimit")
            .or_else(|| p.get("output_byte_limit"))
            .and_then(|v| v.as_u64())
            .map(|n| n as usize)
            .unwrap_or(DEFAULT_OUTPUT_LIMIT)
            .max(1024);

        let env_pairs: Vec<(String, String)> = p
            .get("env")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|item| {
                        let name = item.get("name")?.as_str()?.to_string();
                        let value = item.get("value")?.as_str()?.to_string();
                        Some((name, value))
                    })
                    .collect()
            })
            .unwrap_or_default();

        let mut cmd = build_command(&command, &args, &cwd, &env_pairs);
        cmd.stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        #[cfg(unix)]
        cmd.process_group(0);

        debug!(%command, cwd = %cwd.display(), "terminal/create spawn");

        let mut child = cmd
            .spawn()
            .map_err(|e| AcpError::Protocol(format!("terminal/create spawn failed: {e}")))?;
        #[cfg(unix)]
        let mut process_group = ProcessGroup(child.id());

        let stdout = child.stdout.take();
        let stderr = child.stderr.take();

        let terminal_id = format!("term_{}", Uuid::new_v4().simple());
        let state = Arc::new(Mutex::new(TerminalState {
            output: String::new(),
            truncated: false,
            output_limit,
            exit_code: None,
            signal: None,
            finished: false,
        }));
        let (finished_tx, finished_rx) = watch::channel(false);
        let (cancel_tx, mut cancel_rx) = watch::channel(false);
        let mut pumps = Vec::new();

        // Pump stdout
        if let Some(out) = stdout {
            let st = state.clone();
            pumps.push(tokio::spawn(async move {
                let mut reader = BufReader::new(out);
                let mut buf = [0u8; 8192];
                loop {
                    match reader.read(&mut buf).await {
                        Ok(0) => break,
                        Ok(n) => st.lock().await.push_bytes(&buf[..n]),
                        Err(e) => {
                            warn!(error = %e, "terminal stdout read error");
                            break;
                        }
                    }
                }
            }));
        }

        // Pump stderr into the same output buffer (matches shell UX).
        if let Some(err) = stderr {
            let st = state.clone();
            pumps.push(tokio::spawn(async move {
                let mut reader = BufReader::new(err);
                let mut buf = [0u8; 8192];
                loop {
                    match reader.read(&mut buf).await {
                        Ok(0) => break,
                        Ok(n) => st.lock().await.push_bytes(&buf[..n]),
                        Err(e) => {
                            warn!(error = %e, "terminal stderr read error");
                            break;
                        }
                    }
                }
            }));
        }

        // One task owns the child and selects between exit and cancellation.
        // Never hold a shared child mutex across wait(): Stop would then wait
        // for the very process it is trying to kill (e.g. a preview server).
        {
            let st = state.clone();
            tokio::spawn(async move {
                let status = tokio::select! {
                    biased;
                    // Closing the registry also cancels its processes.
                    _ = cancel_rx.changed() => {
                        #[cfg(unix)]
                        process_group.kill();
                        let _ = child.start_kill();
                        child.wait().await
                    }
                    status = child.wait() => status,
                };
                // Clean up descendants retaining output pipes after their
                // shell exits. The group guard also handles task abortion.
                #[cfg(unix)]
                drop(process_group);
                // Publish exit only after already-written output is drained.
                // A detached descendant must not prevent cancellation forever.
                for mut pump in pumps {
                    if tokio::time::timeout(std::time::Duration::from_millis(250), &mut pump)
                        .await
                        .is_err()
                    {
                        pump.abort();
                    }
                }
                {
                    let mut s = st.lock().await;
                    if let Ok(status) = status {
                        s.exit_code = status.code();
                        #[cfg(unix)]
                        {
                            use std::os::unix::process::ExitStatusExt;
                            if status.code().is_none() {
                                if let Some(sig) = status.signal() {
                                    s.signal = Some(sig.to_string());
                                }
                            }
                        }
                    } else {
                        warn!(error = %status.unwrap_err(), "terminal wait failed");
                        s.exit_code = Some(-1);
                    }
                    s.finished = true;
                }
                let _ = finished_tx.send(true);
            });
        }

        self.terminals.lock().await.insert(
            terminal_id.clone(),
            ManagedTerminal {
                state,
                finished_rx,
                cancel_tx,
            },
        );

        Ok(json!({ "terminalId": terminal_id }))
    }

    async fn output(&self, params: &Option<Value>) -> Result<Value> {
        let id = terminal_id(params)?;
        let map = self.terminals.lock().await;
        let term = map
            .get(&id)
            .ok_or_else(|| AcpError::Protocol(format!("unknown terminalId: {id}")))?;
        let state = term.state.lock().await;
        Ok(state.to_output_result())
    }

    async fn wait_for_exit(&self, params: &Option<Value>) -> Result<Value> {
        let id = terminal_id(params)?;
        let (finished_rx, state) = {
            let map = self.terminals.lock().await;
            let term = map
                .get(&id)
                .ok_or_else(|| AcpError::Protocol(format!("unknown terminalId: {id}")))?;
            (term.finished_rx.clone(), term.state.clone())
        };

        wait_finished(finished_rx).await;

        let s = state.lock().await;
        Ok(s.to_wait_result())
    }

    async fn kill(&self, params: &Option<Value>) -> Result<Value> {
        let id = terminal_id(params)?;
        let (cancel, finished) = {
            let map = self.terminals.lock().await;
            let term = map
                .get(&id)
                .ok_or_else(|| AcpError::Protocol(format!("unknown terminalId: {id}")))?;
            (term.cancel_tx.clone(), term.finished_rx.clone())
        };
        let _ = cancel.send(true);
        wait_finished(finished).await;
        Ok(json!({}))
    }

    /// Kill every live terminal child (turn cancel) — best-effort; entries
    /// stay in the map so the agent's later output/release calls still work.
    pub async fn kill_all(&self) {
        let terminals: Vec<_> = {
            let map = self.terminals.lock().await;
            map.values()
                .map(|t| (t.cancel_tx.clone(), t.finished_rx.clone()))
                .collect()
        };
        // Signal all children first; one command cannot delay stopping others.
        for (cancel, _) in &terminals {
            let _ = cancel.send(true);
        }
        for (_, finished) in terminals {
            wait_finished(finished).await;
        }
    }

    async fn release(&self, params: &Option<Value>) -> Result<Value> {
        let id = terminal_id(params)?;
        let term = {
            let mut map = self.terminals.lock().await;
            map.remove(&id)
        };
        if let Some(term) = term {
            let _ = term.cancel_tx.send(true);
            wait_finished(term.finished_rx).await;
        }
        Ok(json!({}))
    }

    /// Short human line for the center-column terminal mirror.
    pub fn summary_line(method: &str, params: &Option<Value>, result: &Result<Value>) -> String {
        match method {
            "terminal/create" => {
                let cmd = params
                    .as_ref()
                    .and_then(|p| p.get("command"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("?");
                let args = params
                    .as_ref()
                    .and_then(|p| p.get("args"))
                    .and_then(|v| v.as_array())
                    .map(|a| {
                        a.iter()
                            .filter_map(|v| v.as_str())
                            .collect::<Vec<_>>()
                            .join(" ")
                    })
                    .unwrap_or_default();
                let full = if args.is_empty() {
                    cmd.to_string()
                } else {
                    format!("{cmd} {args}")
                };
                let short = if full.len() > 100 {
                    format!("{}…", &full[..100])
                } else {
                    full
                };
                match result {
                    Ok(v) => {
                        let id = v.get("terminalId").and_then(|x| x.as_str()).unwrap_or("?");
                        format!("$ {short}  [{id}]")
                    }
                    Err(e) => format!("$ {short}  [spawn failed: {e}]"),
                }
            }
            "terminal/wait_for_exit" | "terminal/waitForExit" => match result {
                Ok(v) => {
                    let code = v
                        .get("exitCode")
                        .and_then(|c| c.as_i64())
                        .map(|c| c.to_string())
                        .unwrap_or_else(|| "?".into());
                    format!("· terminal exit {code}")
                }
                Err(e) => format!("· terminal wait error: {e}"),
            },
            "terminal/kill" => "· terminal kill".into(),
            "terminal/release" => "· terminal release".into(),
            "terminal/output" => "· terminal output".into(),
            other => format!("· {other}"),
        }
    }
}

fn terminal_id(params: &Option<Value>) -> Result<String> {
    params
        .as_ref()
        .and_then(|p| p.get("terminalId").or_else(|| p.get("terminal_id")))
        .and_then(|v| v.as_str())
        .map(str::to_string)
        .ok_or_else(|| AcpError::Protocol("missing terminalId".into()))
}

/// The user's shell, falling back through common defaults ($SHELL → zsh →
/// bash → sh) so this works beyond zsh-only macOS setups.
pub(crate) fn user_shell() -> String {
    if let Ok(sh) = std::env::var("SHELL") {
        if !sh.trim().is_empty() && Path::new(&sh).exists() {
            return sh;
        }
    }
    for candidate in ["/bin/zsh", "/bin/bash", "/bin/sh"] {
        if Path::new(candidate).exists() {
            return candidate.to_string();
        }
    }
    "/bin/sh".into()
}

/// Build a process command. If `args` is empty and `command` looks like a shell
/// snippet (spaces / metacharacters), run via `$SHELL -lc` so Grok's
/// `run_terminal_command` payloads work.
fn build_command(
    command: &str,
    args: &[String],
    cwd: &Path,
    env_pairs: &[(String, String)],
) -> Command {
    let mut cmd = if args.is_empty() && needs_shell(command) {
        let mut c = Command::new(user_shell());
        c.arg("-lc").arg(command);
        c
    } else {
        let mut c = Command::new(command);
        for a in args {
            c.arg(a);
        }
        c
    };

    cmd.current_dir(cwd);

    // GUI apps often lack a login-shell PATH; ensure common tool locations.
    let path = std::env::var("PATH").unwrap_or_default();
    let augmented = if path.is_empty() {
        "/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin".to_string()
    } else if !path.contains("/opt/homebrew/bin") {
        format!("/opt/homebrew/bin:/usr/local/bin:{path}")
    } else {
        path
    };
    cmd.env("PATH", augmented);
    if let Ok(home) = std::env::var("HOME") {
        cmd.env("HOME", home);
    }
    for (k, v) in env_pairs {
        cmd.env(k, v);
    }
    cmd
}

fn needs_shell(command: &str) -> bool {
    command.contains(' ')
        || command.contains('|')
        || command.contains('&')
        || command.contains(';')
        || command.contains('>')
        || command.contains('<')
        || command.contains('$')
        || command.contains('`')
        || command.contains('\n')
        || command.contains('(')
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    async fn running_terminal(reg: &TerminalRegistry) -> String {
        let created = reg
            .handle(
                "terminal/create",
                &Some(json!({
                    "command": "sh", "args": ["-c", "echo ready; exec sleep 30"]
                })),
            )
            .await
            .unwrap();
        let id = created["terminalId"].as_str().unwrap().to_string();
        tokio::time::timeout(Duration::from_secs(3), async {
            loop {
                let out = reg.output(&Some(json!({"terminalId": id}))).await.unwrap();
                if out["output"].as_str().unwrap().contains("ready") {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("terminal must be running before cancellation");
        id
    }

    #[tokio::test]
    async fn kill_and_release_interrupt_an_active_exit_waiter() {
        for operation in ["terminal/kill", "terminal/release"] {
            let reg = Arc::new(TerminalRegistry::new(std::env::temp_dir()));
            let id = running_terminal(&reg).await;
            // Park an actual ACP wait request before cancelling the terminal.
            let (waiting, parked) = tokio::sync::oneshot::channel();
            let waiter = tokio::spawn({
                let reg = reg.clone();
                let id = id.clone();
                async move {
                    let params = Some(json!({"terminalId": id}));
                    let future = reg.wait_for_exit(&params);
                    tokio::pin!(future);
                    // Poll once so it captures the terminal's watch receiver.
                    assert!(futures::poll!(&mut future).is_pending());
                    let _ = waiting.send(());
                    future.await
                }
            });
            parked.await.unwrap();
            tokio::time::timeout(Duration::from_secs(3), async {
                reg.handle(operation, &Some(json!({"terminalId": id})))
                    .await
                    .unwrap();
                let result = waiter.await.unwrap().unwrap();
                assert!(result["exitCode"] != 0 || !result["signal"].is_null());
            })
            .await
            .expect("Stop must not wait for a long-running process to exit on its own");
        }
    }

    #[tokio::test]
    async fn cancel_all_stops_only_this_sessions_terminals() {
        let reg = TerminalRegistry::new(std::env::temp_dir());
        let other = TerminalRegistry::new(std::env::temp_dir());
        let first = running_terminal(&reg).await;
        let second = running_terminal(&reg).await;
        let unrelated = running_terminal(&other).await;
        tokio::time::timeout(Duration::from_secs(3), reg.kill_all())
            .await
            .unwrap();
        for id in [first, second] {
            let out = reg.output(&Some(json!({"terminalId": id}))).await.unwrap();
            assert!(out.get("exitStatus").is_some());
        }
        let out = other
            .output(&Some(json!({"terminalId": unrelated})))
            .await
            .unwrap();
        assert!(
            out.get("exitStatus").is_none(),
            "another session must keep running"
        );
        tokio::time::timeout(Duration::from_secs(3), other.kill_all())
            .await
            .unwrap();
        // Repeated Stop after completion must also return immediately.
        tokio::time::timeout(Duration::from_secs(1), reg.kill_all())
            .await
            .unwrap();
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn cancellation_stops_shell_descendants() {
        let dir = tempfile::tempdir().unwrap();
        let heartbeat = dir.path().join("heartbeat");
        let reg = TerminalRegistry::new(dir.path().to_path_buf());
        reg.create(&Some(json!({
            "command": "sh",
            "args": ["-c", "(while :; do printf x >> \"$1\"; sleep 0.03; done) & wait", "sh", heartbeat],
        }))).await.unwrap();
        tokio::time::timeout(Duration::from_secs(3), async {
            while std::fs::metadata(&heartbeat).map(|m| m.len()).unwrap_or(0) < 2 {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("shell descendant started");
        tokio::time::timeout(Duration::from_secs(3), reg.kill_all())
            .await
            .unwrap();
        let stopped = std::fs::read(&heartbeat).unwrap();
        tokio::time::sleep(Duration::from_millis(150)).await;
        assert_eq!(
            stopped,
            std::fs::read(&heartbeat).unwrap(),
            "descendant kept working after Stop"
        );
    }

    #[tokio::test]
    async fn create_wait_output_echo() {
        let reg = TerminalRegistry::new(std::env::temp_dir());
        let create = reg
            .handle(
                "terminal/create",
                &Some(json!({
                    "command": "echo",
                    "args": ["hello-acp-term"],
                    "cwd": std::env::temp_dir().to_string_lossy(),
                })),
            )
            .await
            .expect("create");
        let id = create["terminalId"].as_str().unwrap().to_string();

        let wait = reg
            .handle("terminal/wait_for_exit", &Some(json!({ "terminalId": id })))
            .await
            .expect("wait");
        assert_eq!(wait["exitCode"], 0);

        let out = reg
            .handle("terminal/output", &Some(json!({ "terminalId": id })))
            .await
            .expect("output");
        let text = out["output"].as_str().unwrap_or("");
        assert!(text.contains("hello-acp-term"), "output was: {text:?}");
        assert_eq!(out["truncated"], false);
        assert_eq!(out["exitStatus"]["exitCode"], 0);

        reg.handle("terminal/release", &Some(json!({ "terminalId": id })))
            .await
            .expect("release");
    }

    #[tokio::test]
    async fn shell_snippet_via_zsh() {
        let reg = TerminalRegistry::new(std::env::temp_dir());
        let create = reg
            .handle(
                "terminal/create",
                &Some(json!({
                    "command": "echo hi && echo there",
                })),
            )
            .await
            .expect("create");
        let id = create["terminalId"].as_str().unwrap().to_string();
        let wait = reg
            .handle("terminal/wait_for_exit", &Some(json!({ "terminalId": id })))
            .await
            .expect("wait");
        assert_eq!(wait["exitCode"], 0);
        let out = reg
            .handle("terminal/output", &Some(json!({ "terminalId": id })))
            .await
            .expect("output");
        let text = out["output"].as_str().unwrap_or("");
        assert!(text.contains("hi"), "{text:?}");
        assert!(text.contains("there"), "{text:?}");
    }

    #[test]
    fn needs_shell_detects_snippets() {
        assert!(needs_shell("pwd && ls"));
        assert!(needs_shell("echo hi"));
        assert!(!needs_shell("ls"));
        assert!(!needs_shell("/bin/echo"));
    }
}
