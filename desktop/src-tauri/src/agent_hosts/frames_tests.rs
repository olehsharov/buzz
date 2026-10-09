use super::*;
use buzz_core_pkg::observer::encrypt_observer_payload;
use nostr::{EventBuilder, Kind, Tag, Timestamp};

fn telemetry_with(
    signer: &Keys,
    recipient: &PublicKey,
    agent_tag: &PublicKey,
    frame: &str,
    created_at: u64,
    payload: serde_json::Value,
) -> Event {
    let content = encrypt_observer_payload(signer, recipient, &payload).unwrap();
    EventBuilder::new(Kind::Custom(KIND_AGENT_OBSERVER_FRAME as u16), content)
        .tags([
            Tag::parse(["p", &recipient.to_hex()]).unwrap(),
            Tag::parse(["agent", &agent_tag.to_hex()]).unwrap(),
            Tag::parse(["frame", frame]).unwrap(),
        ])
        .custom_created_at(Timestamp::from(created_at))
        .sign_with_keys(signer)
        .unwrap()
}

fn ack(request_id: &str) -> serde_json::Value {
    serde_json::json!({"type": "host.ack", "request_id": request_id, "ok": true})
}

#[test]
fn accepts_well_formed_ack_and_status() {
    let owner = Keys::generate();
    let host = Keys::generate();
    let now = 1_800_000_000;
    let event = telemetry_with(
        &host,
        &owner.public_key(),
        &host.public_key(),
        "telemetry",
        now,
        ack("r1"),
    );
    assert_eq!(
        parse_host_telemetry(&owner, &host.public_key(), &event, now).unwrap(),
        Some(HostTelemetry::Ack(HostAck {
            request_id: "r1".into(),
            ok: true,
            error: None
        }))
    );

    let status = serde_json::json!({
        "type": "host.status", "name": "box", "os": "linux", "arch": "x86_64", "version": "0.1.0",
        "agents": [{"agent_pubkey": "aa", "state": "running", "since": 1}],
        "claude": {"installed": true, "auth_ok": null},
        "tools": {"node": true, "claude_agent_acp": false, "buzz_acp": true}
    });
    let event = telemetry_with(
        &host,
        &owner.public_key(),
        &host.public_key(),
        "telemetry",
        now,
        status,
    );
    let Some(HostTelemetry::Status(status)) =
        parse_host_telemetry(&owner, &host.public_key(), &event, now).unwrap()
    else {
        panic!("expected status");
    };
    assert_eq!(status.agents[0].state, "running");
    assert!(status.tools.buzz_acp && !status.tools.claude_agent_acp);
    assert_eq!(status.claude.auth_ok, None);
}

/// Every rejection axis, each falsifiable on its own: drop any one check in
/// `parse_host_telemetry` and exactly that row starts passing.
#[test]
fn rejects_frames_that_are_not_this_hosts_telemetry_to_this_owner() {
    let owner = Keys::generate();
    let host = Keys::generate();
    let other = Keys::generate();
    let now = 1_800_000_000;
    let cases: Vec<(&str, Event)> = vec![
        (
            "signed by another key",
            telemetry_with(
                &other,
                &owner.public_key(),
                &host.public_key(),
                "telemetry",
                now,
                ack("r"),
            ),
        ),
        (
            "addressed to someone else",
            telemetry_with(
                &host,
                &other.public_key(),
                &host.public_key(),
                "telemetry",
                now,
                ack("r"),
            ),
        ),
        (
            "agent tag not the host",
            telemetry_with(
                &host,
                &owner.public_key(),
                &other.public_key(),
                "telemetry",
                now,
                ack("r"),
            ),
        ),
        (
            "control direction",
            telemetry_with(
                &host,
                &owner.public_key(),
                &host.public_key(),
                "control",
                now,
                ack("r"),
            ),
        ),
        (
            "stale",
            telemetry_with(
                &host,
                &owner.public_key(),
                &host.public_key(),
                "telemetry",
                now - FRESHNESS_SECS - 1,
                ack("r"),
            ),
        ),
        (
            "future",
            telemetry_with(
                &host,
                &owner.public_key(),
                &host.public_key(),
                "telemetry",
                now + FRESHNESS_SECS + 1,
                ack("r"),
            ),
        ),
    ];
    for (label, event) in cases {
        assert!(
            parse_host_telemetry(&owner, &host.public_key(), &event, now).is_err(),
            "{label} must be rejected"
        );
    }
    // Inside the window on both edges is fine.
    let edge = telemetry_with(
        &host,
        &owner.public_key(),
        &host.public_key(),
        "telemetry",
        now - FRESHNESS_SECS,
        ack("r"),
    );
    assert!(parse_host_telemetry(&owner, &host.public_key(), &edge, now).is_ok());
}

#[test]
fn unknown_telemetry_type_is_ignored_not_an_error() {
    let owner = Keys::generate();
    let host = Keys::generate();
    let now = 1_800_000_000;
    let event = telemetry_with(
        &host,
        &owner.public_key(),
        &host.public_key(),
        "telemetry",
        now,
        serde_json::json!({"type": "host.future"}),
    );
    assert_eq!(
        parse_host_telemetry(&owner, &host.public_key(), &event, now).unwrap(),
        None
    );
}

#[test]
fn control_event_is_tagged_like_buzz_acp_and_decrypts_for_the_host() {
    let owner = Keys::generate();
    let host = Keys::generate();
    let payload = undeploy_frame("req-1", "agent-hex");
    let event = build_control_event(&owner, &host.public_key(), &payload).unwrap();
    assert_eq!(event.kind.as_u16() as u32, KIND_AGENT_OBSERVER_FRAME);
    assert_eq!(event.pubkey, owner.public_key());
    let tags: Vec<Vec<String>> = event.tags.iter().map(|t| t.as_slice().to_vec()).collect();
    let h = host.public_key().to_hex();
    assert_eq!(
        tags,
        vec![
            vec!["p".to_string(), h.clone()],
            vec!["agent".to_string(), h],
            vec!["frame".to_string(), "control".to_string()],
        ]
    );
    let decrypted: serde_json::Value =
        buzz_core_pkg::observer::decrypt_observer_payload(&host, &event).unwrap();
    assert_eq!(
        decrypted,
        serde_json::json!({"type": "host.undeploy", "request_id": "req-1", "agent_pubkey": "agent-hex"})
    );
}

#[test]
fn deploy_frame_requires_key_relay_and_launch() {
    let full = serde_json::json!({
        "private_key_nsec": "nsec1x", "relay_url": "wss://r", "launch": {"command": "c"},
    });
    let frame = deploy_frame("r", "a", "tag", None, &full).unwrap();
    assert_eq!(frame["env"], serde_json::json!({}));
    assert!(frame["respond_to"].is_null());
    assert_eq!(frame["respond_to_allowlist"], serde_json::json!([]));
    let mut with_policy = full.clone();
    with_policy["respond_to"] = "allowlist".into();
    with_policy["respond_to_allowlist"] = serde_json::json!(["ab"]);
    let frame = deploy_frame("r", "a", "tag", None, &with_policy).unwrap();
    assert_eq!(frame["respond_to"], "allowlist");
    assert_eq!(frame["respond_to_allowlist"], serde_json::json!(["ab"]));
    for missing in ["private_key_nsec", "relay_url", "launch"] {
        let mut payload = full.clone();
        payload.as_object_mut().unwrap().remove(missing);
        assert!(
            deploy_frame("r", "a", "tag", None, &payload).is_err(),
            "{missing}"
        );
    }
    let mut empty_key = full;
    empty_key["private_key_nsec"] = "".into();
    assert!(deploy_frame("r", "a", "tag", None, &empty_key).is_err());
}

#[test]
fn deploy_frame_carries_the_saved_folder_or_null() {
    let full = serde_json::json!({
        "private_key_nsec": "nsec1x", "relay_url": "wss://r", "launch": {"command": "c"},
    });
    let frame = deploy_frame("r", "a", "tag", Some("  ~/work/agent "), &full).unwrap();
    assert_eq!(frame["workdir"], "~/work/agent");
    for unset in [None, Some(""), Some("   ")] {
        let frame = deploy_frame("r", "a", "tag", unset, &full).unwrap();
        assert!(frame["workdir"].is_null(), "{unset:?}");
    }
}

#[test]
fn host_workdir_validation() {
    assert_eq!(normalize_host_workdir(None), Ok(None));
    assert_eq!(normalize_host_workdir(Some("  ")), Ok(None));
    assert_eq!(
        normalize_host_workdir(Some(" ~/code/app ")),
        Ok(Some("~/code/app".into()))
    );
    assert_eq!(
        normalize_host_workdir(Some("/srv/agents/one")),
        Ok(Some("/srv/agents/one".into()))
    );
    assert_eq!(
        normalize_host_workdir(Some("relative/dir")),
        Ok(Some("relative/dir".into()))
    );
    let longest = "a".repeat(MAX_HOST_WORKDIR_CHARS);
    assert_eq!(normalize_host_workdir(Some(&longest)), Ok(Some(longest)));
    assert!(normalize_host_workdir(Some(&"a".repeat(MAX_HOST_WORKDIR_CHARS + 1))).is_err());
    for bad in ["a\nb", "a\rb", "a\0b"] {
        assert!(normalize_host_workdir(Some(bad)).is_err(), "{bad:?}");
    }
}

#[test]
fn hello_validation() {
    let host = Keys::generate().public_key().to_hex();
    let good = serde_json::json!({
        "type": "buzz-host-hello", "v": 1, "host_pubkey": host.to_uppercase(),
        "name": "  devbox\u{7} ", "os": "linux", "arch": "x86_64", "version": "0.1.0"
    });
    let hello = parse_host_hello(&good.to_string()).unwrap();
    assert_eq!(hello.host_pubkey, host);
    assert_eq!(hello.name, "devbox");

    for (field, value) in [
        ("type", serde_json::json!("buzz-host-grant")),
        ("v", serde_json::json!(2)),
        ("host_pubkey", serde_json::json!("not-hex")),
        ("name", serde_json::json!("")),
        ("os", serde_json::json!("x".repeat(200))),
    ] {
        let mut bad = good.clone();
        bad[field] = value;
        assert!(parse_host_hello(&bad.to_string()).is_err(), "{field}");
    }
    assert!(parse_host_hello("not json").is_err());
}

#[test]
fn grant_auth_tag_verifies_for_the_host_against_the_owner() {
    let owner = Keys::generate();
    let host = Keys::generate();
    let grant = build_host_grant(&owner, &host.public_key(), "wss://relay.example").unwrap();
    assert_eq!(grant["type"], "buzz-host-grant");
    assert_eq!(grant["v"], 1);
    assert_eq!(grant["owner_pubkey"], owner.public_key().to_hex());
    assert_eq!(grant["relay_url"], "wss://relay.example");
    let tag = grant["auth_tag"].as_str().unwrap();
    let verified =
        buzz_sdk_pkg::nip_oa::verify_auth_tag(tag, &host.public_key()).expect("tag verifies");
    assert_eq!(verified, owner.public_key());
    // Bound to the host key: it does not authorize any other key.
    assert!(buzz_sdk_pkg::nip_oa::verify_auth_tag(tag, &Keys::generate().public_key()).is_err());
    // Empty conditions, same as agent auth tags.
    let parts: Vec<String> = serde_json::from_str(tag).unwrap();
    assert_eq!(parts[2], "");
}

#[test]
fn status_sanitizer_bounds_host_text() {
    let status = HostStatus {
        request_id: None,
        name: format!(" {}\u{1b}[31m ", "n".repeat(500)),
        os: "linux".into(),
        arch: "arm64".into(),
        version: None,
        agents: (0..1000)
            .map(|i| HostAgentState {
                agent_pubkey: format!("{i}"),
                state: "running".into(),
                since: None,
            })
            .collect(),
        claude: HostClaudeState::default(),
        tools: HostTools::default(),
    };
    let clean = sanitize_status(&status);
    assert_eq!(clean.name.chars().count(), 128);
    assert!(!clean.name.contains('\u{1b}'));
    assert_eq!(clean.agents.len(), 256);
}
