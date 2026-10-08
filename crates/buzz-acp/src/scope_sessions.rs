//! Durable map of session scope → provider session, so a scope keeps its
//! conversation when the worker that held it goes away.
//!
//! Workers come and go: the idle-pool sleep tears the whole pool down, an
//! adapter crash or OOM respawns a slot, a busy owner's scope moves to another
//! worker. Without this map, each of those silently started the scope over in
//! a `session/new`. With it, the scope's next session on any worker resumes
//! the recorded provider session (`session/resume`, same cwd). A resume that
//! fails is a dead session (fail fast, visible), never a quiet new session.
//! Only a deliberate rotation forgets an entry: `!rotate`, the turn/context
//! limit rotation, or the agent being removed from the channel.
//!
//! The map lives next to the resume and turn-journal records
//! (`<state>/buzz-acp/scope-sessions/<agent>/<cwd hash>.json`), so it also
//! survives a harness restart. It is capped at [`MAX_ENTRIES`]; the least
//! recently used scopes are evicted first.

use std::collections::HashMap;
use std::io::{self, Write as _};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::scope::SessionScope;

const SUBDIR: &str = "scope-sessions";
const FILE_VERSION: u32 = 1;
/// Most scopes remembered per agent and cwd.
pub const MAX_ENTRIES: usize = 1000;

#[derive(Debug, Serialize, Deserialize)]
struct MapFile {
    version: u32,
    cwd: String,
    entries: Vec<Entry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Entry {
    channel_id: Uuid,
    root_event_id: Option<String>,
    session_id: String,
    /// Monotonic use counter, for least-recently-used eviction.
    used: u64,
}

impl Entry {
    fn scope(&self) -> SessionScope {
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
}

/// Where one agent's map for one cwd is stored.
#[derive(Debug, Clone)]
pub struct ScopeSessionStore {
    path: PathBuf,
}

impl ScopeSessionStore {
    /// Store for `agent_pubkey_hex` running in `cwd`, under `state_root`.
    pub fn new(state_root: &Path, agent_pubkey_hex: &str, cwd: &str) -> Self {
        let cwd_hash = hex::encode(Sha256::digest(cwd.as_bytes()));
        Self {
            path: state_root
                .join(SUBDIR)
                .join(agent_pubkey_hex)
                .join(format!("{}.json", &cwd_hash[..16])),
        }
    }

    /// The map file.
    pub fn path(&self) -> &Path {
        &self.path
    }
}

#[derive(Debug, Default)]
struct Inner {
    store: Option<ScopeSessionStore>,
    cwd: String,
    entries: HashMap<SessionScope, Entry>,
    clock: u64,
    /// Per dispatch: another live worker may hold the scope's session, so it
    /// must not be resumed a second time (two holders would race on its
    /// transcript).
    held_elsewhere: HashMap<SessionScope, bool>,
}

/// The scope → provider-session map shared by every worker.
///
/// `Default` is disabled: nothing is remembered and nothing resumed (isolated
/// tasks, and tests that do not opt in).
#[derive(Debug, Clone, Default)]
pub struct ScopeSessions {
    enabled: bool,
    inner: Arc<Mutex<Inner>>,
}

impl ScopeSessions {
    /// Load the map from `store` for `cwd`. A missing file is an empty map.
    /// An unreadable or corrupt file is an error: the caller decides how to
    /// surface it rather than silently forgetting every conversation.
    pub fn load(store: ScopeSessionStore, cwd: &str) -> io::Result<Self> {
        let mut inner = Inner {
            cwd: cwd.to_owned(),
            ..Inner::default()
        };
        match std::fs::read(store.path()) {
            Ok(bytes) => {
                let file: MapFile = serde_json::from_slice(&bytes)
                    .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
                    .and_then(|file: MapFile| {
                        if file.version != FILE_VERSION || file.cwd != cwd {
                            return Err(io::Error::new(
                                io::ErrorKind::InvalidData,
                                "unsupported version or another cwd",
                            ));
                        }
                        Ok(file)
                    })
                    .map_err(|e| map_error(e, "read", store.path()))?;
                for entry in file.entries {
                    inner.clock = inner.clock.max(entry.used);
                    inner.entries.insert(entry.scope(), entry);
                }
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => return Err(map_error(e, "read", store.path())),
        }
        inner.store = Some(store);
        Ok(Self {
            enabled: true,
            inner: Arc::new(Mutex::new(inner)),
        })
    }

    /// An enabled map that is never persisted (tests).
    #[cfg(test)]
    pub fn in_memory() -> Self {
        Self {
            enabled: true,
            inner: Arc::default(),
        }
    }

    /// Whether scopes are remembered and resumed at all.
    #[cfg(test)]
    pub fn enabled(&self) -> bool {
        self.enabled
    }

    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// The provider session `scope` last ran in, if remembered.
    pub fn session_for(&self, scope: &SessionScope) -> Option<String> {
        self.lock()
            .entries
            .get(scope)
            .map(|entry| entry.session_id.clone())
    }

    /// Remember that `scope` now runs in `session_id`, durably. On a write
    /// error the in-memory map keeps the entry (in-process respawns still
    /// resume it) and the error is returned for the caller to surface.
    pub fn record(&self, scope: &SessionScope, session_id: &str) -> io::Result<()> {
        if !self.enabled {
            return Ok(());
        }
        let mut inner = self.lock();
        inner.clock += 1;
        let used = inner.clock;
        let (channel_id, root_event_id) = match scope {
            SessionScope::Conversation { channel_id } => (*channel_id, None),
            SessionScope::Thread {
                channel_id,
                root_event_id,
            } => (*channel_id, Some(root_event_id.clone())),
        };
        inner.entries.insert(
            scope.clone(),
            Entry {
                channel_id,
                root_event_id,
                session_id: session_id.to_owned(),
                used,
            },
        );
        while inner.entries.len() > MAX_ENTRIES {
            let Some(oldest) = inner
                .entries
                .iter()
                .min_by_key(|(_, entry)| entry.used)
                .map(|(scope, _)| scope.clone())
            else {
                break;
            };
            inner.entries.remove(&oldest);
        }
        persist(&inner)
    }

    /// Forget `scope` (deliberate rotation). Returns whether it was known.
    pub fn forget(&self, scope: &SessionScope) -> bool {
        let mut inner = self.lock();
        let known = inner.entries.remove(scope).is_some();
        inner.held_elsewhere.remove(scope);
        if known {
            log_persist(&inner);
        }
        known
    }

    /// Forget every scope of `channel_id` (the agent left the channel).
    pub fn forget_channel(&self, channel_id: Uuid) {
        let mut inner = self.lock();
        let before = inner.entries.len();
        inner
            .entries
            .retain(|scope, _| scope.channel_id() != channel_id);
        inner
            .held_elsewhere
            .retain(|scope, _| scope.channel_id() != channel_id);
        if inner.entries.len() != before {
            log_persist(&inner);
        }
    }

    /// Set, for the dispatch about to run `scope`, whether another live worker
    /// may still hold its session.
    pub fn set_held_elsewhere(&self, scope: &SessionScope, held: bool) {
        let mut inner = self.lock();
        if held {
            inner.held_elsewhere.insert(scope.clone(), true);
        } else {
            inner.held_elsewhere.remove(scope);
        }
    }

    /// Whether another live worker may still hold `scope`'s session.
    pub fn held_elsewhere(&self, scope: &SessionScope) -> bool {
        self.lock()
            .held_elsewhere
            .get(scope)
            .copied()
            .unwrap_or(false)
    }

    /// Number of remembered scopes.
    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.lock().entries.len()
    }
}

fn log_persist(inner: &Inner) {
    if let Err(error) = persist(inner) {
        tracing::error!(target: "scope_sessions", "{error}");
    }
}

/// Atomically write the whole map: temp file, fsync, rename, fsync dir.
fn persist(inner: &Inner) -> io::Result<()> {
    let Some(store) = &inner.store else {
        return Ok(());
    };
    let path = store.path();
    let dir = path
        .parent()
        .ok_or_else(|| io::Error::other("scope-session map has no directory"))?;
    crate::resume_store::create_private_dir_all(dir).map_err(|e| map_error(e, "create", dir))?;
    let mut entries: Vec<Entry> = inner.entries.values().cloned().collect();
    entries.sort_by_key(|entry| entry.used);
    let file = MapFile {
        version: FILE_VERSION,
        cwd: inner.cwd.clone(),
        entries,
    };
    let json = serde_json::to_vec(&file).map_err(io::Error::other)?;
    let write = || -> io::Result<()> {
        let mut tmp = tempfile::NamedTempFile::new_in(dir)?;
        tmp.write_all(&json)?;
        tmp.as_file().sync_all()?;
        tmp.persist(path).map_err(|e| e.error)?;
        crate::resume_store::sync_dir(dir)
    };
    write().map_err(|e| map_error(e, "write", path))
}

fn map_error(error: io::Error, action: &str, path: &Path) -> io::Error {
    io::Error::new(
        error.kind(),
        format!(
            "failed to {action} scope-session map {}: {error}",
            path.display()
        ),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn conv() -> SessionScope {
        SessionScope::Conversation {
            channel_id: Uuid::new_v4(),
        }
    }

    #[test]
    fn map_survives_reload_and_forgets_on_rotation_and_channel_removal() {
        let root = tempfile::tempdir().expect("tempdir");
        let store = ScopeSessionStore::new(root.path(), "aa", "/w");
        let map = ScopeSessions::load(store.clone(), "/w").expect("empty map");
        let a = conv();
        let thread = SessionScope::Thread {
            channel_id: a.channel_id(),
            root_event_id: "f".repeat(64),
        };
        let b = conv();
        map.record(&a, "s-a").unwrap();
        map.record(&thread, "s-t").unwrap();
        map.record(&b, "s-b").unwrap();

        let reloaded = ScopeSessions::load(store.clone(), "/w").expect("reload");
        assert_eq!(reloaded.session_for(&a).as_deref(), Some("s-a"));
        assert_eq!(reloaded.session_for(&thread).as_deref(), Some("s-t"));
        assert_eq!(reloaded.session_for(&b).as_deref(), Some("s-b"));

        assert!(reloaded.forget(&b));
        let after_forget = ScopeSessions::load(store.clone(), "/w").expect("reload");
        assert_eq!(after_forget.session_for(&b), None, "rotation persists");
        assert_eq!(after_forget.len(), 2);
        reloaded.forget_channel(a.channel_id());
        let again = ScopeSessions::load(store.clone(), "/w").expect("reload");
        assert_eq!(again.len(), 0, "rotation and channel removal persist");

        // Another cwd's file is never read as this one.
        assert!(ScopeSessions::load(store, "/elsewhere").is_err());
    }

    #[test]
    fn map_is_capped_least_recently_used_first() {
        let map = ScopeSessions::in_memory();
        let first = conv();
        map.record(&first, "s-0").unwrap();
        let second = conv();
        map.record(&second, "s-1").unwrap();
        map.record(&first, "s-0").unwrap(); // used again
        for i in 0..MAX_ENTRIES - 1 {
            map.record(&conv(), &format!("x-{i}")).unwrap();
        }
        assert_eq!(map.len(), MAX_ENTRIES);
        assert!(map.session_for(&first).is_some(), "recently used stays");
        assert!(map.session_for(&second).is_none(), "least recent goes");
    }

    #[test]
    fn corrupt_map_is_an_error_not_empty() {
        let root = tempfile::tempdir().expect("tempdir");
        let store = ScopeSessionStore::new(root.path(), "aa", "/w");
        std::fs::create_dir_all(store.path().parent().unwrap()).unwrap();
        std::fs::write(store.path(), b"{torn").unwrap();
        let err = ScopeSessions::load(store, "/w").expect_err("corrupt");
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
    }

    #[test]
    fn disabled_map_remembers_nothing() {
        let map = ScopeSessions::default();
        let scope = conv();
        map.record(&scope, "s").unwrap();
        assert!(!map.enabled());
        assert_eq!(map.session_for(&scope), None);
    }
}
