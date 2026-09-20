//! A small Markdown envelope: structured identities/assignments, editable prose.
use super::{err, Feature, Record};
use std::{
    io::Write,
    path::{Path, PathBuf},
};
use uuid::Uuid;
const HEADER: &str = "<!-- bomb-feature/1\n";
const TASK: &str = "<!-- bomb-task:";

pub fn valid_id(id: &str) -> Result<(), String> {
    if id.len() < 3 || id.len() > 64 || !id.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'-')
    {
        return Err("Invalid feature or task identifier.".into());
    }
    Ok(())
}
pub fn safe_path(root: &Path, parts: &[&str]) -> Result<PathBuf, String> {
    let mut path = root.to_path_buf();
    for part in parts {
        if part.is_empty() || *part == "." || *part == ".." || part.contains(['/', '\\']) {
            return Err("Invalid project record path.".into());
        }
        path.push(part);
        match std::fs::symlink_metadata(&path) {
            Ok(meta) if meta.file_type().is_symlink() => {
                return Err(format!(
                    "{} is a symbolic link. Project records need ordinary files and folders.",
                    path.display()
                ))
            }
            Ok(_) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(err(e)),
        }
    }
    Ok(path)
}
pub fn feature_path(root: &Path, id: &str) -> Result<PathBuf, String> {
    valid_id(id)?;
    safe_path(root, &["plan", "features", &format!("{id}.md")])
}
pub fn encode(feature: &Feature) -> Result<String, String> {
    if std::iter::once(&feature.brief)
        .chain(feature.tasks.iter().map(|t| &t.brief))
        .any(|s| s.contains(TASK))
    {
        return Err("The brief contains a reserved bomb-task document marker.".into());
    }
    // Escaping the HTML terminator also protects arbitrary user titles/model IDs.
    let meta = serde_json::to_string_pretty(feature)
        .map_err(err)?
        .replace('>', "\\u003e");
    let mut result = format!(
        "{HEADER}{meta}\n-->\n# {}\n\n{}\n",
        feature.title,
        feature.brief.trim()
    );
    for t in &feature.tasks {
        result.push_str(&format!(
            "\n{TASK}{} -->\n## {}\n\n{}\n",
            t.id,
            t.title,
            t.brief.trim()
        ));
    }
    Ok(result)
}
pub fn decode(content: &str) -> Result<Feature, String> {
    if content.len() > 1024 * 1024 {
        return Err("Feature record exceeds 1 MB.".into());
    }
    let rest = content
        .strip_prefix(HEADER)
        .ok_or("Not a Bomb Code feature document (version 1).")?;
    let (json, body) = rest
        .split_once("\n-->\n")
        .ok_or("Feature metadata is incomplete.")?;
    let mut feature: Feature = serde_json::from_str(json).map_err(err)?;
    let mut sections = body.split(TASK);
    let feature_body = sections.next().unwrap_or_default();
    let (heading, prose) = feature_body
        .split_once('\n')
        .ok_or("Feature heading is missing")?;
    feature.title = heading
        .strip_prefix("# ")
        .ok_or("Use a # heading for the feature title")?
        .to_string();
    feature.brief = prose.trim().to_string();
    let mut seen = std::collections::HashSet::new();
    for section in sections {
        let (id, brief) = section
            .split_once(" -->\n")
            .ok_or("Incomplete task marker")?;
        if !seen.insert(id) {
            return Err("Duplicate task section".into());
        }
        let task = feature
            .tasks
            .iter_mut()
            .find(|t| t.id == id)
            .ok_or("Task section has no matching metadata")?;
        let (heading, prose) = brief.split_once('\n').ok_or("Task heading is missing")?;
        task.title = heading
            .strip_prefix("## ")
            .ok_or("Use a ## heading for each task title")?
            .to_string();
        task.brief = prose.trim().to_owned();
    }
    if seen.len() != feature.tasks.len() {
        return Err("A task's Markdown brief is missing. Restore its bomb-task marker.".into());
    }
    super::validate(&feature)?;
    Ok(feature)
}
pub fn read(path: &Path) -> Result<Record, String> {
    let revision = std::fs::read_to_string(path).map_err(err)?;
    let feature = decode(&revision)?;
    if path.file_stem().and_then(|s| s.to_str()) != Some(&feature.id) {
        return Err("Feature filename and ID differ.".into());
    }
    Ok(Record { feature, revision })
}
pub fn load(root: &Path) -> Result<Vec<Record>, String> {
    let dir = safe_path(root, &["plan", "features"])?;
    if !dir.exists() {
        return Ok(vec![]);
    }
    let mut records = vec![];
    for entry in std::fs::read_dir(&dir).map_err(err)? {
        let path = entry.map_err(err)?.path();
        if path.extension().and_then(|s| s.to_str()) != Some("md") {
            continue;
        }
        let name = path
            .file_name()
            .and_then(|s| s.to_str())
            .ok_or("Invalid record filename")?;
        let path = safe_path(root, &["plan", "features", name])?;
        let content = std::fs::read_to_string(&path).map_err(err)?;
        if content.starts_with("<!-- bomb-feature/") && !content.starts_with(HEADER) {
            return Err(format!(
                "{} uses an unsupported feature document version.",
                path.display()
            ));
        }
        if content.starts_with(HEADER) {
            records.push(read(&path).map_err(|e| format!("{}: {e}", path.display()))?);
        }
    }
    records.sort_by(|a, b| a.feature.title.cmp(&b.feature.title));
    Ok(records)
}
pub fn check_revision(path: &Path, expected: Option<&str>) -> Result<(), String> {
    let current = match std::fs::read_to_string(path) {
        Ok(s) => Some(s),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => return Err(err(e)),
    };
    if current.as_deref() != expected {
        return Err(
            "This brief changed on disk. Reload it before saving; your draft has been kept.".into(),
        );
    }
    Ok(())
}
pub fn write_atomic(path: &Path, content: &str) -> Result<(), String> {
    let parent = path.parent().ok_or("Missing record folder")?;
    std::fs::create_dir_all(parent).map_err(err)?;
    let temp = parent.join(format!(".bomb-record-{}.tmp", Uuid::new_v4()));
    let result = (|| {
        let mut f = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temp)
            .map_err(err)?;
        f.write_all(content.as_bytes()).map_err(err)?;
        f.sync_all().map_err(err)?;
        std::fs::rename(&temp, path).map_err(err)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&temp);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::services::features::{Assignment, FeatureTask};
    fn feature() -> Feature {
        let mut f = Feature::new();
        f.title = "Leaderboard".into();
        f.brief =
            "# Outcome\nCompare player scores.\n\n# Done when\nUI and API work together.".into();
        let mut t = FeatureTask::new("Frontend");
        t.brief = "Use mock rows for now.".into();
        t.assignment = Some(Assignment {
            backend: "claude".into(),
            model: "offered-model".into(),
            effort: "medium".into(),
        });
        f.tasks.push(t);
        f
    }
    #[test]
    fn markdown_round_trip_and_external_edit() {
        let f = feature();
        let text = encode(&f).unwrap();
        assert_eq!(decode(&text).unwrap(), f);
        let edited = text.replace("Use mock rows for now.", "Use real API rows.");
        assert_eq!(
            decode(&edited).unwrap().tasks[0].brief,
            "Use real API rows."
        );
    }
    #[test]
    fn opening_project_is_read_only_and_conflicts_preserve_files() {
        let dir = tempfile::tempdir().unwrap();
        assert!(load(dir.path()).unwrap().is_empty());
        assert!(!dir.path().join("plan").exists());
        let f = feature();
        let p = feature_path(dir.path(), &f.id).unwrap();
        let text = encode(&f).unwrap();
        check_revision(&p, None).unwrap();
        write_atomic(&p, &text).unwrap();
        let external = text.replace("Compare player scores.", "Compare team scores.");
        std::fs::write(&p, &external).unwrap();
        assert!(check_revision(&p, Some(&text)).is_err());
        assert_eq!(std::fs::read_to_string(&p).unwrap(), external);
        assert!(check_revision(&p, None).is_err());
    }
    #[test]
    fn dependencies_are_explicit_and_acyclic() {
        let mut f = feature();
        f.tasks.push(FeatureTask::new("Backend"));
        super::super::validate(&f).unwrap();
        f.tasks[0].waits_for = vec![f.tasks[1].id.clone()];
        super::super::validate(&f).unwrap();
        f.tasks[1].waits_for = vec![f.tasks[0].id.clone()];
        assert!(super::super::validate(&f).is_err());
        f.tasks[1].waits_for.clear();
        f.tasks[0].waits_for = vec!["missing".into()];
        assert!(super::super::validate(&f).is_err());
    }
    #[test]
    fn missing_task_prose_is_not_silently_erased() {
        let f = feature();
        let text = encode(&f).unwrap();
        let truncated = text.split(TASK).next().unwrap();
        assert!(decode(truncated).is_err());
    }
    #[test]
    fn path_traversal_and_symlinks_rejected() {
        let dir = tempfile::tempdir().unwrap();
        assert!(feature_path(dir.path(), "../../outside").is_err());
        #[cfg(unix)]
        {
            let outside = tempfile::tempdir().unwrap();
            std::os::unix::fs::symlink(outside.path(), dir.path().join("plan")).unwrap();
            assert!(load(dir.path()).is_err());
        }
    }
    #[test]
    fn review_prompt_is_scoped_and_does_not_grant_permission() {
        let f = feature();
        let mut t = FeatureTask::new("Review");
        t.review_of = Some(f.tasks[0].id.clone());
        let p = super::super::task_prompt(&f, &t);
        assert!(p.contains("read-only"));
        assert!(p.contains("ONE task"));
    }
}
