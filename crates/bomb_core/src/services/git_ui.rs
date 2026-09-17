//! Read models and explicit file selection for the Git UI.
use grok_worktree::run_git;
use serde::{Deserialize, Serialize};
use std::path::Path;
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct FileChange {
    pub path: String,
    pub status: String,
    pub added: usize,
    pub removed: usize,
}
#[derive(Clone, Debug, Default)]
pub struct BranchChoices {
    pub current: String,
    pub base: String,
    pub branches: Vec<String>,
}
pub async fn branches(root: &str) -> Result<BranchChoices, String> {
    let root = Path::new(root);
    let current = run_git(root, &["branch", "--show-current"])
        .await
        .map_err(|e| e.to_string())?
        .trim()
        .to_string();
    let base = super::workspaces::default_branch(root).await?;
    let branches = run_git(
        root,
        &["for-each-ref", "--format=%(refname:short)", "refs/heads"],
    )
    .await
    .map_err(|e| e.to_string())?
    .lines()
    .map(str::to_owned)
    .collect();
    Ok(BranchChoices {
        current,
        base,
        branches,
    })
}
pub async fn changes(path: &Path, reference: &str) -> Result<Vec<FileChange>, String> {
    let names = run_git(
        path,
        &[
            "diff",
            "--no-renames",
            "--name-status",
            "-z",
            reference,
            "--",
        ],
    )
    .await
    .map_err(|e| e.to_string())?;
    let stats = run_git(
        path,
        &["diff", "--no-renames", "--numstat", "-z", reference, "--"],
    )
    .await
    .map_err(|e| e.to_string())?;
    let mut changes = Vec::new();
    let mut parts = names.split('\0').filter(|s| !s.is_empty());
    while let Some(status) = parts.next() {
        if let Some(path) = parts.next() {
            changes.push(FileChange {
                path: path.into(),
                status: status.into(),
                ..Default::default()
            });
        }
    }
    for line in stats.split('\0') {
        let mut fields = line.splitn(3, '\t');
        if let (Some(a), Some(d), Some(p)) = (fields.next(), fields.next(), fields.next()) {
            if let Some(f) = changes.iter_mut().find(|f| f.path == p) {
                f.added = a.parse().unwrap_or(0);
                f.removed = d.parse().unwrap_or(0);
            }
        }
    }
    let untracked = run_git(path, &["ls-files", "--others", "--exclude-standard", "-z"])
        .await
        .map_err(|e| e.to_string())?;
    for p in untracked.split('\0').filter(|s| !s.is_empty()) {
        if !changes.iter().any(|f| f.path == p) {
            changes.push(FileChange {
                path: p.into(),
                status: "?".into(),
                ..Default::default()
            });
        }
    }
    changes.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(changes)
}
pub async fn file_diff(root: &str, reference: &str, path: &str) -> Result<String, String> {
    let root = Path::new(root);
    let changes = changes(root, reference).await?;
    let file = changes
        .iter()
        .find(|f| f.path == path)
        .ok_or("File is no longer in this change list")?;
    if file.status == "?" {
        let resolved = tokio::fs::canonicalize(root.join(path))
            .await
            .map_err(|e| e.to_string())?;
        let canonical = tokio::fs::canonicalize(root)
            .await
            .map_err(|e| e.to_string())?;
        if !resolved.starts_with(canonical) {
            return Err("File points outside this checkout".into());
        }
        let meta = tokio::fs::metadata(&resolved)
            .await
            .map_err(|e| e.to_string())?;
        if meta.len() > 200_000 {
            return Ok("New file · too large for inline preview".into());
        }
        return match tokio::fs::read_to_string(resolved).await {
            Ok(s) if !s.contains('\0') => Ok(format!(
                "New file: {path}\n{}",
                s.lines()
                    .map(|l| format!("+{l}"))
                    .collect::<Vec<_>>()
                    .join("\n")
            )),
            _ => Ok("New binary file · preview unavailable".into()),
        };
    }
    run_git(
        root,
        &[
            "diff",
            "--no-ext-diff",
            "--no-color",
            "--no-renames",
            reference,
            "--",
            path,
        ],
    )
    .await
    .map_err(|e| e.to_string())
}
#[derive(Deserialize)]
pub struct CommitSelection {
    pub message: String,
    pub files: Vec<String>,
}
pub async fn commit_selected(path: &Path, value: &str) -> Result<String, String> {
    let selection: CommitSelection = serde_json::from_str(value).map_err(|e| e.to_string())?;
    if selection.message.trim().is_empty() || selection.files.is_empty() {
        return Err("Select files and enter a commit message".into());
    }
    let dirty = changes(path, "HEAD").await?;
    if selection
        .files
        .iter()
        .any(|p| !dirty.iter().any(|f| &f.path == p))
    {
        return Err("The file selection changed. Refresh and review it again.".into());
    }
    let mut args = vec!["add", "-A", "--"];
    args.extend(selection.files.iter().map(String::as_str));
    run_git(path, &args).await.map_err(|e| e.to_string())?;
    let mut args = vec!["commit", "--only", "-m", selection.message.trim(), "--"];
    args.extend(selection.files.iter().map(String::as_str));
    run_git(path, &args).await.map_err(|e| e.to_string())?;
    Ok(format!(
        "Committed {} selected files",
        selection.files.len()
    ))
}
#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn selection_preserves_unselected_staged_changes() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path();
        for args in [
            vec!["init", "-b", "trunk"],
            vec!["config", "user.name", "Test"],
            vec!["config", "user.email", "test@example.com"],
            vec!["commit", "--allow-empty", "-m", "initial"],
        ] {
            run_git(p, &args).await.unwrap();
        }
        tokio::fs::write(p.join("one file.txt"), "one\n")
            .await
            .unwrap();
        tokio::fs::write(p.join("two.txt"), "two\n").await.unwrap();
        run_git(p, &["add", "two.txt"]).await.unwrap();
        commit_selected(p, r#"{"message":"Selected file","files":["one file.txt"]}"#)
            .await
            .unwrap();
        assert_eq!(
            run_git(p, &["diff", "--cached", "--name-only"])
                .await
                .unwrap()
                .trim(),
            "two.txt"
        );
        assert_eq!(
            run_git(p, &["show", "--format=", "--name-only", "HEAD"])
                .await
                .unwrap()
                .trim(),
            "one file.txt"
        );
        assert!(
            commit_selected(p, r#"{"message":"Invalid","files":["../outside"]}"#)
                .await
                .is_err()
        );
        assert_eq!(branches(p.to_str().unwrap()).await.unwrap().base, "trunk");
    }
}
