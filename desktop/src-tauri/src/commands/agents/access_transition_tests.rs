use std::collections::HashMap;

use super::*;
use crate::managed_agents::{ManagedAgentRuntimeKey, RespondTo};

const WINDOW_RELAY: &str = "wss://window.example";

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

fn provider() -> BackendKind {
    BackendKind::Provider {
        id: "provider".into(),
        config: serde_json::json!({}),
    }
}

fn host() -> BackendKind {
    BackendKind::Host {
        host_pubkey: "ab".repeat(32),
        workdir: None,
    }
}

fn plan(record: &mut ManagedAgentRecord, changed: bool) -> AccessRuntimeTransition {
    plan_access_runtime_transition(
        record,
        changed,
        &HashMap::<ManagedAgentRuntimeKey, ()>::new(),
        WINDOW_RELAY,
    )
}

#[test]
fn deployed_provider_access_edit_is_marked_pending_and_redeployed() {
    let mut record = record(provider(), Some("deployment"));

    assert_eq!(
        plan(&mut record, true),
        AccessRuntimeTransition::Redeploy(RemoteAccessRedeploy::Provider)
    );
    assert!(
        record.provider_policy_pending,
        "the pending flag is saved in the same write as the policy"
    );
}

#[test]
fn deployed_host_access_edit_is_marked_pending_and_redeployed_in_its_community() {
    let mut record = record(host(), Some(&"ab".repeat(32)));
    record.relay_url = "wss://agent-community.example".into();

    assert_eq!(
        plan(&mut record, true),
        AccessRuntimeTransition::Redeploy(RemoteAccessRedeploy::Host {
            host_pubkey: "ab".repeat(32),
            community_relay: "wss://agent-community.example".into(),
        })
    );
    assert!(record.provider_policy_pending);
}

#[test]
fn undeployed_remote_and_unchanged_policies_plan_nothing() {
    for backend in [provider(), host()] {
        let mut undeployed = record(backend.clone(), None);
        assert_eq!(plan(&mut undeployed, true), AccessRuntimeTransition::None);
        assert!(!undeployed.provider_policy_pending);

        let mut unchanged = record(backend, Some("deployment"));
        assert_eq!(plan(&mut unchanged, false), AccessRuntimeTransition::None);
        assert!(!unchanged.provider_policy_pending);
    }
}

#[test]
fn local_access_edit_restarts_its_running_pairs() {
    let mut local = record(BackendKind::Local, Some("stale"));
    let mut runtimes = HashMap::new();
    runtimes.insert(
        ManagedAgentRuntimeKey {
            pubkey: "agent".into(),
            relay_url: "wss://pair.example".into(),
        },
        (),
    );

    assert_eq!(
        plan_access_runtime_transition(&mut local, true, &runtimes, WINDOW_RELAY),
        AccessRuntimeTransition::RestartLocal {
            relay_urls: vec!["wss://pair.example".into()]
        }
    );
    assert!(!local.provider_policy_pending);
}

#[test]
fn legacy_local_runtime_restarts_in_the_window_community() {
    let mut local = record(BackendKind::Local, None);
    local.runtime_pid = Some(42);

    assert_eq!(
        plan(&mut local, true),
        AccessRuntimeTransition::RestartLocal {
            relay_urls: vec![WINDOW_RELAY.into()]
        }
    );

    let mut stopped = record(BackendKind::Local, None);
    assert_eq!(plan(&mut stopped, true), AccessRuntimeTransition::None);
}

#[test]
fn a_locally_marked_pending_record_is_retried_by_workspace_apply() {
    let mut provider_agent = record(provider(), Some("deployment"));
    plan(&mut provider_agent, true);
    assert!(
        super::super::provider_access::needs_reconciliation_with_policy(&provider_agent, false)
    );

    let mut host_agent = record(host(), Some(&"ab".repeat(32)));
    host_agent.pubkey = "host-agent".into();
    plan(&mut host_agent, true);
    let records = [provider_agent, host_agent];
    let host_retry = (
        "host-agent".to_string(),
        RemoteAccessRedeploy::Host {
            host_pubkey: "ab".repeat(32),
            community_relay: WINDOW_RELAY.into(),
        },
    );
    assert_eq!(
        pending_access_redeploys(&records, WINDOW_RELAY, false),
        vec![
            ("agent".to_string(), RemoteAccessRedeploy::Provider),
            host_retry.clone(),
        ]
    );
    // Owner-only builds redeploy providers inline (fail closed) instead.
    assert_eq!(
        pending_access_redeploys(&records, WINDOW_RELAY, true),
        vec![host_retry]
    );
}

#[test]
fn host_retries_stay_in_the_agents_own_community() {
    let mut elsewhere = record(host(), Some(&"ab".repeat(32)));
    elsewhere.relay_url = "wss://other-community.example".into();
    elsewhere.provider_policy_pending = true;

    assert!(pending_access_redeploys(&[elsewhere], WINDOW_RELAY, false).is_empty());
}

// ── The production redeploy against a scripted provider ─────────────────────

#[cfg(unix)]
struct RedeployOutcome {
    record: ManagedAgentRecord,
    result: Result<(), String>,
    delivered: Option<serde_json::Value>,
}

/// Save a deployed provider agent, apply a desktop access edit to it through
/// the production access step of `update_managed_agent`
/// (`apply_access_edit`: plan + new policy, saved in one write), then run the
/// production redeploy against a scripted provider that answers deploys with
/// `deploy_reply`. `None` on owner-only builds, which never change the
/// projected policy.
#[cfg(unix)]
fn edit_access_and_redeploy(deploy_reply: &str) -> Option<RedeployOutcome> {
    if crate::managed_agents::owner_only_access_build() {
        return None;
    }
    let fixture = super::super::scripted_provider_fixture::ScriptedProvider::new(deploy_reply);
    let (mut record, nsec) = fixture.deployed_agent();
    let pubkey = record.pubkey.clone();
    assert_eq!(record.respond_to, RespondTo::OwnerOnly);
    fixture.save(&[(record.clone(), nsec.clone())]);

    let (changed, transition) = crate::commands::apply_access_edit(
        fixture.app.handle(),
        &mut record,
        &mut HashMap::new(),
        Some(RespondTo::Anyone),
        None,
        WINDOW_RELAY,
    )
    .expect("every backend accepts an access edit");
    assert!(changed);
    let target = match transition {
        AccessRuntimeTransition::Redeploy(target) => target,
        other => panic!("a deployed provider must be redeployed, planned {other:?}"),
    };
    assert!(
        record.provider_policy_pending,
        "marked pending in the same write"
    );
    fixture.save(&[(record, nsec)]);

    let state = fixture.state();
    let result = tauri::async_runtime::block_on(redeploy_for_access_policy(
        fixture.app.handle(),
        &state,
        &pubkey,
        &target,
    ));
    Some(RedeployOutcome {
        record: fixture.load(&pubkey),
        result,
        delivered: fixture.delivered(),
    })
}

#[cfg(unix)]
#[test]
fn provider_access_edit_redeploys_the_new_policy_and_acknowledges_it() {
    let Some(outcome) = edit_access_and_redeploy(r#"{"ok":true,"agent_id":"deployed-after"}"#)
    else {
        return;
    };

    outcome.result.expect("redeploy succeeds");
    let delivered = outcome.delivered.expect("the provider received a deploy");
    assert_eq!(delivered["agent"]["respond_to"], "anyone");
    assert_eq!(outcome.record.respond_to, RespondTo::Anyone);
    assert!(
        !outcome.record.provider_policy_pending,
        "acknowledged by the deploy"
    );
    assert_eq!(
        outcome.record.backend_agent_id.as_deref(),
        Some("deployed-after")
    );
    assert_eq!(outcome.record.last_error, None);
}

#[cfg(unix)]
#[test]
fn failed_provider_access_redeploy_stays_pending_for_retry() {
    let Some(outcome) = edit_access_and_redeploy(r#"{"ok":false,"error":"host unreachable"}"#)
    else {
        return;
    };

    let error = outcome.result.expect_err("redeploy fails");
    assert!(error.contains("host unreachable"), "{error}");
    assert_eq!(outcome.record.respond_to, RespondTo::Anyone);
    assert!(
        outcome.record.provider_policy_pending,
        "retried on workspace apply"
    );
    assert!(outcome
        .record
        .last_error
        .as_deref()
        .is_some_and(|error| error.contains("host unreachable")));
    assert_eq!(
        outcome.record.backend_agent_id.as_deref(),
        Some("deployed-before")
    );
}
