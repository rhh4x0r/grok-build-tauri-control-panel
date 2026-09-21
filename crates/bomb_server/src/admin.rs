//! The only privileged surface on a shared server.
//!
//! The gateway runs unprivileged. To add a person it asks `sudo -n bombd admin
//! <verb> <name>`, where sudoers permits exactly that command for the gateway's
//! user. The helper accepts four verbs and one argument, a server-generated
//! account name, and refuses anything else. People never type these names.

use std::process::Command;

/// Account names the helper will touch: `bc-` plus 4–16 lowercase letters or digits.
pub fn valid_account(name: &str) -> bool {
    match name.strip_prefix("bc-") {
        Some(rest) => (4..=16).contains(&rest.len()) && rest.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit()),
        None => false,
    }
}

pub fn new_account_name() -> String {
    const ALPHABET: &[u8] = b"abcdefghijkmnpqrstuvwxyz23456789";
    let suffix: String = (0..8).map(|_| ALPHABET[rand::random::<usize>() % ALPHABET.len()] as char).collect();
    format!("bc-{suffix}")
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verb {
    CreateUser,
    LockUser,
    StartCore,
    StopCore,
}

impl Verb {
    pub fn parse(text: &str) -> Option<Self> {
        match text {
            "create-user" => Some(Self::CreateUser),
            "lock-user" => Some(Self::LockUser),
            "start-core" => Some(Self::StartCore),
            "stop-core" => Some(Self::StopCore),
            _ => None,
        }
    }
    pub fn as_str(self) -> &'static str {
        match self { Self::CreateUser => "create-user", Self::LockUser => "lock-user", Self::StartCore => "start-core", Self::StopCore => "stop-core" }
    }
}

/// The exact system commands a verb runs, as argv lists. Pure, so it can be checked without root.
pub fn plan(verb: Verb, account: &str) -> Result<Vec<Vec<String>>, String> {
    if !valid_account(account) { return Err("not a Bomb Code account name".into()); }
    let argv = |parts: &[&str]| parts.iter().map(|p| p.to_string()).collect::<Vec<_>>();
    let socket_unit = format!("bombd-core@{account}.socket");
    let service_unit = format!("bombd-core@{account}.service");
    Ok(match verb {
        // A home of their own that nobody else can read, no password, no login shell, no extra groups.
        Verb::CreateUser => vec![
            argv(&["useradd", "--create-home", "--shell", "/usr/sbin/nologin", "--user-group", "--comment", "Bomb Code user", account]),
            argv(&["chmod", "0700", &format!("/home/{account}")]),
        ],
        Verb::LockUser => vec![
            argv(&["systemctl", "stop", &socket_unit, &service_unit]),
            argv(&["systemctl", "disable", &socket_unit]),
            argv(&["usermod", "--lock", "--expiredate", "1", account]),
        ],
        Verb::StartCore => vec![argv(&["systemctl", "enable", "--now", &socket_unit])],
        Verb::StopCore => vec![argv(&["systemctl", "stop", &socket_unit, &service_unit])],
    })
}

/// Run as root by `bombd admin <verb> <account>`.
pub fn run(verb: &str, account: &str) -> Result<(), String> {
    let verb = Verb::parse(verb).ok_or("unknown admin verb")?;
    for argv in plan(verb, account)? {
        let status = Command::new(&argv[0]).args(&argv[1..]).env_clear().env("PATH", "/usr/sbin:/usr/bin:/sbin:/bin").status().map_err(|e| format!("{}: {e}", argv[0]))?;
        if !status.success() { return Err(format!("`{}` failed", argv.join(" "))); }
    }
    Ok(())
}

/// Where a person's core listens. systemd owns this socket; only the gateway's group can reach the folder.
pub fn socket_path(account: &str) -> std::path::PathBuf {
    std::path::PathBuf::from(format!("/run/bombd/{account}.sock"))
}

/// How the gateway asks for privileged work.
pub trait Privileged: Send + Sync {
    fn run(&self, verb: Verb, account: &str) -> Result<(), String>;
}

/// The real thing: `sudo -n <this binary> admin <verb> <account>`.
pub struct Sudo;

impl Privileged for Sudo {
    fn run(&self, verb: Verb, account: &str) -> Result<(), String> {
        if !valid_account(account) { return Err("not a Bomb Code account name".into()); }
        let binary = std::env::current_exe().map_err(|e| e.to_string())?;
        let output = Command::new("sudo").arg("-n").arg(binary).args(["admin", verb.as_str(), account]).output().map_err(|e| format!("could not run sudo: {e}"))?;
        if output.status.success() { Ok(()) } else { Err(format!("The server could not {}: {}", verb.as_str().replace('-', " "), String::from_utf8_lossy(&output.stderr).trim())) }
    }
}

/// Single-person installs have no privileged helper.
pub struct Unavailable;

impl Privileged for Unavailable {
    fn run(&self, _: Verb, _: &str) -> Result<(), String> {
        Err("This server is set up for one person. Reinstall it as a shared server to invite others.".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_generated_account_names_are_ever_passed_to_system_tools() {
        for _ in 0..50 { assert!(valid_account(&new_account_name())); }
        for bad in ["root", "bc-", "bc-abc", "bc-ABCDEF", "bc-abcd;rm", "bc-abcd efgh", "bc-../../etc", "-bc-abcd", "bc-abcdefghijklmnopq", "max"] {
            assert!(!valid_account(bad), "{bad}");
            for verb in [Verb::CreateUser, Verb::LockUser, Verb::StartCore, Verb::StopCore] { assert!(plan(verb, bad).is_err()); }
            assert!(run("create-user", bad).is_err());
        }
        assert!(run("delete-everything", "bc-abcd1234").is_err());
    }

    #[test]
    fn new_people_get_a_private_home_no_shell_and_no_extra_groups() {
        let steps = plan(Verb::CreateUser, "bc-abcd1234").unwrap();
        assert_eq!(steps[0], ["useradd", "--create-home", "--shell", "/usr/sbin/nologin", "--user-group", "--comment", "Bomb Code user", "bc-abcd1234"]);
        assert_eq!(steps[1], ["chmod", "0700", "/home/bc-abcd1234"]);
        assert!(!steps.iter().flatten().any(|a| a.contains("sudo") || a.contains("docker") || a == "--groups"));
        assert_eq!(plan(Verb::StartCore, "bc-abcd1234").unwrap(), [["systemctl", "enable", "--now", "bombd-core@bc-abcd1234.socket"]]);
        assert!(plan(Verb::LockUser, "bc-abcd1234").unwrap().iter().any(|s| s[0] == "usermod" && s.contains(&"--lock".to_string())));
        assert_eq!(socket_path("bc-abcd1234"), std::path::PathBuf::from("/run/bombd/bc-abcd1234.sock"));
    }
}
