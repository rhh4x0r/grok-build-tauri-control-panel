//! Making sure an agent goes away with its thread, and with the app.
//!
//! An agent is a chain of processes: `npm exec` (when it runs through npx), the adapter
//! (`node codex-acp`), and the agent itself (`codex app-server`). Killing only the process we
//! spawned leaves the rest running on their own. So a stopping agent takes its whole chain
//! with it, and at start-up the app stops chains left behind by an app that died without
//! cleaning up (a crash, a force quit).
//!
//! Only the agent's own processes are stopped. A dev server or watcher an agent started keeps
//! running, as it would in a terminal, so it stays reachable (and stoppable from Processes).

/// Where running agents are noted: one empty file per agent, `<app pid>-<agent group>`. (The
/// same folder rule as `grok_config::paths::bomb_home`.)
fn run_dir() -> Option<std::path::PathBuf> {
    let home = std::env::var_os("BOMBCODE_HOME").map(std::path::PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| std::path::PathBuf::from(h).join(".bombcode")))?;
    Some(home.join("run").join("agents"))
}

/// Note an agent started by this app (its process group is `group`), so a later start can stop it
/// if this app dies without doing so.
pub fn note_started(group: u32) {
    if let Some(dir) = run_dir() {
        let _ = std::fs::create_dir_all(&dir);
        let _ = std::fs::write(dir.join(format!("{}-{group}", std::process::id())), b"");
    }
}

fn note_stopped(group: u32) {
    if let Some(dir) = run_dir() { let _ = std::fs::remove_file(dir.join(format!("{}-{group}", std::process::id()))); }
}

/// The processes that make up an agent, never the commands it runs.
fn is_agent(command: &str) -> bool {
    const PARTS: &[&str] = &[
        // Codex: npm exec → node codex-acp → node codex.js app-server → codex app-server.
        "codex-acp",
        "codex.js app-server",
        "codex app-server",
        "codex-code-mode-host",
        // Claude: npm exec → node claude-agent-acp → the SDK's claude (stream-json).
        "claude-agent-acp",
        "claude-code-acp",
        "claude-agent-sdk",
        "grok agent",
        // Bomb Code's own helpers server, started by the agent.
        "--bomb-mcp",
    ];
    PARTS.iter().any(|part| command.contains(part))
}

struct Row {
    pid: i32,
    ppid: i32,
    group: i32,
    command: String,
}

/// Every process: pid, parent, process group, and command line.
fn processes() -> Vec<Row> {
    let Ok(out) = std::process::Command::new("ps").args(["-axww", "-o", "pid=,ppid=,pgid=,command="]).output() else { return Vec::new() };
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter_map(|line| {
            let mut parts = line.split_whitespace();
            let pid = parts.next()?.parse().ok()?;
            let ppid = parts.next()?.parse().ok()?;
            let group = parts.next()?.parse().ok()?;
            Some(Row { pid, ppid, group, command: parts.collect::<Vec<_>>().join(" ") })
        })
        .collect()
}

fn alive(pid: i32) -> bool {
    // SAFETY: signal 0 only checks that the process exists; no pointers are passed.
    pid > 0 && unsafe { libc::kill(pid, 0) } == 0
}

/// Ask `pids` to quit, then force whatever is still there half a second later.
fn stop(pids: &[i32]) {
    let pids: Vec<i32> = pids.iter().copied().filter(|p| *p > 1 && *p != std::process::id() as i32).collect();
    if pids.is_empty() { return; }
    // SAFETY: plain signals to processes found as this agent's; no pointers are passed.
    for pid in &pids { unsafe { libc::kill(*pid, libc::SIGTERM); } }
    std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(500));
        for pid in pids { if alive(pid) { unsafe { libc::kill(pid, libc::SIGKILL); } } }
    });
}

/// Stop the agent started as `root` and every agent process under it.
pub fn stop_agent(root: u32) {
    note_stopped(root);
    let root = root as i32;
    let rows = processes();
    let mut chain = vec![root];
    let mut index = 0;
    while index < chain.len() {
        let parent = chain[index];
        for row in rows.iter().filter(|r| r.ppid == parent && is_agent(&r.command)) {
            if !chain.contains(&row.pid) { chain.push(row.pid); }
        }
        index += 1;
    }
    stop(&chain);
}

/// Stop agents noted by an app that is no longer running (it crashed or was force-quit): the
/// agent processes of each such agent's process group. Returns how many were stopped.
pub fn stop_orphans() -> usize {
    let Some(dir) = run_dir() else { return 0 };
    let Ok(entries) = std::fs::read_dir(&dir) else { return 0 };
    let own = std::process::id() as i32;
    let mut groups = Vec::new();
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        let Some((owner, group)) = name.split_once('-') else { continue };
        let (Ok(owner), Ok(group)) = (owner.parse::<i32>(), group.parse::<i32>()) else { continue };
        if owner == own || alive(owner) { continue; }
        groups.push(group);
        let _ = std::fs::remove_file(entry.path());
    }
    if groups.is_empty() { return 0; }
    // Agent processes only: a dev server the agent started shares the group and keeps running.
    let left: Vec<i32> = processes().into_iter().filter(|r| groups.contains(&r.group) && is_agent(&r.command)).map(|r| r.pid).collect();
    stop(&left);
    left.len()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn agents_are_told_apart_from_what_they_run() {
        assert!(is_agent("node /Users/me/.npm/_npx/x/node_modules/.bin/codex-acp"));
        assert!(is_agent("/x/codex-darwin-arm64/vendor/aarch64-apple-darwin/bin/codex app-server"));
        assert!(is_agent("/Users/me/bin/grok agent stdio"));
        assert!(is_agent("/usr/bin/node /x/node_modules/@openai/codex/bin/codex.js app-server"));
        assert!(is_agent("/x/node_modules/@anthropic-ai/claude-agent-sdk-darwin-arm64/claude --output-format stream-json"));
        assert!(!is_agent("node /Users/me/src/site/node_modules/.bin/vite"));
        assert!(!is_agent("npm run dev"));
    }

    #[test]
    fn agents_whose_app_is_gone_are_stopped_at_start_and_others_are_not() {
        let home = tempfile::tempdir().unwrap();
        std::env::set_var("BOMBCODE_HOME", home.path());
        let dir = run_dir().unwrap();
        std::fs::create_dir_all(&dir).unwrap();
        // Two "agents", each in its own process group, each with a dev server beside it.
        let start = |tag: &str| {
            use std::os::unix::process::CommandExt;
            let script = format!("exec -a 'codex-acp {tag}' bash -c 'sleep 30 & exec -a \"codex app-server {tag}\" sleep 30'");
            let mut cmd = std::process::Command::new("bash");
            cmd.args(["-c", &format!("(sleep 30 &) ; {script}")]).process_group(0);
            cmd.spawn().unwrap()
        };
        let mut dead_agent = start("reap-test-dead");
        let mut live_agent = start("reap-test-live");
        std::thread::sleep(std::time::Duration::from_millis(400));
        // One noted by an app that's gone, one by this (running) process.
        std::fs::write(dir.join(format!("999999-{}", dead_agent.id())), b"").unwrap();
        std::fs::write(dir.join(format!("{}-{}", std::process::id(), live_agent.id())), b"").unwrap();
        let rows = processes();
        let in_group = |g: u32| rows.iter().filter(|r| r.group == g as i32).map(|r| (r.pid, is_agent(&r.command))).collect::<Vec<_>>();
        let (dead_group, live_group) = (in_group(dead_agent.id()), in_group(live_agent.id()));
        assert!(stop_orphans() >= 1);
        std::thread::sleep(std::time::Duration::from_millis(800));
        let _ = dead_agent.try_wait();
        for (pid, agent) in &dead_group {
            assert_eq!(alive(*pid), !agent, "the gone app's agent stops; what it started (pid {pid}) keeps running");
        }
        assert!(live_group.iter().all(|(pid, _)| alive(*pid)), "a running app's agent is left alone");
        assert!(!dir.join(format!("999999-{}", dead_agent.id())).exists());
        for (pid, _) in dead_group.iter().chain(&live_group) { unsafe { libc::kill(*pid, libc::SIGKILL); } }
        let _ = live_agent.kill();
        let (_, _) = (dead_agent.wait(), live_agent.wait());
    }

    #[test]
    fn a_stopped_agent_takes_its_chain_but_not_what_it_started() {
        // An "agent" (named like one) with an agent child and a dev-server child.
        let script = "exec -a codex-acp sh -c 'sh -c \"exec -a codex\\ app-server sleep 30\" & sleep 30 & wait'";
        let mut root = std::process::Command::new("bash").args(["-c", script]).spawn().unwrap();
        std::thread::sleep(std::time::Duration::from_millis(400));
        let rows = processes();
        let kids: Vec<&Row> = rows.iter().filter(|r| r.ppid == root.id() as i32).collect();
        let agent_kid = kids.iter().find(|r| is_agent(&r.command)).map(|r| r.pid);
        let other_kid = kids.iter().find(|r| !is_agent(&r.command)).map(|r| r.pid);
        stop_agent(root.id());
        let _ = root.wait();
        std::thread::sleep(std::time::Duration::from_millis(800));
        if let Some(pid) = agent_kid { assert!(!alive(pid), "the agent's own child stops with it"); }
        if let Some(pid) = other_kid {
            assert!(alive(pid), "a process the agent started keeps running");
            unsafe { libc::kill(pid, libc::SIGKILL); }
        }
    }
}
