//! `delete_managed_agent`: remove an agent from where it runs, then delete
//! its local record.

use tauri::{AppHandle, Manager};

use super::{run_managed_agent_deletion, tombstone_managed_agent_pending};
use crate::managed_agents::channel_cleanup::{
    remove_agent_from_channels, ChannelCleanupReport, RelayMembership,
};
use crate::{
    app_state::AppState,
    managed_agents::{
        bestie_assignment::recover_pending_assignment_cleanup, current_instance_id,
        load_managed_agents, managed_agents_base_dir, save_managed_agents,
        stop_managed_agent_process, sync_managed_agent_processes, try_regenerate_nest, BackendKind,
        ManagedAgentRecord,
    },
};

/// Prefix of the delete error when the agent's machine did not confirm the
/// undeploy. The UI offers "delete anyway" (`force_remote_delete: true`).
pub const HOST_UNDEPLOY_FAILED_PREFIX: &str = "host-undeploy-failed: ";

const REMOTE_DELETE_NEEDS_FORCE: &str =
    "cannot delete a deployed remote agent without force_remote_delete: true";

/// Ask the machine an agent runs on to remove it, over the relay that
/// machine listens on. Not deployed on a machine: nothing to do. Failures
/// carry [`HOST_UNDEPLOY_FAILED_PREFIX`] and are recorded on the agent.
pub(crate) async fn undeploy_before_delete<R: tauri::Runtime, C>(
    app: &AppHandle<R>,
    community_relay: &str,
    pubkey: &str,
    connect: impl FnOnce(&crate::agent_hosts::ops::HostRoute) -> Result<C, String>,
) -> Result<(), String>
where
    C: crate::agent_hosts::channel::HostChannel,
{
    let state = app.state::<AppState>();
    let hosts = app.state::<crate::agent_hosts::HostOps>();
    crate::agent_hosts::ops::undeploy_agent_via_its_host(
        app,
        &state,
        &hosts,
        community_relay,
        pubkey,
        connect,
    )
    .await
    .map_err(|error| format!("{HOST_UNDEPLOY_FAILED_PREFIX}{error}"))
}

/// What a completed delete did beyond removing the record.
#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct DeleteManagedAgentOutcome {
    /// Channels the agent was removed from, and any it is still in.
    pub channels: ChannelCleanupReport,
}

/// The agent's record, read under the store lock.
pub(crate) fn managed_agent_record<R: tauri::Runtime>(
    app: &AppHandle<R>,
    pubkey: &str,
) -> Result<ManagedAgentRecord, String> {
    let state = app.state::<AppState>();
    let _guard = state
        .managed_agents_store_lock
        .lock()
        .map_err(|error| error.to_string())?;
    load_managed_agents(app)?
        .into_iter()
        .find(|record| record.pubkey == pubkey)
        .ok_or_else(|| format!("agent {pubkey} not found"))
}

/// Remove `record`'s agent from every channel it is in, on its own
/// community's relay. Uses the agent's key to find private channels too, so
/// it must run before the key is deleted. Never fails; see the report.
pub(crate) async fn remove_agent_from_its_channels(
    state: &AppState,
    record: &ManagedAgentRecord,
    community_relay: &str,
) -> ChannelCleanupReport {
    let agent_keys = match nostr::Keys::parse(record.private_key_nsec.trim()) {
        Ok(keys) => keys,
        Err(_) => {
            return ChannelCleanupReport {
                lookup_error: Some(
                    "the agent's key is unavailable, so its channels could not be listed"
                        .to_string(),
                ),
                ..ChannelCleanupReport::default()
            }
        }
    };
    let relay = crate::relay::effective_agent_relay_url(&record.relay_url, community_relay);
    let port = RelayMembership {
        state,
        api_base: crate::relay::relay_http_base_url(&relay),
        agent_pubkey: record.pubkey.clone(),
        agent_keys,
        agent_auth_tag: record.auth_tag.clone(),
    };
    let report = remove_agent_from_channels(&port).await;
    tracing::info!(
        agent = %record.pubkey,
        removed = report.removed.len(),
        failed = report.failed.len(),
        lookup_failed = report.lookup_error.is_some(),
        "removed agent from its channels"
    );
    report
}

#[tauri::command]
pub async fn delete_managed_agent(
    pubkey: String,
    force_remote_delete: Option<bool>,
    app: AppHandle,
    relay: crate::window_relay::WindowRelay,
) -> Result<DeleteManagedAgentOutcome, String> {
    let force = force_remote_delete.unwrap_or(false);
    tracing::info!(agent = %pubkey, force, "deleting managed agent");
    let result = delete_managed_agent_inner(&pubkey, force, &app, &relay).await;
    if let Err(error) = &result {
        tracing::warn!(agent = %pubkey, force, "deleting managed agent failed: {error}");
    }
    result
}

async fn delete_managed_agent_inner(
    pubkey: &str,
    force: bool,
    app: &AppHandle,
    relay: &crate::window_relay::WindowRelay,
) -> Result<DeleteManagedAgentOutcome, String> {
    let record = managed_agent_record(app, pubkey)?;
    // A host agent is removed from its machine first, and the machine must
    // acknowledge it. Forcing skips the machine (it may be gone for good) —
    // the user confirmed that in the UI after this failed once.
    if !force && crate::agent_hosts::ops::deployed_host(&record).is_some() {
        let owner_keys = app.state::<AppState>().signing_keys()?;
        undeploy_before_delete(app, relay.ws_url(), pubkey, |route| {
            Ok(crate::agent_hosts::channel::RelayHostChannel {
                relay_url: route.relay_url.clone(),
                owner_keys,
            })
        })
        .await?;
    }
    // Refuse before touching channels: the same guard runs again under the
    // store lock below, but an agent that will not be deleted must not be
    // removed from its channels either.
    let record = managed_agent_record(app, pubkey)?;
    if record.backend != BackendKind::Local && record.backend_agent_id.is_some() && !force {
        return Err(REMOTE_DELETE_NEEDS_FORCE.to_string());
    }
    // Leave every channel while the agent's key still exists. A channel that
    // cannot be cleared is reported back; it never blocks the delete.
    let channels =
        remove_agent_from_its_channels(&app.state::<AppState>(), &record, relay.ws_url()).await;
    app.state::<crate::agent_hosts::HostOps>()
        .invalidate(pubkey)?;
    let app = app.clone();
    let pubkey = pubkey.to_string();
    let community_relay = relay.ws_url().to_string();
    tokio::task::spawn_blocking(move || {
        let state = app.state::<AppState>();
        {
            let _store_guard = state
                .managed_agents_store_lock
                .lock()
                .map_err(|error| error.to_string())?;
            let mut records = load_managed_agents(&app)?;
            let base_dir = managed_agents_base_dir(&app)?;
            recover_pending_assignment_cleanup(&base_dir, |pending_pubkey| {
                records
                    .iter()
                    .any(|record| record.pubkey.eq_ignore_ascii_case(pending_pubkey))
            })?;
            let mut runtimes = state
                .managed_agent_processes
                .lock()
                .map_err(|error| error.to_string())?;

            let (sync_changed, exited_pubkeys) = sync_managed_agent_processes(
                &mut records,
                &mut runtimes,
                &current_instance_id(&app),
            );
            if sync_changed {
                save_managed_agents(&app, &records)?;
            }
            for pubkey in &exited_pubkeys {
                state.clear_agent_session_caches(pubkey);
            }
            // Guard: reject deletion of deployed remote agents unless explicitly forced.
            // This turns "don't orphan remote infra" from a UI convention into a backend
            // invariant — a buggy or compromised IPC caller cannot silently orphan a live
            // remote deployment. The frontend sends force_remote_delete: true only after
            // the user confirms the orphan warning.
            if let Some(record) = records.iter().find(|r| r.pubkey == pubkey) {
                if record.backend != BackendKind::Local
                    && record.backend_agent_id.is_some()
                    && !force
                {
                    return Err(REMOTE_DELETE_NEEDS_FORCE.to_string());
                }
            }

            if !records.iter().any(|record| record.pubkey == pubkey) {
                return Err(format!("agent {pubkey} not found"));
            }
            run_managed_agent_deletion(&base_dir, &pubkey, &mut records, |records| {
                if let Some(record) = records.iter_mut().find(|record| record.pubkey == pubkey) {
                    stop_managed_agent_process(&app, record, &mut runtimes)?;
                }
                state.clear_agent_session_caches(&pubkey);
                records.retain(|record| record.pubkey != pubkey);
                save_managed_agents(&app, records)
            })?;
            crate::managed_agents::delete_agent_key(&pubkey);
            // Tombstone after confirmed removal (inside lock; every published
            // agent tombstones). The NIP-IA kind:9035 archive request — which
            // stops the identity appearing in member pickers and autocomplete —
            // is enqueued in the SAME transaction, its `persona_id` derived from
            // the retained 30177 head.
            tombstone_managed_agent_pending(&app, &state, &community_relay, &pubkey);
        }
        try_regenerate_nest(&app);
        Ok(DeleteManagedAgentOutcome { channels })
    })
    .await
    .map_err(|e| format!("spawn_blocking failed: {e}"))?
}
