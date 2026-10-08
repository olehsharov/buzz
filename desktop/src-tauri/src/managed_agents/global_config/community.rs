//! Agent defaults are scoped to ONE community each.
//!
//! An agent belongs to exactly one community (the relay on its record, see
//! [`crate::relay::effective_agent_relay_url`]), and its defaults — env vars
//! such as a model gateway URL, the fallback provider/model, the preferred
//! runtime — are that community's. Defaults set in one community never reach
//! an agent of another: a gateway valid on one network must not be handed to
//! an agent deployed to a different community's machines.
//!
//! # Storage
//!
//! One file, `<app-data>/agents/agent-defaults-by-community.json`, holding a
//! map keyed by [`crate::relay::community_relay_key`] — the same
//! normalization `agent_belongs_to_relay` compares a record's `relay_url`
//! with. Every write rewrites the whole file atomically, `0o600` (it can hold
//! API keys), under one process-wide lock, so concurrent saves from two
//! community windows never lose each other's community.
//!
//! # Migration
//!
//! The previous single `global-agent-config.json` applied to every agent.
//! [`migrate_legacy_global_agent_config`] moves its values to the FIRST
//! community only (the home community the agent-community-scope migration
//! assigned legacy agents to); every other community starts empty. The
//! migration marker lives inside the new store, so values and marker land in
//! one atomic write. The old file is preserved as a `0o600` backup first.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde::{Deserialize, Serialize};

use super::{normalize_global_config_fields, strip_empty_env_vars, GlobalAgentConfig};
use crate::managed_agents::storage::{atomic_write_json_restricted, managed_agents_base_dir};
use crate::managed_agents::ManagedAgentRecord;

/// The per-community defaults store.
const STORE_FILE: &str = "agent-defaults-by-community.json";
/// The pre-migration, app-wide defaults file.
const LEGACY_FILE: &str = "global-agent-config.json";
/// Where the legacy file is preserved by the migration.
const LEGACY_BACKUP_FILE: &str = "global-agent-config.json.bak-per-community";
/// Marker written by the agent-community-scope migration; its
/// `home_relay_url` names the community legacy agents were assigned to.
const COMMUNITY_SCOPE_MARKER: &str = "community-scope.v1.json";

/// Serializes every read-modify-write of the store file.
static STORE_WRITE_LOCK: Mutex<()> = Mutex::new(());

/// The empty defaults of a community nothing has been saved for.
static EMPTY: GlobalAgentConfig = GlobalAgentConfig {
    env_vars: BTreeMap::new(),
    provider: None,
    model: None,
    preferred_runtime: None,
};

/// Record of the one-time move of the legacy app-wide defaults.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct LegacyMigration {
    /// Community key the legacy values were assigned to.
    pub home_relay_key: String,
    /// Whether a legacy file existed (and so values were moved).
    pub had_legacy_values: bool,
    /// When the migration ran (ISO-8601).
    pub at: String,
}

/// Agent defaults of every community, keyed by community relay key.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct CommunityAgentDefaults {
    /// Present once the legacy app-wide defaults were migrated.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub legacy_migration: Option<LegacyMigration>,
    /// Defaults per community ([`crate::relay::community_relay_key`]).
    #[serde(default)]
    pub communities: BTreeMap<String, GlobalAgentConfig>,
}

impl CommunityAgentDefaults {
    /// The defaults of the community on `relay_url` (empty when none saved).
    pub fn for_relay(&self, relay_url: &str) -> &GlobalAgentConfig {
        self.communities
            .get(&crate::relay::community_relay_key(relay_url))
            .unwrap_or(&EMPTY)
    }

    /// The defaults of the community `record` belongs to. `fallback_relay`
    /// names the community of a record not yet assigned one (see
    /// [`crate::relay::effective_agent_relay_url`]).
    pub fn for_record(
        &self,
        record: &ManagedAgentRecord,
        fallback_relay: &str,
    ) -> &GlobalAgentConfig {
        self.for_relay(&crate::relay::effective_agent_relay_url(
            &record.relay_url,
            fallback_relay,
        ))
    }

    /// [`Self::for_record`] with the active workspace naming the community of
    /// an unassigned record — the same fallback `agent_belongs_to_relay`
    /// uses. An assigned record (every record once the agent-community-scope
    /// migration ran) always gets its own community's defaults.
    pub fn for_agent<R: tauri::Runtime>(
        &self,
        app: &tauri::AppHandle<R>,
        record: &ManagedAgentRecord,
    ) -> &GlobalAgentConfig {
        self.for_record(record, &unassigned_fallback_relay(app))
    }

    /// Replace the defaults of the community on `relay_url`. Empty defaults
    /// remove the entry.
    fn set_for_relay(&mut self, relay_url: &str, config: GlobalAgentConfig) {
        let key = crate::relay::community_relay_key(relay_url);
        if config == EMPTY {
            self.communities.remove(&key);
        } else {
            self.communities.insert(key, config);
        }
    }
}

pub(crate) fn store_path(base_dir: &Path) -> PathBuf {
    base_dir.join(STORE_FILE)
}

pub(crate) fn legacy_path(base_dir: &Path) -> PathBuf {
    base_dir.join(LEGACY_FILE)
}

pub(crate) fn legacy_backup_path(base_dir: &Path) -> PathBuf {
    base_dir.join(LEGACY_BACKUP_FILE)
}

/// Read the store in `base_dir` (empty when it does not exist yet).
pub(crate) fn read_store(base_dir: &Path) -> Result<CommunityAgentDefaults, String> {
    let path = store_path(base_dir);
    match std::fs::read(&path) {
        Ok(bytes) => serde_json::from_slice(&bytes)
            .map_err(|error| format!("failed to parse agent defaults: {error}")),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            Ok(CommunityAgentDefaults::default())
        }
        Err(error) => Err(format!("failed to read agent defaults: {error}")),
    }
}

fn write_store(base_dir: &Path, store: &CommunityAgentDefaults) -> Result<(), String> {
    let payload = serde_json::to_vec_pretty(store)
        .map_err(|error| format!("failed to serialize agent defaults: {error}"))?;
    atomic_write_json_restricted(&store_path(base_dir), &payload)
}

fn lock_store() -> std::sync::MutexGuard<'static, ()> {
    STORE_WRITE_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Save `config` as the defaults of the community on `relay_url` in
/// `base_dir`, leaving every other community untouched. Strips empty env
/// values and blank provider/model first ("inherit"). Returns that
/// community's previous and saved (normalized) defaults, read and written
/// under one lock so the pair is consistent.
pub(crate) fn save_defaults_in(
    base_dir: &Path,
    relay_url: &str,
    config: &GlobalAgentConfig,
) -> Result<(GlobalAgentConfig, GlobalAgentConfig), String> {
    let mut config = config.clone();
    strip_empty_env_vars(&mut config);
    normalize_global_config_fields(&mut config);
    let _guard = lock_store();
    let mut store = read_store(base_dir)?;
    let previous = store.for_relay(relay_url).clone();
    store.set_for_relay(relay_url, config.clone());
    write_store(base_dir, &store)?;
    Ok((previous, config))
}

/// The home community recorded by the agent-community-scope migration, if it
/// has run: legacy defaults follow the legacy agents they were set for.
fn recorded_home_relay(base_dir: &Path) -> Option<String> {
    let bytes = std::fs::read(base_dir.join(COMMUNITY_SCOPE_MARKER)).ok()?;
    let marker: serde_json::Value = serde_json::from_slice(&bytes).ok()?;
    marker
        .get("home_relay_url")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|relay| !relay.is_empty())
        .map(str::to_string)
}

/// One-time move of the legacy app-wide defaults to the first community.
///
/// The home community is the one the agent-community-scope migration
/// recorded, else `first_community_relay_url` (the first saved community).
/// Every other community starts empty. Idempotent: a no-op once the store
/// carries the marker. Order: backup the legacy file (`0o600`) → one atomic
/// store write carrying both the values and the marker → remove the legacy
/// original. A failure before the store write leaves no marker, so the next
/// workspace apply retries; a crash after it leaves only an unread leftover.
/// Returns whether this call migrated.
pub(crate) fn migrate_legacy_global_agent_config(
    base_dir: &Path,
    first_community_relay_url: &str,
) -> Result<bool, String> {
    let _guard = lock_store();
    let mut store = read_store(base_dir)?;
    if store.legacy_migration.is_some() {
        return Ok(false);
    }
    let home =
        recorded_home_relay(base_dir).unwrap_or_else(|| first_community_relay_url.to_string());
    let home = buzz_core_pkg::relay::normalize_relay_url(&home)
        .map_err(|error| format!("invalid home community relay {home:?}: {error}"))?;
    let home_key = crate::relay::community_relay_key(&home);

    let legacy = legacy_path(base_dir);
    let legacy_bytes = match std::fs::read(&legacy) {
        Ok(bytes) => Some(bytes),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => return Err(format!("failed to read legacy agent defaults: {error}")),
    };
    if let Some(bytes) = &legacy_bytes {
        let mut config: GlobalAgentConfig = serde_json::from_slice(bytes)
            .map_err(|error| format!("failed to parse legacy agent defaults: {error}"))?;
        strip_empty_env_vars(&mut config);
        normalize_global_config_fields(&mut config);
        let backup = legacy_backup_path(base_dir);
        if !backup.exists() {
            atomic_write_json_restricted(&backup, bytes)?;
        }
        // Never overwrite defaults already saved for the home community.
        if !store.communities.contains_key(&home_key) {
            store.set_for_relay(&home, config);
        }
    }
    store.legacy_migration = Some(LegacyMigration {
        home_relay_key: home_key,
        had_legacy_values: legacy_bytes.is_some(),
        at: crate::util::now_iso(),
    });
    write_store(base_dir, &store)?;
    if legacy_bytes.is_some() {
        if let Err(error) = std::fs::remove_file(&legacy) {
            eprintln!("buzz-desktop: failed to remove migrated agent defaults file: {error}");
        }
    }
    Ok(true)
}

/// Load every community's agent defaults.
pub fn load_community_agent_defaults<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
) -> Result<CommunityAgentDefaults, String> {
    read_store(&managed_agents_base_dir(app)?)
}

/// The defaults of the community on `relay_url`.
pub fn load_agent_defaults_for_relay<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    relay_url: &str,
) -> Result<GlobalAgentConfig, String> {
    Ok(load_community_agent_defaults(app)?
        .for_relay(relay_url)
        .clone())
}

/// The defaults of the community `record` belongs to (its own relay;
/// `fallback_relay` only for a record not yet assigned a community). Read
/// failures fall back to empty defaults, never to another community's.
pub fn load_agent_defaults_for_record<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    record: &ManagedAgentRecord,
    fallback_relay: &str,
) -> GlobalAgentConfig {
    match load_community_agent_defaults(app) {
        Ok(store) => store.for_record(record, fallback_relay).clone(),
        Err(error) => {
            eprintln!("buzz-desktop: failed to load agent defaults: {error}");
            GlobalAgentConfig::default()
        }
    }
}

/// The community an unassigned record resolves to: the active workspace.
fn unassigned_fallback_relay<R: tauri::Runtime>(app: &tauri::AppHandle<R>) -> String {
    use tauri::Manager;
    match app.try_state::<crate::app_state::AppState>() {
        Some(state) => crate::relay::relay_ws_url_with_override(&state),
        None => crate::relay::relay_ws_url(),
    }
}

/// [`load_agent_defaults_for_record`] with the active workspace naming the
/// community of an unassigned record (see [`CommunityAgentDefaults::for_agent`]).
pub fn load_agent_defaults_for_agent<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    record: &ManagedAgentRecord,
) -> GlobalAgentConfig {
    load_agent_defaults_for_record(app, record, &unassigned_fallback_relay(app))
}

/// Save `config` as the defaults of the community on `relay_url`; returns
/// its previous and saved defaults (see [`save_defaults_in`]).
pub fn save_agent_defaults_for_relay<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    relay_url: &str,
    config: &GlobalAgentConfig,
) -> Result<(GlobalAgentConfig, GlobalAgentConfig), String> {
    save_defaults_in(&managed_agents_base_dir(app)?, relay_url, config)
}

#[cfg(test)]
#[path = "community_tests.rs"]
mod tests;
