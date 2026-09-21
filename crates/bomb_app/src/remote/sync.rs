//! Getting a project onto a server, a copy back onto this Mac, and keeping the two in step.
//! All of it is plain Git history sent as bundles over the paired connection.

use std::path::{Path, PathBuf};

use bomb_core::services::project_sync::{self, BranchSync};
use serde::{Deserialize, Serialize};
use serde_json::json;

use super::RemoteCore;

/// Stored in the local settings table: which folder on this Mac is a copy of which server project.
pub const LINKS_KEY: &str = "project_links";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Link {
    pub local: String,
    /// Namespaced server root (`bomb-server://…`).
    pub server: String,
}

fn scratch(dir: &Path) -> Result<PathBuf, String> {
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    Ok(dir.join(format!("{}.bundle", uuid::Uuid::new_v4())))
}

async fn sha256_of(path: &Path) -> Result<(u64, String), String> {
    use sha2::Digest;
    let bytes = tokio::fs::read(path).await.map_err(|e| e.to_string())?;
    Ok((bytes.len() as u64, hex::encode(sha2::Sha256::digest(&bytes))))
}

/// Upload this Mac's project as a new project on the server. Returns its namespaced root.
pub async fn send_to_server(remote: &RemoteCore, local_root: &str, name: &str, temp: &Path) -> Result<String, String> {
    let bundle = scratch(temp)?;
    let result = async {
        project_sync::create_bundle(local_root, &[], &bundle).await?.ok_or("This project has no commits to send yet.")?;
        let stream = remote.client()?.upload(&bundle).await?;
        let path: String = remote.call("import_project", json!({ "name": name, "stream": stream })).await?;
        Ok(remote.root(&path))
    }
    .await;
    let _ = std::fs::remove_file(&bundle);
    result
}

/// Fetch whatever the server has that `have` lacks, verified, into a local file. `None` when nothing is new.
async fn fetch_bundle(remote: &RemoteCore, server_root: &str, have: Vec<String>, temp: &Path) -> Result<Option<PathBuf>, String> {
    let bundle = scratch(temp)?;
    let answer = remote.client()?.download("export_bundle", json!({ "root": remote.path_of(server_root), "have": have }), &bundle).await?;
    if answer["empty"].as_bool() == Some(true) { return Ok(None); }
    let (size, sha256) = sha256_of(&bundle).await?;
    if Some(size) != answer["size"].as_u64() || Some(sha256.as_str()) != answer["sha256"].as_str() {
        let _ = std::fs::remove_file(&bundle);
        return Err("The download arrived damaged. Try again.".into());
    }
    Ok(Some(bundle))
}

/// A normal local clone of a server project, for working offline.
pub async fn download_copy(remote: &RemoteCore, server_root: &str, target: &Path, temp: &Path) -> Result<(), String> {
    let bundle = fetch_bundle(remote, server_root, Vec::new(), temp).await?.ok_or("That project has no commits to download yet.")?;
    let result = project_sync::init_from_bundle(target, &bundle).await;
    let _ = std::fs::remove_file(&bundle);
    result
}

/// Send what the server lacks, then take what this Mac lacks. Nothing is overwritten on either side.
pub async fn sync(remote: &RemoteCore, local_root: &str, server_root: &str, temp: &Path) -> Result<String, String> {
    let server_path = remote.path_of(server_root).to_string();
    let server_tips: Vec<(String, String)> = remote.call("branch_tips", json!({ "root": server_path })).await?;
    let up: Vec<BranchSync> = {
        let bundle = scratch(temp)?;
        let have: Vec<String> = server_tips.iter().map(|(_, sha)| sha.clone()).collect();
        let sent = match project_sync::create_bundle(local_root, &have, &bundle).await? {
            Some(_) => {
                let stream = remote.client()?.upload(&bundle).await?;
                remote.call("import_bundle", json!({ "root": server_path, "stream": stream })).await?
            }
            None => Vec::new(),
        };
        let _ = std::fs::remove_file(&bundle);
        sent
    };
    let local_have: Vec<String> = project_sync::branch_tips(local_root).await?.into_iter().map(|(_, sha)| sha).collect();
    let down = match fetch_bundle(remote, server_root, local_have, temp).await? {
        Some(bundle) => {
            let taken = project_sync::import_bundle(local_root, &bundle).await;
            let _ = std::fs::remove_file(&bundle);
            taken?
        }
        None => Vec::new(),
    };
    Ok(format!("{} · {}", project_sync::summarize("on the server", &up), project_sync::summarize("on this Mac", &down)))
}
