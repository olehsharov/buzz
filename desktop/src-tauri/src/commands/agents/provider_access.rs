//! Workspace-apply reconciliation for remote managed-agent access: redeploys
//! deployed agents whose saved access policy no successful deployment has
//! acknowledged yet (`provider_policy_pending`), and every provider agent on
//! owner-only builds.

use tauri::{AppHandle, Manager, Runtime};

use crate::{
    app_state::AppState,
    managed_agents::{
        find_managed_agent_mut, load_managed_agents, save_managed_agents, BackendKind,
        ManagedAgentRecord,
    },
    util::now_iso,
};

use super::access_transition::RemoteAccessRedeploy;

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

/// Provider agents an owner-only build redeploys before the community loads.
/// Empty on other builds: their pending provider redeploys are retried in the
/// background (`pending_access_redeploys`).
fn collect_targets_with(
    records: Vec<ManagedAgentRecord>,
    owner_only_access: bool,
    mut build_payload: impl FnMut(&ManagedAgentRecord) -> Result<serde_json::Value, String>,
) -> Vec<ProviderAccessTarget> {
    if !owner_only_access {
        return Vec::new();
    }
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
            // Paired machines are retried in the background.
            BackendKind::Local | BackendKind::Host { .. } => None,
        })
        .collect()
}

/// Redeploy existing remote agents whose access policy requires enforcement.
///
/// Owner-only builds refresh every existing provider deployment before each
/// community UI load and fail the load closed if one rejects the enforced
/// policy. A saved policy that no deployment has acknowledged yet
/// (`provider_policy_pending`, set by an access edit) is retried in the
/// background instead, for provider and paired-machine agents alike: the
/// remote host may be unreachable, and the community must still load so the
/// owner can see the pending agent and press Retry. A failed retry stays on
/// the record (pending flag + `last_error`) for the next apply or Retry.
pub(crate) async fn reconcile_on_workspace_apply<R: Runtime>(
    app: &AppHandle<R>,
    state: &AppState,
) -> Result<(), String> {
    reconcile_with_policy(app, state, crate::managed_agents::owner_only_access_build())
        .await
        .map(drop)
}

/// [`reconcile_on_workspace_apply`] for an explicit build policy. Returns the
/// background retry task, if one was started.
async fn reconcile_with_policy<R: Runtime>(
    app: &AppHandle<R>,
    state: &AppState,
    owner_only_access: bool,
) -> Result<Option<tauri::async_runtime::JoinHandle<()>>, String> {
    // window-relay: workspace apply runs for the main window's community.
    let community_relay = crate::relay::relay_ws_url_with_override(state);
    let (enforced, pending) = {
        let _store_guard = state
            .managed_agents_store_lock
            .lock()
            .map_err(|error| error.to_string())?;
        let records = load_managed_agents(app)?;
        let pending = super::access_transition::pending_access_redeploys(
            &records,
            &community_relay,
            owner_only_access,
        );
        let enforced = collect_targets_with(records, owner_only_access, |record| {
            super::build_deploy_payload(app, state, record)
        });
        (enforced, pending)
    };
    let background = spawn_pending_redeploys(app, pending);

    for target in enforced {
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

    Ok(background)
}

/// Retry pending access redeploys without blocking the community load. Each
/// failure is recorded on its agent by `redeploy_for_access_policy`; the
/// Agents page is told to refresh when the retries settle.
fn spawn_pending_redeploys<R: Runtime>(
    app: &AppHandle<R>,
    targets: Vec<(String, RemoteAccessRedeploy)>,
) -> Option<tauri::async_runtime::JoinHandle<()>> {
    if targets.is_empty() {
        return None;
    }
    let app = app.clone();
    Some(tauri::async_runtime::spawn(async move {
        let state = app.state::<AppState>();
        for (pubkey, target) in targets {
            if let Err(error) =
                super::access_transition::redeploy_for_access_policy(&app, &state, &pubkey, &target)
                    .await
            {
                eprintln!(
                    "buzz-desktop: pending access redeploy for agent {pubkey} failed: {error}"
                );
            }
        }
        let _ = tauri::Emitter::emit(&app, "agents-data-changed", ());
    }))
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
    fn pending_provider_redeploys_run_in_the_background_on_unmarked_builds() {
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
        let records = vec![ordinary, pending];

        // Nothing blocks the community load...
        assert!(
            collect_targets_with(records.clone(), false, |_| { Ok(serde_json::Value::Null) })
                .is_empty()
        );
        // ...the pending agent is retried in the background instead.
        assert_eq!(
            super::super::access_transition::pending_access_redeploys(
                &records,
                "wss://relay.example",
                false
            ),
            vec![("pending-agent".to_string(), RemoteAccessRedeploy::Provider)]
        );
        // Owner-only builds redeploy every provider inline instead.
        assert!(super::super::access_transition::pending_access_redeploys(
            &records,
            "wss://relay.example",
            true
        )
        .is_empty());
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
        let records = vec![undeployed, local];

        assert!(super::super::access_transition::pending_access_redeploys(
            &records,
            "wss://relay.example",
            false
        )
        .is_empty());
        assert!(collect_targets_with(records, true, |_| Ok(serde_json::Value::Null)).is_empty());
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

    // ── Workspace apply against a scripted provider ──────────────────────────

    #[cfg(unix)]
    fn apply_with(
        fixture: &crate::commands::agents::scripted_provider_fixture::ScriptedProvider,
        owner_only_access: bool,
    ) -> Result<(), String> {
        let state = fixture.state();
        tauri::async_runtime::block_on(async {
            let background =
                reconcile_with_policy(fixture.app.handle(), &state, owner_only_access).await?;
            if let Some(background) = background {
                background.await.expect("background retries complete");
            }
            Ok(())
        })
    }

    #[cfg(unix)]
    #[test]
    fn failed_pending_provider_redeploy_does_not_block_the_community_load() {
        let fixture = crate::commands::agents::scripted_provider_fixture::ScriptedProvider::new(
            r#"{"ok":false,"error":"host unreachable"}"#,
        );
        let (mut agent, nsec) = fixture.deployed_agent();
        agent.respond_to = crate::managed_agents::RespondTo::Anyone;
        agent.provider_policy_pending = true;
        let pubkey = agent.pubkey.clone();
        fixture.save(&[(agent, nsec)]);

        apply_with(&fixture, false).expect("the community loads with a pending redeploy");

        assert!(fixture.delivered().is_some(), "the redeploy was retried");
        let agent = fixture.load(&pubkey);
        assert!(agent.provider_policy_pending, "kept for the next retry");
        assert!(agent
            .last_error
            .as_deref()
            .is_some_and(|error| error.contains("host unreachable")));
    }

    #[cfg(unix)]
    #[test]
    fn successful_pending_provider_redeploy_acknowledges_the_policy() {
        let fixture = crate::commands::agents::scripted_provider_fixture::ScriptedProvider::new(
            r#"{"ok":true,"agent_id":"deployed-after"}"#,
        );
        let (mut agent, nsec) = fixture.deployed_agent();
        agent.provider_policy_pending = true;
        let pubkey = agent.pubkey.clone();
        fixture.save(&[(agent, nsec)]);

        apply_with(&fixture, false).unwrap();

        let agent = fixture.load(&pubkey);
        assert!(!agent.provider_policy_pending);
        assert_eq!(agent.backend_agent_id.as_deref(), Some("deployed-after"));
        assert_eq!(agent.last_error, None);
    }

    #[cfg(unix)]
    #[test]
    fn owner_only_enforcement_failure_still_fails_the_load_closed() {
        let fixture = crate::commands::agents::scripted_provider_fixture::ScriptedProvider::new(
            r#"{"ok":false,"error":"host unreachable"}"#,
        );
        let (agent, nsec) = fixture.deployed_agent();
        let pubkey = agent.pubkey.clone();
        fixture.save(&[(agent, nsec)]);

        let error = apply_with(&fixture, true).expect_err("owner-only enforcement fails closed");

        assert!(
            error.contains("provider access reconciliation failed"),
            "{error}"
        );
        assert!(fixture
            .load(&pubkey)
            .last_error
            .as_deref()
            .is_some_and(|error| error.contains("host unreachable")));
    }
}
