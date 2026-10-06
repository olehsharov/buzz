//! On-disk host state under `~/.config/buzz/host/`.
//!
//! | File | Contents |
//! |------|----------|
//! | `host.key` | Host secret key H (hex), 0600 |
//! | `owner.json` | The grant from the paired owner O, 0600 |
//! | `agents/<agent_pubkey>.json` | One deployed agent's workdir + env (holds its nsec), 0600 |
//! | `seen.json` | Recently handled control `request_id`s and their results |
//! | `status.json` | Last published `host.status` snapshot (no secrets) |
//!
//! Every write is atomic (temp file + rename) and created with mode 0600 in a
//! 0700 directory.

use std::collections::BTreeMap;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use nostr::{Keys, SecretKey};
use serde::{Deserialize, Serialize};
use zeroize::Zeroizing;

use crate::error::{HostError, Result};

/// Environment variable overriding the host state directory (tests, dev).
pub const HOST_HOME_ENV: &str = "BUZZ_HOST_HOME";

/// Resolved locations of every host state file.
#[derive(Debug, Clone)]
pub struct HostPaths {
    /// State root (`~/.config/buzz/host`).
    pub root: PathBuf,
}

impl HostPaths {
    /// Resolve the state root from `BUZZ_HOST_HOME`, else `$HOME/.config/buzz/host`.
    pub fn from_env() -> Result<Self> {
        if let Some(dir) = std::env::var_os(HOST_HOME_ENV).filter(|v| !v.is_empty()) {
            return Ok(Self::at(PathBuf::from(dir)));
        }
        let home = home_dir()?;
        Ok(Self::at(home.join(".config").join("buzz").join("host")))
    }

    /// Use an explicit state root.
    pub fn at(root: PathBuf) -> Self {
        Self { root }
    }

    /// Host secret key file.
    pub fn key_file(&self) -> PathBuf {
        self.root.join("host.key")
    }

    /// Owner grant file.
    pub fn owner_file(&self) -> PathBuf {
        self.root.join("owner.json")
    }

    /// Directory of deployed agent configs.
    pub fn agents_dir(&self) -> PathBuf {
        self.root.join("agents")
    }

    /// Config file for one agent. `agent_pubkey` must already be validated
    /// (64 lowercase hex chars) so it cannot traverse paths.
    pub fn agent_file(&self, agent_pubkey: &str) -> PathBuf {
        self.agents_dir().join(format!("{agent_pubkey}.json"))
    }

    /// Control-frame dedupe ledger.
    pub fn seen_file(&self) -> PathBuf {
        self.root.join("seen.json")
    }

    /// Last status snapshot written by the daemon.
    pub fn status_file(&self) -> PathBuf {
        self.root.join("status.json")
    }

    /// Agent log directory.
    pub fn logs_dir(&self) -> PathBuf {
        self.root.join("logs")
    }

    /// Log file for one agent.
    pub fn agent_log(&self, agent_pubkey: &str) -> PathBuf {
        self.logs_dir().join(format!("{agent_pubkey}.log"))
    }
}

/// The current user's home directory.
pub fn home_dir() -> Result<PathBuf> {
    std::env::var_os("HOME")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .ok_or_else(|| HostError::Invalid("HOME is not set".into()))
}

/// Create `dir` (and parents) with mode 0700.
pub fn ensure_private_dir(dir: &Path) -> Result<()> {
    fs::create_dir_all(dir).map_err(|e| HostError::io(format!("create {}", dir.display()), e))?;
    set_mode(dir, 0o700)
}

#[cfg(unix)]
fn set_mode(path: &Path, mode: u32) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(mode))
        .map_err(|e| HostError::io(format!("chmod {}", path.display()), e))
}

#[cfg(not(unix))]
fn set_mode(_path: &Path, _mode: u32) -> Result<()> {
    Ok(())
}

/// Atomically write `bytes` to `path` with mode 0600.
pub fn write_private(path: &Path, bytes: &[u8]) -> Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| HostError::Invalid(format!("{} has no parent", path.display())))?;
    ensure_private_dir(parent)?;
    let tmp = path.with_extension("tmp");
    {
        let mut options = fs::OpenOptions::new();
        options.write(true).create(true).truncate(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options
            .open(&tmp)
            .map_err(|e| HostError::io(format!("open {}", tmp.display()), e))?;
        file.write_all(bytes)
            .and_then(|()| file.sync_all())
            .map_err(|e| HostError::io(format!("write {}", tmp.display()), e))?;
    }
    set_mode(&tmp, 0o600)?;
    fs::rename(&tmp, path).map_err(|e| HostError::io(format!("rename {}", path.display()), e))
}

/// Serialize `value` as JSON and write it privately.
pub fn write_json<T: Serialize>(path: &Path, value: &T) -> Result<()> {
    let bytes = Zeroizing::new(serde_json::to_vec_pretty(value)?);
    write_private(path, &bytes)
}

/// Read and parse a JSON file; `Ok(None)` when it does not exist.
pub fn read_json<T: for<'de> Deserialize<'de>>(path: &Path) -> Result<Option<T>> {
    match fs::read(path) {
        Ok(bytes) => {
            let bytes = Zeroizing::new(bytes);
            Ok(Some(serde_json::from_slice(&bytes)?))
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(HostError::io(format!("read {}", path.display()), e)),
    }
}

/// Remove a file; a missing file is not an error.
pub fn remove_file(path: &Path) -> Result<()> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(HostError::io(format!("remove {}", path.display()), e)),
    }
}

/// Load the host key H, generating and persisting one if absent.
pub fn load_or_create_host_key(paths: &HostPaths) -> Result<Keys> {
    if let Some(keys) = load_host_key(paths)? {
        return Ok(keys);
    }
    let keys = Keys::generate();
    let hex = Zeroizing::new(keys.secret_key().to_secret_hex());
    write_private(&paths.key_file(), hex.as_bytes())?;
    Ok(keys)
}

/// Load the host key H if it exists.
pub fn load_host_key(paths: &HostPaths) -> Result<Option<Keys>> {
    let path = paths.key_file();
    let raw = match fs::read_to_string(&path) {
        Ok(raw) => Zeroizing::new(raw),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(HostError::io(format!("read {}", path.display()), e)),
    };
    let secret = SecretKey::parse(raw.trim())
        .map_err(|_| HostError::Invalid(format!("{} is not a valid key", path.display())))?;
    Ok(Some(Keys::new(secret)))
}

/// The owner's grant, persisted after pairing.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct OwnerRecord {
    /// Owner pubkey O (hex).
    pub owner_pubkey: String,
    /// NIP-OA auth tag JSON authorizing H under O.
    pub auth_tag: String,
    /// Relay the host connects to.
    pub relay_url: String,
    /// Unix seconds when the pairing completed.
    pub paired_at: u64,
    /// Display name sent in the hello.
    pub name: String,
}

/// Load the owner grant, if paired.
pub fn load_owner(paths: &HostPaths) -> Result<Option<OwnerRecord>> {
    read_json(&paths.owner_file())
}

/// One deployed agent, as written to `agents/<agent_pubkey>.json`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AgentRecord {
    /// Agent pubkey (hex).
    pub agent_pubkey: String,
    /// Absolute working directory.
    pub workdir: String,
    /// Full process environment for `buzz-acp` (holds secrets).
    pub env: BTreeMap<String, String>,
    /// Unix seconds of the latest deploy.
    pub deployed_at: u64,
}

/// Load one agent record.
pub fn load_agent(paths: &HostPaths, agent_pubkey: &str) -> Result<Option<AgentRecord>> {
    read_json(&paths.agent_file(agent_pubkey))
}

/// List the pubkeys of every configured agent, sorted.
pub fn list_agents(paths: &HostPaths) -> Result<Vec<String>> {
    let dir = paths.agents_dir();
    let entries = match fs::read_dir(&dir) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(HostError::io(format!("read {}", dir.display()), e)),
    };
    let mut out = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|e| HostError::io(format!("read {}", dir.display()), e))?;
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        if let Some(stem) = name.strip_suffix(".json") {
            if crate::protocol::is_pubkey_hex(stem) {
                out.push(stem.to_string());
            }
        }
    }
    out.sort();
    Ok(out)
}

/// Current Unix time in seconds.
pub fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    #[test]
    fn private_files_are_0600_in_0700_dirs() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().expect("tempdir");
        let paths = HostPaths::at(dir.path().join("host"));
        load_or_create_host_key(&paths).expect("key");
        let mode = |p: &Path| fs::metadata(p).expect("meta").permissions().mode() & 0o777;
        assert_eq!(mode(&paths.key_file()), 0o600);
        assert_eq!(mode(&paths.root), 0o700);
    }

    #[test]
    fn host_key_is_stable_across_loads() {
        let dir = tempfile::tempdir().expect("tempdir");
        let paths = HostPaths::at(dir.path().to_path_buf());
        let a = load_or_create_host_key(&paths).expect("create");
        let b = load_or_create_host_key(&paths).expect("load");
        assert_eq!(a.public_key(), b.public_key());
    }
}
