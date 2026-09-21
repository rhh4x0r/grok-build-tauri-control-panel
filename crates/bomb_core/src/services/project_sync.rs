//! Moving a project's Git history between a Mac and a server.
//!
//! Both sides are ordinary clones. History travels as a `git bundle` over the
//! paired connection, so no SSH or Git server is involved. Importing never
//! resets or overwrites: incoming branches land under `refs/bomb/incoming/`,
//! then each local branch is created, fast-forwarded, or left alone and
//! reported as diverged for a person or an agent to merge.

use std::path::{Path, PathBuf};

use grok_worktree::run_git;
use serde::{Deserialize, Serialize};

const INCOMING: &str = "refs/bomb/incoming";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Outcome {
    /// The branch did not exist here and was created.
    Created,
    /// Moved forward to the incoming commit.
    Updated,
    /// Already identical.
    Same,
    /// This side is ahead; nothing to take.
    Ahead,
    /// Both sides have their own commits. The incoming copy is kept for merging.
    Diverged,
    /// Could move forward, but the folder that has it checked out has unsaved edits.
    Blocked,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BranchSync {
    pub branch: String,
    pub outcome: Outcome,
}

/// Every local branch and the commit it points at.
pub async fn branch_tips(root: &str) -> Result<Vec<(String, String)>, String> {
    let out = run_git(Path::new(root), &["for-each-ref", "--format=%(refname:short) %(objectname)", "refs/heads/"]).await.map_err(|e| e.to_string())?;
    Ok(out.lines().filter_map(|l| l.split_once(' ')).map(|(b, sha)| (b.to_string(), sha.to_string())).collect())
}

/// Bundle every branch, leaving out history the other side already has.
/// `have` is the other side's commit ids; unknown ones are ignored. `Ok(None)` means nothing new to send.
pub async fn create_bundle(root: &str, have: &[String], out: &Path) -> Result<Option<PathBuf>, String> {
    let path = Path::new(root);
    let tips = branch_tips(root).await?;
    if tips.is_empty() { return Err("This project has no commits yet.".into()); }
    let mut excludes = Vec::new();
    for sha in have {
        if sha.len() >= 7 && sha.bytes().all(|b| b.is_ascii_hexdigit()) && run_git(path, &["cat-file", "-e", &format!("{sha}^{{commit}}")]).await.is_ok() {
            excludes.push(format!("^{sha}"));
        }
    }
    // Nothing to say if the other side already has every tip.
    if tips.iter().all(|(_, sha)| have.contains(sha)) { return Ok(None); }
    let out_text = out.display().to_string();
    // HEAD travels too, so a fresh copy checks out the same branch instead of an empty default one.
    let mut args: Vec<&str> = vec!["bundle", "create", &out_text, "HEAD", "--branches"];
    args.extend(excludes.iter().map(String::as_str));
    match run_git(path, &args).await {
        Ok(_) => Ok(Some(out.to_path_buf())),
        Err(e) if e.to_string().contains("empty bundle") => Ok(None),
        Err(e) => Err(format!("Could not package the project’s history: {e}")),
    }
}

/// Create a new project folder from a complete bundle.
pub async fn init_from_bundle(target: &Path, bundle: &Path) -> Result<(), String> {
    if target.exists() { return Err(format!("{} already exists.", target.display())); }
    let parent = target.parent().ok_or("bad project path")?;
    std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    let (bundle_text, target_text) = (bundle.display().to_string(), target.display().to_string());
    run_git(parent, &["clone", "--quiet", &bundle_text, &target_text]).await.map_err(|e| format!("Could not unpack the project: {e}"))?;
    // Every branch becomes a local branch; the bundle is not a remote worth keeping.
    let remote_branches = run_git(target, &["for-each-ref", "--format=%(refname:short)", "refs/remotes/origin/"]).await.map_err(|e| e.to_string())?;
    for name in remote_branches.lines().filter_map(|l| l.strip_prefix("origin/")).filter(|b| *b != "HEAD") {
        if run_git(target, &["show-ref", "--verify", "--quiet", &format!("refs/heads/{name}")]).await.is_err() {
            run_git(target, &["branch", "--quiet", name, &format!("origin/{name}")]).await.map_err(|e| e.to_string())?;
        }
    }
    run_git(target, &["remote", "remove", "origin"]).await.map_err(|e| e.to_string())?;
    // A bundle made while no branch was checked out leaves the copy on an unborn branch: pick a real one.
    if run_git(target, &["rev-parse", "--verify", "--quiet", "HEAD"]).await.is_err() {
        let branches = branch_tips(&target_text).await?;
        let pick = ["main", "master"].iter().find(|n| branches.iter().any(|(b, _)| b == *n)).map(|n| n.to_string()).or_else(|| branches.first().map(|(b, _)| b.clone()));
        if let Some(branch) = pick { run_git(target, &["checkout", "--quiet", &branch]).await.map_err(|e| e.to_string())?; }
    }
    Ok(())
}

/// Take what a bundle offers without ever rewriting this side's work.
pub async fn import_bundle(root: &str, bundle: &Path) -> Result<Vec<BranchSync>, String> {
    let path = Path::new(root);
    let bundle_text = bundle.display().to_string();
    run_git(path, &["bundle", "verify", "--quiet", &bundle_text]).await.map_err(|_| "That history does not connect to this copy of the project. Sync the other way first, or download a fresh copy.".to_string())?;
    // Clear the staging area so a branch deleted on the other side is not offered again.
    for stale in run_git(path, &["for-each-ref", "--format=%(refname)", INCOMING]).await.unwrap_or_default().lines() {
        let _ = run_git(path, &["update-ref", "-d", stale]).await;
    }
    run_git(path, &["fetch", "--quiet", &bundle_text, &format!("+refs/heads/*:{INCOMING}/*")]).await.map_err(|e| format!("Could not read the incoming history: {e}"))?;

    let checked_out = checkouts(path).await;
    let listing = run_git(path, &["for-each-ref", "--format=%(refname) %(objectname)", INCOMING]).await.map_err(|e| e.to_string())?;
    let mut results = Vec::new();
    for line in listing.lines() {
        let Some((reference, incoming)) = line.split_once(' ') else { continue };
        let Some(branch) = reference.strip_prefix(&format!("{INCOMING}/")) else { continue };
        let local_ref = format!("refs/heads/{branch}");
        let local = run_git(path, &["rev-parse", "--verify", "--quiet", &local_ref]).await.ok().map(|s| s.trim().to_string());
        let ancestor = |a: String, b: String| async move { run_git(path, &["merge-base", "--is-ancestor", &a, &b]).await.is_ok() };
        let outcome = match local {
            None => {
                run_git(path, &["update-ref", &local_ref, incoming]).await.map_err(|e| e.to_string())?;
                Outcome::Created
            }
            Some(local) if local == incoming => Outcome::Same,
            Some(local) if ancestor(incoming.to_string(), local.clone()).await => Outcome::Ahead,
            Some(local) if ancestor(local.clone(), incoming.to_string()).await => match checked_out.iter().find(|(b, _)| b == branch) {
                // Checked out somewhere: move the files too, and only when nothing unsaved is in the way.
                Some((_, folder)) => {
                    let clean = run_git(folder, &["status", "--porcelain", "--untracked-files=no"]).await.map(|s| s.trim().is_empty()).unwrap_or(false);
                    if clean && run_git(folder, &["merge", "--ff-only", "--quiet", incoming]).await.is_ok() { Outcome::Updated } else { Outcome::Blocked }
                }
                None => {
                    run_git(path, &["update-ref", &local_ref, incoming, &local]).await.map_err(|e| e.to_string())?;
                    Outcome::Updated
                }
            },
            Some(_) => Outcome::Diverged,
        };
        // Keep the incoming copy only where someone still has to merge it.
        if !matches!(outcome, Outcome::Diverged | Outcome::Blocked) {
            let _ = run_git(path, &["update-ref", "-d", reference]).await;
        }
        results.push(BranchSync { branch: branch.to_string(), outcome });
    }
    results.sort_by(|a, b| a.branch.cmp(&b.branch));
    Ok(results)
}

/// Branches that are checked out, and where (the project folder or a thread's folder).
async fn checkouts(path: &Path) -> Vec<(String, PathBuf)> {
    let out = run_git(path, &["worktree", "list", "--porcelain"]).await.unwrap_or_default();
    let mut found = Vec::new();
    let mut folder: Option<PathBuf> = None;
    for line in out.lines() {
        if let Some(p) = line.strip_prefix("worktree ") { folder = Some(PathBuf::from(p)); }
        if let (Some(b), Some(f)) = (line.strip_prefix("branch refs/heads/"), &folder) { found.push((b.to_string(), f.clone())); }
    }
    found
}

/// One line a person can read after a sync.
pub fn summarize(direction: &str, results: &[BranchSync]) -> String {
    let count = |o: Outcome| results.iter().filter(|r| r.outcome == o).count();
    let taken = count(Outcome::Created) + count(Outcome::Updated);
    let mut parts = vec![match taken { 0 => format!("Nothing new {direction}"), 1 => format!("1 branch updated {direction}"), n => format!("{n} branches updated {direction}") }];
    let names = |o: Outcome| results.iter().filter(|r| r.outcome == o).map(|r| r.branch.as_str()).collect::<Vec<_>>().join(", ");
    if count(Outcome::Diverged) > 0 { parts.push(format!("changed on both sides, needs a merge: {}", names(Outcome::Diverged))); }
    if count(Outcome::Blocked) > 0 { parts.push(format!("waiting on unsaved edits: {}", names(Outcome::Blocked))); }
    parts.join(" · ")
}

/// Clone a repository from a URL into `target` (GitHub or anything `git clone` understands).
pub async fn clone_url(url: &str, target: &Path) -> Result<(), String> {
    let allowed = ["https://", "ssh://", "git@"].iter().any(|p| url.starts_with(p));
    if !allowed || url.contains(char::is_whitespace) || url.starts_with('-') { return Err("Use an https:// or SSH Git address.".into()); }
    if target.exists() { return Err(format!("{} already exists.", target.display())); }
    let parent = target.parent().ok_or("bad project path")?;
    std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    let target_text = target.display().to_string();
    run_git(parent, &["clone", "--quiet", "--", url, &target_text]).await.map_err(|e| format!("Could not clone that repository. If it is private, sign in to GitHub on the server first. ({e})"))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn repo(dir: &Path) {
        std::fs::create_dir_all(dir).unwrap();
        for args in [vec!["init", "-q", "-b", "main"], vec!["config", "user.name", "Test"], vec!["config", "user.email", "test@example.com"]] { run_git(dir, &args).await.unwrap(); }
    }
    async fn commit(dir: &Path, file: &str, text: &str, message: &str) {
        std::fs::write(dir.join(file), text).unwrap();
        run_git(dir, &["add", "-A"]).await.unwrap();
        run_git(dir, &["commit", "-q", "-m", message]).await.unwrap();
    }
    async fn tips_of(dir: &Path) -> Vec<String> {
        branch_tips(dir.to_str().unwrap()).await.unwrap().into_iter().map(|(_, sha)| sha).collect()
    }
    fn outcome(results: &[BranchSync], branch: &str) -> Outcome {
        results.iter().find(|r| r.branch == branch).map(|r| r.outcome.clone()).unwrap_or_else(|| panic!("no result for {branch}: {results:?}"))
    }

    #[tokio::test]
    async fn history_moves_both_ways_incrementally_and_never_overwrites_work() {
        let temp = tempfile::tempdir().unwrap();
        let mac = temp.path().join("mac");
        let server = temp.path().join("server/projects/game");
        let bundle = temp.path().join("b.bundle");
        repo(&mac).await;
        commit(&mac, "a.txt", "one\n", "first").await;
        run_git(&mac, &["branch", "feature"]).await.unwrap();

        // Mac → server: the whole project the first time.
        let (mac_s, server_s) = (mac.to_str().unwrap().to_string(), server.to_str().unwrap().to_string());
        create_bundle(&mac_s, &[], &bundle).await.unwrap().unwrap();
        init_from_bundle(&server, &bundle).await.unwrap();
        for args in [vec!["config", "user.name", "Server"], vec!["config", "user.email", "server@example.com"]] { run_git(&server, &args).await.unwrap(); }
        let mut names: Vec<_> = branch_tips(&server_s).await.unwrap().into_iter().map(|(b, _)| b).collect();
        names.sort();
        assert_eq!(names, ["feature", "main"]);
        assert!(run_git(&server, &["remote"]).await.unwrap().trim().is_empty(), "the bundle is not kept as a remote");
        assert!(init_from_bundle(&server, &bundle).await.is_err(), "never unpack over an existing folder");

        // Nothing new: no bundle at all.
        assert!(create_bundle(&mac_s, &tips_of(&server).await, &bundle).await.unwrap().is_none());

        // Work happens on the server (a thread branch plus main), then comes back as a small bundle.
        commit(&server, "b.txt", "server\n", "server work on main").await;
        run_git(&server, &["checkout", "-q", "-b", "bomb/thread-1"]).await.unwrap();
        commit(&server, "c.txt", "thread\n", "thread work").await;
        run_git(&server, &["checkout", "-q", "main"]).await.unwrap();
        std::fs::remove_file(&bundle).ok();
        create_bundle(&server_s, &tips_of(&mac).await, &bundle).await.unwrap().unwrap();
        let back = import_bundle(&mac_s, &bundle).await.unwrap();
        assert_eq!(outcome(&back, "main"), Outcome::Updated);
        assert_eq!(outcome(&back, "bomb/thread-1"), Outcome::Created);
        assert!(back.iter().all(|r| r.branch != "feature"), "a branch with nothing new is not even sent");
        assert!(mac.join("b.txt").exists(), "main is checked out on the Mac, so its files moved too");
        assert_eq!(summarize("from the server", &back), "2 branches updated from the server");

        // Both sides change main: reported, kept for merging, nothing overwritten.
        commit(&mac, "a.txt", "mac edit\n", "mac work").await;
        commit(&server, "a.txt", "server edit\n", "more server work").await;
        let mac_main = run_git(&mac, &["rev-parse", "main"]).await.unwrap();
        std::fs::remove_file(&bundle).ok();
        create_bundle(&server_s, &tips_of(&mac).await, &bundle).await.unwrap().unwrap();
        let diverged = import_bundle(&mac_s, &bundle).await.unwrap();
        assert_eq!(outcome(&diverged, "main"), Outcome::Diverged);
        assert_eq!(run_git(&mac, &["rev-parse", "main"]).await.unwrap(), mac_main, "local main untouched");
        assert!(run_git(&mac, &["rev-parse", "--verify", "refs/bomb/incoming/main"]).await.is_ok(), "the server's main is kept for a merge");
        assert!(summarize("from the server", &diverged).contains("needs a merge: main"));

        // The other direction sees the Mac as ahead only after a merge; until then it is diverged there too.
        std::fs::remove_file(&bundle).ok();
        create_bundle(&mac_s, &tips_of(&server).await, &bundle).await.unwrap().unwrap();
        assert_eq!(outcome(&import_bundle(&server_s, &bundle).await.unwrap(), "main"), Outcome::Diverged);

        // Unsaved edits in the checked-out folder block a fast-forward instead of being clobbered.
        run_git(&mac, &["merge", "-q", "-X", "ours", "refs/bomb/incoming/main", "-m", "merge server"]).await.unwrap();
        std::fs::remove_file(&bundle).ok();
        create_bundle(&mac_s, &tips_of(&server).await, &bundle).await.unwrap().unwrap();
        std::fs::write(server.join("a.txt"), "unsaved on server\n").unwrap();
        assert_eq!(outcome(&import_bundle(&server_s, &bundle).await.unwrap(), "main"), Outcome::Blocked);
        assert_eq!(std::fs::read_to_string(server.join("a.txt")).unwrap(), "unsaved on server\n");
        run_git(&server, &["checkout", "--", "a.txt"]).await.unwrap();
        assert_eq!(outcome(&import_bundle(&server_s, &bundle).await.unwrap(), "main"), Outcome::Updated);

        // History from an unrelated project is refused with a plain explanation.
        let other = temp.path().join("other");
        repo(&other).await;
        commit(&other, "z.txt", "z\n", "unrelated").await;
        commit(&other, "z.txt", "zz\n", "unrelated 2").await;
        std::fs::remove_file(&bundle).ok();
        let first = run_git(&other, &["rev-list", "--max-parents=0", "HEAD"]).await.unwrap().trim().to_string();
        create_bundle(other.to_str().unwrap(), &[first], &bundle).await.unwrap().unwrap();
        assert!(import_bundle(&mac_s, &bundle).await.unwrap_err().contains("does not connect"));
    }

    #[tokio::test]
    async fn only_real_git_addresses_are_cloned() {
        let temp = tempfile::tempdir().unwrap();
        for bad in ["--upload-pack=touch /tmp/x", "file:///etc", "/local/path", "https://a b"] {
            assert!(clone_url(bad, &temp.path().join("x")).await.unwrap_err().contains("https://"), "{bad}");
        }
    }
}
