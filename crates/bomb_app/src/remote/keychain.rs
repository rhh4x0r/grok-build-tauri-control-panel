//! Where this Mac keeps its device key for each server: the login keychain,
//! with a private file as the fallback when the keychain is unavailable
//! (headless runs, tests).

use std::path::PathBuf;

use bomb_link::Identity;

const SERVICE: &str = "sh.bombcode.server-device-key";

/// Set to keep device keys in plain files under that folder instead of the keychain (tests, headless runs).
const KEY_DIR_ENV: &str = "BOMB_KEY_DIR";

fn fallback_path(server: &str) -> Option<PathBuf> {
    if let Some(dir) = std::env::var_os(KEY_DIR_ENV) { return Some(PathBuf::from(dir).join(format!("{server}.key.json"))); }
    let home = std::env::var_os("HOME")?;
    Some(PathBuf::from(home).join(".grok/control-panel/servers").join(format!("{server}.key.json")))
}

pub fn save(server: &str, identity: &Identity) -> Result<(), String> {
    let bytes = serde_json::to_vec(identity).map_err(|e| e.to_string())?;
    #[cfg(target_os = "macos")]
    if std::env::var_os(KEY_DIR_ENV).is_none() && security_framework::passwords::set_generic_password(SERVICE, server, &bytes).is_ok() {
        return Ok(());
    }
    let path = fallback_path(server).ok_or("no home folder")?;
    std::fs::create_dir_all(path.parent().unwrap()).map_err(|e| e.to_string())?;
    std::fs::write(&path, &bytes).map_err(|e| e.to_string())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
    }
    Ok(())
}

pub fn load(server: &str) -> Option<Identity> {
    #[cfg(target_os = "macos")]
    if let Some(bytes) = std::env::var_os(KEY_DIR_ENV).is_none().then(|| security_framework::passwords::get_generic_password(SERVICE, server).ok()).flatten() {
        if let Ok(identity) = serde_json::from_slice(&bytes) { return Some(identity); }
    }
    serde_json::from_slice(&std::fs::read(fallback_path(server)?).ok()?).ok()
}

pub fn forget(server: &str) {
    #[cfg(target_os = "macos")]
    if std::env::var_os(KEY_DIR_ENV).is_none() { let _ = security_framework::passwords::delete_generic_password(SERVICE, server); }
    if let Some(path) = fallback_path(server) { let _ = std::fs::remove_file(path); }
}
