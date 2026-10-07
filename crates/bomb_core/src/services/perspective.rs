//! Perspective: a profile of how the person works with AI, distilled from their threads
//! (`docs/plan/perspective_skill_plan.md`).
//!
//! 1. Gather (local): the person's messages per thread, each with the end of the reply it
//!    answers. Automated prompts and test threads are dropped; secrets, email addresses
//!    and phone numbers are scrubbed; threads are labelled by date only.
//! 2. Notes: threads go in batches to the person's own logged-in agent CLI (no tools, no
//!    saved session), which returns observations with quotes, people as roles. Cached per
//!    thread, so an update only reads what changed.
//! 3. Merge: one call per section keeps claims seen in at least two threads.
//! 4. Assemble `SKILL.md` + `evidence.md`, with a final check for names.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

use chrono::{DateTime, Utc};
use grok_persistence::Persistence;
use regex::Regex;
use serde::{Deserialize, Serialize};
use tokio::io::AsyncWriteExt;
use uuid::Uuid;

use crate::state::AppState;

/// Sections of the profile, in order: (key, heading, what belongs there).
pub const SECTIONS: &[(&str, &str, &str)] = &[
    ("values", "Values and quality bar", "what 'done' means to them, what they protect, what they won't trade away"),
    ("decisions", "How they decide", "plan-first vs just-build, scope vs speed, when they ask before acting, how they pick between options"),
    ("questions", "Questions they ask", "recurring checks and questions an agent should anticipate"),
    ("communication", "How they communicate", "how they write requests and corrections, what kind of answer they want back"),
    ("tools", "Tools and stack", "languages, frameworks, services, which models for which jobs, git habits, environments"),
    ("reactions", "Reactions", "what earns praise, what frustrates them, red lines"),
    ("others", "Working with others", "how they collaborate, treat others' stake and ownership, communicate outward, look for inputs (feedback, reviews, references), handle dependencies — roles only, never names"),
    ("guidance", "How to work for them", "concrete do / don't advice for an agent working on their behalf"),
];

const BATCH_CHARS: usize = 60_000;
const THREAD_CHARS: usize = 40_000;
const REPLY_EXCERPT: usize = 600;
const PARALLEL: usize = 4;
/// Per agent call. Writing with a reasoning model can take several minutes per chunk.
const CALL_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(20 * 60);

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Options {
    /// The person, as the profile names them ("Max").
    pub name: String,
    /// "claude" or "codex": whose logged-in CLI takes the notes.
    pub backend: String,
    pub notes_model: Option<String>,
    /// Who writes the profile from the notes: "claude", "codex" or "grok".
    pub writer: String,
    /// `None`: the writer CLI's default (for Grok that's its newest model).
    pub merge_model: Option<String>,
}

impl Options {
    pub fn defaults(backend: &str) -> Self {
        let claude = backend != "codex";
        Self {
            name: default_name(),
            backend: if claude { "claude".into() } else { "codex".into() },
            notes_model: claude.then(|| "sonnet".into()),
            writer: if claude { "claude".into() } else { "codex".into() },
            merge_model: claude.then(|| "opus".into()),
        }
    }

    /// Have `writer` write the profile ("claude" → Opus; others → their default model).
    pub fn written_by(mut self, writer: &str) -> Self {
        self.writer = writer.to_string();
        self.merge_model = (writer == "claude").then(|| "opus".into());
        self
    }
}

/// The Grok CLI's default model (its newest), for labelling the choice.
pub fn grok_default_model() -> Option<String> {
    let program = grok_config::discover_grok_binary().ok()?;
    let out = std::process::Command::new(program).arg("models").output().ok()?;
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .find_map(|l| l.trim().strip_prefix("Default model:").map(|m| m.trim().to_string()))
        .filter(|m| !m.is_empty())
}

/// The macOS account's first name, else "the user".
pub fn default_name() -> String {
    std::process::Command::new("id")
        .arg("-F")
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .and_then(|full| full.split_whitespace().next().map(String::from))
        .unwrap_or_else(|| "the user".into())
}

#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Status {
    pub running: bool,
    /// What it's doing now, in words.
    pub stage: String,
    pub done: usize,
    pub total: usize,
    pub error: Option<String>,
}

fn status_cell() -> &'static Mutex<Status> {
    static S: OnceLock<Mutex<Status>> = OnceLock::new();
    S.get_or_init(Default::default)
}

pub fn status() -> Status {
    status_cell().lock().unwrap_or_else(|e| e.into_inner()).clone()
}

fn set_status(f: impl FnOnce(&mut Status)) {
    f(&mut status_cell().lock().unwrap_or_else(|e| e.into_inner()));
}

/// Where the profile lives until the person exports it.
pub fn output_dir(state: &AppState) -> PathBuf {
    state.paths.grok_dir.join("control-panel").join("perspective")
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Built {
    pub at: DateTime<Utc>,
    pub threads: usize,
    pub observations: usize,
    pub skill_md: String,
    pub evidence_md: String,
}

/// The last profile built, if any.
pub fn last_built(state: &AppState) -> Option<Built> {
    let raw = std::fs::read_to_string(output_dir(state).join("meta.json")).ok()?;
    let mut built: Built = serde_json::from_str(&raw).ok()?;
    // The files may have been edited by hand since.
    built.skill_md = std::fs::read_to_string(output_dir(state).join("SKILL.md")).unwrap_or(built.skill_md);
    built.evidence_md = std::fs::read_to_string(output_dir(state).join("evidence.md")).unwrap_or(built.evidence_md);
    Some(built)
}

/// Threads whose notes aren't cached yet (new, or with messages since), for "N new since".
pub fn pending_threads(state: &AppState) -> usize {
    gather(&state.persistence).iter().filter(|t| cached_notes(&state.persistence, t).is_none()).count()
}

/// Build (or update) the profile. Progress is in `status()`.
pub async fn build(state: &AppState, opts: Options) -> Result<Built, String> {
    if status().running {
        return Err("A perspective is already being built.".into());
    }
    set_status(|s| *s = Status { running: true, stage: "Reading your threads".into(), ..Default::default() });
    let result = run(state, &opts).await;
    set_status(|s| {
        s.running = false;
        s.error = result.as_ref().err().cloned();
        s.stage = if result.is_ok() { "Done".into() } else { "Stopped".into() };
    });
    result
}

async fn run(state: &AppState, opts: &Options) -> Result<Built, String> {
    let cli = Cli::find(&opts.backend)?;
    let writer = Cli::find(&opts.writer)?;
    let db = state.persistence.clone();
    let threads = tokio::task::spawn_blocking(move || gather(&db)).await.map_err(|e| e.to_string())?;
    if threads.is_empty() {
        return Err("No threads with your messages yet.".into());
    }

    // Notes, from cache where nothing changed.
    let mut observations: Vec<Observation> = Vec::new();
    let mut todo: Vec<&ThreadDigest> = Vec::new();
    for t in &threads {
        match cached_notes(&state.persistence, t) {
            Some(cached) => observations.extend(cached),
            None => todo.push(t),
        }
    }
    let batches = batch(&todo);
    set_status(|s| { s.stage = "Taking notes on each thread".into(); s.done = 0; s.total = batches.len(); });
    let workdir = scratch_dir()?;
    let semaphore = Arc::new(tokio::sync::Semaphore::new(PARALLEL));
    let mut jobs = tokio::task::JoinSet::new();
    for group in batches {
        let (cli, opts, workdir, semaphore) = (cli.clone(), opts.clone(), workdir.clone(), semaphore.clone());
        let group: Vec<ThreadDigest> = group.into_iter().cloned().collect();
        jobs.spawn(async move {
            let _permit = semaphore.acquire().await;
            let prompt = notes_prompt(&opts.name, &group);
            let reply = cli.ask(&prompt, opts.notes_model.as_deref(), &workdir).await;
            (group, reply)
        });
    }
    let mut failures = 0usize;
    while let Some(joined) = jobs.join_next().await {
        let (group, reply) = joined.map_err(|e| e.to_string())?;
        match reply.map(|r| parse_observations(&r)) {
            Ok(found) => {
                for t in &group {
                    let mine: Vec<Observation> = found.iter().filter(|o| o.thread == t.label).cloned().collect();
                    cache_notes(&state.persistence, t, &mine);
                    observations.extend(mine);
                }
            }
            Err(e) => {
                tracing::warn!(error = %e, "perspective: notes call failed");
                failures += 1;
            }
        }
        set_status(|s| s.done += 1);
    }
    if observations.is_empty() {
        return Err(format!("The agent didn't return any notes ({failures} calls failed). Check that {} is signed in.", opts.backend));
    }

    // Merge. A big merge in one call is too slow for a reasoning model (grok-4.7 took
    // 6 minutes for 60 notes), so each section's notes go in chunks, all sections' chunks
    // run in parallel, and a short call per section combines the chunks' points.
    let dates: HashMap<&str, &str> = threads.iter().map(|t| (t.label.as_str(), t.date.as_str())).collect();
    let by_section: Vec<Vec<Observation>> = SECTIONS
        .iter()
        .map(|(key, _, _)| observations.iter().filter(|o| o.section == *key).cloned().collect())
        .collect();
    let chunked: Vec<Vec<Vec<Observation>>> = by_section.iter().map(|obs| chunk_notes(obs)).collect();
    let combines = chunked.iter().filter(|c| c.len() > 1).count();
    let calls: usize = chunked.iter().map(Vec::len).sum::<usize>() + combines;
    set_status(|s| { s.stage = "Writing the profile".into(); s.done = 0; s.total = calls; });

    let writer_slots = Arc::new(tokio::sync::Semaphore::new(PARALLEL));
    let mut partials = tokio::task::JoinSet::new();
    for (ix, chunks) in chunked.iter().enumerate() {
        let single = chunks.len() == 1;
        for chunk in chunks.clone() {
            let (writer, opts, workdir, slots) = (writer.clone(), opts.clone(), workdir.clone(), writer_slots.clone());
            let (_, heading, about) = SECTIONS[ix];
            partials.spawn(async move {
                let _slot = slots.acquire().await;
                let refs: Vec<&Observation> = chunk.iter().collect();
                // A lone chunk is the whole section, so it can apply the two-conversation rule itself.
                let prompt = merge_prompt(&opts.name, heading, about, &refs, if single { 2 } else { 1 });
                (ix, writer.ask_retrying(&prompt, opts.merge_model.as_deref(), Some("low"), &workdir, heading).await.map(|r| parse_points(&r)))
            });
        }
    }
    // A chunk that fails twice is skipped; the section is written from the rest.
    let mut partial_points: Vec<Vec<Point>> = vec![Vec::new(); SECTIONS.len()];
    let mut skipped = 0usize;
    let mut last_error = String::new();
    while let Some(joined) = partials.join_next().await {
        let (ix, points) = joined.map_err(|e| e.to_string())?;
        match points {
            Ok(points) => partial_points[ix].extend(points),
            Err(e) => {
                skipped += 1;
                last_error = e;
            }
        }
        set_status(|s| s.done += 1);
    }
    if partial_points.iter().all(Vec::is_empty) {
        return Err(format!("{} couldn't write the profile: {last_error}", opts.writer));
    }

    let mut combined: tokio::task::JoinSet<(usize, Vec<Point>)> = tokio::task::JoinSet::new();
    for (ix, points) in partial_points.into_iter().enumerate() {
        let (writer, opts, workdir, slots) = (writer.clone(), opts.clone(), workdir.clone(), writer_slots.clone());
        let needs_combine = chunked[ix].len() > 1;
        combined.spawn(async move {
            if !needs_combine {
                return (ix, points);
            }
            let _slot = slots.acquire().await;
            let (_, heading, about) = SECTIONS[ix];
            let prompt = combine_prompt(&opts.name, heading, about, &points);
            match writer.ask_retrying(&prompt, opts.merge_model.as_deref(), None, &workdir, heading).await {
                Ok(r) => (ix, parse_points(&r)),
                // Without the combine step, the chunks' own points still beat an empty section.
                Err(_) => (ix, points),
            }
        });
    }
    let mut merged: Vec<Vec<Point>> = vec![Vec::new(); SECTIONS.len()];
    while let Some(joined) = combined.join_next().await {
        let (ix, points) = joined.map_err(|e| e.to_string())?;
        if chunked[ix].len() > 1 {
            set_status(|s| s.done += 1);
        }
        merged[ix] = supported(points);
    }
    if skipped > 0 {
        tracing::warn!(skipped, "perspective: some writing calls failed and were skipped");
    }
    let sections: Vec<(&str, Vec<Point>)> = SECTIONS.iter().zip(merged).map(|((_, heading, _), points)| (*heading, points)).collect();

    set_status(|s| { s.stage = "Checking for names".into(); s.done = 0; s.total = 1; });
    let (skill, evidence) = assemble(&opts.name, &sections, &dates, threads.len());
    let skill = scrub(&final_check(&writer, opts, &workdir, &skill).await.unwrap_or(skill));
    let evidence = scrub(&final_check(&writer, opts, &workdir, &evidence).await.unwrap_or(evidence));

    let built = Built { at: Utc::now(), threads: threads.len(), observations: observations.len(), skill_md: skill, evidence_md: evidence };
    let dir = output_dir(state);
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    std::fs::write(dir.join("SKILL.md"), &built.skill_md).map_err(|e| e.to_string())?;
    std::fs::write(dir.join("evidence.md"), &built.evidence_md).map_err(|e| e.to_string())?;
    std::fs::write(dir.join("meta.json"), serde_json::to_string_pretty(&built).unwrap_or_default()).map_err(|e| e.to_string())?;
    Ok(built)
}

/// Copy the profile folder to `dest` (e.g. `~/.claude/skills/perspective`).
pub fn export(state: &AppState, dest: &Path) -> Result<(), String> {
    let src = output_dir(state);
    std::fs::create_dir_all(dest).map_err(|e| e.to_string())?;
    for name in ["SKILL.md", "evidence.md"] {
        std::fs::copy(src.join(name), dest.join(name)).map_err(|e| format!("{name}: {e}"))?;
    }
    Ok(())
}

// ── 1. Gather ────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
struct ThreadDigest {
    id: Uuid,
    /// "T7 · Oct 6": no project names, which can identify clients.
    label: String,
    date: String,
    /// Last message time, for the cache.
    upto: String,
    text: String,
}

fn gather(db: &Persistence) -> Vec<ThreadDigest> {
    let Ok(sessions) = db.list_sessions() else { return Vec::new() };
    let mut sessions: Vec<_> = sessions.into_iter().filter(|s| !s.model.eq_ignore_ascii_case("mock")).collect();
    sessions.sort_by_key(|s| s.created_at);
    let mut out = Vec::new();
    for s in sessions {
        let Ok(rows) = db.transcript_entries(s.id) else { continue };
        let mut text = String::new();
        let mut last_reply = String::new();
        let mut upto = String::new();
        let mut first: Option<String> = None;
        for row in rows {
            match row.role.as_str() {
                "agent" => last_reply = row.body.clone(),
                "user" => {
                    let Some(msg) = own_words(&row.body) else { continue };
                    if !last_reply.trim().is_empty() {
                        text.push_str("AI (end of its reply): ");
                        text.push_str(&tail(&last_reply, REPLY_EXCERPT));
                        text.push('\n');
                    }
                    text.push_str("USER: ");
                    text.push_str(&msg);
                    text.push_str("\n\n");
                    last_reply.clear();
                    upto = row.at.clone();
                    first.get_or_insert_with(|| row.at.clone());
                }
                _ => {}
            }
        }
        if text.is_empty() {
            continue;
        }
        let date = first
            .as_deref()
            .and_then(|a| a.parse::<DateTime<Utc>>().ok())
            .map(|d| d.with_timezone(&chrono::Local).format("%b %-d, %Y").to_string())
            .unwrap_or_default();
        let label = format!("T{}", out.len() + 1);
        out.push(ThreadDigest { id: s.id, label, date, upto, text: scrub(&head_and_tail(&text, THREAD_CHARS)) });
    }
    out
}

/// What the person wrote, or `None` for prompts the app or a workflow wrote for them.
fn own_words(body: &str) -> Option<String> {
    const AUTOMATED: &[&str] = &[
        "[Bomb Code session recovery",
        "You are executing one bounded Foundry stage",
        "You are planning work for the selected project",
        "Write a prompt contract",
        "Implement the plan above",
    ];
    let body = body.trim();
    if body.is_empty() || AUTOMATED.iter().any(|a| body.starts_with(a)) {
        return None;
    }
    // Foundry feature briefs: "Feature: … (F-xyz)".
    if body.starts_with("Feature: ") && body.lines().next().is_some_and(|l| l.contains("(F-")) {
        return None;
    }
    // The note the composer appends for attached files.
    let body = body
        .split("\nAttached file")
        .next()
        .unwrap_or(body)
        .trim();
    (!body.is_empty()).then(|| body.to_string())
}

fn tail(s: &str, max: usize) -> String {
    if s.len() <= max {
        return s.replace('\n', " ");
    }
    let mut start = s.len() - max;
    while !s.is_char_boundary(start) {
        start += 1;
    }
    format!("…{}", s[start..].replace('\n', " "))
}

/// Very long threads keep their start and end: how it began and how it settled.
fn head_and_tail(s: &str, max: usize) -> String {
    if s.len() <= max {
        return s.to_string();
    }
    let half = max / 2;
    let mut a = half;
    while !s.is_char_boundary(a) {
        a -= 1;
    }
    let mut b = s.len() - half;
    while !s.is_char_boundary(b) {
        b += 1;
    }
    format!("{}\n…[middle of a long thread left out]…\n{}", &s[..a], &s[b..])
}

/// Remove what must never leave the machine in a prompt: secrets and contact details.
/// Names are handled by the model (people → roles) and checked again on the output.
pub fn scrub(text: &str) -> String {
    static PATTERNS: OnceLock<Vec<(Regex, &'static str)>> = OnceLock::new();
    let patterns = PATTERNS.get_or_init(|| {
        [
            (r"\b(sk|pk|rk)-[A-Za-z0-9_-]{16,}", "[secret]"),
            (r"\b(ghp|gho|ghu|ghs|github_pat)_[A-Za-z0-9_]{20,}", "[secret]"),
            (r"\bxox[abpr]-[A-Za-z0-9-]{10,}", "[secret]"),
            (r"\bAKIA[0-9A-Z]{16}\b", "[secret]"),
            (r"\beyJ[A-Za-z0-9_-]{10,}\.[A-Za-z0-9_-]{10,}\.[A-Za-z0-9_-]{10,}", "[secret]"),
            (r"(?i)\b(api[_-]?key|secret|token|password|passwd)\b\s*[:=]\s*\S{6,}", "$1=[secret]"),
            (r"\b0x[a-fA-F0-9]{40}\b", "[address]"),
            (r"[A-Za-z0-9._%+-]+\\?@[A-Za-z0-9.-]+\.[A-Za-z]{2,}", "[email]"),
            (r"(?:\+\d{1,3}[\s.-]?)?\(?\b\d{3}\)?[\s.-]\d{3}[\s.-]\d{4}\b", "[phone]"),
        ]
        .into_iter()
        .map(|(re, to)| (Regex::new(re).expect("static regex"), to))
        .collect()
    });
    let mut out = text.to_string();
    for (re, to) in patterns {
        out = re.replace_all(&out, *to).into_owned();
    }
    // Key-like strings: long and mixing upper case, lower case and digits. Paths, hashes in
    // file names and long identifiers stay, since they rarely mix all three.
    static LONG: OnceLock<Regex> = OnceLock::new();
    let long = LONG.get_or_init(|| Regex::new(r"[A-Za-z0-9+_-]{32,}").expect("static regex"));
    long.replace_all(&out, |c: &regex::Captures| {
        let t = &c[0];
        let mixed = t.chars().any(|ch| ch.is_ascii_uppercase()) && t.chars().any(|ch| ch.is_ascii_lowercase()) && t.chars().filter(|ch| ch.is_ascii_digit()).count() >= 4;
        if mixed { "[long token]".to_string() } else { t.to_string() }
    })
    .into_owned()
}

fn batch<'a>(threads: &[&'a ThreadDigest]) -> Vec<Vec<&'a ThreadDigest>> {
    let mut out: Vec<Vec<&ThreadDigest>> = Vec::new();
    let mut size = 0;
    for t in threads {
        if out.is_empty() || size + t.text.len() > BATCH_CHARS {
            out.push(Vec::new());
            size = 0;
        }
        size += t.text.len();
        out.last_mut().expect("pushed").push(t);
    }
    out
}

// ── 2. Notes ─────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
struct Observation {
    section: String,
    claim: String,
    #[serde(default)]
    quote: String,
    thread: String,
    #[serde(default = "one")]
    strength: u8,
}

fn one() -> u8 {
    1
}

fn notes_prompt(name: &str, threads: &[ThreadDigest]) -> String {
    let sections: String = SECTIONS.iter().map(|(k, _, about)| format!("- `{k}`: {about}\n")).collect();
    let mut body = String::new();
    for t in threads {
        body.push_str(&format!("=== {} ({}) ===\n{}\n", t.label, t.date, t.text));
    }
    format!(
        "You are studying how {name} works with AI coding agents, to write a profile another agent can use to work the way {name} would want.\n\
         Below are conversations between {name} (USER) and an AI. Each USER line follows the end of the AI reply it responds to.\n\n\
         Note observations about {name}'s WORKING STYLE only: how they think, decide, ask, communicate, react, which tools they use, and how they work with other people. \
         Ignore facts about their projects' subject matter.\n\n\
         Sections:\n{sections}\n\
         Rules:\n\
         - Third person (\"{name} asks…\"), by name or \"they\"; don't assume pronouns. Specific and behavioural, not flattering or generic (\"values quality\" is useless; \"asks to see it running in the real app before calling it done\" is useful).\n\
         - `quote`: a short verbatim excerpt (under 200 characters) of {name}'s own words that shows it.\n\
         - Never write the name of any other person, company or client, in a claim or a quote: replace it with a role (\"a collaborator\", \"a client\", \"a teammate\") or [person]. Use roles for the `others` section too.\n\
         - `thread`: the label the observation comes from (e.g. \"T7\"). `strength`: 1 (hinted) to 3 (clear).\n\
         - Roughly 3-12 observations per conversation, fewer if it's short.\n\n\
         Reply with only a JSON array: [{{\"section\":\"…\",\"claim\":\"…\",\"quote\":\"…\",\"thread\":\"T7\",\"strength\":2}}, …]\n\n{body}"
    )
}

/// The JSON array in a reply, tolerating prose or code fences around it.
fn json_array<T: for<'de> Deserialize<'de>>(reply: &str) -> Vec<T> {
    let (Some(a), Some(b)) = (reply.find('['), reply.rfind(']')) else { return Vec::new() };
    if b <= a {
        return Vec::new();
    }
    let slice = &reply[a..=b];
    if let Ok(items) = serde_json::from_str::<Vec<T>>(slice) {
        return items;
    }
    // One bad item shouldn't lose the rest.
    serde_json::from_str::<Vec<serde_json::Value>>(slice)
        .map(|vals| vals.into_iter().filter_map(|v| serde_json::from_value(v).ok()).collect())
        .unwrap_or_default()
}

fn parse_observations(reply: &str) -> Vec<Observation> {
    let known: HashSet<&str> = SECTIONS.iter().map(|(k, _, _)| *k).collect();
    json_array::<Observation>(reply)
        .into_iter()
        .filter(|o| known.contains(o.section.as_str()) && !o.claim.trim().is_empty())
        .collect()
}

fn cache_key(id: Uuid) -> String {
    format!("perspective_notes/{id}")
}

#[derive(Serialize, Deserialize)]
struct CachedNotes {
    upto: String,
    notes: Vec<Observation>,
}

fn cached_notes(db: &Persistence, t: &ThreadDigest) -> Option<Vec<Observation>> {
    let raw = db.get_kv(&cache_key(t.id)).ok().flatten()?;
    let cached: CachedNotes = serde_json::from_str(&raw).ok()?;
    // Labels are renumbered each run; the cached notes take this run's label.
    (cached.upto == t.upto).then(|| cached.notes.into_iter().map(|o| Observation { thread: t.label.clone(), ..o }).collect())
}

fn cache_notes(db: &Persistence, t: &ThreadDigest, notes: &[Observation]) {
    let cached = CachedNotes { upto: t.upto.clone(), notes: notes.to_vec() };
    if let Ok(json) = serde_json::to_string(&cached) {
        let _ = db.set_kv(&cache_key(t.id), &json);
    }
}

// ── 3. Merge ─────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Deserialize, PartialEq)]
struct Point {
    text: String,
    #[serde(default)]
    evidence: Vec<Evidence>,
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
struct Evidence {
    quote: String,
    thread: String,
}

/// Notes per merge call: a reasoning model takes minutes per call, so keep calls small.
const CHUNK_NOTES: usize = 60;

/// Split a section's notes into chunks, interleaved so each chunk spans old and new threads.
fn chunk_notes(notes: &[Observation]) -> Vec<Vec<Observation>> {
    if notes.len() < 2 {
        return Vec::new();
    }
    let n = notes.len().div_ceil(CHUNK_NOTES);
    let mut chunks: Vec<Vec<Observation>> = vec![Vec::new(); n];
    for (i, o) in notes.iter().enumerate() {
        chunks[i % n].push(o.clone());
    }
    chunks
}

fn merge_prompt(name: &str, heading: &str, about: &str, notes: &[&Observation], min_threads: usize) -> String {
    let lines: String = notes
        .iter()
        .map(|o| format!("- [{}] (s{}) {} — \"{}\"\n", o.thread, o.strength, o.claim, o.quote))
        .collect();
    let support = if min_threads >= 2 {
        "A point needs support from at least 2 different conversations (different tags); drop the rest."
    } else {
        "Keep points even if only one conversation supports them; they are combined with other batches later."
    };
    format!(
        "These are observations about how {name} works with AI, all for the section \"{heading}\" ({about}). Each is tagged with the conversation it came from.\n\n\
         Merge them into the points that best describe {name} for an agent that will work on their behalf:\n\
         - Combine observations that say the same thing. {support}\n\
         - Order by how often and how clearly it shows up. Where newer conversations contradict older ones, prefer the newer (higher numbers are newer) and mention the change if it matters.\n\
         - Third person, by name or \"they\" (never assume pronouns), one or two plain sentences each, specific and behavioural. No flattery, no filler.\n\
         - Never name any other person, company or client; use roles.\n\
         - For each point give 2-3 supporting quotes with their conversation tag.\n\n\
         Reply with only a JSON array: [{{\"text\":\"…\",\"evidence\":[{{\"quote\":\"…\",\"thread\":\"T7\"}}]}}, …]\n\n{lines}"
    )
}

/// Combine points written from separate chunks of one section.
fn combine_prompt(name: &str, heading: &str, about: &str, points: &[Point]) -> String {
    let lines: String = points
        .iter()
        .map(|p| {
            let quotes: Vec<String> = p.evidence.iter().map(|e| format!("[{}] \"{}\"", e.thread, e.quote)).collect();
            format!("- {} — {}\n", p.text, quotes.join("; "))
        })
        .collect();
    format!(
        "These points about how {name} works with AI were written from separate batches of notes for the section \"{heading}\" ({about}), each with supporting quotes tagged by conversation.\n\n\
         Combine them into the final list for an agent that will work on their behalf:\n\
         - Merge points that say the same thing, pooling their quotes. A final point needs quotes from at least 2 different conversations (different tags); drop the rest.\n\
         - Order by how often and how clearly it shows up. Prefer newer conversations (higher numbers) where points conflict.\n\
         - Third person, by name or \"they\" (never assume pronouns), one or two plain sentences each, specific and behavioural. No flattery, no filler.\n\
         - Never name any other person, company or client; use roles.\n\
         - For each point keep 2-3 of the best quotes with their conversation tags.\n\n\
         Reply with only a JSON array: [{{\"text\":\"…\",\"evidence\":[{{\"quote\":\"…\",\"thread\":\"T7\"}}]}}, …]\n\n{lines}"
    )
}

fn parse_points(reply: &str) -> Vec<Point> {
    json_array::<Point>(reply)
}

/// Keep points backed by at least two different conversations.
fn supported(points: Vec<Point>) -> Vec<Point> {
    points
        .into_iter()
        .filter(|p| p.evidence.iter().map(|e| e.thread.as_str()).collect::<HashSet<_>>().len() >= 2)
        .collect()
}

// ── 4. Assemble ──────────────────────────────────────────────────────────

fn assemble(name: &str, sections: &[(&str, Vec<Point>)], dates: &HashMap<&str, &str>, threads: usize) -> (String, String) {
    let today = chrono::Local::now().format("%B %-d, %Y");
    let mut skill = format!(
        "---\nname: perspective\ndescription: How {name} thinks and works with AI agents. Use when acting on {name}'s behalf or deciding how to do work for them.\n---\n\n\
         # How {name} works\n\nDistilled from {threads} conversations with AI agents (as of {today}). Evidence for each point is in evidence.md.\n"
    );
    let mut evidence = String::from("# Evidence\n\nQuotes behind each point in SKILL.md, with the date of the conversation they come from.\n");
    for (heading, points) in sections {
        if points.is_empty() {
            continue;
        }
        skill.push_str(&format!("\n## {heading}\n\n"));
        evidence.push_str(&format!("\n## {heading}\n"));
        for p in points {
            skill.push_str(&format!("- {}\n", p.text.trim()));
            evidence.push_str(&format!("\n**{}**\n", p.text.trim()));
            let mut seen = BTreeMap::new();
            for e in &p.evidence {
                seen.entry(e.quote.trim().to_string()).or_insert_with(|| dates.get(e.thread.as_str()).copied().unwrap_or(""));
            }
            for (quote, date) in seen {
                evidence.push_str(&format!("- \"{quote}\"{}\n", if date.is_empty() { String::new() } else { format!(" ({date})") }));
            }
        }
    }
    (skill, evidence)
}

async fn final_check(cli: &Cli, opts: &Options, workdir: &Path, text: &str) -> Result<String, String> {
    let prompt = format!(
        "Below is a document about how {name} works. Return it unchanged except: replace the name of any person other than {name}, and any company, client or product name that identifies someone, with a role (\"a collaborator\", \"a client\") or [name]. \
         Keep all formatting. Reply with only the document.\n\n{text}",
        name = opts.name
    );
    let out = cli.ask_with(&prompt, opts.merge_model.as_deref(), Some("low"), workdir).await?;
    let out = out.trim();
    // A reply that lost most of the document is a failed check, not a result.
    if out.len() < text.len() / 2 {
        return Err("the name check returned too little".into());
    }
    Ok(strip_fence(out))
}

fn strip_fence(s: &str) -> String {
    let s = s.trim();
    let s = s.strip_prefix("```markdown").or_else(|| s.strip_prefix("```md")).or_else(|| s.strip_prefix("```")).unwrap_or(s);
    s.strip_suffix("```").unwrap_or(s).trim().to_string() + "\n"
}

// ── Agent CLI ────────────────────────────────────────────────────────────

/// The person's own agent CLI, run one prompt at a time: prompt on stdin, no tools,
/// nothing saved as a session (so these calls never show up as threads to import).
#[derive(Clone)]
struct Cli {
    backend: String,
    program: PathBuf,
}

impl Cli {
    fn find(backend: &str) -> Result<Self, String> {
        if backend == "grok" {
            return grok_config::discover_grok_binary()
                .map(|program| Self { backend: "grok".into(), program })
                .map_err(|_| "Couldn't find the Grok command line tool. Sign in to Grok from the sidebar, then try again.".into());
        }
        let name = if backend == "codex" { "codex" } else { "claude" };
        let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_default();
        let mut dirs: Vec<PathBuf> = std::env::var_os("PATH").map(|p| std::env::split_paths(&p).collect()).unwrap_or_default();
        dirs.extend([home.join(".local/bin"), "/opt/homebrew/bin".into(), "/usr/local/bin".into(), home.join(".npm-global/bin")]);
        dirs.into_iter()
            .map(|d| d.join(name))
            .find(|p| p.is_file())
            .map(|program| Self { backend: name.into(), program })
            .ok_or_else(|| format!("Couldn't find the {name} command line tool. Install it and sign in, then try again."))
    }

    async fn ask(&self, prompt: &str, model: Option<&str>, cwd: &Path) -> Result<String, String> {
        self.ask_with(prompt, model, None, cwd).await
    }

    /// One retry: a long run makes dozens of calls, and one hiccup shouldn't sink it.
    async fn ask_retrying(&self, prompt: &str, model: Option<&str>, effort: Option<&str>, cwd: &Path, what: &str) -> Result<String, String> {
        match self.ask_with(prompt, model, effort, cwd).await {
            Ok(r) => Ok(r),
            Err(first) => {
                tracing::warn!(backend = %self.backend, step = what, error = %first, "perspective: call failed, retrying");
                self.ask_with(prompt, model, effort, cwd).await.inspect_err(|e| {
                    tracing::warn!(backend = %self.backend, step = what, error = %e, "perspective: call failed twice, skipping");
                })
            }
        }
    }

    /// `effort`: Grok's reasoning effort. "low" is ~4× faster on the big merge steps with
    /// points just as good; the default is kept for the short combine step.
    async fn ask_with(&self, prompt: &str, model: Option<&str>, effort: Option<&str>, cwd: &Path) -> Result<String, String> {
        let mut cmd = tokio::process::Command::new(&self.program);
        cmd.current_dir(cwd).kill_on_drop(true).stdin(std::process::Stdio::piped()).stdout(std::process::Stdio::piped()).stderr(std::process::Stdio::piped());
        let mut on_stdin = true;
        if self.backend == "grok" {
            // Grok's one-shot takes the prompt as an argument (no stdin), within macOS's
            // ~1 MB limit for arguments.
            if prompt.len() > 800_000 {
                return Err("This step's prompt is too large for Grok; write the profile with Claude or Codex.".into());
            }
            cmd.args(["-p", prompt, "--output-format", "plain", "--max-turns", "2", "--disable-web-search", "--no-subagents", "--no-memory", "--tools", ""]);
            if let Some(effort) = effort {
                cmd.args(["--reasoning-effort", effort]);
            }
            if let Some(m) = model.filter(|m| !m.is_empty()) {
                cmd.args(["-m", m]);
            }
            on_stdin = false;
        } else if self.backend == "codex" {
            cmd.args(["exec", "--ephemeral", "--skip-git-repo-check", "-s", "read-only"]);
            if let Some(m) = model.filter(|m| !m.is_empty()) {
                cmd.args(["-m", m]);
            }
            cmd.arg("-");
        } else {
            cmd.args(["-p", "--output-format", "text", "--max-turns", "1", "--no-session-persistence", "--tools", ""]);
            if let Some(m) = model.filter(|m| !m.is_empty()) {
                cmd.args(["--model", m]);
            }
        }
        let mut child = cmd.spawn().map_err(|e| format!("{}: {e}", self.backend))?;
        if let Some(mut stdin) = child.stdin.take() {
            if on_stdin {
                stdin.write_all(prompt.as_bytes()).await.map_err(|e| e.to_string())?;
            }
        }
        let out = tokio::time::timeout(CALL_TIMEOUT, child.wait_with_output())
            .await
            .map_err(|_| format!("{} took too long", self.backend))?
            .map_err(|e| e.to_string())?;
        if !out.status.success() {
            let err = String::from_utf8_lossy(&out.stderr);
            return Err(format!("{} failed: {}", self.backend, err.lines().last().unwrap_or("").trim()));
        }
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    }
}

/// An empty folder to run the CLI in, so no project instructions are picked up.
fn scratch_dir() -> Result<PathBuf, String> {
    let dir = std::env::temp_dir().join("bombcode-perspective");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    Ok(dir)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scrubs_secrets_and_contacts() {
        let s = scrub("key sk-ant-abcdefghijklmnop1234 mail a.b@x.io or me\\@gmail.com call +1 (555) 123-4567 token=hunter2secret k AbC3dEf4GhI5jKl6MnO7pQr8StU9vWx0Yz ok");
        for gone in ["sk-ant", "a.b@x.io", "gmail", "555", "hunter2", "AbC3dEf4"] {
            assert!(!s.contains(gone), "{gone} left in {s}");
        }
        // Ordinary text survives: dates, versions, paths, task-words.
        let kept = "On 2026-10-07 v1.2.3 cost $1,234.56 in /Users/me/src/very_long_project_folder_name_here/file.rs; ask-again task-readiness";
        assert_eq!(scrub(kept), kept);
    }

    #[test]
    fn drops_automated_prompts() {
        assert_eq!(own_words("Implement the plan above. Work through it"), None);
        assert_eq!(own_words("[Bomb Code session recovery — history-only mode] ..."), None);
        assert_eq!(own_words("Feature: Lobbies (F-mp-lobby)\nLet people…"), None);
        assert_eq!(own_words("make it blue\nAttached file (open it from this path):\n- /x.png (2 KB)").as_deref(), Some("make it blue"));
    }

    #[test]
    fn reads_model_json_around_prose() {
        let reply = "Here you go:\n```json\n[{\"section\":\"questions\",\"claim\":\"Asks if it's pushed\",\"quote\":\"is it pushed?\",\"thread\":\"T2\"},{\"section\":\"nope\",\"claim\":\"x\",\"thread\":\"T1\"},{\"bad\":1}]\n```";
        let obs = parse_observations(reply);
        assert_eq!(obs.len(), 1);
        assert_eq!((obs[0].thread.as_str(), obs[0].strength), ("T2", 1));
    }

    #[test]
    fn a_point_needs_two_conversations() {
        let p = |threads: &[&str]| Point { text: "x".into(), evidence: threads.iter().map(|t| Evidence { quote: "q".into(), thread: t.to_string() }).collect() };
        assert_eq!(supported(vec![p(&["T1", "T1"]), p(&["T1", "T2"])]).len(), 1);
    }

    #[test]
    fn assembles_a_skill_with_evidence_dates() {
        let points = vec![Point { text: "Asks whether work is pushed.".into(), evidence: vec![Evidence { quote: "is it pushed?".into(), thread: "T1".into() }, Evidence { quote: "push it".into(), thread: "T2".into() }] }];
        let dates = HashMap::from([("T1", "Oct 6, 2026"), ("T2", "Oct 7, 2026")]);
        let (skill, evidence) = assemble("Max", &[("Questions they ask", points), ("Reactions", vec![])], &dates, 2);
        assert!(skill.starts_with("---\nname: perspective\n"));
        assert!(skill.contains("## Questions they ask\n\n- Asks whether work is pushed.") && !skill.contains("Reactions"));
        assert!(evidence.contains("\"is it pushed?\" (Oct 6, 2026)"));
    }

    #[test]
    fn chunks_spread_notes_evenly() {
        let o = |i: usize| Observation { section: "values".into(), claim: format!("c{i}"), quote: String::new(), thread: format!("T{i}"), strength: 1 };
        let notes: Vec<Observation> = (0..130).map(o).collect();
        let chunks = chunk_notes(&notes);
        assert_eq!(chunks.iter().map(Vec::len).collect::<Vec<_>>(), [44, 43, 43]);
        // Interleaved: each chunk spans early and late threads.
        assert_eq!((chunks[1][0].thread.as_str(), chunks[1].last().unwrap().thread.as_str()), ("T1", "T127"));
        assert!(chunk_notes(&notes[..1]).is_empty());
        assert_eq!(chunk_notes(&notes[..60]).len(), 1);
    }

    #[test]
    fn batches_by_size() {
        let t = |n: usize| ThreadDigest { id: Uuid::new_v4(), label: "T".into(), date: String::new(), upto: String::new(), text: "x".repeat(n) };
        let (a, b, c) = (t(BATCH_CHARS - 10), t(5), t(BATCH_CHARS));
        assert_eq!(batch(&[&a, &b, &c]).iter().map(Vec::len).collect::<Vec<_>>(), [2, 1]);
    }
}



