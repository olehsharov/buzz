//! How a saved access-policy change reaches the agent that is running.
//!
//! The harness reads its inbound-author gate (`BUZZ_ACP_RESPOND_TO*`) once,
//! at startup, so a changed policy takes effect only when the agent restarts:
//! a local agent is stopped and started again, a deployed provider or paired
//! machine agent is redeployed. Both the desktop edit
//! (`update_managed_agent`) and the cross-device inbound apply plan the change
//! here so the two paths cannot drift.

use std::collections::HashMap;

use tauri::AppHandle;

use crate::{
    app_state::AppState,
    managed_agents::{BackendKind, ManagedAgentRecord, ManagedAgentRuntimeKey},
};

/// The runtime work a saved access-policy change requires.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum AccessRuntimeTransition {
    /// Nothing is running that holds the previous policy.
    None,
    /// Stop these local pairs before saving, start them again afterwards.
    RestartLocal { relay_urls: Vec<String> },
    /// Redeploy the remote agent with the saved policy.
    Redeploy(RemoteAccessRedeploy),
}

/// The remote deployment that must be refreshed with the saved policy.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum RemoteAccessRedeploy {
    Provider,
    Host {
        host_pubkey: String,
        /// The agent's own community: where its machine is approved.
        community_relay: String,
    },
}

/// Decide how a changed access policy reaches the running instance.
///
/// For a deployed remote agent this also sets `provider_policy_pending` on
/// `record`; the caller saves it in the same write as the new policy, so a
/// redeploy that fails, or never runs because the app exits first, is retried
/// on the next workspace apply (and by Deploy). The flag is cleared only by a
/// deployment that delivered the saved policy.
///
/// `community_relay` is the invoking window's community. It names the
/// community of a legacy record with no pinned relay; a pinned relay wins.
pub(crate) fn plan_access_runtime_transition<T>(
    record: &mut ManagedAgentRecord,
    access_policy_changed: bool,
    runtimes: &HashMap<ManagedAgentRuntimeKey, T>,
    community_relay: &str,
) -> AccessRuntimeTransition {
    if !access_policy_changed {
        return AccessRuntimeTransition::None;
    }
    let agent_relay = crate::relay::effective_agent_relay_url(&record.relay_url, community_relay);
    let redeploy = match &record.backend {
        BackendKind::Local => {
            let mut relay_urls: Vec<String> =
                crate::managed_agents::managed_agent_runtime_keys(runtimes, &record.pubkey)
                    .into_iter()
                    .map(|key| key.relay_url)
                    .collect();
            if relay_urls.is_empty() && record.runtime_pid.is_some() {
                relay_urls.push(agent_relay);
            }
            return if relay_urls.is_empty() {
                AccessRuntimeTransition::None
            } else {
                AccessRuntimeTransition::RestartLocal { relay_urls }
            };
        }
        // Not deployed: the next deploy builds its payload from the saved
        // record, so there is nothing running to refresh.
        BackendKind::Provider { .. } | BackendKind::Host { .. }
            if record.backend_agent_id.is_none() =>
        {
            return AccessRuntimeTransition::None;
        }
        BackendKind::Provider { .. } => RemoteAccessRedeploy::Provider,
        BackendKind::Host { host_pubkey } => RemoteAccessRedeploy::Host {
            host_pubkey: host_pubkey.clone(),
            community_relay: agent_relay,
        },
    };
    record.provider_policy_pending = true;
    AccessRuntimeTransition::Redeploy(redeploy)
}

/// Redeploy a remote agent so it runs with its saved access policy.
///
/// Both backends rebuild the deploy payload from the current record under
/// their own per-agent lock, so the newest saved policy is what ships. On
/// failure the record keeps `provider_policy_pending` and carries the error
/// in `last_error`.
pub(crate) async fn redeploy_for_access_policy<R: tauri::Runtime>(
    app: &AppHandle<R>,
    state: &AppState,
    pubkey: &str,
    target: &RemoteAccessRedeploy,
) -> Result<(), String> {
    let result = match target {
        // `deploy_to_provider` re-reads the record after taking the deploy
        // lock and derives the provider, config, binary, and payload from it;
        // the caller-side snapshot arguments are not used for the invocation.
        RemoteAccessRedeploy::Provider => {
            super::deploy_to_provider(
                app,
                state,
                pubkey,
                "",
                &serde_json::Value::Null,
                serde_json::Value::Null,
                None,
                None,
                None,
                None,
            )
            .await
        }
        RemoteAccessRedeploy::Host {
            host_pubkey,
            community_relay,
        } => super::deploy_host_agent(app, state, community_relay, pubkey, host_pubkey).await,
    };
    if let Err(error) = &result {
        // A payload or lock failure returns before the deploy records it;
        // make every failure visible on the agent row.
        if let Err(persist_error) =
            super::provider_access::persist_failure(app, state, pubkey, error)
        {
            return Err(format!(
                "{error} (recording the failure also failed: {persist_error})"
            ));
        }
    }
    result
}

/// Pending access redeploys that workspace apply retries in the background
/// for `community_relay`: paired-machine agents of that community (their
/// machine is approved there), and, unless this build enforces owner-only
/// access (which redeploys every provider agent inline and fails closed),
/// provider agents, whose payload always targets their own community.
pub(crate) fn pending_access_redeploys(
    records: &[ManagedAgentRecord],
    community_relay: &str,
    owner_only_access: bool,
) -> Vec<(String, RemoteAccessRedeploy)> {
    records
        .iter()
        .filter(|record| super::provider_access::needs_reconciliation_with_policy(record, false))
        .filter_map(|record| match &record.backend {
            BackendKind::Host { host_pubkey }
                if crate::relay::agent_belongs_to_relay(
                    &record.relay_url,
                    community_relay,
                    community_relay,
                ) =>
            {
                Some((
                    record.pubkey.clone(),
                    RemoteAccessRedeploy::Host {
                        host_pubkey: host_pubkey.clone(),
                        community_relay: community_relay.to_string(),
                    },
                ))
            }
            BackendKind::Provider { .. } if !owner_only_access => {
                Some((record.pubkey.clone(), RemoteAccessRedeploy::Provider))
            }
            BackendKind::Local | BackendKind::Host { .. } | BackendKind::Provider { .. } => None,
        })
        .collect()
}

#[cfg(test)]
#[path = "access_transition_tests.rs"]
mod tests;
