//! Processes running from a thread's folder: what an agent launched (a dev server, a
//! desktop app, a watcher) whether it went through Bomb Code or the agent's own shell.
//!
//! macOS gives no "started by this thread" link for a detached process, so a process
//! belongs to a folder when its working directory is inside it, or its program lives
//! there. Shells, agents and editor tooling working in the folder are left out.

use std::collections::HashMap;
use std::path::Path;

use serde::Serialize;
use tokio::process::Command;

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ProcessInfo {
    pub pid: u32,
    /// Short program name ("cloud-widget", "node").
    pub name: String,
    /// Full command line.
    pub command: String,
    pub cwd: String,
    pub running_secs: u64,
    pub cpu_percent: f32,
    pub memory_kb: u64,
    /// TCP ports it is listening on.
    pub ports: Vec<u16>,
}

/// Processes younger than this are left out: agents and Git checks run short commands in
/// project folders constantly, and listing them makes the panel flicker.
const MIN_AGE_SECS: u64 = 2;

/// Processes working in `folder`, longest running first.
pub async fn list_for_folder(folder: &str) -> Result<Vec<ProcessInfo>, String> {
    let mut all = list_for_folders(&[folder.to_string()]).await?;
    Ok(all.remove(folder.trim_end_matches('/')).unwrap_or_default())
}

/// One scan, split by folder: every folder's list for the price of one `ps` + `lsof`.
/// Keys are the folders without a trailing slash.
pub async fn list_for_folders(folders: &[String]) -> Result<HashMap<String, Vec<ProcessInfo>>, String> {
    let folders: Vec<&str> = folders
        .iter()
        .map(|f| f.trim_end_matches('/'))
        .filter(|f| !f.is_empty() && Path::new(f).is_absolute())
        .collect();
    if folders.is_empty() {
        return Err("no project folder".into());
    }
    let (table, args, cwds, ports) = tokio::join!(
        run("ps", &["-axo", "pid=,ppid=,etime=,%cpu=,rss=,comm="]),
        run("ps", &["-axo", "pid=,args="]),
        run("lsof", &["-a", "-d", "cwd", "-Fpn"]),
        run("lsof", &["-nP", "-iTCP", "-sTCP:LISTEN", "-Fpn"]),
    );
    let args = parse_args(&args?);
    let cwds = parse_lsof_names(&cwds.unwrap_or_default());
    let ports = parse_ports(&ports.unwrap_or_default());
    let own = std::process::id();
    let mut out: HashMap<String, Vec<ProcessInfo>> = folders.iter().map(|f| (f.to_string(), Vec::new())).collect();
    for row in parse_table(&table?) {
        if row.pid == own || row.running_secs < MIN_AGE_SECS {
            continue;
        }
        let command = args.get(&row.pid).cloned().unwrap_or_else(|| row.comm.clone());
        // Login shells report as "-zsh".
        let name = row.comm.rsplit('/').next().unwrap_or(&row.comm).trim_start_matches('-').to_string();
        if is_tooling(&name, &command) {
            continue;
        }
        let cwd = cwds.get(&row.pid).cloned().unwrap_or_default();
        let program = command.split_whitespace().next().unwrap_or("").to_string();
        for folder in &folders {
            if inside(&cwd, folder) || (program.starts_with('/') && inside(&program, folder)) {
                out.get_mut(*folder).expect("seeded").push(ProcessInfo {
                    pid: row.pid,
                    name: name.clone(),
                    command: command.clone(),
                    cwd: cwd.clone(),
                    running_secs: row.running_secs,
                    cpu_percent: row.cpu,
                    memory_kb: row.rss_kb,
                    ports: ports.get(&row.pid).cloned().unwrap_or_default(),
                });
            }
        }
    }
    for list in out.values_mut() {
        list.sort_by_key(|p| std::cmp::Reverse(p.running_secs));
    }
    Ok(out)
}

/// Ask a process to quit (`force`: kill it). Only a process still listed for `folder`
/// can be stopped, so a stale or forged pid can't reach anything else.
pub async fn stop(folder: &str, pid: u32, force: bool) -> Result<(), String> {
    if !list_for_folder(folder).await?.iter().any(|p| p.pid == pid) {
        return Err("That process has already exited.".into());
    }
    let signal = if force { "-KILL" } else { "-TERM" };
    run("kill", &[signal, &pid.to_string()]).await.map(|_| ())
}

async fn run(program: &str, args: &[&str]) -> Result<String, String> {
    let out = Command::new(program)
        .args(args)
        .output()
        .await
        .map_err(|e| format!("{program}: {e}"))?;
    // lsof exits 1 when some processes can't be read; its output is still good.
    if !out.status.success() && out.stdout.is_empty() {
        return Err(format!("{program} failed: {}", String::from_utf8_lossy(&out.stderr).trim()));
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

fn inside(path: &str, folder: &str) -> bool {
    path == folder || path.strip_prefix(folder).is_some_and(|rest| rest.starts_with('/'))
}

/// Shells, agents, editors and language servers sit in project folders all day;
/// they are how work happens, not something the thread launched.
fn is_tooling(name: &str, command: &str) -> bool {
    const NAMES: &[&str] = &[
        "zsh", "bash", "sh", "fish", "dash", "login", "tmux", "screen", "ssh", "git",
        "lsof", "ps", "less", "man", "vim", "nvim", "caffeinate", "node_repl", "rust-analyzer",
        "rust-analyzer-proc-macro-srv", "gopls", "sourcekit-lsp", "clangd", "pyright",
    ];
    const PREFIXES: &[&str] = &["claude", "codex", "grok", "bomb_app", "bombd", "Cursor", "Code Helper", "Electron"];
    const NODE_TOOLS: &[&str] = &[
        "claude-agent-acp", "claude-code", "codex-acp", "@openai/codex", "tsserver",
        "language-server", "eslint", "prettier", "copilot",
    ];
    // Agents' own helpers: MCP servers, and anything run from an installed app bundle.
    const ARGS: &[&str] = &["mcp start", "mcp-server", "@modelcontextprotocol", "/cua_node/"];
    NAMES.contains(&name)
        || PREFIXES.iter().any(|p| name.starts_with(p))
        || command.starts_with("/Applications/")
        || ARGS.iter().any(|a| command.contains(a))
        || (name.starts_with("node") && NODE_TOOLS.iter().any(|t| command.contains(t)))
}

struct Row {
    pid: u32,
    running_secs: u64,
    cpu: f32,
    rss_kb: u64,
    comm: String,
}

/// `pid ppid etime %cpu rss comm`; comm is last and may contain spaces.
fn parse_table(text: &str) -> Vec<Row> {
    text.lines()
        .filter_map(|line| {
            let mut it = line.split_whitespace();
            let pid = it.next()?.parse().ok()?;
            let _ppid = it.next()?;
            let running_secs = parse_etime(it.next()?)?;
            let cpu = it.next()?.parse().ok()?;
            let rss_kb = it.next()?.parse().ok()?;
            let comm = it.collect::<Vec<_>>().join(" ");
            (!comm.is_empty()).then_some(Row { pid, running_secs, cpu, rss_kb, comm })
        })
        .collect()
}

fn parse_args(text: &str) -> HashMap<u32, String> {
    text.lines()
        .filter_map(|line| {
            let line = line.trim_start();
            let (pid, rest) = line.split_once(char::is_whitespace)?;
            Some((pid.parse().ok()?, rest.trim().to_string()))
        })
        .collect()
}

/// `lsof -F pn` output: a `p<pid>` line, then `n<name>` lines for that process.
fn parse_lsof_names(text: &str) -> HashMap<u32, String> {
    let mut out = HashMap::new();
    let mut pid = None;
    for line in text.lines() {
        if let Some(p) = line.strip_prefix('p') {
            pid = p.parse().ok();
        } else if let (Some(name), Some(pid)) = (line.strip_prefix('n'), pid) {
            out.entry(pid).or_insert_with(|| name.to_string());
        }
    }
    out
}

fn parse_ports(text: &str) -> HashMap<u32, Vec<u16>> {
    let mut out: HashMap<u32, Vec<u16>> = HashMap::new();
    let mut pid = None;
    for line in text.lines() {
        if let Some(p) = line.strip_prefix('p') {
            pid = p.parse().ok();
        } else if let (Some(name), Some(pid)) = (line.strip_prefix('n'), pid) {
            if let Some(port) = name.rsplit(':').next().and_then(|p| p.parse::<u16>().ok()) {
                let ports = out.entry(pid).or_default();
                if !ports.contains(&port) {
                    ports.push(port);
                }
            }
        }
    }
    out
}

/// `[[dd-]hh:]mm:ss` as seconds.
fn parse_etime(s: &str) -> Option<u64> {
    let (days, rest) = match s.split_once('-') {
        Some((d, r)) => (d.parse::<u64>().ok()?, r),
        None => (0, s),
    };
    let parts: Vec<u64> = rest.split(':').map(|p| p.parse().ok()).collect::<Option<_>>()?;
    let secs = match parts.as_slice() {
        [m, s] => m * 60 + s,
        [h, m, s] => h * 3600 + m * 60 + s,
        _ => return None,
    };
    Some(days * 86_400 + secs)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_ps_and_lsof_output() {
        let rows = parse_table("  4242     1 01-02:03:04   1.5  20480 cloud-widget\n  77 1 00:09 0.0 100 Google Chrome Helper\n");
        assert_eq!(rows[0].pid, 4242);
        assert_eq!(rows[0].running_secs, 86_400 + 2 * 3600 + 3 * 60 + 4);
        assert_eq!(rows[0].rss_kb, 20480);
        assert_eq!(rows[1].comm, "Google Chrome Helper");
        let cwds = parse_lsof_names("p4242\nfcwd\nn/Users/me/src/app\np77\nfcwd\nn/\n");
        assert_eq!(cwds[&4242], "/Users/me/src/app");
        let ports = parse_ports("p10\nf20\nn*:5173\nf21\nn[::1]:5173\np11\nf3\nn127.0.0.1:3000\n");
        assert_eq!(ports[&10], [5173]);
        assert_eq!(ports[&11], [3000]);
        assert_eq!(parse_args("  4242 ./target/release/cloud-widget --flag\n")[&4242], "./target/release/cloud-widget --flag");
    }

    #[test]
    fn folder_match_is_by_path_component() {
        assert!(inside("/a/app", "/a/app"));
        assert!(inside("/a/app/target", "/a/app"));
        assert!(!inside("/a/app-two", "/a/app"));
    }

    #[test]
    fn agents_and_shells_are_not_listed() {
        assert!(is_tooling("zsh", "-zsh"));
        assert!(is_tooling("node", "node /x/node_modules/.bin/convex mcp start --project-dir /x"));
        assert!(is_tooling("node", "/Applications/ChatGPT.app/Contents/Resources/cua_node/bin/node /y"));
        assert!(!is_tooling("Python", "/opt/homebrew/bin/python3 -m http.server 1420"));
        assert!(is_tooling("claude", "claude --resume"));
        assert!(is_tooling("node", "node /x/.bin/claude-agent-acp"));
        assert!(!is_tooling("node", "node node_modules/.bin/vite"));
        assert!(!is_tooling("cloud-widget", "./target/release/cloud-widget"));
    }

    #[tokio::test]
    async fn finds_a_process_started_in_the_folder() {
        let dir = tempfile::tempdir().unwrap();
        let folder = dir.path().canonicalize().unwrap();
        let mut child = std::process::Command::new("sleep").arg("30").current_dir(&folder).spawn().unwrap();
        // Brand-new processes are skipped until they've lasted MIN_AGE_SECS.
        assert!(list_for_folder(folder.to_str().unwrap()).await.unwrap().iter().all(|p| p.pid != child.id()));
        tokio::time::sleep(std::time::Duration::from_millis(MIN_AGE_SECS * 1000 + 1100)).await;
        let listed = list_for_folder(folder.to_str().unwrap()).await.unwrap();
        let found = listed.iter().find(|p| p.pid == child.id());
        assert!(found.is_some_and(|p| p.name == "sleep"), "{listed:?}");
        stop(folder.to_str().unwrap(), child.id(), false).await.unwrap();
        assert!(!child.wait().unwrap().success());
        assert!(stop(folder.to_str().unwrap(), 1, true).await.is_err(), "pids outside the folder are refused");
    }
}


/// A web server a thread started, for a phone to open: the port and what's listening.
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Server {
    pub port: u16,
    pub name: String,
}

/// The ports processes in a thread's folder (its worktree, and its project) are listening on.
pub async fn servers_for_thread(state: &crate::AppState, id: &str) -> Result<Vec<Server>, String> {
    let id = uuid::Uuid::parse_str(id).map_err(|e| e.to_string())?;
    let (cwd, root) = match state.registry.get_snapshot(id) {
        Ok(snap) => (snap.metadata.cwd, snap.metadata.project_root),
        Err(_) => {
            let rec = state.persistence.get_session(id).map_err(|e| e.to_string())?;
            let root = serde_json::from_str::<serde_json::Value>(&rec.metadata_json).ok()
                .and_then(|v| v.pointer("/metadata/projectRoot").and_then(|r| r.as_str()).map(String::from));
            (rec.cwd, root)
        }
    };
    let folders: Vec<String> = std::iter::once(cwd).chain(root).collect();
    let mut servers: Vec<Server> = Vec::new();
    for list in list_for_folders(&folders).await?.into_values() {
        for process in list {
            for port in process.ports {
                if !servers.iter().any(|s| s.port == port) { servers.push(Server { port, name: process.name.clone() }); }
            }
        }
    }
    servers.sort_by_key(|s| s.port);
    Ok(servers)
}
