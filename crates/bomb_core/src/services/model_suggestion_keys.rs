//! Routing credentials live only in the app's SQLite settings table. Keychain
//! is accessed once to migrate older installations, never on the request path.
use grok_persistence::Persistence;
use std::sync::{Arc, Mutex};

static MUTATION: Mutex<()> = Mutex::new(());
#[cfg(target_os = "macos")]
const LEGACY_SERVICE: &str = "Bomb Code model suggestions";
fn slot(connection: &str) -> String {
    format!("settings/credentials/jev/{connection}")
}
fn valid(key: &str) -> bool {
    !key.trim().is_empty() && !key.contains(['\r', '\n'])
}

fn protect_database(db: &Persistence) -> Result<(), String> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        for suffix in ["", "-wal", "-shm"] {
            let mut name = db.path().as_os_str().to_os_string();
            name.push(suffix);
            let path = std::path::PathBuf::from(name);
            match std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound && !suffix.is_empty() => {}
                Err(_) => {
                    return Err(
                        "Could not restrict credential database access to your account.".into(),
                    )
                }
            }
        }
    }
    #[cfg(not(unix))]
    let _ = db;
    Ok(())
}

pub async fn save_key(db: Arc<Persistence>, connection: String, key: String) -> Result<(), String> {
    super::endpoint(&connection)?;
    if !valid(&key) {
        return Err("Enter a valid API key.".into());
    }
    tokio::task::spawn_blocking(move || {
        let _guard = MUTATION
            .lock()
            .map_err(|_| "Credential update unavailable.")?;
        protect_database(&db)?;
        db.set_kv(&slot(&connection), key.trim())
            .map_err(|_| "Could not save the key in the settings database.".to_string())
    })
    .await
    .map_err(|_| "Credential update failed.".to_string())?
}
pub async fn remove_key(db: Arc<Persistence>, connection: String) -> Result<(), String> {
    super::endpoint(&connection)?;
    tokio::task::spawn_blocking(move || {
        let _guard = MUTATION
            .lock()
            .map_err(|_| "Credential update unavailable.")?;
        // A tombstone also prevents importing an old key after the user removes it.
        db.set_kv(&slot(&connection), "")
            .map_err(|_| "Could not remove the saved key.".to_string())
    })
    .await
    .map_err(|_| "Credential update failed.".to_string())?
}
pub(super) async fn load_key(db: Arc<Persistence>, connection: String) -> Result<String, String> {
    let (_, _, env) = super::endpoint(&connection)?;
    tokio::task::spawn_blocking(move || {
        if let Some(key) = db
            .get_kv(&slot(&connection))
            .map_err(|_| "Could not read the settings database.")?
        {
            if valid(&key) {
                return Ok(key);
            }
        }
        std::env::var(env)
            .ok()
            .filter(|s| valid(s))
            .ok_or_else(|| "Add a routing API key in Settings → Model suggestions.".into())
    })
    .await
    .map_err(|_| "Credential read failed.".to_string())?
}

/// Copy legacy credentials durably before removing the old Keychain item.
/// Failures leave the original key intact. Existing SQLite values always win.
pub async fn migrate_legacy_key(db: Arc<Persistence>, connection: String) -> Result<bool, String> {
    super::endpoint(&connection)?;
    tokio::task::spawn_blocking(move || {
        let _guard=MUTATION.lock().map_err(|_|"Credential migration unavailable.")?;
        protect_database(&db)?;
        let marker=format!("{}/legacy-migrated",slot(&connection));
        if db.get_kv(&marker).map_err(|_|"Could not read migration status.")?.is_some() {return Ok(false);}
        #[cfg(target_os = "macos")]
        {
            let mut imported=false;
            if db.get_kv(&slot(&connection)).map_err(|_|"Could not read saved credentials.")?.is_none() {
                match security_framework::passwords::get_generic_password(LEGACY_SERVICE,&connection) {
                    Ok(bytes)=>{
                        let key=String::from_utf8(bytes).map_err(|_|"Invalid legacy routing key.")?;
                        if !valid(&key) {return Err("Invalid legacy routing key.".into());}
                        db.set_kv(&slot(&connection),&key).map_err(|_|"Could not migrate the key; the original remains in Keychain.")?;
                        if db.get_kv(&slot(&connection)).map_err(|_|"Could not verify migrated key.")?.as_deref()!=Some(&key) {
                            return Err("Could not verify migrated key; the original remains in Keychain.".into());
                        }
                        imported=true;
                    }
                    Err(error) if error.code()==-25300=>{},
                    Err(error)=>return Err(format!("Could not import the previous key from Keychain ({}). The original key is unchanged.",error.code())),
                }
            }
            match security_framework::passwords::delete_generic_password(LEGACY_SERVICE,&connection) {
                Ok(())=>{},Err(error) if error.code()==-25300=>{},
                Err(_)=>return Err("The SQLite key is ready, but the old Keychain item could not be removed.".into()),
            }
            db.set_kv(&marker,"true").map_err(|_|"Could not save migration status.")?;
            Ok(imported)
        }
        #[cfg(not(target_os = "macos"))]
        {db.set_kv(&marker,"true").map_err(|_|"Could not save migration status.")?;Ok(false)}
    }).await.map_err(|_|"Credential migration failed.".to_string())?
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn credentials_survive_reopen_and_replace_remove_without_keychain() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.db");
        let db = Arc::new(Persistence::open(&path).unwrap());
        save_key(db.clone(), "vercel".into(), "first-test-value".into())
            .await
            .unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
        drop(db);
        let db = Arc::new(Persistence::open(&path).unwrap());
        assert_eq!(
            load_key(db.clone(), "vercel".into()).await.unwrap(),
            "first-test-value"
        );
        save_key(db.clone(), "vercel".into(), "replacement-test-value".into())
            .await
            .unwrap();
        assert_eq!(
            load_key(db.clone(), "vercel".into()).await.unwrap(),
            "replacement-test-value"
        );
        assert!(save_key(db.clone(), "vercel".into(), "\n".into())
            .await
            .is_err());
        assert_eq!(
            load_key(db.clone(), "vercel".into()).await.unwrap(),
            "replacement-test-value"
        );
        remove_key(db.clone(), "vercel".into()).await.unwrap();
        assert_eq!(db.get_kv(&slot("vercel")).unwrap(), Some(String::new()));
    }
}
