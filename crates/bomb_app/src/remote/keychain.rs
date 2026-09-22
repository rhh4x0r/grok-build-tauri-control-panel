//! Where this device keeps its key for each server: a private file, like an SSH key.
//!
//! `~/.grok/control-panel/servers/<server>.key.json`, readable only by the owner. No keychain: it
//! prompted for a password on every rebuild, and it does not exist on Linux or Windows. Set
//! `BOMB_KEY_DIR` to keep the files elsewhere (tests).

use std::path::PathBuf;

use bomb_link::Identity;

fn dir() -> Option<PathBuf> {
    if let Some(dir) = std::env::var_os("BOMB_KEY_DIR") { return Some(PathBuf::from(dir)); }
    let home = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE"))?;
    Some(PathBuf::from(home).join(".grok/control-panel/servers"))
}

fn path(server: &str) -> Option<PathBuf> {
    // The id is ours (`s` + hex), never a path.
    let safe: String = server.chars().filter(|c| c.is_ascii_alphanumeric()).collect();
    Some(dir()?.join(format!("{safe}.key.json")))
}

pub fn save(server: &str, identity: &Identity) -> Result<(), String> {
    let path = path(server).ok_or("no home folder")?;
    let folder = path.parent().ok_or("bad key path")?;
    std::fs::create_dir_all(folder).map_err(|e| e.to_string())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(folder, std::fs::Permissions::from_mode(0o700));
    }
    let bytes = serde_json::to_vec(identity).map_err(|e| e.to_string())?;
    // Write next to it and rename, so a crash never leaves a half-written key.
    let temp = path.with_extension("tmp");
    std::fs::write(&temp, &bytes).map_err(|e| e.to_string())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&temp, std::fs::Permissions::from_mode(0o600)).map_err(|e| e.to_string())?;
    }
    std::fs::rename(&temp, &path).map_err(|e| e.to_string())
}

pub fn load(server: &str) -> Option<Identity> {
    if let Some(identity) = std::fs::read(path(server)?).ok().and_then(|bytes| serde_json::from_slice(&bytes).ok()) { return Some(identity); }
    // Keys saved by earlier builds went to the macOS keychain; read one back once and move it to the file.
    #[cfg(target_os = "macos")]
    {
        let out = std::process::Command::new("security").args(["find-generic-password", "-s", "sh.bombcode.server-device-key", "-a", server, "-w"]).output().ok()?;
        tracing::info!(ok = out.status.success(), "looking for this Mac's old key for {server} in the keychain");
        if out.status.success() {
            let hex = String::from_utf8_lossy(&out.stdout).trim().to_string();
            // `-w` prints non-text secrets as hex.
            let bytes = (0..hex.len()).step_by(2).filter_map(|i| u8::from_str_radix(hex.get(i..i + 2)?, 16).ok()).collect::<Vec<u8>>();
            let identity: Identity = match serde_json::from_slice(&bytes).or_else(|_| serde_json::from_str(&hex)) {
                Ok(identity) => identity,
                Err(error) => { tracing::warn!(%error, "could not read this Mac's old key for {server} from the keychain"); return None; }
            };
            let _ = save(server, &identity);
            return Some(identity);
        }
    }
    None
}

pub fn forget(server: &str) {
    if let Some(path) = path(server) { let _ = std::fs::remove_file(path); }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_round_trip_in_a_private_file_and_ids_cannot_escape_the_folder() {
        let temp = tempfile::tempdir().unwrap();
        std::env::set_var("BOMB_KEY_DIR", temp.path());
        let identity = Identity::generate("mac").unwrap();
        save("s1234abcd", &identity).unwrap();
        assert_eq!(load("s1234abcd").map(|i| i.cert_pem), Some(identity.cert_pem.clone()));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(std::fs::metadata(temp.path().join("s1234abcd.key.json")).unwrap().permissions().mode() & 0o777, 0o600);
        }
        save("../../etc/evil", &identity).unwrap();
        assert!(temp.path().join("etcevil.key.json").exists());
        forget("s1234abcd");
        assert!(load("s1234abcd").is_none());
    }
}
