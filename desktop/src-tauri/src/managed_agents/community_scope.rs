//! Community ownership of managed agents.
//!
//! Every managed agent belongs to exactly ONE community: the relay stored in
//! its record's `relay_url` (see [`crate::relay::effective_agent_relay_url`]).
//! It runs, publishes, and is listed only there and is hidden from every other
//! community. This narrows #2122 ("agents everywhere").
//!
//! Key-less user definitions (templates) are scoped the same way, so a
//! definition created in one community does not appear in another's pickers.
//! Built-in definitions (`is_builtin`) are product templates and stay global.
//!
//! Two writers assign the community:
//!
//! - [`assign_legacy_agent_communities`] runs once (guarded by a marker file)
//!   from `apply_workspace`, before the relay override is installed. It
//!   assigns every unassigned record to the FIRST saved community — the one
//!   the user started from — and keeps every existing pin. The old store is
//!   backed up next to it first.
//! - [`stamp_relay_for_save`] is the write-time net for later records: once
//!   the legacy assignment has run, any record a creation path saves without
//!   a community is stamped with the active community's relay.

use std::path::{Path, PathBuf};

use tauri::Manager;

use super::storage::{atomic_write_json, atomic_write_json_restricted};
use super::ManagedAgentRecord;

/// Marker that the one-time legacy assignment has completed.
const MARKER_FILE: &str = "community-scope.v1.json";
/// Pre-assignment copy of `managed-agents.json`, written once.
const BACKUP_FILE: &str = "managed-agents.json.bak-community-scope";

pub(crate) fn marker_path(base_dir: &Path) -> PathBuf {
    base_dir.join(MARKER_FILE)
}

pub(crate) fn backup_path(base_dir: &Path) -> PathBuf {
    base_dir.join(BACKUP_FILE)
}

/// Whether the one-time legacy assignment has completed in `base_dir`.
pub(crate) fn legacy_assignment_done(base_dir: &Path) -> bool {
    marker_path(base_dir).exists()
}

/// Built-in definitions are global product templates — never scoped.
fn is_global_template(record: &serde_json::Value) -> bool {
    let pubkey_empty = record
        .get("pubkey")
        .and_then(serde_json::Value::as_str)
        .is_none_or(str::is_empty);
    let builtin = record
        .get("is_builtin")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false);
    pubkey_empty && builtin
}

fn is_unassigned(record: &serde_json::Value) -> bool {
    record
        .get("relay_url")
        .and_then(serde_json::Value::as_str)
        .is_none_or(|relay| relay.trim().is_empty())
}

/// Assign every unassigned, scopable record in the raw store to `relay_url`.
/// Existing pins are kept. Returns how many records were assigned. Operates on
/// raw JSON so every other field (including inline key residue) round-trips
/// byte-for-byte in meaning.
pub(crate) fn assign_unassigned(records: &mut [serde_json::Value], relay_url: &str) -> usize {
    let mut assigned = 0;
    for record in records.iter_mut() {
        if is_global_template(record) || !is_unassigned(record) {
            continue;
        }
        if let Some(object) = record.as_object_mut() {
            object.insert(
                "relay_url".to_string(),
                serde_json::Value::String(relay_url.to_string()),
            );
            assigned += 1;
        }
    }
    assigned
}

/// One-time assignment of legacy (unassigned) records to `home_relay_url`,
/// the first community in the user's saved community list.
///
/// Idempotent: a no-op once the marker exists. Order is backup → atomic store
/// rewrite → marker, so a crash at any point either leaves the old store
/// intact with no marker (retried on the next apply) or leaves every record
/// assigned (a retry assigns nothing). A failure leaves no marker, so it is
/// retried on the next workspace apply.
pub(crate) fn assign_legacy_agent_communities(
    base_dir: &Path,
    home_relay_url: &str,
) -> Result<usize, String> {
    if legacy_assignment_done(base_dir) {
        return Ok(0);
    }
    let home = buzz_core_pkg::relay::normalize_relay_url(home_relay_url)
        .map_err(|error| format!("invalid home community relay {home_relay_url:?}: {error}"))?;
    let store = base_dir.join("managed-agents.json");
    let mut assigned = 0;
    if store.exists() {
        let bytes = std::fs::read(&store)
            .map_err(|error| format!("failed to read agent store: {error}"))?;
        let mut records: Vec<serde_json::Value> = serde_json::from_slice(&bytes)
            .map_err(|error| format!("failed to parse agent store: {error}"))?;
        assigned = assign_unassigned(&mut records, &home);
        if assigned > 0 {
            let backup = backup_path(base_dir);
            if !backup.exists() {
                // The store can carry inline agent keys: keep the copy 0o600.
                atomic_write_json_restricted(&backup, &bytes)?;
            }
            let payload = serde_json::to_vec_pretty(&records)
                .map_err(|error| format!("failed to serialize agent store: {error}"))?;
            atomic_write_json_restricted(&store, &payload)?;
        }
    }
    let marker = serde_json::json!({
        "home_relay_url": home,
        "assigned": assigned,
        "at": crate::util::now_iso(),
    });
    atomic_write_json(
        &marker_path(base_dir),
        serde_json::to_string_pretty(&marker)
            .map_err(|error| error.to_string())?
            .as_bytes(),
    )?;
    Ok(assigned)
}

/// The relay a record saved without a community should be stamped with: the
/// active community, once the legacy assignment has run. `None` before that
/// (boot migrations, pre-apply saves), so the legacy assignment — not the
/// active community — decides legacy records.
pub(crate) fn stamp_relay_for_save<R: tauri::Runtime>(app: &tauri::AppHandle<R>) -> Option<String> {
    let state = app.try_state::<crate::app_state::AppState>()?;
    let relay = crate::relay::workspace_relay_override(&state)?;
    let base_dir = super::managed_agents_base_dir(app).ok()?;
    legacy_assignment_done(&base_dir).then_some(relay)
}

/// Stamp unassigned records with `relay_url` (see [`stamp_relay_for_save`]).
/// Built-in definitions stay global.
pub(crate) fn stamp_unassigned(records: &mut [ManagedAgentRecord], relay_url: &str) {
    for record in records.iter_mut() {
        if record.pubkey.is_empty() && record.is_builtin {
            continue;
        }
        if record.relay_url.trim().is_empty() {
            record.relay_url = relay_url.to_string();
        }
    }
}

/// Whether a record (instance or definition) is visible and may act in the
/// community on `relay_url`. Built-in definitions are visible everywhere.
pub(crate) fn record_in_community(
    record: &ManagedAgentRecord,
    workspace_relay: &str,
    relay_url: &str,
) -> bool {
    (record.pubkey.is_empty() && record.is_builtin)
        || crate::relay::agent_belongs_to_relay(&record.relay_url, workspace_relay, relay_url)
}

/// Lower-case pubkeys of this device's agents that belong to a community
/// other than the one on `workspace_relay`.
pub(crate) fn other_community_agent_pubkeys(
    records: &[ManagedAgentRecord],
    workspace_relay: &str,
) -> std::collections::HashSet<String> {
    records
        .iter()
        .filter(|record| {
            !record.pubkey.is_empty()
                && !crate::relay::agent_belongs_to_relay(
                    &record.relay_url,
                    workspace_relay,
                    workspace_relay,
                )
        })
        .map(|record| record.pubkey.to_ascii_lowercase())
        .collect()
}

/// [`other_community_agent_pubkeys`] read from this device's agent store
/// (without touching the keyring), for relay-state readers that must hide
/// them: channel rosters, the relay agent directory, mention revalidation.
/// Such an agent can still have a profile and memberships on this relay from
/// before agents were scoped to one community.
pub(crate) fn load_other_community_agent_pubkeys<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    workspace_relay: &str,
) -> Result<std::collections::HashSet<String>, String> {
    Ok(other_community_agent_pubkeys(
        &load_agent_records_without_keys(app)?,
        workspace_relay,
    ))
}

/// This device's agent instances under the store lock, without keys.
pub(crate) fn load_agent_records_without_keys<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
) -> Result<Vec<ManagedAgentRecord>, String> {
    let state = app.state::<crate::app_state::AppState>();
    let _store = state
        .managed_agents_store_lock
        .lock()
        .map_err(|error| error.to_string())?;
    super::storage::load_managed_agents_without_keys(app)
}

#[cfg(test)]
#[path = "community_scope_tests.rs"]
mod tests;
