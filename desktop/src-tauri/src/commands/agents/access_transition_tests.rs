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
    let retries = pending_host_redeploys(&[provider_agent, host_agent], WINDOW_RELAY);
    assert_eq!(
        retries,
        vec![(
            "host-agent".to_string(),
            RemoteAccessRedeploy::Host {
                host_pubkey: "ab".repeat(32),
                community_relay: WINDOW_RELAY.into(),
            }
        )]
    );
}

#[test]
fn host_retries_stay_in_the_agents_own_community() {
    let mut elsewhere = record(host(), Some(&"ab".repeat(32)));
    elsewhere.relay_url = "wss://other-community.example".into();
    elsewhere.provider_policy_pending = true;

    assert!(pending_host_redeploys(&[elsewhere], WINDOW_RELAY).is_empty());
}

// ── The production redeploy against a scripted provider ─────────────────────

#[cfg(unix)]
struct ScopedEnv {
    key: &'static str,
    prior: Option<std::ffi::OsString>,
}

#[cfg(unix)]
impl ScopedEnv {
    /// The caller holds the crate-wide process-env lock (`lock_path_mutex`).
    fn set(key: &'static str, value: &std::ffi::OsStr) -> Self {
        let prior = std::env::var_os(key);
        std::env::set_var(key, value);
        Self { key, prior }
    }
}

#[cfg(unix)]
impl Drop for ScopedEnv {
    fn drop(&mut self) {
        match &self.prior {
            Some(value) => std::env::set_var(self.key, value),
            None => std::env::remove_var(self.key),
        }
    }
}

#[cfg(unix)]
struct RedeployOutcome {
    record: ManagedAgentRecord,
    result: Result<(), String>,
    delivered: Option<serde_json::Value>,
}

/// Save a deployed provider agent, apply a desktop access edit to it the way
/// `update_managed_agent` does (plan + new policy in one write), then run the
/// production redeploy against a fake `buzz-backend-*` provider that answers
/// deploys with `deploy_reply`. `None` on owner-only builds, which never
/// change the projected policy.
#[cfg(unix)]
fn edit_access_and_redeploy(deploy_reply: &str) -> Option<RedeployOutcome> {
    use std::os::unix::fs::PermissionsExt;

    use crate::managed_agents::{load_managed_agents, save_managed_agents};

    if crate::managed_agents::owner_only_access_build() {
        return None;
    }
    let _env_guard = crate::managed_agents::lock_path_mutex();
    #[cfg(feature = "system-keyring")]
    keyring::set_default_credential_builder(keyring::mock::default_credential_builder());
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("home");
    let bin = temp.path().join("bin");
    std::fs::create_dir_all(&home).unwrap();
    std::fs::create_dir_all(&bin).unwrap();
    let request_log = temp.path().join("deploy-request.json");
    let provider = bin.join("buzz-backend-accesstest");
    let script = format!(
        r#"#!/bin/sh
set -eu
read request
case "$request" in
  *\"op\":\"info\"*) printf '%s\n' '{{"ok":true,"name":"t","version":"1","protocol_version":1,"description":"t","config_schema":{{}}}}' ;;
  *\"op\":\"deploy\"*) printf '%s' "$request" > '{log}'; printf '%s\n' '{reply}' ;;
esac
"#,
        log = request_log.display(),
        reply = deploy_reply,
    );
    std::fs::write(&provider, script).unwrap();
    std::fs::set_permissions(&provider, std::fs::Permissions::from_mode(0o700)).unwrap();
    // Prepend (never replace) so concurrently running tests keep their tools;
    // the provider id is unique to this test.
    let mut search = vec![bin.clone()];
    search.extend(std::env::split_paths(
        &std::env::var_os("PATH").unwrap_or_default(),
    ));
    let _path = ScopedEnv::set("PATH", &std::env::join_paths(search).unwrap());

    let mut context = tauri::test::mock_context(tauri::test::noop_assets());
    context.config_mut().identifier = home.to_str().unwrap().to_owned();
    let state = crate::app_state::build_app_state();
    *state.relay_url_override.lock().unwrap() = Some("wss://relay.example".into());
    let app = tauri::test::mock_builder()
        .manage(state)
        .build(context)
        .unwrap();
    let state = tauri::Manager::state::<AppState>(&app);

    let keys = nostr::Keys::generate();
    let pubkey = keys.public_key().to_hex();
    let nsec = nostr::ToBech32::to_bech32(keys.secret_key()).unwrap();
    let mut record: ManagedAgentRecord = serde_json::from_value(serde_json::json!({
        "pubkey": pubkey,
        "name": "Remote Agent",
        "relay_url": "wss://relay.example",
        "acp_command": "buzz-acp", "agent_command": "goose", "agent_args": ["acp"],
        "mcp_command": "", "turn_timeout_seconds": 0,
        "created_at": "2026-01-01T00:00:00Z", "updated_at": "2026-01-01T00:00:00Z"
    }))
    .unwrap();
    record.backend = BackendKind::Provider {
        id: "accesstest".into(),
        config: serde_json::json!({}),
    };
    record.backend_agent_id = Some("deployed-before".into());
    assert_eq!(record.respond_to, RespondTo::OwnerOnly);

    // Phase 1 of the desktop edit: plan (marks pending), then the new policy,
    // both in one saved record.
    let target = match plan(&mut record, true) {
        AccessRuntimeTransition::Redeploy(target) => target,
        other => panic!("a deployed provider must be redeployed, planned {other:?}"),
    };
    record.respond_to = RespondTo::Anyone;
    save_managed_agents(app.handle(), &[record]).unwrap();
    // The mock keyring keeps nothing across entries and is process-global, so
    // carry the key inline (the keyringless-fallback shape `load` reads)
    // rather than racing other tests for the shared secret cache.
    let store_path = crate::managed_agents::managed_agents_store_path(app.handle()).unwrap();
    let mut stored: Vec<serde_json::Value> =
        serde_json::from_slice(&std::fs::read(&store_path).unwrap()).unwrap();
    for entry in &mut stored {
        if entry["pubkey"] == pubkey.as_str() {
            entry["private_key_nsec"] = nsec.clone().into();
        }
    }
    std::fs::write(&store_path, serde_json::to_vec(&stored).unwrap()).unwrap();

    let result = tauri::async_runtime::block_on(redeploy_for_access_policy(
        app.handle(),
        &state,
        &pubkey,
        &target,
    ));
    let record = load_managed_agents(app.handle())
        .unwrap()
        .into_iter()
        .find(|r| r.pubkey == pubkey)
        .unwrap();
    let delivered = std::fs::read_to_string(&request_log)
        .ok()
        .map(|raw| serde_json::from_str(&raw).unwrap());
    Some(RedeployOutcome {
        record,
        result,
        delivered,
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
