use super::{media_relay_base, parse_media_path, MediaTarget};
use crate::app_state::build_app_state;

const HASH: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

#[test]
fn host_prefixed_path_splits_authority_from_upstream_path() {
    let path = format!("/media/relay-b.example:8443/{HASH}.jpg?w=40");
    assert_eq!(
        parse_media_path(&path),
        Some(MediaTarget {
            authority: Some("relay-b.example:8443"),
            upstream_path: format!("/media/{HASH}.jpg?w=40"),
        })
    );
}

#[test]
fn legacy_single_segment_path_has_no_authority() {
    let path = format!("/media/{HASH}.thumb.webp");
    assert_eq!(
        parse_media_path(&path),
        Some(MediaTarget {
            authority: None,
            upstream_path: path.clone(),
        })
    );
}

#[test]
fn malformed_media_paths_are_rejected() {
    for path in [
        "/other/x.jpg".to_string(),
        "/media/".to_string(),
        "/media/host/".to_string(),
        format!("/media//{HASH}.jpg"),
        format!("/media/host/extra/{HASH}.jpg"),
        format!("/media/ho%2Fst/{HASH}.jpg"),
        format!("/media/user@host/{HASH}.jpg"),
    ] {
        assert_eq!(parse_media_path(&path), None, "{path}");
    }
}

#[test]
fn authority_resolves_only_to_a_bound_relay() {
    let state = build_app_state();
    *state.relay_url_override.lock().unwrap() = Some("wss://relay-a.example".into());
    state
        .window_relays
        .lock()
        .unwrap()
        .insert("community-b".into(), "ws://Relay-B.example:3000".into());
    let target = |authority| MediaTarget {
        authority: Some(authority),
        upstream_path: format!("/media/{HASH}.jpg"),
    };

    assert_eq!(
        media_relay_base(&state, &target("relay-a.example")).as_deref(),
        Some("https://relay-a.example")
    );
    // Authorities compare case-insensitively, ports included.
    assert_eq!(
        media_relay_base(&state, &target("relay-b.example:3000")).as_deref(),
        Some("http://Relay-B.example:3000")
    );
    // An origin no window is bound to never receives minted media auth.
    assert_eq!(media_relay_base(&state, &target("evil.example")), None);
    assert_eq!(media_relay_base(&state, &target("relay-b.example")), None);
    // The legacy form keeps targeting the main workspace relay.
    let legacy = MediaTarget {
        authority: None,
        upstream_path: format!("/media/{HASH}.jpg"),
    };
    assert_eq!(
        media_relay_base(&state, &legacy).as_deref(),
        Some("https://relay-a.example")
    );
}
