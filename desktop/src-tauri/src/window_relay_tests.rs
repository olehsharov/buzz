use super::*;
use crate::app_state::build_app_state;

const LABEL: &str = "community-0b6c3a9e-2f5f-4f9e-9a43-0b8b2a6f3c11";
const RELAY_A: &str = "wss://relay-a.example";
const RELAY_B: &str = "wss://relay-b.example";

fn state_with_saved(saved: &[&str]) -> AppState {
    let state = build_app_state();
    *state.relay_url_override.lock().unwrap() = Some(RELAY_A.into());
    *state.agent_avatar_communities.lock().unwrap() =
        saved.iter().map(|url| relay_origin(url).unwrap()).collect();
    state
}

#[test]
fn community_labels_require_a_safe_id() {
    assert!(is_community_window_label(LABEL));
    assert!(is_community_window_label("community-e2e-default-community"));
    assert!(!is_community_window_label("community-"));
    assert!(!is_community_window_label("community-a/b"));
    assert!(!is_community_window_label("popout-x"));
    assert!(!is_community_window_label("main"));
    assert_eq!(community_window_label("abc").unwrap(), "community-abc");
    assert!(community_window_label("../x").is_err());
    assert!(community_window_label(&"a".repeat(129)).is_err());
}

#[test]
fn resolution_scopes_bound_windows_and_fails_closed_when_unbound() {
    let state = state_with_saved(&[RELAY_B]);
    // Unbound community window: never the global relay.
    assert_eq!(
        relay_ws_url_for_window(&state, LABEL).unwrap_err(),
        UNBOUND_COMMUNITY_WINDOW
    );
    // Main, pop-outs, huddles: the workspace relay.
    for label in ["main", "popout-1", "huddle-1"] {
        assert_eq!(relay_ws_url_for_window(&state, label).unwrap(), RELAY_A);
    }
    bind_window_relay(&state, LABEL, RELAY_B).unwrap();
    let relay = WindowRelay::resolve(&state, LABEL).unwrap();
    assert_eq!(relay.ws_url(), RELAY_B);
    assert_eq!(relay.api_base(), "https://relay-b.example");
    // A main-window workspace switch never retargets a bound window.
    *state.relay_url_override.lock().unwrap() = Some("wss://relay-c.example".into());
    assert_eq!(relay_ws_url_for_window(&state, LABEL).unwrap(), RELAY_B);
}

#[test]
fn binding_is_validated_against_saved_communities() {
    let state = state_with_saved(&[RELAY_B]);
    assert!(bind_window_relay(&state, "main", RELAY_B).is_err());
    assert!(bind_window_relay(&state, "popout-1", RELAY_B).is_err());
    assert!(bind_window_relay(&state, LABEL, "wss://evil.example").is_err());
    assert!(bind_window_relay(&state, LABEL, "wss://relay-b.example/path").is_err());
    assert!(state.window_relays.lock().unwrap().is_empty());
    // Case and a trailing slash still name the saved origin.
    bind_window_relay(&state, LABEL, " wss://Relay-B.example/ ").unwrap();
    assert!(relay_ws_url_for_window(&state, LABEL).is_ok());
}

#[test]
fn rebind_moves_the_fence_and_release_unbinds() {
    let state = state_with_saved(&[RELAY_A, RELAY_B]);
    bind_window_relay(&state, LABEL, RELAY_B).unwrap();
    let captured = WindowRelay::resolve(&state, LABEL).unwrap();
    assert!(captured.is_current(&state));
    bind_window_relay(&state, LABEL, RELAY_A).unwrap();
    assert!(
        !captured.is_current(&state),
        "rebind must stale the capture"
    );
    assert_eq!(relay_ws_url_for_window(&state, LABEL).unwrap(), RELAY_A);
    release_window_relay(&state, LABEL);
    assert!(!captured.is_current(&state));
    assert!(relay_ws_url_for_window(&state, LABEL).is_err());

    // Main-window captures go stale on a workspace switch.
    let main = WindowRelay::resolve(&state, "main").unwrap();
    assert!(main.is_current(&state));
    *state.relay_url_override.lock().unwrap() = Some(RELAY_B.into());
    assert!(!main.is_current(&state));
}

#[test]
fn origins_and_authorities_normalize_like_the_browser() {
    assert_eq!(
        relay_origin("wss://Relay.Example/").as_deref(),
        Some("https://relay.example")
    );
    assert_eq!(
        relay_origin("ws://localhost:3000").as_deref(),
        Some("http://localhost:3000")
    );
    assert_eq!(relay_origin("wss://relay.example/sub"), None);
    assert_eq!(relay_origin("wss://u:p@relay.example"), None);
    assert_eq!(
        relay_authority("wss://Relay.Example:443").as_deref(),
        Some("relay.example")
    );
    assert_eq!(
        relay_authority("ws://localhost:3000").as_deref(),
        Some("localhost:3000")
    );
}

#[test]
fn bound_relay_lookup_by_authority_covers_main_and_windows() {
    let state = state_with_saved(&[RELAY_B]);
    bind_window_relay(&state, LABEL, RELAY_B).unwrap();
    assert_eq!(
        bound_relay_base_for_authority(&state, "relay-a.example").as_deref(),
        Some("https://relay-a.example")
    );
    assert_eq!(
        bound_relay_base_for_authority(&state, "RELAY-B.example").as_deref(),
        Some("https://relay-b.example")
    );
    assert_eq!(
        bound_relay_base_for_authority(&state, "relay-c.example"),
        None
    );
    release_window_relay(&state, LABEL);
    assert_eq!(
        bound_relay_base_for_authority(&state, "relay-b.example"),
        None
    );
}

#[test]
fn unread_catch_up_fence_rejects_a_rebound_window_or_swapped_signer() {
    use crate::unread_catch_up::ensure_catch_up_scope_current;
    let state = state_with_saved(&[RELAY_A, RELAY_B]);
    bind_window_relay(&state, LABEL, RELAY_B).unwrap();
    let relay = WindowRelay::resolve(&state, LABEL).unwrap();
    let owner = state.signing_keys().unwrap().public_key().to_hex();
    ensure_catch_up_scope_current(&state, &relay, &owner).unwrap();

    // The main window switching communities does not stale a bound window.
    *state.relay_url_override.lock().unwrap() = Some("wss://relay-c.example".into());
    ensure_catch_up_scope_current(&state, &relay, &owner).unwrap();

    bind_window_relay(&state, LABEL, RELAY_A).unwrap();
    assert!(ensure_catch_up_scope_current(&state, &relay, &owner).is_err());

    bind_window_relay(&state, LABEL, RELAY_B).unwrap();
    *state.keys.lock().unwrap() = nostr::Keys::generate();
    assert!(ensure_catch_up_scope_current(&state, &relay, &owner).is_err());
}

#[test]
fn titles_are_bounded_and_default_to_buzz() {
    assert_eq!(validate_title("  ").unwrap(), "Buzz");
    assert_eq!(validate_title(" Acme ").unwrap(), "Acme");
    assert!(validate_title("a\nb").is_err());
    assert!(validate_title(&"x".repeat(201)).is_err());
}

#[test]
fn one_window_per_community_focuses_the_existing_window() {
    let app = tauri::test::mock_builder()
        .manage(build_app_state())
        .build(tauri::test::mock_context(tauri::test::noop_assets()))
        .unwrap();
    let handle = app.handle();
    let id = "0b6c3a9e-2f5f-4f9e-9a43-0b8b2a6f3c11";
    assert!(!focus_existing_community_window(handle, id).unwrap());

    let first = open_or_focus_community_window(handle, id, Some("Acme")).unwrap();
    assert_eq!(first.label, LABEL);
    assert!(first.created);
    let second = open_or_focus_community_window(handle, id, Some("Acme")).unwrap();
    assert_eq!(second.label, LABEL);
    assert!(!second.created, "a second open must focus, not duplicate");
    assert!(focus_existing_community_window(handle, id).unwrap());
    let community_windows = handle
        .webview_windows()
        .keys()
        .filter(|label| is_community_window_label(label))
        .count();
    assert_eq!(community_windows, 1);

    let other = open_or_focus_community_window(handle, "other-community", None).unwrap();
    assert!(other.created);
    assert!(open_or_focus_community_window(handle, "bad/id", None).is_err());
}
