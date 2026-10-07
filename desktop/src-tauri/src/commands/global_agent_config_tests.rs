//! The agent-defaults settings commands act on the INVOKING WINDOW's
//! community: the main window edits the active community's defaults, a
//! community window its own. Reads go through the real IPC seam (WindowRelay
//! resolution included); writes through the command's own phase-1 body with
//! the relay WindowRelay resolves for each window.

use std::collections::BTreeMap;

use serde_json::{json, Value};
use tauri::ipc::{CallbackFn, InvokeBody};
use tauri::test::{mock_builder, mock_context, noop_assets, MockRuntime, INVOKE_KEY};
use tauri::webview::InvokeRequest;
use tauri::{Manager, WebviewWindow, WebviewWindowBuilder};

use super::{get_global_agent_config, save_community_defaults};
use crate::app_state::{build_app_state, AppState};
use crate::managed_agents::GlobalAgentConfig;
use crate::window_relay::WindowRelay;

const MAIN_RELAY: &str = "wss://first.example";
const COMMUNITY_RELAY: &str = "wss://second.example";
const COMMUNITY_LABEL: &str = "community-0b6c3a9e-2f5f-4f9e-9a43-0b8b2a6f3c11";

struct Harness {
    app: tauri::App<MockRuntime>,
    _data: tempfile::TempDir,
    main: WebviewWindow<MockRuntime>,
    community: WebviewWindow<MockRuntime>,
}

fn harness() -> Harness {
    #[cfg(feature = "system-keyring")]
    keyring::set_default_credential_builder(keyring::mock::default_credential_builder());
    let state = build_app_state();
    *state.relay_url_override.lock().unwrap() = Some(MAIN_RELAY.into());
    state
        .window_relays
        .lock()
        .unwrap()
        .insert(COMMUNITY_LABEL.into(), COMMUNITY_RELAY.into());
    let data = tempfile::tempdir().unwrap();
    let mut context = mock_context(noop_assets());
    context.config_mut().identifier = data.path().to_str().unwrap().to_owned();
    let app = mock_builder()
        .manage(state)
        .invoke_handler(tauri::generate_handler![get_global_agent_config])
        .build(context)
        .unwrap();
    let main = WebviewWindowBuilder::new(&app, "main", Default::default())
        .build()
        .unwrap();
    let community = WebviewWindowBuilder::new(&app, COMMUNITY_LABEL, Default::default())
        .build()
        .unwrap();
    Harness {
        app,
        _data: data,
        main,
        community,
    }
}

fn get(window: &WebviewWindow<MockRuntime>) -> GlobalAgentConfig {
    let body = tauri::test::get_ipc_response(
        window,
        InvokeRequest {
            cmd: "get_global_agent_config".into(),
            callback: CallbackFn(0),
            error: CallbackFn(1),
            url: "tauri://localhost".parse().unwrap(),
            body: InvokeBody::Json(json!({})),
            headers: Default::default(),
            invoke_key: INVOKE_KEY.to_string(),
        },
    )
    .unwrap();
    serde_json::from_value(body.deserialize::<Value>().unwrap()).unwrap()
}

/// The relay `set_global_agent_config` receives from `window`.
fn window_relay(harness: &Harness, window: &WebviewWindow<MockRuntime>) -> String {
    WindowRelay::resolve(&harness.app.state::<AppState>(), window.label())
        .unwrap()
        .ws_url()
        .to_string()
}

fn defaults(value: &str) -> GlobalAgentConfig {
    GlobalAgentConfig {
        env_vars: BTreeMap::from([("ANTHROPIC_BASE_URL".to_string(), value.to_string())]),
        ..Default::default()
    }
}

#[test]
fn each_window_reads_and_writes_its_own_communitys_defaults() {
    let harness = harness();
    let handle = harness.app.handle();

    let main_relay = window_relay(&harness, &harness.main);
    let community_relay = window_relay(&harness, &harness.community);
    assert_eq!(main_relay, MAIN_RELAY);
    assert_eq!(community_relay, COMMUNITY_RELAY);

    save_community_defaults(handle, &main_relay, &defaults("http://main")).unwrap();
    assert_eq!(get(&harness.main), defaults("http://main"));
    assert_eq!(
        get(&harness.community),
        GlobalAgentConfig::default(),
        "the main window's save leaked into the community window's community"
    );

    let saved =
        save_community_defaults(handle, &community_relay, &defaults("http://community")).unwrap();
    assert_eq!(saved.old_global, GlobalAgentConfig::default());
    assert_eq!(saved.new_global, defaults("http://community"));
    assert_eq!(get(&harness.community), defaults("http://community"));
    assert_eq!(get(&harness.main), defaults("http://main"));
}

#[test]
fn switching_the_active_community_switches_the_defaults_the_main_window_reads() {
    let harness = harness();
    let handle = harness.app.handle();
    save_community_defaults(handle, MAIN_RELAY, &defaults("http://first")).unwrap();
    assert_eq!(get(&harness.main), defaults("http://first"));

    *harness
        .app
        .state::<AppState>()
        .relay_url_override
        .lock()
        .unwrap() = Some("wss://third.example".into());
    assert_eq!(get(&harness.main), GlobalAgentConfig::default());
}

#[test]
fn saving_rejects_invalid_defaults_without_writing() {
    let harness = harness();
    let mut bad = defaults("http://x");
    bad.env_vars
        .insert("BUZZ_PRIVATE_KEY".into(), "nope".into());
    assert!(save_community_defaults(harness.app.handle(), MAIN_RELAY, &bad).is_err());
    assert_eq!(get(&harness.main), GlobalAgentConfig::default());
}
