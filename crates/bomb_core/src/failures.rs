//! Failed threads the user has already dealt with. A thread's "Failed" badge
//! works like the unseen dot: it shows until the thread is opened or the
//! failure is dismissed, and comes back only if the thread fails again.
//!
//! The error itself stays in the transcript; this only remembers which
//! failure was acknowledged. Persisted in the kv table under [`DISMISSED_KEY`].

use std::collections::HashMap;

use chrono::{DateTime, Utc};
use grok_persistence::Persistence;
use uuid::Uuid;

/// JSON map of thread id → `updated_at` of the failure that was dismissed.
pub const DISMISSED_KEY: &str = "failed_dismissed";

#[derive(Debug, Clone, Default, PartialEq)]
pub struct DismissedFailures {
    by_thread: HashMap<Uuid, DateTime<Utc>>,
}

impl DismissedFailures {
    /// Unreadable or missing data means nothing was dismissed.
    pub fn parse(raw: Option<&str>) -> Self {
        let by_thread = raw.and_then(|r| serde_json::from_str(r).ok()).unwrap_or_default();
        Self { by_thread }
    }

    pub fn to_json(&self) -> String {
        serde_json::to_string(&self.by_thread).unwrap_or_else(|_| "{}".into())
    }

    /// Hide the failure a thread had at `failed_at` (its `updated_at`). Returns
    /// whether anything changed, so callers only save when it did.
    pub fn dismiss(&mut self, id: Uuid, failed_at: &str) -> bool {
        let Ok(at) = failed_at.parse::<DateTime<Utc>>() else { return false };
        if self.by_thread.get(&id).is_some_and(|d| *d >= at) { return false; }
        self.by_thread.insert(id, at);
        true
    }

    /// Fold in another copy (e.g. the one on disk); the later dismissal wins.
    pub fn merge(&mut self, other: &DismissedFailures) {
        for (id, at) in &other.by_thread {
            self.by_thread.entry(*id).and_modify(|d| *d = (*d).max(*at)).or_insert(*at);
        }
    }

    /// The failure at `updated_at` was dismissed; anything later was not.
    pub fn is_dismissed(&self, id: Uuid, updated_at: &str) -> bool {
        let Ok(at) = updated_at.parse::<DateTime<Utc>>() else { return false };
        self.by_thread.get(&id).is_some_and(|d| at <= *d)
    }

    /// Whether a thread with this saved status should still show its "Failed" badge.
    pub fn shows_failed(&self, id: Uuid, status: &str, updated_at: &str) -> bool {
        status == "failed" && !self.is_dismissed(id, updated_at)
    }

    pub fn load(db: &Persistence) -> grok_persistence::Result<Self> {
        Ok(Self::parse(db.get_kv(DISMISSED_KEY)?.as_deref()))
    }

    /// Record one dismissal on disk, merged with whatever is already saved.
    pub fn save_one(db: &Persistence, id: Uuid, failed_at: &str) -> grok_persistence::Result<Self> {
        let mut saved = Self::load(db)?;
        if saved.dismiss(id, failed_at) { db.set_kv(DISMISSED_KEY, &saved.to_json())?; }
        Ok(saved)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const T1: &str = "2026-10-08T10:00:00+00:00";
    const T2: &str = "2026-10-08T11:00:00+00:00";

    #[test]
    fn dismissed_failure_is_hidden_and_a_later_one_shows_again() {
        let id = Uuid::new_v4();
        let mut d = DismissedFailures::default();
        assert!(d.shows_failed(id, "failed", T1));
        assert!(d.dismiss(id, T1));
        assert!(!d.shows_failed(id, "failed", T1));
        // The thread failed again after the dismissal.
        assert!(d.shows_failed(id, "failed", T2));
        // Only failed threads ever show the badge.
        assert!(!d.shows_failed(id, "idle", T2));
        // Other threads are unaffected.
        assert!(d.shows_failed(Uuid::new_v4(), "failed", T1));
    }

    #[test]
    fn dismissals_only_move_forward() {
        let id = Uuid::new_v4();
        let mut d = DismissedFailures::default();
        assert!(d.dismiss(id, T2));
        assert!(!d.dismiss(id, T1));
        assert!(!d.dismiss(id, T2));
        assert!(!d.shows_failed(id, "failed", T1));
        assert!(!d.dismiss(id, "not a time"));
        let mut older = DismissedFailures::default();
        older.dismiss(id, T1);
        older.merge(&d);
        assert!(older.is_dismissed(id, T2));
    }

    #[test]
    fn bad_saved_data_means_nothing_dismissed() {
        assert_eq!(DismissedFailures::parse(None), DismissedFailures::default());
        assert_eq!(DismissedFailures::parse(Some("not json")), DismissedFailures::default());
    }

    #[test]
    fn dismissals_survive_a_restart() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("bomb.db");
        let (a, b) = (Uuid::new_v4(), Uuid::new_v4());
        {
            let db = Persistence::open(&path).unwrap();
            DismissedFailures::save_one(&db, a, T1).unwrap();
            let both = DismissedFailures::save_one(&db, b, T2).unwrap();
            assert!(both.is_dismissed(a, T1) && both.is_dismissed(b, T2));
        }
        let db = Persistence::open(&path).unwrap();
        let loaded = DismissedFailures::load(&db).unwrap();
        assert!(!loaded.shows_failed(a, "failed", T1));
        assert!(!loaded.shows_failed(b, "failed", T2));
        assert!(loaded.shows_failed(a, "failed", T2));
    }
}
