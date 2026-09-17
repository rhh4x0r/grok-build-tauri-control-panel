//! Read-only provider support data, separate from project write permissions.
use std::path::{Path, PathBuf};

pub(crate) fn roots(provider: &str, home: &Path, configured_home: Option<PathBuf>) -> Vec<PathBuf> {
    let folder = match provider {
        "grok" => ".grok",
        "claude" => ".claude",
        "codex" => ".codex",
        _ => return Vec::new(),
    };
    let base = configured_home.unwrap_or_else(|| home.join(folder));
    let dirs: &[&str] = match provider {
        "grok" => &["bundled/skills", "skills", "plugins", "memory-v2"],
        "claude" => &["skills", "plugins"],
        "codex" => &["skills", "plugins", "memories"],
        _ => return Vec::new(),
    };
    let mut roots: Vec<_> = dirs.iter().map(|d| base.join(d)).collect();
    roots.push(home.join(".agents/skills"));
    if provider == "claude" {
        roots.push(base.join("CLAUDE.md"));
        if let Ok(projects) = std::fs::read_dir(base.join("projects")) {
            roots.extend(
                projects
                    .flatten()
                    .map(|project| project.path().join("memory")),
            );
        }
    } else if provider == "codex" {
        roots.push(base.join("AGENTS.md"));
    }
    roots
}

pub(crate) fn resolve(path: &Path, roots: &[PathBuf]) -> Option<PathBuf> {
    let canonical = path.canonicalize().ok()?;
    roots
        .iter()
        .filter_map(|root| root.canonicalize().ok())
        .any(|root| canonical == root || (root.is_dir() && canonical.starts_with(root)))
        .then_some(canonical)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn each_provider_allows_support_reads_but_not_credentials() {
        let dir = tempfile::tempdir().unwrap();
        for (provider, folder, skill, secret) in [
            (
                "grok",
                ".grok",
                "bundled/skills/imagine/SKILL.md",
                "auth.json",
            ),
            (
                "claude",
                ".claude",
                "projects/demo/memory/MEMORY.md",
                ".credentials.json",
            ),
            ("codex", ".codex", "skills/demo/SKILL.md", "auth.json"),
        ] {
            let base = dir.path().join(folder);
            let file = base.join(skill);
            std::fs::create_dir_all(file.parent().unwrap()).unwrap();
            std::fs::write(&file, "instructions").unwrap();
            let credential = base.join(secret);
            std::fs::write(&credential, "secret").unwrap();
            let allowed = roots(provider, dir.path(), None);
            assert!(resolve(&file, &allowed).is_some());
            assert!(resolve(&credential, &allowed).is_none());
            #[cfg(unix)]
            {
                let link = file.parent().unwrap().join("escape.md");
                std::os::unix::fs::symlink(&credential, &link).unwrap();
                assert!(resolve(&link, &allowed).is_none());
            }
        }
    }
}
