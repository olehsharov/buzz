//! End-to-end: a real `buzz host` process against a live relay, driven from
//! the desktop (source/owner) side exactly as the contract describes.
//!
//! Ignored by default. Run with a local relay:
//!
//! ```text
//! BUZZ_HOST_E2E_RELAY=ws://localhost:3030 \
//! BUZZ_HOST_E2E_BIN=target/debug/buzz \
//! cargo test -p buzz-host --test e2e_local_relay -- --ignored --nocapture
//! ```
//!
//! Optional: `BUZZ_HOST_E2E_ACP=<path to buzz-acp>` runs the real harness as
//! the deployed agent; otherwise a stub `buzz-acp` that sleeps is used.

use std::path::Path;
use std::process::Stdio;
use std::time::Duration;

use buzz_core::kind::{KIND_AGENT_OBSERVER_FRAME, KIND_PAIRING};
use buzz_core::observer::{
    decrypt_observer_payload, encrypt_observer_payload, OBSERVER_FRAME_CONTROL,
};
use buzz_core::pairing::session::PairingSession;
use buzz_core::pairing::types::PayloadType;
use buzz_ws_client::{NostrWsConnection, RelayMessage};
use nostr::{Event, EventBuilder, Keys, RelayUrl, ToBech32};
use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, BufReader};
use zeroize::Zeroizing;

const STEP: Duration = Duration::from_secs(60);

fn env(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|v| !v.is_empty())
}

async fn next_event(conn: &mut NostrWsConnection, sub: &str) -> Event {
    let deadline = tokio::time::Instant::now() + STEP;
    loop {
        let left = deadline.saturating_duration_since(tokio::time::Instant::now());
        assert!(!left.is_zero(), "timed out waiting on {sub}");
        match conn.next_event(left).await {
            Ok(RelayMessage::Event {
                subscription_id,
                event,
            }) if subscription_id == sub => return *event,
            Ok(RelayMessage::Closed {
                subscription_id,
                message,
            }) if subscription_id == sub => {
                panic!("relay closed {sub}: {message}")
            }
            Ok(_) => {}
            Err(e) => panic!("relay error on {sub}: {e}"),
        }
    }
}

async fn wait_eose(conn: &mut NostrWsConnection, sub: &str) {
    loop {
        match conn.next_event(STEP).await.expect("eose") {
            RelayMessage::Eose { subscription_id } if subscription_id == sub => return,
            RelayMessage::Closed {
                subscription_id,
                message,
            } if subscription_id == sub => {
                panic!("relay closed {sub}: {message}")
            }
            _ => {}
        }
    }
}

async fn publish(conn: &mut NostrWsConnection, event: Event) {
    let ok = conn.send_event(event).await.expect("publish");
    assert!(ok.accepted, "relay rejected event: {}", ok.message);
}

/// The desktop side of pairing (NIP-AB source). Returns the host pubkey.
async fn desktop_pair(
    relay: &str,
    owner: &Keys,
    uri_tx: tokio::sync::oneshot::Sender<String>,
) -> String {
    let (mut session, _qr) = PairingSession::new_source(relay.to_string());
    let uri = session.qr_uri().expect("uri");
    let mut conn = NostrWsConnection::connect(relay).await.expect("connect");
    // Ephemeral NIP-42, as the desktop pairing task does.
    if let Ok(RelayMessage::Auth { challenge }) = conn.next_event(Duration::from_secs(5)).await {
        let auth = session
            .sign_event(EventBuilder::auth(
                challenge,
                RelayUrl::parse(relay).expect("url"),
            ))
            .expect("auth");
        let id = auth.id.to_hex();
        conn.send_raw(&json!(["AUTH", auth]))
            .await
            .expect("send auth");
        loop {
            if let RelayMessage::Ok(ok) = conn.next_event(STEP).await.expect("auth ok") {
                if ok.event_id == id {
                    assert!(ok.accepted, "pairing auth refused: {}", ok.message);
                    break;
                }
            }
        }
    }
    conn.send_raw(
        &json!(["REQ", "pair", {"kinds": [KIND_PAIRING], "#p": [session.pubkey().to_hex()]}]),
    )
    .await
    .expect("req");
    wait_eose(&mut conn, "pair").await;
    uri_tx.send(uri).expect("hand uri to host");

    let sas = loop {
        let ev = next_event(&mut conn, "pair").await;
        if let Ok(sas) = session.handle_offer(&ev) {
            break sas;
        }
    };
    println!("desktop: SAS {sas} — approving");
    publish(&mut conn, session.confirm_sas().expect("confirm")).await;

    let hello: Value = loop {
        let ev = next_event(&mut conn, "pair").await;
        if let Ok((PayloadType::Custom, payload)) = session.handle_return_payload(&ev) {
            break serde_json::from_str(&payload).expect("hello json");
        }
    };
    assert_eq!(hello["type"], "buzz-host-hello");
    assert_eq!(hello["v"], 1);
    let host_hex = hello["host_pubkey"]
        .as_str()
        .expect("host_pubkey")
        .to_string();
    println!("desktop: hello {hello}");
    let host_pk = nostr::PublicKey::from_hex(&host_hex).expect("host pk");
    let auth_tag = buzz_sdk::nip_oa::compute_auth_tag(owner, &host_pk, "").expect("tag");
    let grant = json!({
        "type": "buzz-host-grant", "v": 1,
        "owner_pubkey": owner.public_key().to_hex(),
        "auth_tag": auth_tag, "relay_url": relay,
    })
    .to_string();
    let reply = session
        .send_reply_payload(PayloadType::Custom, Zeroizing::new(grant))
        .expect("grant");
    publish(&mut conn, reply).await;
    loop {
        let ev = next_event(&mut conn, "pair").await;
        if session.handle_complete(&ev).is_ok() {
            break;
        }
    }
    println!("desktop: pairing complete");
    host_hex
}

fn control(owner: &Keys, host: &nostr::PublicKey, payload: Value) -> Event {
    let enc = encrypt_observer_payload(owner, host, &payload).expect("encrypt");
    buzz_sdk::build_agent_observer_frame(
        &host.to_hex(),
        &host.to_hex(),
        OBSERVER_FRAME_CONTROL,
        &enc,
    )
    .expect("frame")
    .sign_with_keys(owner)
    .expect("sign")
}

/// Wait for the next telemetry payload of `kind` (optionally for `request_id`).
async fn telemetry(
    conn: &mut NostrWsConnection,
    owner: &Keys,
    host: &str,
    kind: &str,
    request_id: Option<&str>,
) -> Value {
    loop {
        let ev = next_event(conn, "telemetry").await;
        if ev.pubkey.to_hex() != host {
            continue;
        }
        let v: Value = decrypt_observer_payload(owner, &ev).expect("decrypt telemetry");
        if v["type"] == kind && request_id.is_none_or(|id| v["request_id"] == id) {
            return v;
        }
    }
}

fn write_stub_acp(dir: &Path) {
    use std::os::unix::fs::PermissionsExt;
    let p = dir.join("buzz-acp");
    std::fs::write(&p, "#!/bin/sh\nexec sleep 600\n").expect("stub");
    std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).expect("chmod");
}

#[tokio::test]
#[ignore = "needs a live relay: set BUZZ_HOST_E2E_RELAY and BUZZ_HOST_E2E_BIN"]
async fn pair_deploy_undeploy_forget_against_live_relay() {
    let (Some(relay), Some(bin)) = (env("BUZZ_HOST_E2E_RELAY"), env("BUZZ_HOST_E2E_BIN")) else {
        panic!("set BUZZ_HOST_E2E_RELAY and BUZZ_HOST_E2E_BIN");
    };
    let bin = std::fs::canonicalize(bin).expect("bin path");
    let dir = tempfile::tempdir().expect("tempdir");
    let home = dir.path().join("host");
    let tools = dir.path().join("tools");
    std::fs::create_dir_all(&tools).expect("tools");
    match env("BUZZ_HOST_E2E_ACP") {
        Some(acp) => std::os::unix::fs::symlink(acp, tools.join("buzz-acp")).expect("link acp"),
        None => write_stub_acp(&tools),
    }
    let path = format!("{}:/usr/bin:/bin", tools.display());
    let owner = Keys::generate();

    // 1. Pair: desktop source in-process, `buzz host pair <uri> --yes` as target.
    let (uri_tx, uri_rx) = tokio::sync::oneshot::channel();
    let desktop = tokio::spawn({
        let relay = relay.clone();
        let owner = owner.clone();
        async move { desktop_pair(&relay, &owner, uri_tx).await }
    });
    let uri = uri_rx.await.expect("uri");
    let pair = tokio::process::Command::new(&bin)
        .args(["host", "pair", &uri, "--yes", "--name", "e2e-box"])
        .env("BUZZ_HOST_HOME", &home)
        .env("PATH", &path)
        .status()
        .await
        .expect("run pair");
    assert!(pair.success(), "buzz host pair failed");
    let host_hex = tokio::time::timeout(STEP, desktop)
        .await
        .expect("desktop")
        .expect("join");
    let host_pk = nostr::PublicKey::from_hex(&host_hex).expect("pk");
    let owner_json: Value =
        serde_json::from_slice(&std::fs::read(home.join("owner.json")).expect("owner.json"))
            .expect("json");
    assert_eq!(owner_json["owner_pubkey"], owner.public_key().to_hex());

    // 2. Desktop listens for telemetry and presence before the daemon starts.
    let mut desk = NostrWsConnection::connect_authenticated(&relay, &owner, None)
        .await
        .expect("owner connect");
    desk.send_raw(&json!(["REQ", "telemetry", {"kinds": [KIND_AGENT_OBSERVER_FRAME], "#p": [owner.public_key().to_hex()]}]))
        .await
        .expect("req telemetry");
    wait_eose(&mut desk, "telemetry").await;
    desk.send_raw(&json!(["REQ", "presence", {"kinds": [20001], "authors": [host_hex]}]))
        .await
        .expect("req presence");

    // 3. Run the daemon.
    let mut daemon = tokio::process::Command::new(&bin)
        .args(["host", "run"])
        .env("BUZZ_HOST_HOME", &home)
        .env("PATH", &path)
        .env("RUST_LOG", "debug")
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .expect("spawn daemon");
    let stderr = daemon.stderr.take().expect("stderr");
    let log = std::sync::Arc::new(tokio::sync::Mutex::new(String::new()));
    let log_task = tokio::spawn({
        let log = log.clone();
        async move {
            let mut lines = BufReader::new(stderr).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                eprintln!("host| {line}");
                log.lock().await.push_str(&format!("{line}\n"));
            }
        }
    });

    let status = telemetry(&mut desk, &owner, &host_hex, "host.status", None).await;
    println!("desktop: startup status {status}");
    assert_eq!(status["name"], "e2e-box");
    assert_eq!(status["tools"]["buzz_acp"], true);

    // 4. Deploy an agent (the desktop's exact wire format).
    let agent = Keys::generate();
    let agent_hex = agent.public_key().to_hex();
    let deploy = |request_id: &str, user_value: &str| {
        json!({
            "type": "host.deploy", "request_id": request_id,
            "agent_pubkey": agent_hex,
            "agent_nsec": agent.secret_key().to_bech32().expect("nsec"),
            "auth_tag": buzz_sdk::nip_oa::compute_auth_tag(&owner, &agent.public_key(), "").expect("tag"),
            "relay_url": relay, "workdir": null, "env": {"USER_KEY": user_value},
            "launch": {"command": "sh", "args": [], "env": {"USER_KEY": user_value},
                       "policy_env": {"BUZZ_ACP_LAZY_POOL": "true"},
                       "owner_pubkey": owner.public_key().to_hex()}
        })
    };
    publish(&mut desk, control(&owner, &host_pk, deploy("dep-1", "a"))).await;
    let ack = telemetry(&mut desk, &owner, &host_hex, "host.ack", Some("dep-1")).await;
    assert_eq!(ack["ok"], true, "deploy ack: {ack}");
    let agent_file = home.join("agents").join(format!("{agent_hex}.json"));
    assert!(agent_file.exists());
    use std::os::unix::fs::PermissionsExt;
    assert_eq!(
        std::fs::metadata(&agent_file)
            .expect("meta")
            .permissions()
            .mode()
            & 0o777,
        0o600
    );

    publish(
        &mut desk,
        control(
            &owner,
            &host_pk,
            json!({"type": "host.status", "request_id": "st-1"}),
        ),
    )
    .await;
    let st = telemetry(&mut desk, &owner, &host_hex, "host.status", Some("st-1")).await;
    assert_eq!(st["agents"].as_array().map(Vec::len), Some(1), "{st}");
    assert_eq!(st["agents"][0]["state"], "running", "{st}");

    // 5. Redeploy = restart with the new config, still one agent.
    publish(&mut desk, control(&owner, &host_pk, deploy("dep-2", "b"))).await;
    let ack = telemetry(&mut desk, &owner, &host_hex, "host.ack", Some("dep-2")).await;
    assert_eq!(ack["ok"], true, "redeploy ack: {ack}");
    publish(
        &mut desk,
        control(
            &owner,
            &host_pk,
            json!({"type": "host.status", "request_id": "st-2"}),
        ),
    )
    .await;
    let st = telemetry(&mut desk, &owner, &host_hex, "host.status", Some("st-2")).await;
    assert_eq!(st["agents"].as_array().map(Vec::len), Some(1), "{st}");

    // With the real harness, the deployed agent must come online on the relay.
    if env("BUZZ_HOST_E2E_ACP").is_some() {
        let mut watch = NostrWsConnection::connect_authenticated(&relay, &owner, None)
            .await
            .expect("connect");
        watch
            .send_raw(&json!(["REQ", "agent-presence", {"kinds": [20001], "authors": [agent_hex]}]))
            .await
            .expect("req");
        let p = next_event(&mut watch, "agent-presence").await;
        println!("desktop: agent presence {}", p.content);
        let log = home.join("logs").join(format!("{agent_hex}.log"));
        let text = std::fs::read_to_string(&log).unwrap_or_default();
        let nsec = agent.secret_key().to_bech32().expect("nsec");
        assert!(!text.contains(&nsec), "agent nsec in agent log");
    }

    // A stranger's control frame is ignored (no ack within a few seconds).
    let stranger = Keys::generate();
    let strange = control(
        &stranger,
        &host_pk,
        json!({"type": "host.forget", "request_id": "evil"}),
    );
    let _ = desk.send_event(strange).await;

    // 6. Undeploy.
    publish(
        &mut desk,
        control(
            &owner,
            &host_pk,
            json!({"type": "host.undeploy", "request_id": "un-1", "agent_pubkey": agent_hex}),
        ),
    )
    .await;
    let ack = telemetry(&mut desk, &owner, &host_hex, "host.ack", Some("un-1")).await;
    assert_eq!(ack["ok"], true, "undeploy ack: {ack}");
    assert!(!agent_file.exists());

    // 7. Presence was published by H.
    let presence = next_event(&mut desk, "presence").await;
    assert_eq!(presence.pubkey.to_hex(), host_hex);
    println!("desktop: host presence {}", presence.content);

    // 8. Forget: ack, then the daemon exits 0 and wipes its state.
    publish(
        &mut desk,
        control(
            &owner,
            &host_pk,
            json!({"type": "host.forget", "request_id": "fg-1"}),
        ),
    )
    .await;
    let ack = telemetry(&mut desk, &owner, &host_hex, "host.ack", Some("fg-1")).await;
    assert_eq!(ack["ok"], true, "forget ack: {ack}");
    let exit = tokio::time::timeout(STEP, daemon.wait())
        .await
        .expect("daemon exits")
        .expect("wait");
    assert!(exit.success(), "daemon exit {exit:?}");
    let _ = log_task.await;
    for f in ["host.key", "owner.json", "seen.json"] {
        assert!(!home.join(f).exists(), "{f} survived forget");
    }

    let log = log.lock().await.clone();
    assert!(!log.contains("evil"), "stranger frame was not handled");
    for secret in [
        agent.secret_key().to_bech32().expect("nsec"),
        agent.secret_key().to_secret_hex(),
    ] {
        assert!(!log.contains(&secret), "agent secret in daemon log");
    }
    let tag = owner_json["auth_tag"].as_str().expect("tag");
    assert!(!log.contains(tag), "host auth tag in daemon log");
    assert!(
        !log.contains("rejected"),
        "relay rejected something:\n{log}"
    );
}
