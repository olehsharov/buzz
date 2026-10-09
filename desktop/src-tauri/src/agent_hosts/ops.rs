//! Host lifecycle operations: deploy, undeploy, status, forget.
//!
//! Invariants (hosts contract v1):
//! - A deploy is persisted only after the host's `host.ack` with `ok: true`
//!   arrives within the ack timeout. A timeout or refusal leaves the record
//!   undeployed and records the error on it.
//! - An agent lives on exactly one host. Moving it sends `host.undeploy` to
//!   the old host first and requires that ack before deploying elsewhere.
//! - Each operation holds the agent's lock and a generation ticket. Forgetting
//!   the host or deleting the agent while a frame is in flight invalidates the
//!   ticket, so a late ack can never resurrect the record.

use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
    time::Duration,
};

use nostr::{Keys, PublicKey};
use tauri::{AppHandle, Runtime};

use super::{
    channel::HostChannel,
    frames::{self, HostTelemetry},
    store::{self, AgentHostRecord},
};
use crate::{
    app_state::AppState,
    managed_agents::{
        load_managed_agents, managed_agent_runtime_keys, managed_agents_base_dir,
        save_managed_agents, BackendKind, ManagedAgentRecord,
    },
    util::now_iso,
};

/// Contract: wait up to 60 s for `host.ack`.
pub const ACK_TIMEOUT: Duration = Duration::from_secs(60);
/// Status and forget are cheap; do not hold the UI for a minute.
pub const STATUS_TIMEOUT: Duration = Duration::from_secs(15);

/// Desktop-wide host bookkeeping (Tauri managed state).
pub struct HostOps {
    pub(crate) store_lock: Mutex<()>,
    agent_locks: Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>,
    generations: Mutex<HashMap<String, u64>>,
    pub(crate) ack_timeout: Duration,
}

impl Default for HostOps {
    fn default() -> Self {
        Self {
            store_lock: Mutex::new(()),
            agent_locks: Mutex::new(HashMap::new()),
            generations: Mutex::new(HashMap::new()),
            ack_timeout: ACK_TIMEOUT,
        }
    }
}

impl HostOps {
    fn agent_lock(&self, agent: &str) -> Result<Arc<tokio::sync::Mutex<()>>, String> {
        let mut locks = self.agent_locks.lock().map_err(|e| e.to_string())?;
        Ok(Arc::clone(
            locks
                .entry(agent.to_ascii_lowercase())
                .or_insert_with(|| Arc::new(tokio::sync::Mutex::new(()))),
        ))
    }

    /// Start a new operation for `agent`; any earlier ticket becomes stale.
    pub(crate) fn begin(&self, agent: &str) -> Result<u64, String> {
        let mut generations = self.generations.lock().map_err(|e| e.to_string())?;
        let generation = generations.entry(agent.to_ascii_lowercase()).or_insert(0);
        *generation += 1;
        Ok(*generation)
    }

    /// Invalidate any in-flight operation for `agent` (forget, delete).
    pub(crate) fn invalidate(&self, agent: &str) -> Result<(), String> {
        self.begin(agent).map(|_| ())
    }

    pub(crate) fn is_current(&self, agent: &str, ticket: u64) -> Result<bool, String> {
        let generations = self.generations.lock().map_err(|e| e.to_string())?;
        Ok(generations.get(&agent.to_ascii_lowercase()) == Some(&ticket))
    }
}

pub(crate) const SUPERSEDED: &str =
    "This machine operation was superseded by a newer one; nothing was saved.";

fn parse_host(host_pubkey: &str) -> Result<PublicKey, String> {
    PublicKey::from_hex(host_pubkey.trim()).map_err(|_| "invalid machine key".to_string())
}

fn hosts_dir<R: Runtime>(app: &AppHandle<R>) -> Result<std::path::PathBuf, String> {
    managed_agents_base_dir(app)
}

/// The approved host `host_pubkey` in `community_relay`'s community.
pub fn approved_host<R: Runtime>(
    app: &AppHandle<R>,
    ops: &HostOps,
    community_relay: &str,
    host_pubkey: &str,
) -> Result<AgentHostRecord, String> {
    let _guard = ops.store_lock.lock().map_err(|e| e.to_string())?;
    let hosts = store::load_hosts(&hosts_dir(app)?)?;
    store::find_in_scope(&hosts, community_relay, host_pubkey)
        .cloned()
        .ok_or_else(|| "This machine is not approved in this community.".to_string())
}

fn load_record<R: Runtime>(
    app: &AppHandle<R>,
    state: &AppState,
    agent: &str,
) -> Result<ManagedAgentRecord, String> {
    let _guard = state
        .managed_agents_store_lock
        .lock()
        .map_err(|e| e.to_string())?;
    load_managed_agents(app)?
        .into_iter()
        .find(|record| record.pubkey == agent)
        .ok_or_else(|| format!("agent {agent} not found"))
}

/// Re-load, apply `update` only while `ticket` is current, and save.
fn persist_if_current<R: Runtime>(
    app: &AppHandle<R>,
    state: &AppState,
    ops: &HostOps,
    agent: &str,
    ticket: u64,
    update: impl FnOnce(&mut ManagedAgentRecord),
) -> Result<(), String> {
    let _guard = state
        .managed_agents_store_lock
        .lock()
        .map_err(|e| e.to_string())?;
    if !ops.is_current(agent, ticket)? {
        return Err(SUPERSEDED.to_string());
    }
    let mut records = load_managed_agents(app)?;
    let record = records
        .iter_mut()
        .find(|record| record.pubkey == agent)
        .ok_or_else(|| format!("agent {agent} not found"))?;
    update(record);
    record.updated_at = now_iso();
    save_managed_agents(app, &records)
}

/// Send one control payload and require a matching `host.ack { ok: true }`.
async fn send_expecting_ack(
    channel: &dyn HostChannel,
    owner_keys: &Keys,
    host: &PublicKey,
    request_id: String,
    payload: serde_json::Value,
    timeout: Duration,
) -> Result<(), String> {
    let event = frames::build_control_event(owner_keys, host, &payload)?;
    drop(payload);
    // The deadline is enforced here, not only inside the transport: no
    // channel implementation can hold an operation past the ack timeout.
    let reply = tokio::time::timeout(timeout, channel.exchange(event, *host, request_id, timeout))
        .await
        .map_err(|_| super::channel::timeout_error())??;
    match reply {
        HostTelemetry::Ack(ack) if ack.ok => Ok(()),
        HostTelemetry::Ack(ack) => Err(ack
            .error
            .map(|error| format!("The machine refused: {error}"))
            .unwrap_or_else(|| "The machine refused the command.".to_string())),
        HostTelemetry::Status(_) => {
            Err("The machine answered with status instead of an acknowledgement.".into())
        }
    }
}

fn local_process_running(state: &AppState, record: &ManagedAgentRecord) -> Result<bool, String> {
    let runtimes = state
        .managed_agent_processes
        .lock()
        .map_err(|e| e.to_string())?;
    Ok(!managed_agent_runtime_keys(&runtimes, &record.pubkey).is_empty())
}

/// The host an agent is currently deployed on, if any.
pub fn deployed_host(record: &ManagedAgentRecord) -> Option<&str> {
    match &record.backend {
        BackendKind::Host { host_pubkey, .. } if record.backend_agent_id.is_some() => {
            Some(host_pubkey.as_str())
        }
        _ => None,
    }
}

/// The folder saved for an agent on a machine (`None`: the machine default).
pub fn host_workdir(record: &ManagedAgentRecord) -> Option<&str> {
    match &record.backend {
        BackendKind::Host { workdir, .. } => workdir.as_deref(),
        _ => None,
    }
}

/// Change the folder an agent on a machine runs in.
///
/// One write saves the new folder and, for a deployed agent, marks it
/// `provider_policy_pending`; then the agent is redeployed to its machine so
/// it restarts there. A redeploy that fails keeps the saved folder and the
/// pending mark (workspace apply retries it) and records the error on the
/// agent. An agent that is not deployed picks the folder up on its next
/// deploy. Saving the folder it already has does nothing.
#[allow(clippy::too_many_arguments)]
pub async fn set_host_agent_workdir<R: Runtime>(
    app: &AppHandle<R>,
    state: &AppState,
    ops: &HostOps,
    channel: &dyn HostChannel,
    agent: &str,
    workdir: Option<&str>,
    community_relay: &str,
    build_payload: impl FnOnce(&ManagedAgentRecord) -> Result<serde_json::Value, String>,
) -> Result<(), String> {
    let workdir = frames::normalize_host_workdir(workdir)?;
    let redeploy_to = {
        let _guard = state
            .managed_agents_store_lock
            .lock()
            .map_err(|e| e.to_string())?;
        let mut records = load_managed_agents(app)?;
        let record = records
            .iter_mut()
            .find(|record| record.pubkey == agent)
            .ok_or_else(|| format!("agent {agent} not found"))?;
        crate::relay::ensure_agent_belongs_to_relay(
            &record.name,
            &record.relay_url,
            community_relay,
            community_relay,
        )?;
        let deployed = record.backend_agent_id.is_some();
        let BackendKind::Host {
            host_pubkey,
            workdir: saved,
        } = &mut record.backend
        else {
            return Err("Only an agent on a machine has a folder on that machine.".into());
        };
        if *saved == workdir {
            return Ok(());
        }
        *saved = workdir;
        let host_pubkey = host_pubkey.clone();
        if deployed {
            record.provider_policy_pending = true;
        }
        record.updated_at = now_iso();
        save_managed_agents(app, &records)?;
        deployed.then_some(host_pubkey)
    };
    let Some(host_pubkey) = redeploy_to else {
        return Ok(());
    };
    let Err(error) = deploy_agent_to_host(
        app,
        state,
        ops,
        channel,
        agent,
        &host_pubkey,
        community_relay,
        build_payload,
    )
    .await
    else {
        return Ok(());
    };
    // A failure before the frame went out (payload, unapproved machine) is
    // not recorded by the deploy; record every failure so the agent row
    // shows it. A superseded operation has nothing to report.
    if error != SUPERSEDED {
        record_error(app, state, agent, &error)?;
    }
    Err(format!(
        "The folder was saved, but restarting the agent in it failed: {error} Buzz retries the next time this community loads."
    ))
}

fn record_error<R: Runtime>(
    app: &AppHandle<R>,
    state: &AppState,
    agent: &str,
    error: &str,
) -> Result<(), String> {
    let _guard = state
        .managed_agents_store_lock
        .lock()
        .map_err(|e| e.to_string())?;
    let mut records = load_managed_agents(app)?;
    let Some(record) = records.iter_mut().find(|record| record.pubkey == agent) else {
        return Ok(());
    };
    record.last_error = Some(error.to_string());
    record.updated_at = now_iso();
    save_managed_agents(app, &records)
}

/// Deploy (or redeploy, or move) `agent` onto `host_pubkey`.
///
/// `build_payload` is the desktop's standard deploy payload builder
/// (production passes `build_deploy_payload`).
#[allow(clippy::too_many_arguments)]
pub async fn deploy_agent_to_host<R: Runtime>(
    app: &AppHandle<R>,
    state: &AppState,
    ops: &HostOps,
    channel: &dyn HostChannel,
    agent: &str,
    host_pubkey: &str,
    community_relay: &str,
    build_payload: impl FnOnce(&ManagedAgentRecord) -> Result<serde_json::Value, String>,
) -> Result<(), String> {
    let lock = ops.agent_lock(agent)?;
    let _serial = lock.lock().await;
    let ticket = ops.begin(agent)?;
    let host = parse_host(host_pubkey)?;
    let host_hex = host.to_hex();
    let host_record = approved_host(app, ops, community_relay, &host_hex)?;
    let owner_keys = state.signing_keys()?;

    let record = load_record(app, state, agent)?;
    // An agent belongs to ONE community: deploy it only from (and to a
    // machine of) its own community.
    crate::relay::ensure_agent_belongs_to_relay(
        &record.name,
        &record.relay_url,
        community_relay,
        community_relay,
    )?;
    match &record.backend {
        BackendKind::Provider { .. } => {
            return Err("Agents deployed through a provider cannot be moved to a machine.".into())
        }
        BackendKind::Local if local_process_running(state, &record)? => {
            return Err(format!(
                "Stop the agent on this computer before moving it to {}.",
                host_record.name
            ));
        }
        _ => {}
    }

    // Move: the old host must confirm the agent is gone before it starts
    // anywhere else (one agent, one host).
    if let Some(old_host) =
        deployed_host(&record).filter(|old| !old.eq_ignore_ascii_case(&host_hex))
    {
        let old_host = parse_host(old_host)?;
        let request_id = frames::new_request_id();
        send_expecting_ack(
            channel,
            &owner_keys,
            &old_host,
            request_id.clone(),
            frames::undeploy_frame(&request_id, agent),
            ops.ack_timeout,
        )
        .await
        .map_err(|error| format!("Could not remove the agent from its current machine: {error}"))?;
        persist_if_current(app, state, ops, agent, ticket, |record| {
            record.backend_agent_id = None;
            record.last_stopped_at = Some(now_iso());
        })?;
    }

    let record = load_record(app, state, agent)?;
    let payload = build_payload(&record)?;
    let auth_tag = match record.auth_tag.as_deref() {
        Some(tag) => tag.to_string(),
        None => {
            let agent_pk = parse_host(agent)?;
            buzz_sdk_pkg::nip_oa::compute_auth_tag(&owner_keys, &agent_pk, "")
                .map_err(|error| format!("failed to compute NIP-OA auth tag: {error}"))?
        }
    };
    // The saved folder travels with the agent, including on a move between
    // machines; an agent arriving from this computer uses the default.
    let delivered_workdir = host_workdir(&record).map(str::to_string);
    let request_id = frames::new_request_id();
    let frame = frames::deploy_frame(
        &request_id,
        agent,
        &auth_tag,
        delivered_workdir.as_deref(),
        &payload,
    )?;
    // Only the access policy outlives the payload (it carries the agent nsec):
    // the ack acknowledges a pending policy only if it is still the saved one.
    let delivered_policy = serde_json::json!({
        "respond_to": payload.get("respond_to"),
        "respond_to_allowlist": payload.get("respond_to_allowlist"),
    });
    drop(payload);
    let outcome = send_expecting_ack(
        channel,
        &owner_keys,
        &host,
        request_id,
        frame,
        ops.ack_timeout,
    )
    .await;

    match outcome {
        Ok(()) => persist_if_current(app, state, ops, agent, ticket, |record| {
            // Keep the folder saved NOW: one changed while this frame was in
            // flight stays pending below and is delivered by its own redeploy.
            let saved_workdir = host_workdir(record).map(str::to_string);
            let folder_delivered = saved_workdir == delivered_workdir;
            record.backend = BackendKind::Host {
                host_pubkey: host_hex.clone(),
                workdir: saved_workdir,
            };
            record.backend_agent_id = Some(host_hex.clone());
            record.start_on_app_launch = false;
            record.last_started_at = Some(now_iso());
            record.last_error = None;
            // The machine restarted the agent with this payload's policy and
            // folder. A newer policy or folder saved while the frame was in
            // flight stays pending.
            if folder_delivered
                && crate::managed_agents::access_policy::deployed_policy_matches_record(
                    record,
                    &delivered_policy,
                )
            {
                record.provider_policy_pending = false;
            }
        }),
        Err(error) => {
            let message = format!("Deploy to {} failed: {error}", host_record.name);
            // The record stays undeployed; the error is durable on it.
            persist_if_current(app, state, ops, agent, ticket, |record| {
                record.last_error = Some(message.clone());
            })?;
            Err(message)
        }
    }
}

/// The machine an agent is deployed on, and the relay that reaches it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostRoute {
    pub host_pubkey: String,
    pub host_name: String,
    /// The relay the machine was approved on: the one it listens to.
    pub relay_url: String,
}

/// Where to send control frames for the machine `record` is deployed on.
///
/// The machine only listens on the relay it was approved on, so the frame
/// must go there, not to whichever community the invoking window shows. A
/// machine approved in several communities is reached through the agent's
/// own community when possible. `Ok(None)`: the agent is not deployed on a
/// machine.
pub fn deployed_host_route<R: Runtime>(
    app: &AppHandle<R>,
    ops: &HostOps,
    record: &ManagedAgentRecord,
    community_relay: &str,
) -> Result<Option<HostRoute>, String> {
    let Some(host) = deployed_host(record) else {
        return Ok(None);
    };
    let _guard = ops.store_lock.lock().map_err(|e| e.to_string())?;
    let hosts = store::load_hosts(&hosts_dir(app)?)?;
    let agent_scope = store::scope_key(&crate::relay::effective_agent_relay_url(
        &record.relay_url,
        community_relay,
    ));
    let approvals: Vec<&AgentHostRecord> = hosts
        .iter()
        .filter(|candidate| candidate.pubkey.eq_ignore_ascii_case(host))
        .collect();
    let approval = approvals
        .iter()
        .find(|candidate| store::scope_key(&candidate.relay_url) == agent_scope)
        .or_else(|| approvals.first())
        .ok_or_else(|| {
            "The machine this agent was deployed on is no longer approved on this computer, \
             so it cannot be asked to remove the agent."
                .to_string()
        })?;
    Ok(Some(HostRoute {
        host_pubkey: approval.pubkey.clone(),
        host_name: approval.name.clone(),
        relay_url: approval.relay_url.clone(),
    }))
}

/// Remove `agent` from the host it is deployed on, requiring the ack: takes
/// the agent's lock and runs the production undeploy. Production callers
/// hold the lock themselves and use [`undeploy_agent_via_its_host`].
///
/// A failure is recorded on the agent (`last_error`) so it stays visible
/// after the dialog that reported it is gone.
#[cfg(test)]
pub async fn undeploy_agent_from_host<R: Runtime>(
    app: &AppHandle<R>,
    state: &AppState,
    ops: &HostOps,
    channel: &dyn HostChannel,
    agent: &str,
) -> Result<(), String> {
    let held = ops.hold(agent).await?;
    undeploy_held(app, state, ops, &held, channel, agent)
        .await
        .map_err(UndeployError::into_message)
}

/// Why removing an agent from its machine failed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UndeployError {
    /// The machine could not be reached, is no longer approved, or did not
    /// confirm. Deleting locally anyway is the user's way out.
    Machine(String),
    /// Something on this computer failed (the store, a newer operation);
    /// the machine is not the problem and a forced delete would not help.
    Local(String),
}

impl UndeployError {
    pub fn into_message(self) -> String {
        match self {
            Self::Machine(message) | Self::Local(message) => message,
        }
    }
}

/// Proof that the caller holds an agent's operation lock (see
/// [`HostOps::hold`]).
pub type AgentHold = tokio::sync::OwnedMutexGuard<()>;

impl HostOps {
    /// Hold `agent`'s operation lock (deploy, undeploy, delete) until the
    /// returned guard drops; a deploy started meanwhile waits for it.
    pub(crate) async fn hold(&self, agent: &str) -> Result<AgentHold, String> {
        Ok(self.agent_lock(agent)?.lock_owned().await)
    }
}

async fn undeploy_held<R: Runtime>(
    app: &AppHandle<R>,
    state: &AppState,
    ops: &HostOps,
    _held: &AgentHold,
    channel: &dyn HostChannel,
    agent: &str,
) -> Result<(), UndeployError> {
    let ticket = ops.begin(agent).map_err(UndeployError::Local)?;
    let record = load_record(app, state, agent).map_err(UndeployError::Local)?;
    let Some(host) = deployed_host(&record) else {
        return Ok(());
    };
    let host = parse_host(host).map_err(UndeployError::Local)?;
    let owner_keys = state.signing_keys().map_err(UndeployError::Local)?;
    let request_id = frames::new_request_id();
    tracing::info!(agent = %agent, host = %host.to_hex(), request_id = %request_id, "sending host.undeploy");
    let outcome = send_expecting_ack(
        channel,
        &owner_keys,
        &host,
        request_id.clone(),
        frames::undeploy_frame(&request_id, agent),
        ops.ack_timeout,
    )
    .await;
    if let Err(error) = outcome {
        tracing::warn!(agent = %agent, request_id = %request_id, "host.undeploy failed: {error}");
        let message = format!("Removing the agent from its machine failed: {error}");
        match persist_if_current(app, state, ops, agent, ticket, |record| {
            record.last_error = Some(message.clone());
        }) {
            Ok(()) => {}
            Err(persist) if persist == SUPERSEDED => {}
            Err(persist) => {
                tracing::warn!(agent = %agent, "could not record the undeploy failure: {persist}")
            }
        }
        return Err(UndeployError::Machine(error));
    }
    // The machine confirmed; a failure from here on is local.
    persist_if_current(app, state, ops, agent, ticket, |record| {
        record.backend_agent_id = None;
        record.last_stopped_at = Some(now_iso());
        record.last_error = None;
    })
    .map_err(UndeployError::Local)
}

/// [`undeploy_agent_from_host`] over the relay the agent's machine listens
/// on (see [`deployed_host_route`]), for a caller already holding the
/// agent's lock. `connect` opens the channel to that relay. Every machine
/// failure, including an unreachable route, is recorded on the agent.
pub async fn undeploy_agent_via_its_host<R: Runtime, C: HostChannel>(
    app: &AppHandle<R>,
    state: &AppState,
    ops: &HostOps,
    held: &AgentHold,
    community_relay: &str,
    agent: &str,
    connect: impl FnOnce(&HostRoute) -> Result<C, String>,
) -> Result<(), UndeployError> {
    let record = load_record(app, state, agent).map_err(UndeployError::Local)?;
    let route = match deployed_host_route(app, ops, &record, community_relay) {
        Ok(Some(route)) => route,
        Ok(None) => return Ok(()),
        Err(error) => {
            tracing::warn!(agent = %agent, "no route to the agent's machine: {error}");
            record_error(app, state, agent, &error).map_err(UndeployError::Local)?;
            return Err(UndeployError::Machine(error));
        }
    };
    let channel = connect(&route).map_err(UndeployError::Local)?;
    undeploy_held(app, state, ops, held, &channel, agent)
        .await
        .map_err(|error| match error {
            UndeployError::Machine(error) => {
                UndeployError::Machine(format!("{} did not confirm: {error}", route.host_name))
            }
            local => local,
        })
}

/// Ask the host for `host.status` and fold the reply into the store.
pub async fn refresh_host_status<R: Runtime>(
    app: &AppHandle<R>,
    state: &AppState,
    ops: &HostOps,
    channel: &dyn HostChannel,
    community_relay: &str,
    host_pubkey: &str,
) -> Result<AgentHostRecord, String> {
    let host = parse_host(host_pubkey)?;
    approved_host(app, ops, community_relay, &host.to_hex())?;
    let owner_keys = state.signing_keys()?;
    let request_id = frames::new_request_id();
    let event =
        frames::build_control_event(&owner_keys, &host, &frames::status_frame(&request_id))?;
    let reply = tokio::time::timeout(
        STATUS_TIMEOUT,
        channel.exchange(event, host, request_id, STATUS_TIMEOUT),
    )
    .await
    .map_err(|_| super::channel::timeout_error())??;
    match reply {
        HostTelemetry::Status(status) => apply_status_to_store(
            app,
            ops,
            community_relay,
            &host.to_hex(),
            &status,
            frames::now_secs(),
        ),
        HostTelemetry::Ack(_) => {
            Err("The machine acknowledged instead of reporting status.".into())
        }
    }
}

pub fn apply_status_to_store<R: Runtime>(
    app: &AppHandle<R>,
    ops: &HostOps,
    community_relay: &str,
    host_hex: &str,
    status: &frames::HostStatus,
    received_at: u64,
) -> Result<AgentHostRecord, String> {
    let status = frames::sanitize_status(status);
    let _guard = ops.store_lock.lock().map_err(|e| e.to_string())?;
    let dir = hosts_dir(app)?;
    let mut hosts = store::load_hosts(&dir)?;
    let scope = store::scope_key(community_relay);
    let record = hosts
        .iter_mut()
        .find(|host| {
            host.pubkey.eq_ignore_ascii_case(host_hex) && store::scope_key(&host.relay_url) == scope
        })
        .ok_or_else(|| "This machine is not approved in this community.".to_string())?;
    if store::apply_status(record, &status, received_at) {
        let updated = record.clone();
        store::save_hosts(&dir, &hosts)?;
        Ok(updated)
    } else {
        Ok(record.clone())
    }
}

/// Outcome of forgetting a machine. The local removal always happens; the
/// host's own confirmation is reported separately so the UI can say so.
#[derive(Debug, Clone, serde::Serialize)]
pub struct ForgetHostOutcome {
    pub host_acknowledged: bool,
    pub host_error: Option<String>,
    pub undeployed_agents: Vec<String>,
}

/// `host.forget`, then remove the host locally and mark its agents
/// undeployed. An unreachable host must not strand the user with a machine
/// they cannot remove, so the local removal does not wait on the ack.
pub async fn forget_host<R: Runtime>(
    app: &AppHandle<R>,
    state: &AppState,
    ops: &HostOps,
    channel: &dyn HostChannel,
    community_relay: &str,
    host_pubkey: &str,
) -> Result<ForgetHostOutcome, String> {
    let host = parse_host(host_pubkey)?;
    let host_hex = host.to_hex();
    approved_host(app, ops, community_relay, &host_hex)?;

    // Invalidate in-flight operations targeting this host first, so a late
    // deploy ack cannot mark an agent deployed on a forgotten machine.
    let affected: Vec<String> = {
        let _guard = state
            .managed_agents_store_lock
            .lock()
            .map_err(|e| e.to_string())?;
        load_managed_agents(app)?
            .into_iter()
            .filter(|record| {
                matches!(&record.backend, BackendKind::Host { host_pubkey, .. } if host_pubkey.eq_ignore_ascii_case(&host_hex))
            })
            .map(|record| record.pubkey)
            .collect()
    };
    for agent in &affected {
        ops.invalidate(agent)?;
    }

    let owner_keys = state.signing_keys()?;
    let request_id = frames::new_request_id();
    let host_result = send_expecting_ack(
        channel,
        &owner_keys,
        &host,
        request_id.clone(),
        frames::forget_frame(&request_id),
        STATUS_TIMEOUT,
    )
    .await;

    {
        let _guard = state
            .managed_agents_store_lock
            .lock()
            .map_err(|e| e.to_string())?;
        let mut records = load_managed_agents(app)?;
        let mut changed = false;
        for record in records
            .iter_mut()
            .filter(|record| affected.contains(&record.pubkey))
        {
            if record.backend_agent_id.take().is_some() {
                record.last_stopped_at = Some(now_iso());
            }
            record.updated_at = now_iso();
            changed = true;
        }
        if changed {
            save_managed_agents(app, &records)?;
        }
    }
    {
        let _guard = ops.store_lock.lock().map_err(|e| e.to_string())?;
        let dir = hosts_dir(app)?;
        let mut hosts = store::load_hosts(&dir)?;
        if store::remove_host(&mut hosts, community_relay, &host_hex) {
            store::save_hosts(&dir, &hosts)?;
        }
    }
    Ok(ForgetHostOutcome {
        host_acknowledged: host_result.is_ok(),
        host_error: host_result.err(),
        undeployed_agents: affected,
    })
}

/// Persist an approved host (pairing completed).
pub fn add_approved_host<R: Runtime>(
    app: &AppHandle<R>,
    ops: &HostOps,
    record: AgentHostRecord,
) -> Result<(), String> {
    let _guard = ops.store_lock.lock().map_err(|e| e.to_string())?;
    let dir = hosts_dir(app)?;
    let mut hosts = store::load_hosts(&dir)?;
    store::upsert_host(&mut hosts, record);
    store::save_hosts(&dir, &hosts)
}

pub fn list_hosts<R: Runtime>(
    app: &AppHandle<R>,
    ops: &HostOps,
    community_relay: &str,
) -> Result<Vec<AgentHostRecord>, String> {
    let _guard = ops.store_lock.lock().map_err(|e| e.to_string())?;
    Ok(store::hosts_in_scope(
        store::load_hosts(&hosts_dir(app)?)?,
        community_relay,
    ))
}

#[cfg(test)]
#[path = "ops_tests.rs"]
mod tests;
