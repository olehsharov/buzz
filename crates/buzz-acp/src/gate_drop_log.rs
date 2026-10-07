//! Rate limit for the inbound author gate's "dropping event" log line.
//!
//! A dropped event is the only trace an operator gets when an agent silently
//! ignores someone (a DM from a non-owner, a stranger under owner-only), so
//! the line is logged at `info`. Anyone who can post in a channel the agent
//! reads can trigger it, though, so each (author, channel) pair is logged at
//! most once per window; repeats inside the window are counted and reported
//! with the next logged drop. The tracked pairs are capped, so a flood of
//! distinct authors cannot grow memory or the log without bound either.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use uuid::Uuid;

/// How long a logged (author, channel) pair stays quiet.
const WINDOW: Duration = Duration::from_secs(60);
/// Most (author, channel) pairs tracked at once.
const MAX_TRACKED: usize = 512;

struct Entry {
    logged_at: Instant,
    suppressed: u64,
}

pub(crate) struct DropLogLimiter {
    window: Duration,
    max_tracked: usize,
    entries: HashMap<(String, Uuid), Entry>,
}

impl Default for DropLogLimiter {
    fn default() -> Self {
        Self::new(WINDOW, MAX_TRACKED)
    }
}

impl DropLogLimiter {
    pub(crate) fn new(window: Duration, max_tracked: usize) -> Self {
        Self {
            window,
            max_tracked,
            entries: HashMap::new(),
        }
    }

    /// Record a dropped event. `Some(suppressed)` means log this drop at
    /// `info`, reporting how many drops of the same pair were suppressed since
    /// its last logged one; `None` means it falls inside the pair's window.
    pub(crate) fn record(&mut self, author: &str, channel_id: Uuid, now: Instant) -> Option<u64> {
        let key = (author.to_string(), channel_id);
        if let Some(entry) = self.entries.get_mut(&key) {
            if now.saturating_duration_since(entry.logged_at) < self.window {
                entry.suppressed = entry.suppressed.saturating_add(1);
                return None;
            }
            let suppressed = entry.suppressed;
            *entry = Entry {
                logged_at: now,
                suppressed: 0,
            };
            return Some(suppressed);
        }
        if self.entries.len() >= self.max_tracked {
            let window = self.window;
            self.entries
                .retain(|_, entry| now.saturating_duration_since(entry.logged_at) < window);
            if self.entries.len() >= self.max_tracked {
                // Every tracked pair is still inside its window: this many
                // distinct senders in one window is itself the flood the cap
                // exists for, so stay quiet until a slot frees up.
                return None;
            }
        }
        self.entries.insert(
            key,
            Entry {
                logged_at: now,
                suppressed: 0,
            },
        );
        Some(0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_pair_logs_once_per_window_and_reports_what_it_suppressed() {
        let mut limiter = DropLogLimiter::new(Duration::from_secs(60), 8);
        let channel = Uuid::new_v4();
        let start = Instant::now();

        assert_eq!(limiter.record("stranger", channel, start), Some(0));
        assert_eq!(limiter.record("stranger", channel, start), None);
        assert_eq!(
            limiter.record("stranger", channel, start + Duration::from_secs(59)),
            None
        );
        assert_eq!(
            limiter.record("stranger", channel, start + Duration::from_secs(60)),
            Some(2)
        );
    }

    #[test]
    fn other_authors_and_channels_log_independently() {
        let mut limiter = DropLogLimiter::new(Duration::from_secs(60), 8);
        let (first, second) = (Uuid::new_v4(), Uuid::new_v4());
        let now = Instant::now();

        assert_eq!(limiter.record("stranger", first, now), Some(0));
        assert_eq!(limiter.record("stranger", second, now), Some(0));
        assert_eq!(limiter.record("someone-else", first, now), Some(0));
    }

    #[test]
    fn tracked_pairs_are_bounded() {
        let mut limiter = DropLogLimiter::new(Duration::from_secs(60), 2);
        let channel = Uuid::new_v4();
        let now = Instant::now();

        assert_eq!(limiter.record("a", channel, now), Some(0));
        assert_eq!(limiter.record("b", channel, now), Some(0));
        // Full of pairs still inside their window: a third sender is quiet.
        assert_eq!(limiter.record("c", channel, now), None);
        assert_eq!(limiter.entries.len(), 2);

        // Once the window passes, expired pairs make room again.
        let later = now + Duration::from_secs(61);
        assert_eq!(limiter.record("c", channel, later), Some(0));
        assert!(limiter.entries.len() <= 2);
    }
}
