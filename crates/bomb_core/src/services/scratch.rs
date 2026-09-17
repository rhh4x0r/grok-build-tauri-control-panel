//! Private, empty project backing a folder-free chat.
use std::path::{Path, PathBuf};

pub async fn create(base: &Path) -> Result<PathBuf, String> {
    let path = base.join(uuid::Uuid::new_v4().to_string());
    tokio::fs::create_dir_all(&path).await.map_err(|e| e.to_string())?;
    for args in [
        vec!["init", "--template=", "--initial-branch=main"],
        vec!["-c", "user.name=Bomb Code", "-c", "user.email=scratch@bombcode.local", "-c", "commit.gpgsign=false", "-c", "core.hooksPath=/dev/null", "commit", "--allow-empty", "--no-verify", "-m", "Initialize scratch chat"],
    ] {
        let output = tokio::process::Command::new("git").args(args).current_dir(&path).output().await.map_err(|e| e.to_string())?;
        if !output.status.success() { return Err(String::from_utf8_lossy(&output.stderr).trim().to_string()); }
    }
    Ok(path)
}

#[cfg(test)]
mod tests {
    #[tokio::test]
    async fn scratch_has_an_empty_main_branch_without_modifying_parent() {
        let base = tempfile::tempdir().unwrap();
        let path = super::create(base.path()).await.unwrap();
        assert!(path.starts_with(base.path()));
        assert!(!base.path().join(".git").exists());
        let output = tokio::process::Command::new("git").args(["ls-tree", "--name-only", "HEAD"]).current_dir(&path).output().await.unwrap();
        assert!(output.status.success());
        assert!(output.stdout.is_empty());
    }
}
