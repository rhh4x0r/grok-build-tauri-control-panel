//! Setting up a one-person server from the app, over the SSH access the person already has.
//!
//! Uses the system `ssh` and `scp`, so keys, `~/.ssh/config`, jump hosts and known_hosts all work as
//! they do in a terminal. SSH is used once; afterwards this Mac talks to the server's own port.

use std::path::PathBuf;
use std::process::Stdio;

use tokio::io::AsyncWriteExt;
use tokio::process::Command;

pub const DEFAULT_PORT: u16 = 7443;

/// `user@host`, `host`, or an `~/.ssh/config` alias. Never anything `ssh` could read as an option.
pub fn valid_target(target: &str) -> bool {
    !target.is_empty() && target.len() <= 255 && !target.starts_with('-')
        && target.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_' | '@' | ':' | '[' | ']'))
        && target.matches('@').count() <= 1
}

/// `host:port` as written into pairing links.
pub fn valid_public(public: &str) -> bool {
    match public.rsplit_once(':') {
        Some((host, port)) => !host.is_empty() && !host.starts_with('-') && host.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '[' | ']' | ':')) && port.parse::<u16>().is_ok_and(|p| p > 0),
        None => false,
    }
}

/// Which prebuilt `bombd` a server needs, from `uname -sm`.
pub fn binary_for(uname: &str) -> Result<&'static str, String> {
    let mut parts = uname.split_whitespace();
    match (parts.next(), parts.next()) {
        (Some("Linux"), Some("x86_64" | "amd64")) => Ok("bombd-linux-x86_64"),
        (Some("Linux"), Some("aarch64" | "arm64")) => Ok("bombd-linux-aarch64"),
        (Some("Linux"), Some(other)) => Err(format!("This server's processor ({other}) isn't supported yet.")),
        (Some(other), _) => Err(format!("The server runs {other}. Bomb Code servers need Linux.")),
        _ => Err("Could not tell what kind of machine the server is.".into()),
    }
}

/// Where the app looks for server builds: `$BOMBD_BINARIES`, else `~/.bombcode/server`.
pub fn local_binary(name: &str) -> Result<PathBuf, String> {
    let dir = std::env::var_os("BOMBD_BINARIES").map(PathBuf::from).or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".bombcode/server"))).ok_or("no home folder")?;
    let path = dir.join(name);
    if path.is_file() { Ok(path) } else { Err(format!("The server program for this machine isn't on this Mac yet. Download `{name}` from the project's latest build and put it in {}.", dir.display())) }
}

/// What runs on the server. Everything it needs is validated first, so nothing here is quoted user text.
pub fn setup_script(public: &str, port: u16) -> Result<String, String> {
    if !valid_public(public) { return Err("The server address should look like example.com:7443.".into()); }
    Ok(format!(r#"set -eu
mkdir -p "$HOME/.local/bin" "$HOME/.bombd" "$HOME/.config/systemd/user"
install -m 0755 "$HOME/.bombd-upload" "$HOME/.local/bin/bombd"
rm -f "$HOME/.bombd-upload"
# The service gets this login shell's PATH, so tools installed per user (mise, nvm, ~/.local/bin) are found.
BOMB_PATH="$HOME/.local/bin:$PATH"
cat > "$HOME/.config/systemd/user/bombd.service" <<UNIT
[Unit]
Description=Bomb Code server
After=network-online.target

[Service]
ExecStart=%h/.local/bin/bombd up --data %h/.bombd --listen 0.0.0.0:{port} --public {public}
Environment=PATH=$BOMB_PATH
Restart=on-failure
RestartSec=2

[Install]
WantedBy=default.target
UNIT
loginctl enable-linger "$(id -un)" >/dev/null 2>&1 || echo "BOMB_NOTE: To keep threads running after you log out of SSH, run once on the server: sudo loginctl enable-linger $(id -un)"
systemctl --user daemon-reload
systemctl --user enable bombd.service >/dev/null 2>&1
systemctl --user restart bombd.service
for tool in git node claude codex grok; do command -v "$tool" >/dev/null 2>&1 || echo "BOMB_MISSING: $tool"; done
if sudo -n ufw allow {port}/tcp >/dev/null 2>&1 || sudo -n firewall-cmd --permanent --add-port={port}/tcp >/dev/null 2>&1; then echo "BOMB_NOTE: Opened port {port} in the server's firewall."; else echo "BOMB_NOTE: Make sure TCP port {port} is open in the server's firewall (for ufw: sudo ufw allow {port}/tcp) and at your hosting provider."; fi
tries=0
until [ -S "$HOME/.bombd/core.sock" ] || [ "$tries" -ge 20 ]; do sleep 0.5; tries=$((tries+1)); done
echo "BOMB_LINK: $("$HOME/.local/bin/bombd" invite --data "$HOME/.bombd" --public {public} --user "$("$HOME/.local/bin/bombd" owner-name)")"
"#))
}

#[derive(Debug, Default, PartialEq)]
pub struct Outcome {
    pub link: Option<String>,
    /// Tools the server lacks (`git`, `node`, or an agent CLI).
    pub missing: Vec<String>,
    pub notes: Vec<String>,
}

pub fn read_output(output: &str) -> Outcome {
    let mut outcome = Outcome::default();
    for line in output.lines() {
        if let Some(link) = line.strip_prefix("BOMB_LINK: ") { outcome.link = Some(link.trim().to_string()); }
        if let Some(tool) = line.strip_prefix("BOMB_MISSING: ") { outcome.missing.push(tool.trim().to_string()); }
        if let Some(note) = line.strip_prefix("BOMB_NOTE: ") { outcome.notes.push(note.trim().to_string()); }
    }
    outcome
}

fn ssh(target: &str) -> Command {
    let mut command = Command::new("ssh");
    // Never hang on a password or host-key question the app cannot show.
    command.args(["-o", "BatchMode=yes", "-o", "ConnectTimeout=15", "--", target]);
    command
}

async fn run(mut command: Command, input: Option<&str>, what: &str) -> Result<String, String> {
    command.stdin(if input.is_some() { Stdio::piped() } else { Stdio::null() }).stdout(Stdio::piped()).stderr(Stdio::piped()).kill_on_drop(true);
    let mut child = command.spawn().map_err(|e| format!("Could not {what}: {e}"))?;
    if let (Some(input), Some(mut stdin)) = (input, child.stdin.take()) {
        stdin.write_all(input.as_bytes()).await.map_err(|e| e.to_string())?;
    }
    let output = tokio::time::timeout(std::time::Duration::from_secs(180), child.wait_with_output()).await.map_err(|_| format!("Timed out trying to {what}."))?.map_err(|e| e.to_string())?;
    if output.status.success() { Ok(String::from_utf8_lossy(&output.stdout).into_owned()) } else {
        let reason = String::from_utf8_lossy(&output.stderr).trim().to_string();
        Err(format!("Could not {what}. {}", if reason.contains("Permission denied") || reason.contains("Host key") { format!("SSH said: {reason}. Check that `ssh {}` works in Terminal first.", "<your server>") } else { reason }))
    }
}

/// Install and start the server, returning its pairing link and anything the person should know.
pub async fn install(target: &str, public: &str, progress: impl Fn(&str)) -> Result<Outcome, String> {
    if !valid_target(target) { return Err("The SSH login should look like you@example.com (or a name from your SSH config).".into()); }
    let port = public.rsplit_once(':').and_then(|(_, p)| p.parse::<u16>().ok()).ok_or("The server address should look like example.com:7443.")?;
    let script = setup_script(public, port)?;
    progress("Checking the server…");
    let uname = run({ let mut c = ssh(target); c.arg("uname -sm"); c }, None, "reach the server over SSH").await?;
    let binary = local_binary(binary_for(uname.trim())?)?;
    progress("Uploading the server program…");
    let mut scp = Command::new("scp");
    scp.args(["-q", "-o", "BatchMode=yes", "-o", "ConnectTimeout=15", "--"]).arg(&binary).arg(format!("{target}:.bombd-upload"));
    run(scp, None, "upload the server program").await?;
    progress("Starting it…");
    let output = run({ let mut c = ssh(target); c.arg("sh -s"); c }, Some(&script), "set up the server").await?;
    let outcome = read_output(&output);
    if outcome.link.as_deref().is_none_or(|l| !l.starts_with("bomb://pair?")) { return Err("The server started but did not return a pairing link. Check `systemctl --user status bombd` on the server.".into()); }
    Ok(outcome)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nothing_a_person_types_can_become_a_command_or_an_ssh_option() {
        for ok in ["max@203.0.113.7", "my-vps", "deploy@host.example.com", "max@[2001:db8::1]"] { assert!(valid_target(ok), "{ok}"); }
        for bad in ["", "-oProxyCommand=evil", "max@host; rm -rf ~", "host $(id)", "a@b@c", "host\nother", "max@host'"] { assert!(!valid_target(bad), "{bad}"); }
        for ok in ["example.com:7443", "203.0.113.7:7443", "[2001:db8::1]:7443"] { assert!(valid_public(ok), "{ok}"); }
        for bad in ["example.com", "example.com:0", "example.com:99999", "ex ample.com:1", "$(id):7443", "-x:7443", "a.com:74 43"] { assert!(!valid_public(bad), "{bad}"); assert!(setup_script(bad, 7443).is_err()); }
        let script = setup_script("example.com:7443", 7443).unwrap();
        assert!(script.contains("--listen 0.0.0.0:7443 --public example.com:7443"));
        assert!(script.starts_with("set -eu"));
    }

    #[test]
    fn the_right_build_is_chosen_and_the_servers_report_is_understood() {
        assert_eq!(binary_for("Linux x86_64").unwrap(), "bombd-linux-x86_64");
        assert_eq!(binary_for("Linux aarch64\n").unwrap(), "bombd-linux-aarch64");
        assert!(binary_for("Darwin arm64").unwrap_err().contains("need Linux"));
        assert!(binary_for("Linux riscv64").unwrap_err().contains("isn't supported"));
        let outcome = read_output("noise\nBOMB_MISSING: node\nBOMB_MISSING: codex\nBOMB_NOTE: Make sure TCP port 7443 is open.\nBOMB_LINK: bomb://pair?h=a:1&fp=x&s=y\n");
        assert_eq!(outcome.link.as_deref(), Some("bomb://pair?h=a:1&fp=x&s=y"));
        assert_eq!(outcome.missing, ["node", "codex"]);
        assert_eq!(outcome.notes.len(), 1);
    }
}
