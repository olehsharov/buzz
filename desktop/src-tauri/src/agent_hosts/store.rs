//! Approved agent hosts, persisted next to the managed-agent store
//! (`<app data>/agents/agent-hosts.json`) and scoped per community by the
//! relay URL the host was approved on.

use std::path::Path;

use serde::{Deserialize, Serialize};

use super::frames::{HostAgentState, HostClaudeState, HostStatus, HostTools};

const HOSTS_FILE: &str = "agent-hosts.json";

/// Last `host.status` the desktop accepted from the host.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct HostStatusSnapshot {
    #[serde(default)]
    pub version: Option<String>,
    #[serde(default)]
    pub agents: Vec<HostAgentState>,
    #[serde(default)]
    pub claude: HostClaudeState,
    #[serde(default)]
    pub tools: HostTools,
    /// Unix seconds of the frame this snapshot came from.
    pub received_at: u64,
}

/// One approved machine.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AgentHostRecord {
    pub pubkey: String,
    pub name: String,
    pub os: String,
    pub arch: String,
    /// The community relay this host was approved on (its scope).
    pub relay_url: String,
    pub added_at: String,
    #[serde(default)]
    pub status: Option<HostStatusSnapshot>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct HostsFile {
    #[serde(default)]
    hosts: Vec<AgentHostRecord>,
}

/// Canonical community scope key for a relay URL.
pub fn scope_key(relay_url: &str) -> String {
    relay_url.trim().trim_end_matches('/').to_ascii_lowercase()
}

pub fn load_hosts(dir: &Path) -> Result<Vec<AgentHostRecord>, String> {
    let path = dir.join(HOSTS_FILE);
    match std::fs::read(&path) {
        Ok(bytes) => serde_json::from_slice::<HostsFile>(&bytes)
            .map(|file| file.hosts)
            .map_err(|error| format!("failed to parse {}: {error}", path.display())),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(error) => Err(format!("failed to read {}: {error}", path.display())),
    }
}

/// Atomic replace (write temp + rename) so a crash never leaves a torn file.
pub fn save_hosts(dir: &Path, hosts: &[AgentHostRecord]) -> Result<(), String> {
    std::fs::create_dir_all(dir).map_err(|error| format!("failed to create hosts dir: {error}"))?;
    let path = dir.join(HOSTS_FILE);
    let tmp = dir.join(format!("{HOSTS_FILE}.tmp"));
    let body = serde_json::to_vec_pretty(&HostsFile {
        hosts: hosts.to_vec(),
    })
    .map_err(|error| format!("failed to serialize hosts: {error}"))?;
    std::fs::write(&tmp, body).map_err(|error| format!("failed to write hosts: {error}"))?;
    std::fs::rename(&tmp, &path).map_err(|error| format!("failed to save hosts: {error}"))
}

/// Hosts approved on `relay_url`'s community.
pub fn hosts_in_scope(hosts: Vec<AgentHostRecord>, relay_url: &str) -> Vec<AgentHostRecord> {
    let scope = scope_key(relay_url);
    hosts
        .into_iter()
        .filter(|host| scope_key(&host.relay_url) == scope)
        .collect()
}

pub fn find_in_scope<'a>(
    hosts: &'a [AgentHostRecord],
    relay_url: &str,
    pubkey: &str,
) -> Option<&'a AgentHostRecord> {
    let scope = scope_key(relay_url);
    hosts.iter().find(|host| {
        host.pubkey.eq_ignore_ascii_case(pubkey) && scope_key(&host.relay_url) == scope
    })
}

/// Insert or replace (same host, same community) — re-pairing refreshes it.
pub fn upsert_host(hosts: &mut Vec<AgentHostRecord>, record: AgentHostRecord) {
    let scope = scope_key(&record.relay_url);
    hosts.retain(|host| {
        !(host.pubkey.eq_ignore_ascii_case(&record.pubkey) && scope_key(&host.relay_url) == scope)
    });
    hosts.push(record);
}

/// Remove a host from one community. Returns whether anything was removed.
pub fn remove_host(hosts: &mut Vec<AgentHostRecord>, relay_url: &str, pubkey: &str) -> bool {
    let scope = scope_key(relay_url);
    let before = hosts.len();
    hosts.retain(|host| {
        !(host.pubkey.eq_ignore_ascii_case(pubkey) && scope_key(&host.relay_url) == scope)
    });
    hosts.len() != before
}

/// Fold an accepted `host.status` into the record. An older frame never
/// overwrites a newer snapshot (relay reconnect replays).
pub fn apply_status(record: &mut AgentHostRecord, status: &HostStatus, received_at: u64) -> bool {
    if record
        .status
        .as_ref()
        .is_some_and(|current| current.received_at > received_at)
    {
        return false;
    }
    record.name = status.name.clone();
    record.os = status.os.clone();
    record.arch = status.arch.clone();
    record.status = Some(HostStatusSnapshot {
        version: status.version.clone(),
        agents: status.agents.clone(),
        claude: status.claude.clone(),
        tools: status.tools.clone(),
        received_at,
    });
    true
}
