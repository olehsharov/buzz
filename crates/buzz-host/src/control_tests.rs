//! Tests for [`super::Host::handle_event`], the production control seam.

use super::*;
use crate::env::tests::{deploy_request, fake_tool_path};
use crate::protocol::{CONTROL_DEPLOY, CONTROL_FORGET, CONTROL_STATUS, CONTROL_UNDEPLOY};
use crate::supervisor::systemd::{tests::RecordingRunner, unit_name, SystemdSupervisor};
use nostr::{Timestamp, ToBech32};
use serde_json::json;

struct Fixture {
    _dir: tempfile::TempDir,
    owner: Keys,
    host: Host,
    runner: RecordingRunner,
}

fn fixture() -> Fixture {
    let dir = tempfile::tempdir().expect("tempdir");
    let tools = dir.path().join("tools");
    std::fs::create_dir_all(&tools).expect("tools dir");
    let path = fake_tool_path(&tools);
    let paths = HostPaths::at(dir.path().join("host"));
    let keys = store::load_or_create_host_key(&paths).expect("host key");
    let owner = Keys::generate();
    let record = OwnerRecord {
        owner_pubkey: owner.public_key().to_hex(),
        auth_tag: buzz_sdk::nip_oa::compute_auth_tag(&owner, &keys.public_key(), "").expect("tag"),
        relay_url: "wss://relay.example".into(),
        paired_at: 1,
        name: "test-box".into(),
    };
    store::write_json(&paths.owner_file(), &record).expect("owner");
    let runner = RecordingRunner::default();
    let supervisor = Supervisor::Systemd(SystemdSupervisor::with_unit_dir(
        paths.clone(),
        dir.path().join("units"),
        vec!["/opt/buzz".into(), "host".into()],
        Box::new(runner.clone()),
    ));
    let home = dir.path().join("home");
    let host = Host::new(paths, keys, record, supervisor, path, home).expect("host");
    Fixture {
        _dir: dir,
        owner,
        host,
        runner,
    }
}

fn control_frame(sender: &Keys, host: &PublicKey, payload: serde_json::Value, ts: u64) -> Event {
    let enc = encrypt_observer_payload(sender, host, &payload).expect("encrypt");
    buzz_sdk::build_agent_observer_frame(
        &host.to_hex(),
        &host.to_hex(),
        OBSERVER_FRAME_CONTROL,
        &enc,
    )
    .expect("frame")
    .custom_created_at(Timestamp::from(ts))
    .sign_with_keys(sender)
    .expect("sign")
}

fn deploy_json(agent: &Keys, owner: &Keys, request_id: &str) -> serde_json::Value {
    let req = deploy_request(agent, owner, request_id);
    json!({
        "type": CONTROL_DEPLOY, "request_id": request_id,
        "agent_pubkey": req.agent_pubkey, "agent_nsec": req.agent_nsec,
        "auth_tag": req.auth_tag, "relay_url": req.relay_url, "workdir": null,
        "env": {}, "launch": {
            "command": "sh", "args": [], "env": {"USER_KEY": "v"},
            "policy_env": {}, "owner_pubkey": owner.public_key().to_hex()
        }
    })
}

fn acks(out: &Outcome) -> Vec<Ack> {
    out.replies
        .iter()
        .filter_map(|r| match r {
            Reply::Ack(a) => Some(a.clone()),
            Reply::Status(_) => None,
        })
        .collect()
}

const NOW: u64 = 1_800_000_000;

/// Only the paired owner's frames run.
///
/// Mutation: drop the `event.pubkey != *owner` check → the stranger's
/// deploy runs and writes a config → RED.
#[tokio::test]
async fn control_from_non_owner_is_dropped() {
    let mut f = fixture();
    let host_pk = f.host.keys.public_key();
    let stranger = Keys::generate();
    let agent = Keys::generate();
    let frame = control_frame(
        &stranger,
        &host_pk,
        deploy_json(&agent, &stranger, "r1"),
        NOW,
    );
    let out = f.host.handle_event(&frame, NOW).await;
    assert_eq!(out.dropped, Some(DropReason::NotOwner));
    assert!(out.replies.is_empty());
    assert!(store::list_agents(&f.host.paths).expect("list").is_empty());
}

/// Frames outside ±300 s are dropped; the boundary is accepted.
///
/// Mutation: drop the freshness check → the stale frame is answered → RED.
#[tokio::test]
async fn control_outside_freshness_window_is_dropped() {
    let mut f = fixture();
    let host_pk = f.host.keys.public_key();
    let status = |id: &str| json!({"type": CONTROL_STATUS, "request_id": id});
    for (ts, id) in [
        (NOW - FRESHNESS_SECS - 1, "old"),
        (NOW + FRESHNESS_SECS + 1, "future"),
    ] {
        let frame = control_frame(&f.owner, &host_pk, status(id), ts);
        let out = f.host.handle_event(&frame, NOW).await;
        assert_eq!(out.dropped, Some(DropReason::Stale), "{id}");
    }
    let edge = control_frame(&f.owner, &host_pk, status("edge"), NOW - FRESHNESS_SECS);
    let out = f.host.handle_event(&edge, NOW).await;
    assert_eq!(out.dropped, None);
    assert!(matches!(out.replies.as_slice(), [Reply::Status(_)]));
}

/// A frame for another host, or a telemetry frame, is not a command.
#[tokio::test]
async fn control_with_wrong_route_is_dropped() {
    let mut f = fixture();
    let other = Keys::generate().public_key();
    let frame = control_frame(
        &f.owner,
        &other,
        json!({"type": CONTROL_STATUS, "request_id": "x"}),
        NOW,
    );
    let out = f.host.handle_event(&frame, NOW).await;
    assert_eq!(out.dropped, Some(DropReason::WrongRoute));
}

/// A replayed request id replays the cached ack without running again.
///
/// Mutation: skip the ledger lookup → the second deploy calls systemctl
/// restart again → RED.
#[tokio::test]
async fn duplicate_request_id_replays_ack_without_rerunning() {
    let mut f = fixture();
    let host_pk = f.host.keys.public_key();
    let agent = Keys::generate();
    let frame = control_frame(
        &f.owner,
        &host_pk,
        deploy_json(&agent, &f.owner, "dup"),
        NOW,
    );
    let first = f.host.handle_event(&frame, NOW).await;
    assert_eq!(acks(&first), vec![Ack::new("dup", Ok(()))]);
    f.runner.take();

    // Same request id in a new event (a resend), and the identical event.
    let resend = control_frame(
        &f.owner,
        &host_pk,
        deploy_json(&agent, &f.owner, "dup"),
        NOW + 1,
    );
    for ev in [&resend, &frame] {
        let again = f.host.handle_event(ev, NOW + 2).await;
        assert_eq!(acks(&again), vec![Ack::new("dup", Ok(()))]);
    }
    assert!(
        f.runner.take().is_empty(),
        "duplicate must not touch systemd"
    );

    // The ledger survives a daemon restart.
    let reloaded = SeenLedger::load(&f.host.paths);
    assert!(reloaded.get("dup").is_some());
}

/// The ledger stays bounded.
#[test]
fn seen_ledger_is_bounded() {
    let mut ledger = SeenLedger::default();
    for i in 0..(SEEN_MAX as u64 + 50) {
        // Spread over less than the retention window so only the cap evicts.
        let at = NOW + i / 2;
        ledger.insert(&format!("r{i}"), SeenEntry { at, ack: None }, at);
    }
    assert_eq!(ledger.len(), SEEN_MAX);
    assert!(ledger.get("r0").is_none(), "oldest evicted");
    ledger.insert(
        "late",
        SeenEntry {
            at: NOW + 10_000,
            ack: None,
        },
        NOW + 10_000,
    );
    assert_eq!(ledger.len(), 1, "expired entries pruned");
}

/// Deploying the same agent twice restarts one unit with the new config;
/// it never creates a second instance or a second config.
///
/// Mutation: make `apply` use `start` instead of `restart` → RED; key the
/// unit or file by request id instead of pubkey → RED.
#[tokio::test]
async fn deploy_is_idempotent_per_agent() {
    let mut f = fixture();
    let host_pk = f.host.keys.public_key();
    let agent = Keys::generate();
    let pk = agent.public_key().to_hex();
    let unit = unit_name(&pk);

    let one = control_frame(&f.owner, &host_pk, deploy_json(&agent, &f.owner, "d1"), NOW);
    assert_eq!(
        acks(&f.host.handle_event(&one, NOW).await),
        vec![Ack::new("d1", Ok(()))]
    );
    let first_calls = f.runner.take();
    assert!(first_calls
        .iter()
        .any(|c| c.ends_with(&format!("restart {unit}"))));
    assert!(first_calls.iter().any(|c| c.ends_with("daemon-reload")));

    let mut second = deploy_json(&agent, &f.owner, "d2");
    second["launch"]["env"]["USER_KEY"] = json!("v2");
    let two = control_frame(&f.owner, &host_pk, second, NOW + 1);
    assert_eq!(
        acks(&f.host.handle_event(&two, NOW + 1).await),
        vec![Ack::new("d2", Ok(()))]
    );
    let second_calls = f.runner.take();
    assert!(second_calls
        .iter()
        .any(|c| c.ends_with(&format!("restart {unit}"))));
    assert!(
        !second_calls.iter().any(|c| c.ends_with("daemon-reload")),
        "template unchanged"
    );

    assert_eq!(
        store::list_agents(&f.host.paths).expect("list"),
        vec![pk.clone()]
    );
    let rec = store::load_agent(&f.host.paths, &pk)
        .expect("load")
        .expect("record");
    assert_eq!(rec.env["USER_KEY"], "v2", "new config applied");
}

/// Undeploy disables the unit and deletes the config.
///
/// Mutation: drop `remove_file` in `undeploy` → RED; drop the supervisor
/// `remove` → no `disable --now` call → RED.
#[tokio::test]
async fn undeploy_removes_service_and_config() {
    let mut f = fixture();
    let host_pk = f.host.keys.public_key();
    let agent = Keys::generate();
    let pk = agent.public_key().to_hex();
    let deploy = control_frame(&f.owner, &host_pk, deploy_json(&agent, &f.owner, "d"), NOW);
    f.host.handle_event(&deploy, NOW).await;
    f.runner.take();

    let undeploy = control_frame(
        &f.owner,
        &host_pk,
        json!({"type": CONTROL_UNDEPLOY, "request_id": "u", "agent_pubkey": pk}),
        NOW,
    );
    assert_eq!(
        acks(&f.host.handle_event(&undeploy, NOW).await),
        vec![Ack::new("u", Ok(()))]
    );
    let calls = f.runner.take();
    assert!(calls
        .iter()
        .any(|c| c.ends_with(&format!("disable --now {}", unit_name(&pk)))));
    assert!(!f.host.paths.agent_file(&pk).exists());
}

/// Forget stops every agent and wipes all state except the key, which the
/// daemon drops after the ack is sent.
///
/// Mutation: skip `wipe_except_key` → owner.json survives → RED; skip the
/// per-agent undeploy → agent configs survive → RED.
#[tokio::test]
async fn forget_wipes_everything() {
    let mut f = fixture();
    let host_pk = f.host.keys.public_key();
    let agents = [Keys::generate(), Keys::generate()];
    for (i, agent) in agents.iter().enumerate() {
        let ev = control_frame(
            &f.owner,
            &host_pk,
            deploy_json(agent, &f.owner, &format!("d{i}")),
            NOW,
        );
        f.host.handle_event(&ev, NOW).await;
    }
    f.runner.take();
    let forget = control_frame(
        &f.owner,
        &host_pk,
        json!({"type": CONTROL_FORGET, "request_id": "f"}),
        NOW,
    );
    let out = f.host.handle_event(&forget, NOW).await;
    assert!(out.forgotten);
    assert_eq!(acks(&out), vec![Ack::new("f", Ok(()))]);
    let disabled = f
        .runner
        .take()
        .iter()
        .filter(|c| c.contains("disable --now"))
        .count();
    assert_eq!(disabled, 2);
    let paths = &f.host.paths;
    assert!(!paths.owner_file().exists());
    assert!(!paths.agents_dir().exists());
    assert!(!paths.seen_file().exists());
    assert!(store::load_owner(paths).expect("load").is_none());
}

/// An invalid deploy is acked with an error and leaves nothing behind.
#[tokio::test]
async fn failed_first_deploy_leaves_no_config() {
    let mut f = fixture();
    let host_pk = f.host.keys.public_key();
    let agent = Keys::generate();
    let mut bad = deploy_json(&agent, &f.owner, "bad");
    bad["launch"]["command"] = json!("not-installed-anywhere");
    let ev = control_frame(&f.owner, &host_pk, bad, NOW);
    let acked = acks(&f.host.handle_event(&ev, NOW).await);
    assert_eq!(acked.len(), 1);
    assert!(!acked[0].ok);
    assert!(store::list_agents(&f.host.paths).expect("list").is_empty());
}

/// Captures every log line at TRACE level.
#[derive(Clone, Default)]
struct LogCapture(std::sync::Arc<std::sync::Mutex<Vec<u8>>>);

impl std::io::Write for LogCapture {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        if let Ok(mut b) = self.0.lock() {
            b.extend_from_slice(buf);
        }
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for LogCapture {
    type Writer = LogCapture;
    fn make_writer(&'a self) -> Self::Writer {
        self.clone()
    }
}

/// Deploy, failing deploy, status, undeploy and forget never log an agent
/// nsec, its hex key, or an auth tag, even at TRACE.
///
/// Mutation: add `tracing::debug!(?req.agent_nsec)` (or log the deploy
/// payload) in `handle_event` → RED.
#[tokio::test(flavor = "current_thread")]
async fn secrets_never_reach_logs() {
    // Global, not thread-local: tracing caches callsite interest process-wide,
    // so a thread-local subscriber can miss events under parallel tests.
    static CAPTURE: std::sync::OnceLock<LogCapture> = std::sync::OnceLock::new();
    let capture = CAPTURE
        .get_or_init(|| {
            let capture = LogCapture::default();
            let subscriber = tracing_subscriber::fmt()
                .with_max_level(tracing::Level::TRACE)
                .with_writer(capture.clone())
                .with_ansi(false)
                .finish();
            tracing::subscriber::set_global_default(subscriber).expect("global subscriber");
            capture
        })
        .clone();

    let mut f = fixture();
    let host_pk = f.host.keys.public_key();
    let agent = Keys::generate();
    let nsec = agent.secret_key().to_bech32().expect("nsec");
    let hex_key = agent.secret_key().to_secret_hex();
    let deploy = deploy_json(&agent, &f.owner, "d");
    let tag = deploy["auth_tag"].as_str().expect("tag").to_string();
    let sig = tag.rsplit('"').nth(1).expect("sig").to_string();

    let mut bad = deploy.clone();
    bad["launch"]["command"] = json!("missing-tool");
    bad["request_id"] = json!("bad");
    let pk = agent.public_key().to_hex();
    let frames = [
        deploy,
        bad,
        json!({"type": CONTROL_STATUS, "request_id": "s"}),
        json!({"type": CONTROL_UNDEPLOY, "request_id": "u", "agent_pubkey": pk}),
        json!({"type": CONTROL_FORGET, "request_id": "f"}),
    ];
    for payload in frames {
        let ev = control_frame(&f.owner, &host_pk, payload, NOW);
        f.host.handle_event(&ev, NOW).await;
    }
    let logs = String::from_utf8(capture.0.lock().expect("lock").clone()).expect("utf8");
    assert!(logs.contains("deploy"), "capture works: {logs}");
    for secret in [&nsec, &hex_key, &sig, &f.host.owner.auth_tag] {
        assert!(
            !logs.contains(secret.as_str()),
            "secret leaked into logs:\n{logs}"
        );
    }
}
