//! An agent receives the defaults of its OWN community at every production
//! merge site — never the active community's, never another's. Each test runs
//! the real seam (spawn, deploy payload) with community A active while the
//! agent belongs to community B.

use std::collections::BTreeMap;

use tauri::test::MockRuntime;
use tauri::Manager;

use crate::managed_agents::save_agent_defaults_for_relay;
use crate::managed_agents::{GlobalAgentConfig, ManagedAgentRecord};

const COMMUNITY_A: &str = "wss://first.example";
const COMMUNITY_B: &str = "wss://second.example";
const A_GATEWAY: &str = "http://gateway-a.invalid:4001";
const B_GATEWAY: &str = "http://gateway-b.invalid:4002";
const ENV_KEY: &str = "ANTHROPIC_BASE_URL";

struct TestApp {
    app: tauri::App<MockRuntime>,
    _data: tempfile::TempDir,
}

/// A mock app on community A, with distinct defaults saved for A and B.
fn app_on_a() -> TestApp {
    #[cfg(feature = "system-keyring")]
    keyring::set_default_credential_builder(keyring::mock::default_credential_builder());
    let data = tempfile::tempdir().unwrap();
    let mut context = tauri::test::mock_context(tauri::test::noop_assets());
    context.config_mut().identifier = data.path().to_str().unwrap().to_owned();
    let state = crate::app_state::build_app_state();
    *state.relay_url_override.lock().unwrap() = Some(COMMUNITY_A.into());
    let app = tauri::test::mock_builder()
        .manage(state)
        .build(context)
        .unwrap();
    for (relay, gateway) in [(COMMUNITY_A, A_GATEWAY), (COMMUNITY_B, B_GATEWAY)] {
        save_agent_defaults_for_relay(
            app.handle(),
            relay,
            &GlobalAgentConfig {
                env_vars: BTreeMap::from([(ENV_KEY.to_string(), gateway.to_string())]),
                ..Default::default()
            },
        )
        .unwrap();
    }
    TestApp { app, _data: data }
}

fn agent_of_b(acp_command: &str) -> ManagedAgentRecord {
    let keys = nostr::Keys::generate();
    serde_json::from_value(serde_json::json!({
        "pubkey": keys.public_key().to_hex(),
        "name": "bravo-agent",
        "relay_url": COMMUNITY_B,
        "private_key_nsec": nostr::ToBech32::to_bech32(keys.secret_key()).unwrap(),
        "acp_command": acp_command,
        "agent_command": "goose",
        "agent_args": ["acp"],
        "mcp_command": "",
        "turn_timeout_seconds": 0,
        "created_at": "2026-01-01T00:00:00Z",
        "updated_at": "2026-01-01T00:00:00Z"
    }))
    .unwrap()
}

/// Provider and host deploys both send `build_deploy_payload` (provider
/// deploy, redeploy, access-policy redeploy, host deploy and move).
#[test]
fn deploy_payload_carries_the_agents_own_community_defaults() {
    let test = app_on_a();
    let state = test.app.state::<crate::app_state::AppState>();
    let record = agent_of_b("buzz-acp");

    let payload =
        crate::commands::agents::build_deploy_payload(test.app.handle(), &state, &record).unwrap();

    assert_eq!(payload["relay_url"], COMMUNITY_B);
    assert_eq!(payload["env_vars"][ENV_KEY], B_GATEWAY);
    let serialized = payload.to_string();
    assert!(
        !serialized.contains(A_GATEWAY),
        "community A's defaults leaked into a community B deploy"
    );
}

/// Local spawn (start, restore, fan-out, restart all go through
/// `spawn_agent_child`): the child's environment carries B's defaults.
#[cfg(unix)]
#[test]
fn local_spawn_env_carries_the_agents_own_community_defaults() {
    use std::os::unix::fs::PermissionsExt;

    let test = app_on_a();
    let script = test._data.path().join("print-env.sh");
    std::fs::write(&script, "#!/bin/sh\nenv\n").unwrap();
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o700)).unwrap();
    let record = agent_of_b(script.to_str().unwrap());

    let admissions = crate::managed_agents::RelayAdmissions::default();
    let admitted = admissions
        .admit(
            &crate::managed_agents::AdmissionSnapshot::default(),
            COMMUNITY_B,
        )
        .unwrap();
    let mut process = crate::managed_agents::spawn_agent_child(
        test.app.handle(),
        &record,
        COMMUNITY_B,
        &admitted,
        true,
        None,
        None,
    )
    .unwrap();
    process.child.wait().unwrap();

    let log = std::fs::read_to_string(&process.log_path).unwrap();
    assert!(
        log.lines()
            .any(|line| line == format!("{ENV_KEY}={B_GATEWAY}")),
        "spawned agent did not receive its community's defaults"
    );
    assert!(
        !log.contains(A_GATEWAY),
        "community A's defaults leaked into a community B agent"
    );
}
