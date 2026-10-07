//! Live agent-host lifecycle: the desktop's production pairing task and host
//! `ops` (over the real [`RelayHostChannel`]) against a real `buzz host`
//! process on a live relay.
//!
//! Ignored by default. Run with a local relay:
//!
//! ```text
//! BUZZ_HOST_E2E_RELAY=ws://localhost:3030 \
//! BUZZ_HOST_E2E_BIN=/path/to/target/debug/buzz \
//! cargo test --manifest-path desktop/src-tauri/Cargo.toml \
//!   pairing_host_live -- --ignored --nocapture
//! ```
//!
//! Optional: `BUZZ_HOST_E2E_ACP=<path to buzz-acp>` deploys the real harness
//! (the agent must then come online on the relay); otherwise a stub
//! `buzz-acp` that sleeps is used.

use std::path::Path;
use std::time::Duration;

use buzz_ws_client_pkg::{NostrWsConnection, RelayMessage};
use nostr::Keys;
use tauri::test::MockRuntime;
use tauri::Listener;

use super::*;
use crate::agent_hosts::channel::RelayHostChannel;
use crate::agent_hosts::ops::{self, deployed_host, HostOps};
use crate::managed_agents::{load_managed_agents, save_managed_agents, ManagedAgentRecord};

const STEP: Duration = Duration::from_secs(60);

fn env(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|v| !v.is_empty())
}

fn write_stub_acp(dir: &Path) {
    use std::os::unix::fs::PermissionsExt;
    let path = dir.join("buzz-acp");
    std::fs::write(&path, "#!/bin/sh\nexec sleep 600\n").unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
}

/// Forward the pairing events the renderer listens to into a channel.
fn pairing_events(
    app: &tauri::App<MockRuntime>,
) -> tokio::sync::mpsc::UnboundedReceiver<(String, String)> {
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    for name in [
        "pairing-sas-received",
        "host-pairing-hello",
        "pairing-complete",
        "pairing-error",
        "pairing-aborted",
    ] {
        let tx = tx.clone();
        app.listen(name, move |event| {
            let _ = tx.send((name.to_string(), event.payload().to_string()));
        });
    }
    rx
}

async fn next_pairing_event(
    rx: &mut tokio::sync::mpsc::UnboundedReceiver<(String, String)>,
    want: &str,
) -> serde_json::Value {
    loop {
        let (name, payload) = tokio::time::timeout(STEP, rx.recv())
            .await
            .unwrap_or_else(|_| panic!("timed out waiting for {want}"))
            .expect("event channel");
        assert!(
            name != "pairing-error" && name != "pairing-aborted",
            "{name}: {payload}"
        );
        if name == want {
            return serde_json::from_str(&payload).unwrap();
        }
    }
}

fn agent_record(keys: &Keys, relay: &str) -> ManagedAgentRecord {
    let nsec = nostr::ToBech32::to_bech32(keys.secret_key()).unwrap();
    serde_json::from_value(serde_json::json!({
        "pubkey": keys.public_key().to_hex(), "name": "Live Host Agent",
        "relay_url": relay, "acp_command": "buzz-acp", "agent_command": "goose",
        "agent_args": ["acp"], "mcp_command": "", "turn_timeout_seconds": 0,
        "system_prompt": null, "private_key_nsec": nsec,
        "created_at": "2026-01-01T00:00:00Z", "updated_at": "2026-01-01T00:00:00Z",
        "last_started_at": null, "last_stopped_at": null, "last_exit_code": null,
        "last_error": null
    }))
    .unwrap()
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a live relay: set BUZZ_HOST_E2E_RELAY and BUZZ_HOST_E2E_BIN"]
async fn pairing_host_live_approve_deploy_undeploy_forget() {
    let (Some(relay), Some(bin)) = (env("BUZZ_HOST_E2E_RELAY"), env("BUZZ_HOST_E2E_BIN")) else {
        panic!("set BUZZ_HOST_E2E_RELAY and BUZZ_HOST_E2E_BIN");
    };
    let bin = std::fs::canonicalize(bin).unwrap();
    #[cfg(feature = "system-keyring")]
    keyring::set_default_credential_builder(keyring::mock::default_credential_builder());

    // Desktop: a mock-runtime app with the production managed state.
    let data = tempfile::tempdir().unwrap();
    let mut context = tauri::test::mock_context(tauri::test::noop_assets());
    context.config_mut().identifier = data.path().to_str().unwrap().to_owned();
    let state = crate::app_state::build_app_state();
    *state.relay_url_override.lock().unwrap() = Some(relay.clone());
    let owner = state.signing_keys().unwrap();
    let app = tauri::test::mock_builder()
        .manage(state)
        .manage(PairingHandle::new())
        .manage(HostOps::default())
        .build(context)
        .unwrap();
    let handle = app.handle().clone();
    let mut events = pairing_events(&app);

    // Machine side.
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("host");
    let tools = dir.path().join("tools");
    std::fs::create_dir_all(&tools).unwrap();
    let real_acp = env("BUZZ_HOST_E2E_ACP");
    match &real_acp {
        Some(acp) => std::os::unix::fs::symlink(acp, tools.join("buzz-acp")).unwrap(),
        None => write_stub_acp(&tools),
    }
    let path = format!("{}:/usr/bin:/bin", tools.display());

    // 1. "Add machine": the production start_host_pairing path.
    let uri = start_pairing_session(
        handle.clone(),
        app.state(),
        app.state(),
        PairingMode::ApproveHost,
        crate::relay::relay_ws_url_with_override(&app.state::<AppState>()),
    )
    .await
    .expect("start host pairing");
    println!("desktop: pairing uri issued");
    let mut pair = tokio::process::Command::new(&bin)
        .args(["host", "pair", &uri, "--yes", "--name", "live-box"])
        .env("BUZZ_HOST_HOME", &home)
        .env("PATH", &path)
        .kill_on_drop(true)
        .spawn()
        .expect("spawn buzz host pair");

    // 2. SAS shown -> user clicks Approve.
    let sas = next_pairing_event(&mut events, "pairing-sas-received").await;
    println!("desktop: SAS {sas} -> Approve");
    confirm_pairing_sas(app.state()).await.expect("approve");
    let hello = next_pairing_event(&mut events, "host-pairing-hello").await;
    println!("desktop: hello {hello}");
    assert_eq!(hello["name"], "live-box");
    let host_hex = hello["host_pubkey"].as_str().unwrap().to_string();
    let complete = next_pairing_event(&mut events, "pairing-complete").await;
    assert_eq!(complete["host_pubkey"], host_hex);
    let pair_status = tokio::time::timeout(STEP, pair.wait())
        .await
        .unwrap()
        .unwrap();
    assert!(pair_status.success(), "buzz host pair failed");

    let hosts = ops::list_hosts(&handle, &app.state::<HostOps>(), &relay).unwrap();
    assert_eq!(hosts.len(), 1, "{hosts:?}");
    assert_eq!(hosts[0].pubkey, host_hex);
    let owner_json: serde_json::Value =
        serde_json::from_slice(&std::fs::read(home.join("owner.json")).unwrap()).unwrap();
    assert_eq!(owner_json["owner_pubkey"], owner.public_key().to_hex());
    println!("desktop: machine approved and stored");

    // 3. The machine runs its daemon.
    let mut daemon = tokio::process::Command::new(&bin)
        .args(["host", "run"])
        .env("BUZZ_HOST_HOME", &home)
        .env("PATH", &path)
        .env("RUST_LOG", "info")
        .kill_on_drop(true)
        .spawn()
        .expect("spawn daemon");

    let channel = RelayHostChannel {
        relay_url: relay.clone(),
        owner_keys: owner.clone(),
    };
    let host_ops = app.state::<HostOps>();
    let app_state = app.state::<AppState>();

    // 4. Status refresh (the machines list).
    let refreshed = {
        let mut last = Err(String::new());
        for _ in 0..10 {
            last = ops::refresh_host_status(
                &handle, &app_state, &host_ops, &channel, &relay, &host_hex,
            )
            .await;
            if last.is_ok() {
                break;
            }
            tokio::time::sleep(Duration::from_secs(2)).await;
        }
        last.expect("host status")
    };
    let status = refreshed.status.as_ref().expect("status stored");
    println!("desktop: status {}", serde_json::to_string(status).unwrap());

    // 5. Deploy a managed agent with the production payload builder.
    let agent = Keys::generate();
    let agent_hex = agent.public_key().to_hex();
    save_managed_agents(&handle, &[agent_record(&agent, &relay)]).unwrap();
    ops::deploy_agent_to_host(
        &handle,
        &app_state,
        &host_ops,
        &channel,
        &agent_hex,
        &host_hex,
        &relay,
        |record| crate::commands::agents::build_deploy_payload(&handle, &app_state, record),
    )
    .await
    .expect("deploy to host");
    let record = load_managed_agents(&handle)
        .unwrap()
        .into_iter()
        .find(|r| r.pubkey == agent_hex)
        .unwrap();
    assert_eq!(deployed_host(&record), Some(host_hex.as_str()));
    let agent_file = home.join("agents").join(format!("{agent_hex}.json"));
    assert!(agent_file.exists(), "agent not stored on the machine");
    let stored: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&agent_file).unwrap()).unwrap();
    let env = stored["env"].as_object().expect("agent env");
    assert_eq!(env["BUZZ_RELAY_URL"], relay.as_str());
    let secret = |key: &str| {
        ["KEY", "NSEC", "TAG", "TOKEN", "SECRET"]
            .iter()
            .any(|s| key.contains(s))
    };
    let shown: Vec<String> = env
        .iter()
        .map(|(k, v)| {
            if secret(k) {
                format!("{k}=<redacted>")
            } else {
                format!("{k}={v}")
            }
        })
        .collect();
    println!("desktop: deployed; host agent env = {shown:?}");

    let refreshed =
        ops::refresh_host_status(&handle, &app_state, &host_ops, &channel, &relay, &host_hex)
            .await
            .expect("status after deploy");
    let agents = &refreshed.status.as_ref().unwrap().agents;
    assert_eq!(agents.len(), 1, "{agents:?}");
    println!("desktop: agent state on host = {:?}", agents[0].state);

    if real_acp.is_some() {
        let mut watch = NostrWsConnection::connect_authenticated(&relay, &owner, None)
            .await
            .unwrap();
        watch
            .send_raw(&serde_json::json!(["REQ", "p", {"kinds": [20001], "authors": [agent_hex]}]))
            .await
            .unwrap();
        let deadline = tokio::time::Instant::now() + STEP;
        loop {
            let left = deadline.saturating_duration_since(tokio::time::Instant::now());
            assert!(!left.is_zero(), "deployed agent never came online");
            if let Ok(RelayMessage::Event { event, .. }) = watch.next_event(left).await {
                println!("desktop: agent presence {}", event.content);
                break;
            }
        }
    }

    // 6. Undeploy.
    ops::undeploy_agent_from_host(&handle, &app_state, &host_ops, &channel, &agent_hex)
        .await
        .expect("undeploy");
    assert!(!agent_file.exists(), "agent survived undeploy");
    let record = load_managed_agents(&handle)
        .unwrap()
        .into_iter()
        .find(|r| r.pubkey == agent_hex)
        .unwrap();
    assert_eq!(deployed_host(&record), None);
    println!("desktop: undeployed");

    // 7. Forget the machine: it acks, exits 0 and wipes its identity.
    let outcome = ops::forget_host(&handle, &app_state, &host_ops, &channel, &relay, &host_hex)
        .await
        .expect("forget");
    println!(
        "desktop: forget outcome {}",
        serde_json::to_string(&outcome).unwrap()
    );
    let exit = tokio::time::timeout(STEP, daemon.wait())
        .await
        .expect("daemon exits after forget")
        .unwrap();
    assert!(exit.success(), "daemon exit {exit:?}");
    for f in ["host.key", "owner.json"] {
        assert!(!home.join(f).exists(), "{f} survived forget");
    }
    assert!(ops::list_hosts(&handle, &host_ops, &relay)
        .unwrap()
        .is_empty());
    println!("desktop: machine forgotten");
}
