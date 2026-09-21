//! Read-only destination checks and explicit setup for a new isolated thread.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Readiness {
    Ready,
    NeedsGit,
    NeedsCommit,
}

pub async fn check(root: &str) -> Result<Readiness, String> {
    if !std::path::Path::new(root).is_absolute() || !std::path::Path::new(root).is_dir() {
        return Err("Choose an existing project folder.".into());
    }
    if !super::project_overview::repository_detected(std::path::Path::new(root)).await? {
        return Ok(Readiness::NeedsGit);
    }
    let output = tokio::process::Command::new("git")
        .args(["rev-parse", "--verify", "HEAD"])
        .current_dir(root)
        .output()
        .await
        .map_err(|e| e.to_string())?;
    if output.status.success() {
        Ok(Readiness::Ready)
    } else {
        Ok(Readiness::NeedsCommit)
    }
}

/// A newly named project gets its own repository, even inside another checkout.
pub async fn create(root: &std::path::Path) -> Result<(), String> {
    tokio::fs::create_dir(root)
        .await
        .map_err(|e| format!("Could not create project folder: {e}"))?;
    let output = tokio::process::Command::new("git")
        .args(["init", "--initial-branch=main"])
        .current_dir(root)
        .output()
        .await
        .map_err(|e| e.to_string())?;
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).trim().to_owned());
    }
    initialize(&root.to_string_lossy()).await
}

/// Explicit button action: no project files are staged or committed.
pub async fn initialize(root: &str) -> Result<(), String> {
    super::project_overview::initialize_repository(root).await?;
    if check(root).await? == Readiness::Ready {
        return Ok(());
    }
    let output = tokio::process::Command::new("git")
        .args([
            "-c",
            "user.name=Bomb Code",
            "-c",
            "user.email=local@bombcode.local",
            "-c",
            "commit.gpgsign=false",
            "-c",
            "core.hooksPath=/dev/null",
            "commit",
            "--allow-empty",
            "--only",
            "--no-verify",
            "-m",
            "Initialize project for Bomb Code",
        ])
        .current_dir(root)
        .output()
        .await
        .map_err(|e| e.to_string())?;
    if !output.status.success() {
        return Err(format!(
            "Could not create initial commit: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    Ok(())
}
#[cfg(test)]
mod tests {
    #[tokio::test]
    async fn create_uses_own_repository_and_does_not_overwrite_existing_folder() {
        let dir = tempfile::tempdir().unwrap();
        let parent = dir.path().join("parent");
        super::create(&parent).await.unwrap();
        let child = parent.join("child");
        super::create(&child).await.unwrap();
        assert!(child.join(".git").is_dir());
        assert!(super::create(&child).await.is_err());
        assert_eq!(
            super::check(child.to_str().unwrap()).await.unwrap(),
            super::Readiness::Ready
        );
    }
    #[tokio::test]
    async fn setup_keeps_staged_and_untracked_files_out_of_initial_commit() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().to_str().unwrap();
        assert_eq!(
            super::check(root).await.unwrap(),
            super::Readiness::NeedsGit
        );
        crate::services::project_overview::initialize_repository(root)
            .await
            .unwrap();
        std::fs::write(dir.path().join("staged.txt"), "keep").unwrap();
        std::fs::write(dir.path().join("untracked.txt"), "keep").unwrap();
        assert!(tokio::process::Command::new("git")
            .args(["add", "staged.txt"])
            .current_dir(root)
            .status()
            .await
            .unwrap()
            .success());
        assert_eq!(
            super::check(root).await.unwrap(),
            super::Readiness::NeedsCommit
        );
        super::initialize(root).await.unwrap();
        assert_eq!(super::check(root).await.unwrap(), super::Readiness::Ready);
        let tree = tokio::process::Command::new("git")
            .args(["ls-tree", "--name-only", "HEAD"])
            .current_dir(root)
            .output()
            .await
            .unwrap();
        assert!(tree.stdout.is_empty());
        let status = tokio::process::Command::new("git")
            .args(["status", "--porcelain"])
            .current_dir(root)
            .output()
            .await
            .unwrap();
        let text = String::from_utf8(status.stdout).unwrap();
        assert!(text.contains("A  staged.txt"));
        assert!(text.contains("?? untracked.txt"));
    }
}
