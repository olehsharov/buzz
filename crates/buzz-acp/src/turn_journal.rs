//! Durable journal of channel turns in flight, so a turn the harness could
//! not finish (shutdown, crash) resumes in the same provider session when the
//! harness starts again.
//!
//! ```text
//! run_prompt_task ── begin() before session/prompt ──► record (atomic write)
//!        │ turn completes, or ends while the harness keeps running
//!        └──► TurnJournalGuard drop ──► record removed
//!        │ harness shutting down and the turn did not complete
//!        └──► record kept
//! next start ── plan_restore() ──► requeue once in the recorded session
//! ```
//!
//! One record per session scope lives under the XDG state directory next to
//! the resume-session records (`<state>/buzz-acp/turn-journal/<agent>/`). A
//! record holds the scope, the provider session id, the cwd, the batch's
//! signed events (enough to re-prompt), a timestamp and how many automatic
//! resumes it already had. Each record is resumed at most once: the restore
//! pass stamps `resume_attempts = 1` durably before requeueing, and the
//! resumed turn's own record inherits it, so a restart loop surfaces a notice
//! instead of re-running forever. The record of a resumed turn is replaced
//! only by the resumed turn's own record, written once its prompt is about
//! to be sent, and removed only when that turn ends — never before the
//! retry is under way.

use std::collections::HashMap;
use std::io::{self, Write as _};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::queue::{BatchEvent, CancelReason, FlushBatch, ResolvedEdit, ThreadTags};
use crate::scope::SessionScope;

/// Subdirectory of the harness state root holding the per-agent journals.
const JOURNAL_SUBDIR: &str = "turn-journal";
/// Record format version.
const RECORD_VERSION: u32 = 1;
/// Records older than this are not resumed automatically.
pub const MAX_RESUME_AGE_SECS: u64 = 24 * 60 * 60;

/// Durable record of one interrupted channel turn.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TurnRecord {
    version: u32,
    channel_id: Uuid,
    /// Thread root for a thread scope; `None` for a conversation scope.
    root_event_id: Option<String>,
    /// Provider session the turn ran in.
    pub session_id: String,
    /// Harness cwd the session belongs to.
    pub cwd: String,
    /// Unix seconds when the turn's prompt was sent.
    pub created_at: u64,
    /// Automatic resumes already attempted for this turn (0 or 1).
    pub resume_attempts: u32,
    events: Vec<RecordedEvent>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct RecordedEvent {
    event: nostr::Event,
    prompt_tag: String,
    edit: Option<RecordedEdit>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct RecordedEdit {
    target_event_id: String,
    root_event_id: Option<String>,
    parent_event_id: Option<String>,
    mentioned_pubkeys: Vec<String>,
}

impl TurnRecord {
    /// Record the batch a turn in `session_id` is about to run. Every event in
    /// the batch — new and previously cancelled — was part of the prompt.
    /// A batch that is itself a restart resume carries one used attempt.
    pub fn for_turn(batch: &FlushBatch, session_id: &str, cwd: &str, created_at: u64) -> Self {
        let resume_attempts = u32::from(batch.cancel_reason == Some(CancelReason::Restart));
        let events = batch
            .cancelled_events
            .iter()
            .chain(batch.events.iter())
            .map(|be| RecordedEvent {
                event: be.event.clone(),
                prompt_tag: be.prompt_tag.clone(),
                edit: be.edit.as_ref().map(|edit| RecordedEdit {
                    target_event_id: edit.target_event_id.clone(),
                    root_event_id: edit.target_thread_tags.root_event_id.clone(),
                    parent_event_id: edit.target_thread_tags.parent_event_id.clone(),
                    mentioned_pubkeys: edit.target_thread_tags.mentioned_pubkeys.clone(),
                }),
            })
            .collect();
        let (channel_id, root_event_id) = match &batch.scope {
            SessionScope::Conversation { channel_id } => (*channel_id, None),
            SessionScope::Thread {
                channel_id,
                root_event_id,
            } => (*channel_id, Some(root_event_id.clone())),
        };
        Self {
            version: RECORD_VERSION,
            channel_id,
            root_event_id,
            session_id: session_id.to_owned(),
            cwd: cwd.to_owned(),
            created_at,
            resume_attempts,
            events,
        }
    }

    /// The session scope the turn ran in.
    pub fn scope(&self) -> SessionScope {
        match &self.root_event_id {
            None => SessionScope::Conversation {
                channel_id: self.channel_id,
            },
            Some(root) => SessionScope::Thread {
                channel_id: self.channel_id,
                root_event_id: root.clone(),
            },
        }
    }

    /// The batch that re-runs this turn, framed as a restart resume.
    pub fn to_batch(&self) -> FlushBatch {
        let now = std::time::Instant::now();
        FlushBatch {
            channel_id: self.channel_id,
            scope: self.scope(),
            events: self
                .events
                .iter()
                .map(|recorded| BatchEvent {
                    event: recorded.event.clone(),
                    prompt_tag: recorded.prompt_tag.clone(),
                    received_at: now,
                    edit: recorded.edit.as_ref().map(|edit| ResolvedEdit {
                        target_event_id: edit.target_event_id.clone(),
                        target_thread_tags: ThreadTags {
                            root_event_id: edit.root_event_id.clone(),
                            parent_event_id: edit.parent_event_id.clone(),
                            mentioned_pubkeys: edit.mentioned_pubkeys.clone(),
                        },
                    }),
                })
                .collect(),
            cancelled_events: vec![],
            cancel_reason: Some(CancelReason::Restart),
        }
    }
}

/// Per-agent directory of [`TurnRecord`]s, one file per session scope.
#[derive(Debug, Clone)]
pub struct TurnJournalStore {
    dir: PathBuf,
}

impl TurnJournalStore {
    /// Store for `agent_pubkey_hex` under the harness `state_root`.
    pub fn new(state_root: &Path, agent_pubkey_hex: &str) -> Self {
        Self {
            dir: state_root.join(JOURNAL_SUBDIR).join(agent_pubkey_hex),
        }
    }

    /// Directory holding this agent's records.
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    fn record_path(&self, scope: &SessionScope) -> PathBuf {
        let key = match scope {
            SessionScope::Conversation { channel_id } => format!("c:{channel_id}"),
            SessionScope::Thread {
                channel_id,
                root_event_id,
            } => format!("t:{channel_id}:{root_event_id}"),
        };
        let hash = hex::encode(Sha256::digest(key.as_bytes()));
        self.dir.join(format!("{}.json", &hash[..32]))
    }

    /// Atomically write `record` as its scope's record: temp file in the same
    /// directory, fsync, rename over the record, fsync the directory.
    pub fn save(&self, record: &TurnRecord) -> io::Result<()> {
        let path = self.record_path(&record.scope());
        crate::resume_store::create_private_dir_all(&self.dir)
            .map_err(|e| journal_error(e, "create", &self.dir))?;
        let json = serde_json::to_vec_pretty(record).map_err(io::Error::other)?;
        let write = || -> io::Result<()> {
            let mut tmp = tempfile::NamedTempFile::new_in(&self.dir)?;
            tmp.write_all(&json)?;
            tmp.as_file().sync_all()?;
            tmp.persist(&path).map_err(|e| e.error)?;
            crate::resume_store::sync_dir(&self.dir)
        };
        write().map_err(|e| journal_error(e, "write", &path))
    }

    /// Remove `scope`'s record. A missing record is not an error.
    pub fn remove(&self, scope: &SessionScope) -> io::Result<()> {
        let path = self.record_path(scope);
        match std::fs::remove_file(&path) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(journal_error(e, "remove", &path)),
        }
    }

    /// Every record file with its parse result. A missing directory is empty.
    /// Unreadable or corrupt records are returned as errors so the caller can
    /// surface them instead of mistaking them for "nothing to resume".
    pub fn load_all(&self) -> io::Result<Vec<(PathBuf, io::Result<TurnRecord>)>> {
        let entries = match std::fs::read_dir(&self.dir) {
            Ok(entries) => entries,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(journal_error(e, "list", &self.dir)),
        };
        let mut records = Vec::new();
        for entry in entries {
            let path = entry
                .map_err(|e| journal_error(e, "list", &self.dir))?
                .path();
            if path.extension().and_then(|e| e.to_str()) != Some("json") {
                continue;
            }
            let parsed = std::fs::read(&path)
                .and_then(|bytes| {
                    serde_json::from_slice::<TurnRecord>(&bytes)
                        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
                })
                .and_then(|record| {
                    if record.version != RECORD_VERSION || record.events.is_empty() {
                        return Err(io::Error::new(
                            io::ErrorKind::InvalidData,
                            "unsupported version or empty batch",
                        ));
                    }
                    Ok(record)
                })
                .map_err(|e| journal_error(e, "read", &path));
            records.push((path, parsed));
        }
        records.sort_by(|a, b| a.0.cmp(&b.0));
        Ok(records)
    }
}

fn journal_error(error: io::Error, action: &str, path: &Path) -> io::Error {
    io::Error::new(
        error.kind(),
        format!(
            "failed to {action} turn-journal record {}: {error}",
            path.display()
        ),
    )
}

/// Process-wide journal handle carried on the prompt context.
///
/// Disabled (no store) for isolated tasks and tests that do not opt in.
#[derive(Debug, Clone, Default)]
pub struct TurnJournal {
    store: Option<TurnJournalStore>,
    /// Fires when the harness starts shutting down. `None` = never.
    shutdown: Option<tokio::sync::watch::Receiver<()>>,
    /// Scopes whose next session must resume a restored turn's session.
    restarts: Arc<Mutex<HashMap<SessionScope, String>>>,
}

impl TurnJournal {
    /// Journal writing to `store`, keeping records of turns cut short once
    /// `shutdown` fires.
    pub fn new(store: TurnJournalStore, shutdown: tokio::sync::watch::Receiver<()>) -> Self {
        Self {
            store: Some(store),
            shutdown: Some(shutdown),
            restarts: Arc::default(),
        }
    }

    /// The backing store, when journaling is enabled.
    pub fn store(&self) -> Option<&TurnJournalStore> {
        self.store.as_ref()
    }

    /// Whether the harness has begun shutting down. A dropped sender (the
    /// harness is gone) counts as shutting down.
    pub fn shutting_down(&self) -> bool {
        self.shutdown
            .as_ref()
            .is_some_and(|rx| rx.has_changed().unwrap_or(true))
    }

    /// Record the turn about to be prompted. A write failure is returned: the
    /// caller refuses the turn visibly rather than run it without a record a
    /// restart could resume from.
    pub fn begin(
        &self,
        batch: &FlushBatch,
        session_id: &str,
        cwd: &str,
    ) -> io::Result<TurnJournalGuard> {
        let Some(store) = &self.store else {
            return Ok(TurnJournalGuard::disarmed());
        };
        let record = TurnRecord::for_turn(batch, session_id, cwd, now_unix());
        store.save(&record)?;
        Ok(TurnJournalGuard {
            journal: Some(self.clone()),
            scope: Some(batch.scope.clone()),
            completed: false,
        })
    }

    /// Remove `scope`'s record and its pending restart, logging failures.
    pub fn discard(&self, scope: &SessionScope) {
        self.finish_restart(scope);
        if let Some(store) = &self.store {
            if let Err(error) = store.remove(scope) {
                tracing::error!(target: "turn_journal", "{error}");
            }
        }
    }

    /// Mark `scope`'s next session as a resume of `session_id`.
    pub fn register_restart(&self, scope: SessionScope, session_id: String) {
        self.restarts
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(scope, session_id);
    }

    /// The session a restored turn for `scope` must resume, if any.
    pub fn restart_session(&self, scope: &SessionScope) -> Option<String> {
        self.restarts
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(scope)
            .cloned()
    }

    /// The restored session is live (or the restore was abandoned).
    pub fn finish_restart(&self, scope: &SessionScope) {
        self.restarts
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(scope);
    }
}

/// Removes a turn's record when the turn ends, unless the harness is
/// shutting down and the turn did not complete — then the record is kept for
/// the next start. Runs on every exit path, including task abort.
#[derive(Debug)]
pub struct TurnJournalGuard {
    journal: Option<TurnJournal>,
    scope: Option<SessionScope>,
    completed: bool,
}

impl TurnJournalGuard {
    /// A guard that touches nothing (heartbeat, journaling disabled).
    pub fn disarmed() -> Self {
        Self {
            journal: None,
            scope: None,
            completed: false,
        }
    }

    /// The turn ran to an end the agent chose; never resume it.
    pub fn complete(&mut self) {
        self.completed = true;
    }
}

impl Drop for TurnJournalGuard {
    fn drop(&mut self) {
        let (Some(journal), Some(scope)) = (&self.journal, &self.scope) else {
            return;
        };
        if !self.completed && journal.shutting_down() {
            tracing::info!(
                target: "turn_journal",
                scope = %scope.telemetry_label(),
                "harness is shutting down mid-turn; keeping the turn for resume on restart"
            );
            return;
        }
        if let Some(store) = &journal.store {
            if let Err(error) = store.remove(scope) {
                tracing::error!(target: "turn_journal", "{error}");
            }
        }
    }
}

/// What to do with one journal record at startup.
#[derive(Debug)]
pub enum RestoreAction {
    /// Requeue this batch; its scope's next session resumes `session_id`.
    Resume {
        batch: FlushBatch,
        session_id: String,
    },
    /// Do not resume; tell the thread why. The record is already removed.
    Notice { batch: FlushBatch, message: String },
}

/// Decide each journal record's fate at startup and make it durable: a
/// resumable record is restamped with its one automatic attempt *before* it
/// is requeued, so a crash during the resumed turn cannot resume it again.
/// Corrupt records are left in place and logged; they never block startup.
pub fn plan_restore(store: &TurnJournalStore, cwd: &str, now: u64) -> Vec<RestoreAction> {
    let records = match store.load_all() {
        Ok(records) => records,
        Err(error) => {
            tracing::error!(target: "turn_journal", "cannot read the turn journal: {error}");
            return Vec::new();
        }
    };
    let mut actions = Vec::new();
    for (path, record) in records {
        let mut record = match record {
            Ok(record) => record,
            Err(error) => {
                tracing::error!(
                    target: "turn_journal",
                    path = %path.display(),
                    "skipping unreadable turn-journal record: {error}"
                );
                continue;
            }
        };
        let batch = record.to_batch();
        let refusal = if now.saturating_sub(record.created_at) > MAX_RESUME_AGE_SECS {
            Some(
                "⚠️ My work on this was interrupted by a restart more than 24 hours ago, so I \
                 won't resume it automatically. Please re-send if it's still needed."
                    .to_string(),
            )
        } else if record.resume_attempts >= 1 {
            Some(
                "⚠️ My work on this was interrupted by a restart again after I already resumed it \
                 once, so I won't resume it automatically. Please re-send if it's still needed."
                    .to_string(),
            )
        } else if record.cwd != cwd {
            Some(format!(
                "⚠️ My work on this was interrupted by a restart, and I now run in a different \
                 directory ({cwd}, was {}), so I can't resume that session. Please re-send if \
                 it's still needed.",
                record.cwd
            ))
        } else {
            None
        };
        if let Some(message) = refusal {
            tracing::warn!(
                target: "turn_journal",
                scope = %batch.scope.telemetry_label(),
                session = %record.session_id,
                "not resuming interrupted turn: {message}"
            );
            if let Err(error) = store.remove(&batch.scope) {
                tracing::error!(target: "turn_journal", "{error}");
            }
            actions.push(RestoreAction::Notice { batch, message });
            continue;
        }
        record.resume_attempts = 1;
        if let Err(error) = store.save(&record) {
            // Without the stamp a crash could resume it again; do not resume.
            tracing::error!(
                target: "turn_journal",
                scope = %batch.scope.telemetry_label(),
                "cannot stamp the resume attempt, not resuming: {error}"
            );
            actions.push(RestoreAction::Notice {
                batch,
                message: format!(
                    "⚠️ My work on this was interrupted by a restart and I couldn't resume it \
                     ({error}). Please re-send if it's still needed."
                ),
            });
            continue;
        }
        tracing::info!(
            target: "turn_journal",
            scope = %batch.scope.telemetry_label(),
            session = %record.session_id,
            events = batch.events.len(),
            "resuming turn interrupted by a restart"
        );
        actions.push(RestoreAction::Resume {
            batch,
            session_id: record.session_id,
        });
    }
    actions
}

/// Current unix time in seconds.
pub fn now_unix() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use nostr::{EventBuilder, Keys, Kind};

    fn event(content: &str) -> nostr::Event {
        EventBuilder::new(Kind::Custom(9), content)
            .sign_with_keys(&Keys::generate())
            .unwrap()
    }

    fn batch(scope: SessionScope, reason: Option<CancelReason>) -> FlushBatch {
        let be = |content: &str, edit| BatchEvent {
            event: event(content),
            prompt_tag: "@mention".into(),
            received_at: std::time::Instant::now(),
            edit,
        };
        FlushBatch {
            channel_id: scope.channel_id(),
            scope,
            events: vec![be(
                "new",
                Some(ResolvedEdit {
                    target_event_id: "e".repeat(64),
                    target_thread_tags: ThreadTags {
                        root_event_id: Some("r".repeat(64)),
                        parent_event_id: Some("p".repeat(64)),
                        mentioned_pubkeys: vec!["a".repeat(64)],
                    },
                }),
            )],
            cancelled_events: vec![be("earlier", None)],
            cancel_reason: reason,
        }
    }

    fn thread() -> SessionScope {
        SessionScope::Thread {
            channel_id: Uuid::new_v4(),
            root_event_id: "f".repeat(64),
        }
    }

    #[test]
    fn record_round_trips_scope_session_and_every_event() {
        let root = tempfile::tempdir().expect("tempdir");
        let store = TurnJournalStore::new(root.path(), "aa");
        let original = batch(thread(), None);
        let record = TurnRecord::for_turn(&original, "sess-1", "/w", 42);
        assert_eq!(record.resume_attempts, 0);
        store.save(&record).expect("save");

        let loaded = store.load_all().expect("load");
        assert_eq!(loaded.len(), 1);
        let loaded = loaded.into_iter().next().unwrap().1.expect("parse");
        assert_eq!(loaded, record);
        assert_eq!(loaded.scope(), original.scope);
        let restored = loaded.to_batch();
        assert_eq!(restored.cancel_reason, Some(CancelReason::Restart));
        assert!(restored.cancelled_events.is_empty());
        let ids: Vec<_> = restored.events.iter().map(|e| e.event.id).collect();
        let want: Vec<_> = original
            .cancelled_events
            .iter()
            .chain(original.events.iter())
            .map(|e| e.event.id)
            .collect();
        assert_eq!(ids, want, "cancelled then new events, in prompt order");
        assert_eq!(restored.events[1].edit, original.events[0].edit);

        // A restart resume's own record carries its one used attempt.
        let resumed = TurnRecord::for_turn(&restored, "sess-1", "/w", 43);
        assert_eq!(resumed.resume_attempts, 1);

        store.remove(&original.scope).expect("remove");
        assert!(store.load_all().expect("load").is_empty());
        store
            .remove(&original.scope)
            .expect("removing twice is fine");
    }

    #[cfg(unix)]
    #[test]
    fn record_is_private() {
        use std::os::unix::fs::PermissionsExt as _;
        let root = tempfile::tempdir().expect("tempdir");
        let store = TurnJournalStore::new(root.path(), "aa");
        let b = batch(thread(), None);
        store
            .save(&TurnRecord::for_turn(&b, "s", "/w", 1))
            .expect("save");
        let path = store.record_path(&b.scope);
        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
    }

    #[test]
    fn plan_restore_resumes_once_and_refuses_stale_repeat_or_moved_records() {
        let root = tempfile::tempdir().expect("tempdir");
        let store = TurnJournalStore::new(root.path(), "aa");
        let now = 10 * MAX_RESUME_AGE_SECS;
        let fresh = batch(thread(), None);
        let stale = batch(thread(), None);
        let repeated = batch(thread(), Some(CancelReason::Restart));
        let moved = batch(thread(), None);
        let save = |b: &FlushBatch, sid: &str, cwd: &str, at: u64| {
            store
                .save(&TurnRecord::for_turn(b, sid, cwd, at))
                .expect("save");
        };
        save(&fresh, "s-fresh", "/w", now - 60);
        save(&stale, "s-stale", "/w", now - MAX_RESUME_AGE_SECS - 1);
        save(&repeated, "s-repeat", "/w", now - 60);
        save(&moved, "s-moved", "/elsewhere", now - 60);
        std::fs::write(store.dir().join("corrupt.json"), b"{torn").unwrap();

        let mut resumed = Vec::new();
        let mut noticed = Vec::new();
        for action in plan_restore(&store, "/w", now) {
            match action {
                RestoreAction::Resume { batch, session_id } => resumed.push((batch, session_id)),
                RestoreAction::Notice { batch, message } => noticed.push((batch.scope, message)),
            }
        }
        assert_eq!(resumed.len(), 1);
        assert_eq!(resumed[0].1, "s-fresh");
        assert_eq!(resumed[0].0.scope, fresh.scope);
        assert_eq!(noticed.len(), 3);
        let notice_for = |scope: &SessionScope| {
            noticed
                .iter()
                .find(|(s, _)| s == scope)
                .map(|(_, m)| m.clone())
                .expect("notice")
        };
        assert!(notice_for(&stale.scope).contains("24 hours"));
        assert!(notice_for(&repeated.scope).contains("already resumed it once"));
        assert!(notice_for(&moved.scope).contains("different directory"));

        // Only the resumed record remains, stamped with its one attempt; the
        // corrupt one is left for inspection, never resumed.
        let remaining = store.load_all().unwrap();
        let ok: Vec<_> = remaining
            .iter()
            .filter_map(|(_, r)| r.as_ref().ok())
            .collect();
        assert_eq!(ok.len(), 1);
        assert_eq!(ok[0].session_id, "s-fresh");
        assert_eq!(ok[0].resume_attempts, 1);
        assert_eq!(remaining.iter().filter(|(_, r)| r.is_err()).count(), 1);

        // A second start (the resumed turn never wrote its own record) does
        // not resume it again.
        let again = plan_restore(&store, "/w", now);
        assert!(!again.is_empty());
        assert!(again
            .iter()
            .all(|a| matches!(a, RestoreAction::Notice { .. })));
        assert!(store.load_all().unwrap().iter().all(|(_, r)| r.is_err()));
    }

    #[test]
    fn guard_keeps_record_only_for_an_unfinished_turn_during_shutdown() {
        for (label, shutting_down, completed, kept) in [
            ("ended while running", false, false, false),
            ("completed while running", false, true, false),
            ("cut short by shutdown", true, false, true),
            ("completed during shutdown", true, true, false),
        ] {
            let root = tempfile::tempdir().expect("tempdir");
            let store = TurnJournalStore::new(root.path(), "aa");
            let (tx, rx) = tokio::sync::watch::channel(());
            let journal = TurnJournal::new(store.clone(), rx);
            let b = batch(thread(), None);
            let mut guard = journal.begin(&b, "s", "/w").expect("journal written");
            assert_eq!(store.load_all().unwrap().len(), 1, "{label}: written");
            if shutting_down {
                tx.send(()).unwrap();
            }
            if completed {
                guard.complete();
            }
            drop(guard);
            assert_eq!(
                store.load_all().unwrap().len(),
                usize::from(kept),
                "{label}"
            );
        }
    }
}
