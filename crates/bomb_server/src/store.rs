//! The gateway's small on-disk registry: people, their devices, and open invites.
//!
//! One JSON file, rewritten atomically under a file lock, so the running
//! gateway and the `bombd` command line can both change it safely. It never
//! holds a pairing secret, only its hash.

use std::fs::{File, OpenOptions};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

pub const INVITE_TTL_SECS: u64 = 10 * 60;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Registry {
    #[serde(default)]
    pub users: Vec<User>,
    #[serde(default)]
    pub devices: Vec<Device>,
    #[serde(default)]
    pub invites: Vec<Invite>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct User {
    pub name: String,
    /// Unix socket of this person's core.
    pub socket: PathBuf,
    #[serde(default)]
    pub admin: bool,
    #[serde(default)]
    pub locked: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Device {
    pub id: String,
    pub label: String,
    pub user: String,
    pub fingerprint: String,
    pub created: u64,
    #[serde(default)]
    pub last_seen: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Invite {
    pub secret_hash: String,
    pub user: String,
    pub expires: u64,
}

pub fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

pub struct Store {
    dir: PathBuf,
}

impl Store {
    pub fn open(dir: &Path) -> std::io::Result<Self> {
        std::fs::create_dir_all(dir)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
        }
        Ok(Self { dir: dir.to_path_buf() })
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    fn path(&self) -> PathBuf {
        self.dir.join("registry.json")
    }

    fn lock(&self) -> std::io::Result<File> {
        let file = OpenOptions::new().create(true).truncate(false).write(true).open(self.dir.join("registry.lock"))?;
        file.lock()?;
        Ok(file)
    }

    fn read_unlocked(&self) -> Registry {
        std::fs::read(self.path()).ok().and_then(|bytes| serde_json::from_slice(&bytes).ok()).unwrap_or_default()
    }

    pub fn read(&self) -> std::io::Result<Registry> {
        let _lock = self.lock()?;
        Ok(self.read_unlocked())
    }

    /// Read, change and write back as one step. Expired invites are swept on every change.
    pub fn update<T>(&self, change: impl FnOnce(&mut Registry) -> T) -> std::io::Result<T> {
        let _lock = self.lock()?;
        let mut registry = self.read_unlocked();
        let result = change(&mut registry);
        let time = now();
        registry.invites.retain(|i| i.expires > time);
        let temp = self.dir.join("registry.json.tmp");
        std::fs::write(&temp, serde_json::to_vec_pretty(&registry)?)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&temp, std::fs::Permissions::from_mode(0o600))?;
        }
        std::fs::rename(&temp, self.path())?;
        Ok(result)
    }
}
