//! Claude usage is shared across app processes; a temporary outage keeps the last good bars.
use super::AccountUsage;
use serde::{Deserialize, Serialize};
use std::{collections::hash_map::DefaultHasher, fs::OpenOptions, future::Future, hash::{Hash, Hasher}, io::{Read, Seek, SeekFrom, Write}, path::Path};

#[derive(Default, Serialize, Deserialize)]
struct Cache {
    credential: u64,
    retry_at: i64,
    failures: u32,
    usage: AccountUsage,
}

pub async fn claude<F, Fut>(token: &str, fetch: F) -> AccountUsage
where F: FnOnce() -> Fut, Fut: Future<Output = AccountUsage> {
    let Some(home) = std::env::var_os("HOME") else { return fetch().await; };
    let path = std::path::PathBuf::from(home).join(".grok/control-panel/cache/claude-usage.json");
    // A non-secret fingerprint prevents retaining another account's usage after sign-in changes.
    let mut fingerprint = DefaultHasher::new();
    token.hash(&mut fingerprint);
    cached(&path, fingerprint.finish(), chrono::Utc::now().timestamp(), fetch).await
}

async fn cached<F, Fut>(path: &Path, credential: u64, now: i64, fetch: F) -> AccountUsage
where F: FnOnce() -> Fut, Fut: Future<Output = AccountUsage> {
    let fallback = || AccountUsage {backend:"claude".into(), error:Some("Usage refresh pending".into()), ..Default::default()};
    if let Some(parent) = path.parent() {
        if std::fs::create_dir_all(parent).is_err() { return fallback(); }
    }
    let mut options = OpenOptions::new();
    options.read(true).write(true).create(true).truncate(false);
    #[cfg(unix)] {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let Ok(mut file) = options.open(path) else { return fallback(); };
    // Nonblocking advisory lock is released on process exit as well as normal return.
    let locked = file.try_lock().is_ok();
    let mut text = String::new();
    let _ = file.read_to_string(&mut text);
    let mut cache: Cache = serde_json::from_str(&text).unwrap_or_default();
    if cache.credential != credential { cache = Cache::default(); }
    if !locked { return if cache.usage.backend.is_empty() { fallback() } else { cache.usage }; }
    if cache.retry_at > now { return cache.usage; }
    let fresh = fetch().await;
    if fresh.error.is_some() {
        cache.failures = cache.failures.saturating_add(1);
        if fresh.error.as_deref().is_some_and(|e| e.contains("sign-in")) { cache.usage.windows.clear(); }
        cache.usage.backend = fresh.backend;
        cache.usage.error = fresh.error;
    } else {
        cache.failures = 0;
        cache.usage = fresh;
    }
    cache.credential = credential;
    cache.retry_at = now + if cache.failures == 0 { 120 } else { (300_i64 * 2_i64.pow(cache.failures.saturating_sub(1).min(4))).min(3600) };
    if let Ok(bytes) = serde_json::to_vec(&cache) {
        let _ = file.seek(SeekFrom::Start(0));
        let _ = file.set_len(0);
        let _ = file.write_all(&bytes);
    }
    cache.usage
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::usage::UsageWindow;

    fn good() -> AccountUsage {
        AccountUsage { backend:"claude".into(), windows:vec![UsageWindow {label:"5h".into(),used_pct:24.,resets_at:None}], ..Default::default() }
    }
    #[tokio::test]
    async fn caches_success_and_keeps_stale_bars_during_backoff() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("usage.json");
        assert_eq!(cached(&path, 1, 1000, || async { good() }).await, good());
        assert_eq!(cached(&path, 1, 1001, || async { panic!("duplicate request") }).await, good());
        let limited = cached(&path, 1, 1121, || async { AccountUsage {backend:"claude".into(),error:Some("Usage rate limited; retrying later".into()),..Default::default()} }).await;
        assert_eq!(limited.windows, good().windows);
        assert!(limited.error.is_some());
        assert_eq!(cached(&path, 1, 1300, || async { panic!("ignored backoff") }).await, limited);
        let switched = cached(&path, 2, 1301, || async { AccountUsage {backend:"claude".into(),error:Some("Usage unavailable".into()),..Default::default()} }).await;
        assert!(switched.windows.is_empty());
    }
    #[tokio::test]
    async fn concurrent_instances_do_not_fetch() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("usage.json");
        let file = OpenOptions::new().read(true).write(true).create(true).truncate(false).open(&path).unwrap();
        file.lock().unwrap();
        let result = cached(&path, 1, 1000, || async { panic!("another instance is fetching") }).await;
        assert_eq!(result.error.as_deref(), Some("Usage refresh pending"));
    }
}
