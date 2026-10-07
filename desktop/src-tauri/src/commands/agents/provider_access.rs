//! Workspace-apply reconciliation for remote managed-agent access: redeploys
//! deployed agents whose saved access policy no successful deployment has
//! acknowledged yet (`provider_policy_pending`), and every provider agent on
//! owner-only builds.

use tauri::{AppHandle, Manager};

use crate::{
    app_state::AppState,
    managed_agents::{
        find_managed_agent_mut, load_managed_agents, save_managed_agents, BackendKind,
        ManagedAgentRecord,
    },
    util::now_iso,
};

/// Whether workspace apply must redeploy `record` to enforce its access
/// policy. Provider agents: on owner-only builds always, otherwise while a
/// saved policy is pending. Paired-machine agents: while pending (owner-only
/// builds project owner-only into every host deploy already).
pub(super) fn needs_reconciliation_with_policy(
    record: &ManagedAgentRecord,
    owner_only_access: bool,
) -> bool {
    if record.backend_agent_id.is_none() {
        return false;
    }
    match record.backend {
        BackendKind::Provider { .. } => owner_only_access || record.provider_policy_pending,
        BackendKind::Host { .. } => record.provider_policy_pending,
        BackendKind::Local => false,
    }
}

#[derive(Debug)]
struct ProviderAccessTarget {
    pubkey: String,
    provider_id: String,
    config: serde_json::Value,
    cached_binary_path: Option<String>,
    agent_json: Result<serde_json::Value, String>,
}

fn collect_targets_with(
    records: Vec<ManagedAgentRecord>,
    owner_only_access: bool,
    mut build_payload: impl FnMut(&ManagedAgentRecord) -> Result<serde_json::Value, String>,
) -> Vec<ProviderAccessTarget> {
    records
        .into_iter()
        .filter(|record| needs_reconciliation_with_policy(record, owner_only_access))
        .filter_map(|record| match record.backend.clone() {
            BackendKind::Provider { id, config } => Some(ProviderAccessTarget {
                agent_json: build_payload(&record),
                pubkey: record.pubkey,
                provider_id: id,
                config,
                cached_binary_path: record.provider_binary_path,
            }),
            // Paired machines are retried in the background by
            // `spawn_pending_host_redeploys`.
            BackendKind::Local | BackendKind::Host { .. } => None,
        })
        .collect()
}

/// Redeploy existing remote agents whose access policy requires enforcement.
///
/// Owner-only builds refresh every existing provider deployment before each
/// community UI load. All builds also retry records whose saved policy has not
/// yet been acknowledged by a successful deployment. Workspace apply fails
/// closed if any selected provider rejects the current policy. Paired machines
/// are user computers that may legitimately be offline, so their retries run
/// in the background instead of blocking the community load; a failure stays
/// on the record (pending flag + `last_error`) for the next apply or Deploy.
pub(crate) async fn reconcile_on_workspace_apply(
    app: &AppHandle,
    state: &AppState,
) -> Result<(), String> {
    spawn_pending_host_redeploys(app, state)?;
    let owner_only_access = crate::managed_agents::owner_only_access_build();
    let targets = {
        let _store_guard = state
            .managed_agents_store_lock
            .lock()
            .map_err(|error| error.to_string())?;
        collect_targets_with(load_managed_agents(app)?, owner_only_access, |record| {
            super::build_deploy_payload(app, state, record)
        })
    };

    for target in targets {
        let ProviderAccessTarget {
            pubkey,
            provider_id,
            config,
            cached_binary_path,
            agent_json,
        } = target;
        let agent_json = match agent_json {
            Ok(agent_json) => agent_json,
            Err(error) => {
                persist_failure(app, state, &pubkey, &error)?;
                return Err(format!(
                    "provider access reconciliation failed for agent {pubkey}: {error}"
                ));
            }
        };
        if let Err(error) = super::deploy_to_provider(
            app,
            state,
            &pubkey,
            &provider_id,
            &config,
            agent_json,
            cached_binary_path.as_deref(),
            None,
            None,
            None,
        )
        .await
        {
            return Err(format!(
                "provider access reconciliation failed for agent {pubkey}: {error}"
            ));
        }
    }

    Ok(())
}

/// Retry pending paired-machine access redeploys for the applied community,
/// once per apply, without blocking it.
fn spawn_pending_host_redeploys(app: &AppHandle, state: &AppState) -> Result<(), String> {
    // window-relay: workspace apply runs for the main window's community.
    let community_relay = crate::relay::relay_ws_url_with_override(state);
    let targets = {
        let _store_guard = state
            .managed_agents_store_lock
            .lock()
            .map_err(|error| error.to_string())?;
        super::access_transition::pending_host_redeploys(
            &load_managed_agents(app)?,
            &community_relay,
        )
    };
    if targets.is_empty() {
        return Ok(());
    }
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let state = app.state::<AppState>();
        for (pubkey, target) in targets {
            if let Err(error) =
                super::access_transition::redeploy_for_access_policy(&app, &state, &pubkey, &target)
                    .await
            {
                eprintln!("buzz-desktop: access redeploy to the paired machine for agent {pubkey} failed: {error}");
            }
        }
        let _ = tauri::Emitter::emit(&app, "agents-data-changed", ());
    });
    Ok(())
}

pub(crate) fn persist_failure<R: tauri::Runtime>(
    app: &AppHandle<R>,
    state: &AppState,
    pubkey: &str,
    error: &str,
) -> Result<(), String> {
    let _store_guard = state
        .managed_agents_store_lock
        .lock()
        .map_err(|lock_error| lock_error.to_string())?;
    let mut records = load_managed_agents(app)?;
    let record = find_managed_agent_mut(&mut records, pubkey)?;
    record.last_error = Some(error.to_string());
    record.updated_at = now_iso();
    save_managed_agents(app, &records)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(backend: BackendKind, backend_agent_id: Option<&str>) -> ManagedAgentRecord {
        let mut record: ManagedAgentRecord = serde_json::from_value(serde_json::json!({
            "pubkey": "agent", "name": "Agent", "relay_url": "", "acp_command": "",
            "agent_command": "", "agent_args": [], "mcp_command": "",
            "turn_timeout_seconds": 0, "system_prompt": null, "created_at": "",
            "updated_at": "", "last_started_at": null, "last_stopped_at": null,
            "last_exit_code": null, "last_error": null
        }))
        .unwrap();
        record.backend = backend;
        record.backend_agent_id = backend_agent_id.map(str::to_string);
        record
    }

    #[test]
    fn upgrade_collects_existing_provider_and_builds_projected_payload() {
        let records = vec![
            record(
                BackendKind::Provider {
                    id: "provider".into(),
                    config: serde_json::json!({"region": "test"}),
                },
                Some("existing"),
            ),
            record(
                BackendKind::Provider {
                    id: "not-deployed".into(),
                    config: serde_json::json!({}),
                },
                None,
            ),
            record(BackendKind::Local, Some("stale")),
        ];

        let targets = collect_targets_with(records, true, |_| {
            Ok(serde_json::json!({"respond_to": "owner-only"}))
        });

        assert_eq!(targets.len(), 1);
        assert_eq!(targets[0].pubkey, "agent");
        assert_eq!(targets[0].provider_id, "provider");
        assert_eq!(targets[0].config["region"], "test");
        assert_eq!(
            targets[0].agent_json.as_ref().unwrap()["respond_to"],
            "owner-only"
        );
    }

    #[test]
    fn unmarked_build_collects_only_pending_targets() {
        let mut pending = record(
            BackendKind::Provider {
                id: "pending-provider".into(),
                config: serde_json::json!({}),
            },
            Some("existing-pending"),
        );
        pending.pubkey = "pending-agent".into();
        pending.provider_policy_pending = true;
        let ordinary = record(
            BackendKind::Provider {
                id: "ordinary-provider".into(),
                config: serde_json::json!({}),
            },
            Some("existing-ordinary"),
        );

        let targets = collect_targets_with(vec![ordinary, pending], false, |record| {
            Ok(serde_json::json!({"pubkey": record.pubkey}))
        });

        assert_eq!(targets.len(), 1);
        assert_eq!(targets[0].pubkey, "pending-agent");
        assert_eq!(targets[0].provider_id, "pending-provider");
        assert_eq!(
            targets[0].agent_json.as_ref().unwrap()["pubkey"],
            "pending-agent"
        );
    }

    #[test]
    fn pending_policy_requires_an_existing_provider_deployment() {
        let mut undeployed = record(
            BackendKind::Provider {
                id: "provider".into(),
                config: serde_json::json!({}),
            },
            None,
        );
        undeployed.provider_policy_pending = true;
        let mut local = record(BackendKind::Local, Some("stale-provider-id"));
        local.provider_policy_pending = true;

        assert!(collect_targets_with(vec![undeployed, local], false, |_| {
            Ok(serde_json::Value::Null)
        })
        .is_empty());
    }

    #[test]
    fn deployed_host_agents_are_not_provider_reconciliation_targets() {
        let mut host = record(
            BackendKind::Host {
                host_pubkey: "ab".repeat(32),
            },
            Some(&"ab".repeat(32)),
        );
        host.provider_policy_pending = true;
        assert!(collect_targets_with(vec![host], true, |_| Ok(serde_json::Value::Null)).is_empty());
    }

    #[test]
    fn pending_host_agents_need_reconciliation_only_while_deployed() {
        let host = BackendKind::Host {
            host_pubkey: "ab".repeat(32),
        };
        let mut pending = record(host.clone(), Some(&"ab".repeat(32)));
        pending.provider_policy_pending = true;
        assert!(needs_reconciliation_with_policy(&pending, false));

        let acknowledged = record(host.clone(), Some(&"ab".repeat(32)));
        assert!(!needs_reconciliation_with_policy(&acknowledged, true));

        let mut undeployed = record(host, None);
        undeployed.provider_policy_pending = true;
        assert!(!needs_reconciliation_with_policy(&undeployed, false));
    }
}
