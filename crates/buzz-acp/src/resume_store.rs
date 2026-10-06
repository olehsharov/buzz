//! Durable record of the fork created for `BUZZ_ACP_RESUME_SESSION`.
//!
//! The first channel/DM session forks the configured source provider session
//! and resumes the fork. Without a record, every harness restart would fork
//! the source again and discard what the agent did in Buzz since. Once a
//! fork has been resumed successfully, its ID is written here so later
//! processes resume that fork instead.
//!
//! Records live under the XDG state directory
//! (`$XDG_STATE_HOME/buzz-acp`, else `~/.local/state/buzz-acp`), not under
//! the harness cwd: the cwd is often the agent's git work tree, and a record
//! there could be committed or deleted by the agent itself. The path is keyed
//! by agent pubkey, source session ID, and a hash of the cwd, because a
//! provider session belongs to one project directory and one agent identity.
//! A record holds only session IDs and the cwd — never key material.

use std::ffi::OsString;
use std::io::{self, Write as _};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// Directory name under the state root.
const STATE_SUBDIR: &str = "buzz-acp";
/// Subdirectory holding one record per (agent, source, cwd).
const RECORDS_SUBDIR: &str = "resume-sessions";

/// Resolve the harness state root from the environment: `$XDG_STATE_HOME`
/// when absolute (the XDG spec says relative values are ignored), else
/// `$HOME/.local/state`. `None` when neither yields an absolute path.
pub fn state_root_from_env(env: impl Fn(&str) -> Option<OsString>) -> Option<PathBuf> {
    let absolute = |key: &str| {
        env(key)
            .filter(|v| !v.is_empty())
            .map(PathBuf::from)
            .filter(|p| p.is_absolute())
    };
    absolute("XDG_STATE_HOME")
        .or_else(|| absolute("HOME").map(|home| home.join(".local").join("state")))
        .map(|root| root.join(STATE_SUBDIR))
}

#[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
struct ForkRecord {
    source: String,
    cwd: String,
    fork: String,
}

/// Per-agent store of source-session → fork records.
#[derive(Debug, Clone)]
pub struct ResumeForkStore {
    dir: PathBuf,
}

impl ResumeForkStore {
    /// Store for `agent_pubkey_hex` under `state_root`.
    pub fn new(state_root: &Path, agent_pubkey_hex: &str) -> Self {
        Self {
            dir: state_root.join(RECORDS_SUBDIR).join(agent_pubkey_hex),
        }
    }

    /// Directory holding this agent's records.
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// Path of the record for `source` run from `cwd`. `source` is a
    /// validated UUID (see config), so it is a safe file-name component.
    pub fn record_path(&self, source: &str, cwd: &str) -> PathBuf {
        let cwd_hash = hex::encode(Sha256::digest(cwd.as_bytes()));
        self.dir.join(format!("{source}.{}.json", &cwd_hash[..16]))
    }

    /// The persisted fork for `source` in `cwd`, if any. A missing record is
    /// `Ok(None)`; an unreadable or mismatched record is an error so a
    /// corrupt record is never mistaken for "no fork yet" (which would fork
    /// the source again).
    pub fn load(&self, source: &str, cwd: &str) -> io::Result<Option<String>> {
        let path = self.record_path(source, cwd);
        let bytes = match std::fs::read(&path) {
            Ok(bytes) => bytes,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(with_path(e, "read", &path)),
        };
        let record: ForkRecord = serde_json::from_slice(&bytes).map_err(|e| {
            with_path(
                io::Error::new(io::ErrorKind::InvalidData, e),
                "parse",
                &path,
            )
        })?;
        if record.source != source || record.cwd != cwd || record.fork.trim().is_empty() {
            return Err(with_path(
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    "record does not match this source session and cwd",
                ),
                "validate",
                &path,
            ));
        }
        Ok(Some(record.fork))
    }

    /// Atomically record `fork` as the continuation of `source` in `cwd`:
    /// write a temp file in the same directory, fsync it, rename it over the
    /// record, then fsync the directory. A crash leaves either the old record
    /// or the new one, never a torn file.
    pub fn save(&self, source: &str, cwd: &str, fork: &str) -> io::Result<()> {
        let path = self.record_path(source, cwd);
        create_private_dir_all(&self.dir).map_err(|e| with_path(e, "create", &self.dir))?;
        let record = ForkRecord {
            source: source.to_owned(),
            cwd: cwd.to_owned(),
            fork: fork.to_owned(),
        };
        let json = serde_json::to_vec_pretty(&record).map_err(io::Error::other)?;
        let write = || -> io::Result<()> {
            // NamedTempFile is created 0600 on Unix.
            let mut tmp = tempfile::NamedTempFile::new_in(&self.dir)?;
            tmp.write_all(&json)?;
            tmp.as_file().sync_all()?;
            tmp.persist(&path).map_err(|e| e.error)?;
            sync_dir(&self.dir)
        };
        write().map_err(|e| with_path(e, "write", &path))
    }
}

fn with_path(error: io::Error, action: &str, path: &Path) -> io::Error {
    io::Error::new(
        error.kind(),
        format!(
            "failed to {action} resume-session record {}: {error}",
            path.display()
        ),
    )
}

#[cfg(unix)]
pub(crate) fn create_private_dir_all(dir: &Path) -> io::Result<()> {
    use std::os::unix::fs::DirBuilderExt as _;
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(dir)
}

#[cfg(not(unix))]
pub(crate) fn create_private_dir_all(dir: &Path) -> io::Result<()> {
    std::fs::create_dir_all(dir)
}

#[cfg(unix)]
pub(crate) fn sync_dir(dir: &Path) -> io::Result<()> {
    std::fs::File::open(dir)?.sync_all()
}

#[cfg(not(unix))]
pub(crate) fn sync_dir(_dir: &Path) -> io::Result<()> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const SOURCE: &str = "11111111-1111-4111-8111-111111111111";

    type Env = &'static [(&'static str, &'static str)];

    fn env_of(pairs: Env) -> impl Fn(&str) -> Option<OsString> {
        move |key| {
            pairs
                .iter()
                .find(|(k, _)| *k == key)
                .map(|(_, v)| OsString::from(v))
        }
    }

    #[test]
    fn state_root_prefers_absolute_xdg_state_home_then_home() {
        let cases: &[(Env, Option<&str>)] = &[
            (
                &[("XDG_STATE_HOME", "/xdg"), ("HOME", "/home/u")],
                Some("/xdg/buzz-acp"),
            ),
            (
                &[("XDG_STATE_HOME", "relative"), ("HOME", "/home/u")],
                Some("/home/u/.local/state/buzz-acp"),
            ),
            (
                &[("XDG_STATE_HOME", ""), ("HOME", "/home/u")],
                Some("/home/u/.local/state/buzz-acp"),
            ),
            (
                &[("HOME", "/home/u")],
                Some("/home/u/.local/state/buzz-acp"),
            ),
            (&[("HOME", "relative")], None),
            (&[], None),
        ];
        for (env, expected) in cases {
            assert_eq!(
                state_root_from_env(env_of(env)),
                expected.map(PathBuf::from),
                "env {env:?}"
            );
        }
    }

    #[test]
    fn save_then_load_round_trips_per_agent_source_and_cwd() {
        let root = tempfile::tempdir().expect("tempdir");
        let store = ResumeForkStore::new(root.path(), "aa");
        assert_eq!(store.load(SOURCE, "/w").expect("load"), None);

        store.save(SOURCE, "/w", "fork-1").expect("save");
        assert_eq!(
            store.load(SOURCE, "/w").expect("load").as_deref(),
            Some("fork-1")
        );
        // Replacing is atomic and leaves no temp files behind.
        store.save(SOURCE, "/w", "fork-2").expect("save");
        assert_eq!(
            store.load(SOURCE, "/w").expect("load").as_deref(),
            Some("fork-2")
        );
        let entries: Vec<_> = std::fs::read_dir(root.path().join("resume-sessions/aa"))
            .expect("read_dir")
            .collect();
        assert_eq!(entries.len(), 1);

        // Another cwd, another agent, or another source sees no record.
        assert_eq!(store.load(SOURCE, "/other").expect("load"), None);
        let other_agent = ResumeForkStore::new(root.path(), "bb");
        assert_eq!(other_agent.load(SOURCE, "/w").expect("load"), None);
        let other_source = "22222222-2222-4222-8222-222222222222";
        assert_eq!(store.load(other_source, "/w").expect("load"), None);
    }

    #[test]
    fn corrupt_or_mismatched_record_is_an_error_not_absent() {
        let root = tempfile::tempdir().expect("tempdir");
        let store = ResumeForkStore::new(root.path(), "aa");
        store.save(SOURCE, "/w", "fork-1").expect("save");
        let path = store.record_path(SOURCE, "/w");

        std::fs::write(&path, b"{not json").expect("write");
        let err = store.load(SOURCE, "/w").expect_err("corrupt record");
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
        assert!(err.to_string().contains(&path.display().to_string()));

        std::fs::write(&path, br#"{"source":"x","cwd":"/w","fork":"f"}"#).expect("write");
        let err = store.load(SOURCE, "/w").expect_err("mismatched record");
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
    }

    #[cfg(unix)]
    #[test]
    fn record_and_directory_are_private() {
        use std::os::unix::fs::PermissionsExt as _;
        let root = tempfile::tempdir().expect("tempdir");
        let store = ResumeForkStore::new(root.path(), "aa");
        store.save(SOURCE, "/w", "fork-1").expect("save");
        let mode = |p: &Path| std::fs::metadata(p).expect("metadata").permissions().mode() & 0o777;
        assert_eq!(mode(&store.record_path(SOURCE, "/w")), 0o600);
        assert_eq!(mode(&root.path().join("resume-sessions/aa")), 0o700);
    }
}
