//! Short labels for an agent's steps, shared by every screen that folds them:
//! "Thought · Ran 4 commands · Edited 1 file" for a finished run of steps and
//! "Running `npm test`" for the one in progress.

use crate::transcript::ToolRow;

/// command | edit | read | web | other
pub fn tool_kind(name: &str) -> &'static str {
    let n = name.to_ascii_lowercase();
    if ["bash", "shell", "terminal", "exec", "command"]
        .iter()
        .any(|k| n.contains(k))
    {
        "command"
    } else if ["edit", "write", "patch", "create", "apply"]
        .iter()
        .any(|k| n.contains(k))
    {
        "edit"
    } else if [
        "read", "glob", "grep", "search", "ls", "list", "find", "cat", "view",
    ]
    .iter()
    .any(|k| n.contains(k))
    {
        "read"
    } else if ["fetch", "web", "http", "browse"]
        .iter()
        .any(|k| n.contains(k))
    {
        "web"
    } else {
        "other"
    }
}

/// The part of a step's arguments a person would recognise (the command, the file, the search),
/// when they arrive as JSON. Claude, Codex and Grok name these differently
/// (`file_path` / `path` / `target_file`, `pattern` / `query` …).
pub fn arg_detail(args: &str) -> Option<String> {
    let v = serde_json::from_str::<serde_json::Value>(args.trim()).ok()?;
    const KEYS: &[&str] = &[
        "command", "cmd", "file_path", "target_file", "path", "file", "url",
        "pattern", "query", "glob_pattern", "search_query", "target_directory", "dir_path", "directory",
    ];
    KEYS.iter().find_map(|k| match v.get(*k)? {
        serde_json::Value::String(s) if !s.trim().is_empty() => Some(s.clone()),
        // `["bash", "-lc", "npm test"]`: the last part is what was run.
        serde_json::Value::Array(parts) => parts.last().and_then(|p| p.as_str()).map(str::to_owned),
        _ => None,
    })
}

/// A running step in a few words: "Running `npm test`", "Editing src/app.ts", "Reading 3 files…".
pub fn running_label(row: &ToolRow) -> String {
    let first = row.args.lines().next().unwrap_or_default().trim();
    // Arguments often arrive as JSON; pull out the part a person would recognise.
    let detail = arg_detail(&row.args).unwrap_or_else(|| first.to_string());
    let detail: String = detail.lines().next().unwrap_or_default().chars().take(80).collect();
    let verb = match tool_kind(&row.name) { "command" => "Running", "edit" => "Editing", "read" => "Reading", "web" => "Fetching", _ => "Working on" };
    match (detail.is_empty(), tool_kind(&row.name)) {
        (true, "command") => "Running a command".into(),
        (true, _) => format!("{verb} {}", if row.name.is_empty() || row.name == "tool" { "a step" } else { row.name.as_str() }),
        (false, "command") => format!("{verb} `{detail}`"),
        (false, _) => format!("{verb} {detail}"),
    }
}

/// "Ran 2 commands · Edited 1 file · Read 3 files"
pub fn tool_group_summary<'a>(rows: impl Iterator<Item = &'a ToolRow>) -> String {
    let (mut cmd, mut edit, mut read, mut web, mut other) = (0, 0, 0, 0, 0);
    let mut last_name = String::new();
    for r in rows {
        last_name = r.name.clone();
        match tool_kind(&r.name) {
            "command" => cmd += 1,
            "edit" => edit += 1,
            "read" => read += 1,
            "web" => web += 1,
            _ => other += 1,
        }
    }
    let mut parts = Vec::new();
    if cmd > 0 {
        parts.push(format!("Ran {cmd} command{}", plural(cmd)));
    }
    if edit > 0 {
        parts.push(format!("Edited {edit} file{}", plural(edit)));
    }
    if read > 0 {
        parts.push(format!("Read {read} file{}", plural(read)));
    }
    if web > 0 {
        parts.push(format!("Fetched {web} page{}", plural(web)));
    }
    if other > 0 {
        if parts.is_empty() && other == 1 && !last_name.is_empty() && last_name != "tool" {
            parts.push(last_name);
        } else {
            parts.push(format!("{other} other tool call{}", plural(other)));
        }
    }
    parts.join(" · ")
}

/// "Thought · Ran 4 commands · Edited 1 file", for `thoughts` thinking blocks and these tool rows.
pub fn activity_label<'a>(thoughts: usize, rows: impl Iterator<Item = &'a ToolRow>) -> String {
    let base = tool_group_summary(rows);
    match (base.is_empty(), thoughts) {
        (_, 0) => base,
        (true, 1) => "Thought".into(),
        (true, n) => format!("Thought {n} times"),
        (false, 1) => format!("Thought · {base}"),
        (false, n) => format!("Thought {n} times · {base}"),
    }
}

fn plural(n: usize) -> &'static str {
    if n == 1 {
        ""
    } else {
        "s"
    }
}

#[cfg(test)]
mod arg_detail_tests {
    use super::arg_detail;

    #[test]
    fn grok_claude_and_codex_arguments_read_plainly() {
        assert_eq!(arg_detail(r#"{"variant":"Bash","command":"ls -la ~/.grok"}"#).as_deref(), Some("ls -la ~/.grok"));
        assert_eq!(arg_detail(r#"{"variant":"ReadFile","target_file":"/a/config.toml","limit":40}"#).as_deref(), Some("/a/config.toml"));
        assert_eq!(arg_detail(r#"{"variant":"Grep","pattern":"agent.?profile"}"#).as_deref(), Some("agent.?profile"));
        assert_eq!(arg_detail(r#"{"file_path":"src/app.ts","new_string":"x"}"#).as_deref(), Some("src/app.ts"));
        assert_eq!(arg_detail(r#"{"command":["bash","-lc","npm test"]}"#).as_deref(), Some("npm test"));
        assert_eq!(arg_detail("npm test"), None);
    }
}
