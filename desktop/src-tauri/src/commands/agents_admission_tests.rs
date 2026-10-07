//! The ordinary local start against a removed relay, through the production
//! `start_local_agent_with_preflight` on a mock app, and across its preflight
//! await.

use super::{start_local_agent_after_preflight, start_local_agent_with_preflight};
use crate::app_state::AppState;
use crate::managed_agents::admission_test_support::{
    app_with_keyless_agent, reached_spawn, refused, RELAY,
};
use crate::managed_agents::{readd_relay, remove_relay};
use tauri::Manager;

#[tokio::test]
async fn ordinary_start_refuses_a_removed_relay_until_readded() {
    let test = app_with_keyless_agent();
    let (app, state) = (test.app.handle(), test.app.state::<AppState>());
    let start = || {
        start_local_agent_with_preflight(app, &state, RELAY, &test.pubkey, false, None, None, None)
    };

    remove_relay(&state, RELAY).unwrap();
    let error = start().await.unwrap_err();
    assert!(refused(&error), "{error}");

    readd_relay(&state, RELAY).unwrap();
    let error = start().await.unwrap_err();
    assert!(reached_spawn(&error), "{error}");
}

/// Runs the ordinary start with its preflight held open while `during` runs.
async fn start_across_preflight(during: impl FnOnce(&AppState)) -> String {
    let test = app_with_keyless_agent();
    let (app, state) = (test.app.handle(), test.app.state::<AppState>());
    let (entered_tx, entered_rx) = tokio::sync::oneshot::channel();
    let (release_tx, release_rx) = tokio::sync::oneshot::channel::<()>();
    let start = start_local_agent_after_preflight(
        app,
        &state,
        RELAY,
        &test.pubkey,
        None,
        None,
        None,
        |_| async move {
            entered_tx.send(()).unwrap();
            release_rx.await.unwrap();
            Ok(())
        },
    );
    let control = async {
        entered_rx.await.unwrap();
        during(&state);
        release_tx.send(()).unwrap();
    };
    let (result, ()) = tokio::join!(start, control);
    result.unwrap_err()
}

#[tokio::test]
async fn ordinary_start_removed_and_readded_during_preflight_is_refused() {
    let error = start_across_preflight(|state| {
        remove_relay(state, RELAY).unwrap();
        readd_relay(state, RELAY).unwrap();
    })
    .await;
    assert!(refused(&error), "{error}");
}

#[tokio::test]
async fn ordinary_start_with_no_removal_during_preflight_reaches_spawn() {
    let error = start_across_preflight(|_| {}).await;
    assert!(reached_spawn(&error), "{error}");
}

/// A start from community window B runs the pair on B's relay even while the
/// main window is on A; the same start resolved against A is refused because
/// the agent belongs to B. Pins the relay `start_managed_agent` threads in.
#[tokio::test]
async fn start_targets_the_invoking_windows_community() {
    let test = app_with_keyless_agent();
    let (app, state) = (test.app.handle(), test.app.state::<AppState>());
    // The main window is on another community.
    *state.relay_url_override.lock().unwrap() = Some("wss://main-a.example".into());

    let from_main = start_local_agent_with_preflight(
        app,
        &state,
        "wss://main-a.example",
        &test.pubkey,
        false,
        None,
        None,
        None,
    )
    .await
    .unwrap_err();
    assert!(
        !reached_spawn(&from_main),
        "an agent of B must not start on A: {from_main}"
    );

    let from_window_b =
        start_local_agent_with_preflight(app, &state, RELAY, &test.pubkey, false, None, None, None)
            .await
            .unwrap_err();
    assert!(reached_spawn(&from_window_b), "{from_window_b}");
}

/// A create in community window B assigns the new agent to B, not to the
/// main window's community, and a stale captured community fails closed.
#[test]
fn create_in_a_community_window_pins_the_agent_to_that_community() {
    use crate::window_relay::{bind_window_relay, WindowRelay};
    let state = crate::app_state::build_app_state();
    *state.relay_url_override.lock().unwrap() = Some("wss://main-a.example".into());
    *state.agent_avatar_communities.lock().unwrap() = vec!["https://b.example".into()];
    let label = "community-b";
    bind_window_relay(&state, label, "wss://b.example").unwrap();

    let window_b = WindowRelay::resolve(&state, label).unwrap();
    assert_eq!(
        super::creation_community(None, &window_b).unwrap(),
        "wss://b.example"
    );
    assert_eq!(
        super::creation_community(Some("wss://b.example"), &window_b).unwrap(),
        "wss://b.example"
    );
    assert!(super::creation_community(Some("wss://main-a.example"), &window_b).is_err());

    let main = WindowRelay::resolve(&state, "main").unwrap();
    assert_eq!(
        super::creation_community(None, &main).unwrap(),
        "wss://main-a.example"
    );
}
