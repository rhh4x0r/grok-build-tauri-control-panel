//! Turn-phase state machine — the single source of truth for "what is the
//! agent doing right now" in a thread. Ported from `frontend/presence.js`
//! with the flavor puns, mood debounce and stage rail removed: the new UI
//! shows one status line and one meter bar, nothing else.
//!
//! Pure data + functions; no clocks, no I/O. Callers pass `now` explicitly
//! so the machine is deterministic under test.

use std::time::{Duration, Instant};

/// Silence longer than this during an active turn counts as a stall.
pub const STALL_AFTER: Duration = Duration::from_secs(25);
/// How long the "boom" (done) state is held before returning to idle.
pub const BOOM_HOLD: Duration = Duration::from_millis(1000);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum Phase {
    #[default]
    Idle,
    Send,
    Think,
    Tools,
    Reply,
    Wait,
    Done,
    Error,
}

impl Phase {
    /// Ranked so a late, lower-ranked signal never drags the phase backwards.
    fn rank(self) -> u8 {
        match self {
            Phase::Idle => 0,
            Phase::Send => 1,
            Phase::Think => 2,
            Phase::Tools => 3,
            Phase::Reply => 4,
            Phase::Wait => 5,
            Phase::Done | Phase::Error => 6,
        }
    }

    /// Phases that end a turn (or reset it) and are always accepted.
    fn is_terminal(self) -> bool {
        matches!(self, Phase::Wait | Phase::Error | Phase::Done | Phase::Idle)
    }
}

/// Why a turn has gone quiet.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stall {
    /// The agent is waiting on the user (approval). Not a fault.
    AwaitingUser,
    /// A tool is still marked running with no output.
    ToolHang,
    /// Nothing at all has arrived since the prompt.
    NoFirstSignal,
    /// Streaming started and then went silent.
    StreamGap,
}

/// What the meter bar should draw.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum MeterMode {
    Idle,
    /// Sweeping fuse: prompt sent, no first token or tool yet.
    Indeterminate,
    /// Reply streaming; fill = asymptotic progress from reply chars.
    Progress(f32),
    /// Tools running; fill grows with tool count.
    Tools(f32),
    Stall,
    Complete,
    Error,
}

/// Visual mood for the single bomb sprite next to the status line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mood {
    Idle,
    Thinking,
    Tooling,
    Stream,
    Wait,
    Boom,
    Error,
}

#[derive(Debug, Clone, Default)]
pub struct Presence {
    pub phase: Phase,
    pub started_at: Option<Instant>,
    pub last_signal_at: Option<Instant>,
    pub prompt_chars: usize,
    pub thought_chars: usize,
    pub reply_chars: usize,
    pub context_tokens: Option<u64>,
    pub tool_count: usize,
    pub tools_active: usize,
    pub last_tool: Option<String>,
    pub last_tool_status: Option<String>,
    /// Short free-text detail (last tool args, approval summary, error).
    pub note: String,
}

/// Incremental patch applied together with a phase signal.
#[derive(Debug, Clone, Default)]
pub struct Patch {
    pub prompt_chars: Option<usize>,
    pub add_thought_chars: usize,
    pub add_reply_chars: usize,
    pub context_tokens: Option<u64>,
    pub last_tool: Option<String>,
    pub last_tool_status: Option<String>,
    pub tools_active: Option<usize>,
    pub note: Option<String>,
}

impl Presence {
    pub fn turn_active(&self) -> bool {
        matches!(
            self.phase,
            Phase::Send | Phase::Think | Phase::Tools | Phase::Reply | Phase::Wait
        )
    }

    /// True while there is anything worth showing in the status line.
    pub fn visible(&self) -> bool {
        self.turn_active() || matches!(self.phase, Phase::Done | Phase::Error)
    }

    pub fn elapsed(&self, now: Instant) -> Option<Duration> {
        self.started_at.map(|s| now.saturating_duration_since(s))
    }

    /// Apply a phase signal plus a patch. Mirrors `applySignal` in presence.js:
    /// patches land first, tools stay sticky while in flight, and terminal
    /// phases are always accepted; anything else only advances by rank.
    pub fn signal(&mut self, phase: Phase, patch: Patch, now: Instant) {
        if self.started_at.is_none() && phase != Phase::Idle && phase != Phase::Done {
            self.started_at = Some(now);
        }

        if let Some(n) = patch.prompt_chars {
            self.prompt_chars = n;
        }
        self.thought_chars += patch.add_thought_chars;
        self.reply_chars += patch.add_reply_chars;
        if patch.context_tokens.is_some() {
            self.context_tokens = patch.context_tokens;
        }
        if patch.last_tool.is_some() {
            self.last_tool = patch.last_tool;
        }
        if patch.last_tool_status.is_some() {
            self.last_tool_status = patch.last_tool_status;
        }
        if let Some(n) = patch.tools_active {
            self.tools_active = n;
        }
        if let Some(n) = patch.note {
            self.note = n;
        }

        let sticky_tools =
            self.tools_active > 0 && phase == Phase::Reply && self.phase == Phase::Tools;

        // A tool starting mid-reply is a real signal, not a regression.
        let tools_after_reply = phase == Phase::Tools && self.phase == Phase::Reply;

        if phase.is_terminal() {
            self.phase = phase;
        } else if sticky_tools {
            // stay on tools; patches already applied
        } else if tools_after_reply
            || phase.rank() >= self.phase.rank()
            || self.phase == Phase::Wait
        {
            self.phase = phase;
        }

        self.last_signal_at = Some(now);

        if phase == Phase::Idle {
            *self = Presence::default();
        }
    }

    pub fn tool_started(&mut self, tool: &str, now: Instant) {
        self.tool_count += 1;
        let active = self.tools_active + 1;
        self.signal(
            Phase::Tools,
            Patch {
                last_tool: Some(tool.to_string()),
                last_tool_status: Some("running".into()),
                tools_active: Some(active),
                note: Some(tool.to_string()),
                ..Default::default()
            },
            now,
        );
    }

    pub fn tool_finished(&mut self, tool: &str, status: &str, now: Instant) {
        let active = self.tools_active.saturating_sub(1);
        let next = if active > 0 || self.reply_chars == 0 {
            Phase::Tools
        } else {
            Phase::Reply
        };
        self.signal(
            next,
            Patch {
                last_tool: Some(tool.to_string()),
                last_tool_status: Some(status.to_string()),
                tools_active: Some(active),
                note: Some(tool.to_string()),
                ..Default::default()
            },
            now,
        );
    }

    /// Stall classification for `now`. `None` when the turn is healthy.
    pub fn stall(&self, now: Instant) -> Option<Stall> {
        if self.phase == Phase::Wait {
            return Some(Stall::AwaitingUser);
        }
        if !self.turn_active() {
            return None;
        }
        let last = self.last_signal_at?;
        if now.saturating_duration_since(last) < STALL_AFTER {
            return None;
        }
        let st = self
            .last_tool_status
            .as_deref()
            .unwrap_or("")
            .to_ascii_lowercase();
        let tool_busy = self.tools_active > 0
            || (self.last_tool.is_some()
                && !st.is_empty()
                && !st.contains("complete")
                && !st.contains("done")
                && !st.contains("fail")
                && !st.contains("success"));
        if tool_busy {
            return Some(Stall::ToolHang);
        }
        if self.reply_chars == 0 && self.thought_chars == 0 && self.tool_count == 0 {
            return Some(Stall::NoFirstSignal);
        }
        Some(Stall::StreamGap)
    }

    /// A stall that is the agent's fault (not "waiting for you").
    pub fn stalled(&self, now: Instant) -> bool {
        matches!(
            self.stall(now),
            Some(Stall::ToolHang | Stall::NoFirstSignal | Stall::StreamGap)
        )
    }

    pub fn mood(&self, now: Instant) -> Mood {
        if self.stalled(now) {
            // Calm, never a panic shake: keep the phase's own mood.
            return match self.phase {
                Phase::Tools => Mood::Tooling,
                Phase::Reply => Mood::Stream,
                _ => Mood::Thinking,
            };
        }
        match self.phase {
            Phase::Idle => Mood::Idle,
            Phase::Send | Phase::Think => Mood::Thinking,
            Phase::Tools => Mood::Tooling,
            Phase::Reply => Mood::Stream,
            Phase::Wait => Mood::Wait,
            Phase::Done => Mood::Boom,
            Phase::Error => Mood::Error,
        }
    }

    /// Soft progress 0–1: asymptotic in reply chars, stepped in tool count.
    pub fn progress(&self) -> f32 {
        match self.phase {
            Phase::Done | Phase::Error => 1.0,
            _ if self.reply_chars > 0 => {
                (1.0 - (-(self.reply_chars as f32) / 800.0).exp()).min(0.92)
            }
            _ if self.tool_count > 0 => (0.15 + self.tool_count as f32 * 0.12).min(0.85),
            _ => 0.0,
        }
    }

    pub fn meter(&self, now: Instant) -> MeterMode {
        match self.phase {
            Phase::Done => MeterMode::Complete,
            Phase::Error => MeterMode::Error,
            Phase::Idle => MeterMode::Idle,
            _ if self.stalled(now) => MeterMode::Stall,
            Phase::Reply if self.reply_chars > 0 => MeterMode::Progress(self.progress()),
            Phase::Tools if self.tool_count > 0 => MeterMode::Tools(self.progress()),
            _ => MeterMode::Indeterminate,
        }
    }

    /// The one status label the UI shows.
    pub fn label(&self, now: Instant) -> String {
        match self.stall(now) {
            Some(Stall::AwaitingUser) => return "Needs you".into(),
            Some(Stall::ToolHang) => return "Quiet · tool".into(),
            Some(Stall::NoFirstSignal | Stall::StreamGap) => return "Quiet".into(),
            None => {}
        }
        match self.phase {
            Phase::Tools => match &self.last_tool {
                Some(t) => format!("Running · {t}"),
                None => "Running".into(),
            },
            Phase::Reply => "Writing".into(),
            Phase::Think => "Thinking".into(),
            Phase::Send => "Sent".into(),
            Phase::Done => "Done".into(),
            Phase::Error => "Failed".into(),
            Phase::Wait => "Needs you".into(),
            Phase::Idle => "Idle".into(),
        }
    }
}

/// `1m 4s` style elapsed formatting.
pub fn format_elapsed(d: Duration) -> String {
    let sec = d.as_secs();
    if sec < 60 {
        return format!("{sec}s");
    }
    let m = sec / 60;
    let s = sec % 60;
    if m < 60 {
        return format!("{m}m {s}s");
    }
    format!("{}h {}m", m / 60, m % 60)
}

/// `1.2k` style counts.
pub fn format_count(n: usize) -> String {
    if n < 1000 {
        return n.to_string();
    }
    if n < 10_000 {
        format!("{:.1}k", n as f64 / 1000.0)
    } else {
        format!("{:.0}k", n as f64 / 1000.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t0() -> Instant {
        Instant::now()
    }

    #[test]
    fn walks_through_a_turn_with_sticky_tools() {
        let now = t0();
        let mut p = Presence::default();
        p.signal(
            Phase::Send,
            Patch {
                prompt_chars: Some(10),
                ..Default::default()
            },
            now,
        );
        assert_eq!(p.phase, Phase::Send);
        p.signal(Phase::Think, Patch::default(), now);
        assert_eq!(p.phase, Phase::Think);
        p.tool_started("read_file", now);
        assert_eq!(p.phase, Phase::Tools);
        assert_eq!(p.tools_active, 1);
        p.signal(
            Phase::Reply,
            Patch {
                add_reply_chars: 5,
                ..Default::default()
            },
            now,
        );
        assert_eq!(p.phase, Phase::Tools, "tools stay sticky while in flight");
        p.tool_finished("read_file", "completed", now);
        assert_eq!(p.tools_active, 0);
        assert_eq!(p.phase, Phase::Reply, "reply resumes after the tool");
    }

    #[test]
    fn tool_after_reply_moves_to_tools_and_back() {
        let now = t0();
        let mut p = Presence::default();
        p.signal(
            Phase::Reply,
            Patch {
                add_reply_chars: 40,
                ..Default::default()
            },
            now,
        );
        p.tool_started("Edit", now);
        assert_eq!(p.phase, Phase::Tools);
        p.tool_finished("Edit", "completed", now);
        assert_eq!(p.phase, Phase::Reply);
    }

    #[test]
    fn failed_tool_is_terminal() {
        let now = t0();
        let mut p = Presence::default();
        p.tool_started("x", now);
        p.tool_finished("x", "failed", now);
        assert_eq!(p.tools_active, 0);
    }

    #[test]
    fn stall_is_calm_not_panicked() {
        let now = t0();
        let mut p = Presence::default();
        p.signal(Phase::Send, Patch::default(), now);
        p.signal(
            Phase::Reply,
            Patch {
                add_reply_chars: 20,
                ..Default::default()
            },
            now,
        );
        let later = now + Duration::from_secs(30);
        assert_eq!(p.stall(later), Some(Stall::StreamGap));
        assert_eq!(p.mood(later), Mood::Stream);
        assert_eq!(p.meter(later), MeterMode::Stall);
        assert_eq!(p.label(later), "Quiet");
    }

    #[test]
    fn no_first_signal_and_tool_hang() {
        let now = t0();
        let mut p = Presence::default();
        p.signal(Phase::Send, Patch::default(), now);
        p.signal(Phase::Think, Patch::default(), now);
        let later = now + STALL_AFTER;
        assert_eq!(p.stall(later), Some(Stall::NoFirstSignal));
        p.tool_started("Bash", later);
        let later2 = later + STALL_AFTER;
        assert_eq!(p.stall(later2), Some(Stall::ToolHang));
        assert_eq!(p.label(later2), "Quiet · tool");
    }

    #[test]
    fn done_is_boom_and_complete() {
        let now = t0();
        let mut p = Presence::default();
        p.signal(Phase::Send, Patch::default(), now);
        p.signal(Phase::Done, Patch::default(), now);
        assert_eq!(p.mood(now), Mood::Boom);
        assert_eq!(p.meter(now), MeterMode::Complete);
        assert_eq!(p.progress(), 1.0);
    }

    #[test]
    fn wait_is_awaiting_user_immediately() {
        let now = t0();
        let mut p = Presence::default();
        p.signal(Phase::Think, Patch::default(), now);
        p.signal(Phase::Wait, Patch::default(), now);
        assert_eq!(p.stall(now), Some(Stall::AwaitingUser));
        assert!(!p.stalled(now));
        assert_eq!(p.label(now), "Needs you");
        assert_eq!(p.meter(now), MeterMode::Indeterminate);
        // wait accepts a lower-ranked phase afterwards (approval resolved)
        p.signal(Phase::Think, Patch::default(), now);
        assert_eq!(p.phase, Phase::Think);
    }

    #[test]
    fn phase_never_regresses_by_rank() {
        let now = t0();
        let mut p = Presence::default();
        p.signal(
            Phase::Reply,
            Patch {
                add_reply_chars: 3,
                ..Default::default()
            },
            now,
        );
        p.signal(Phase::Think, Patch::default(), now);
        assert_eq!(p.phase, Phase::Reply);
    }

    #[test]
    fn meter_modes_and_progress_formula() {
        let now = t0();
        let mut p = Presence::default();
        p.signal(Phase::Send, Patch::default(), now);
        assert_eq!(p.meter(now), MeterMode::Indeterminate);
        p.signal(
            Phase::Reply,
            Patch {
                add_reply_chars: 800,
                ..Default::default()
            },
            now,
        );
        let expected = (1.0 - (-1.0f32).exp()).min(0.92);
        assert_eq!(p.meter(now), MeterMode::Progress(expected));
        let mut q = Presence::default();
        q.tool_started("a", now);
        q.tool_started("b", now);
        assert_eq!(q.meter(now), MeterMode::Tools(0.15 + 2.0 * 0.12));
    }

    #[test]
    fn idle_resets_everything() {
        let now = t0();
        let mut p = Presence::default();
        p.signal(
            Phase::Reply,
            Patch {
                add_reply_chars: 9,
                ..Default::default()
            },
            now,
        );
        p.signal(Phase::Idle, Patch::default(), now);
        assert_eq!(p.phase, Phase::Idle);
        assert_eq!(p.reply_chars, 0);
        assert!(p.started_at.is_none());
    }

    #[test]
    fn formatting_helpers() {
        assert_eq!(format_elapsed(Duration::from_secs(5)), "5s");
        assert_eq!(format_elapsed(Duration::from_secs(64)), "1m 4s");
        assert_eq!(format_elapsed(Duration::from_secs(3725)), "1h 2m");
        assert_eq!(format_count(999), "999");
        assert_eq!(format_count(1234), "1.2k");
        assert_eq!(format_count(12345), "12k");
    }
}
