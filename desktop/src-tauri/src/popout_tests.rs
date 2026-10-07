use super::*;

fn launch(route: &str) -> PopoutLaunch {
    PopoutLaunch {
        route: route.to_owned(),
        community: serde_json::json!({ "id": "community-a" }),
    }
}

#[test]
fn route_validation_table() {
    let accepted = [
        "/",
        "/channels/0b6c3a9e-2f5f-4f9e-9a43-0b8b2a6f3c11",
        "/channels/abc?thread=deadbeef",
        "/channels/abc?thread=deadbeef#m1",
        "/dms/abc?x=a:b",
        "/channels/a:b",
    ];
    for route in accepted {
        assert!(validate_route(route).is_ok(), "expected accept: {route:?}");
    }

    let long = format!("/{}", "a".repeat(MAX_ROUTE_BYTES));
    let rejected: [&str; 13] = [
        "",
        "channels/abc",
        "//evil.example/path",
        "///triple",
        "https://evil.example",
        "javascript:alert(1)",
        "/javascript:alert(1)",
        "/a\\b",
        "/\\evil.example",
        "/a\nb",
        "/a\u{0}b",
        "/a\u{7f}b",
        &long,
    ];
    for route in rejected {
        assert!(validate_route(route).is_err(), "expected reject: {route:?}");
    }

    let at_limit = format!("/{}", "a".repeat(MAX_ROUTE_BYTES - 1));
    assert!(validate_route(&at_limit).is_ok());
}

#[test]
fn community_size_is_capped() {
    let fits = serde_json::Value::String("a".repeat(MAX_COMMUNITY_BYTES - 2));
    assert!(validate_community(&fits).is_ok());
    let too_big = serde_json::Value::String("a".repeat(MAX_COMMUNITY_BYTES - 1));
    assert!(validate_community(&too_big).is_err());
}

#[test]
fn take_returns_payload_once() {
    let registry = PopoutRegistry::default();
    registry
        .reserve("popout-1", launch("/channels/a"))
        .expect("reserve");

    assert_eq!(registry.take("popout-other"), None);
    assert_eq!(registry.take("popout-1"), Some(launch("/channels/a")));
    assert_eq!(registry.take("popout-1"), None);
    // Taking the payload does not free the window's slot.
    assert_eq!(registry.open_count(), 1);
}

#[test]
fn release_drops_untaken_payload_and_slot() {
    let registry = PopoutRegistry::default();
    registry
        .reserve("popout-1", launch("/channels/a"))
        .expect("reserve");
    registry.release("popout-1");
    assert_eq!(registry.take("popout-1"), None);
    assert_eq!(registry.open_count(), 0);
}

#[test]
fn reserve_is_capped_and_release_frees_a_slot() {
    let registry = PopoutRegistry::default();
    for index in 0..MAX_POPOUT_WINDOWS {
        registry
            .reserve(&format!("popout-{index}"), launch("/"))
            .expect("within cap");
    }
    let error = registry
        .reserve("popout-over", launch("/"))
        .expect_err("cap reached");
    assert!(error.contains("too many"), "{error}");
    assert_eq!(registry.take("popout-over"), None);

    registry.release("popout-0");
    registry
        .reserve("popout-over", launch("/"))
        .expect("slot freed");
}

#[test]
fn only_main_and_community_window_state_is_persisted() {
    assert!(persists_window_state("main"));
    assert!(persists_window_state(
        "community-0b6c3a9e-2f5f-4f9e-9a43-0b8b2a6f3c11"
    ));
    assert!(!persists_window_state("community-"));
    assert!(!persists_window_state("community-a/b"));
    assert!(!persists_window_state(
        "popout-0b6c3a9e-2f5f-4f9e-9a43-0b8b2a6f3c11"
    ));
    assert!(!persists_window_state("huddle-0b6c3a9e"));
    assert!(is_popout_label("popout-x"));
    assert!(!is_popout_label("main"));
    assert!(!is_popout_label("huddle-x"));
}

#[test]
fn launch_payload_serializes_camel_case() {
    let value = serde_json::to_value(launch("/channels/a")).expect("serialize");
    assert_eq!(
        value,
        serde_json::json!({ "route": "/channels/a", "community": { "id": "community-a" } })
    );
}

#[test]
fn bare_root_route_is_the_focus_only_request() {
    // Pop-outs send exactly "/" to `focus_main_window_route` to mean "just
    // bring the main window forward"; the main window ignores it as a
    // non-destination route.
    assert!(validate_route("/").is_ok());
    assert!(validate_route("//").is_err());
    assert!(validate_route("/\\").is_err());
}
