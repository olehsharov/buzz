//! Host lifecycle tests against the production `ops` functions, the real
//! frame encryption/verification, and a scripted host on the other end of
//! the [`HostChannel`] seam. The fake host decrypts each control frame with
//! its own key and answers with a real signed telemetry frame that goes
//! through `parse_host_telemetry`, so the wire format is exercised too.

use std::collections::VecDeque;
use std::sync::Mutex as StdMutex;

use futures_util::future::BoxFuture;
use nostr::{Event, JsonUtil, Keys, PublicKey};
use tauri::test::MockRuntime;
use tauri::Manager;
use tokio::sync::{oneshot, Notify};

use super::*;
use crate::agent_hosts::channel::HostChannel;
use crate::agent_hosts::frames::{parse_host_telemetry, HostTelemetry};
use crate::agent_hosts::store::AgentHostRecord;

const RELAY: &str = "wss://hosts.example";

enum Reply {
    AckOk,
    AckErr(&'static str),
    /// Never answer (the ack timeout must fire).
    Silent,
    /// Answer `ok` only after the test releases the gate.
    Gated(oneshot::Receiver<()>),
}

struct FakeHosts {
    owner: Keys,
    hosts: Vec<Keys>,
    script: StdMutex<VecDeque<Reply>>,
    /// (host pubkey, decrypted control payload) in arrival order.
    received: StdMutex<Vec<(String, serde_json::Value)>>,
    /// Raw control events as they would go on the wire.
    wire: StdMutex<Vec<String>>,
    frame_arrived: Notify,
}

impl FakeHosts {
    fn new(owner: Keys, hosts: Vec<Keys>, script: Vec<Reply>) -> Self {
        Self {
            owner,
            hosts,
            script: StdMutex::new(script.into()),
            received: StdMutex::new(Vec::new()),
            wire: StdMutex::new(Vec::new()),
            frame_arrived: Notify::new(),
        }
    }

    fn types(&self) -> Vec<(String, String)> {
        self.received
            .lock()
            .unwrap()
            .iter()
            .map(|(host, payload)| (host.clone(), payload["type"].as_str().unwrap().to_string()))
            .collect()
    }

    fn telemetry(&self, host: &Keys, payload: serde_json::Value) -> Event {
        let encrypted = buzz_core_pkg::observer::encrypt_observer_payload(
            host,
            &self.owner.public_key(),
            &payload,
        )
        .unwrap();
        buzz_sdk_pkg::build_agent_observer_frame(
            &self.owner.public_key().to_hex(),
            &host.public_key().to_hex(),
            buzz_core_pkg::observer::OBSERVER_FRAME_TELEMETRY,
            &encrypted,
        )
        .unwrap()
        .sign_with_keys(host)
        .unwrap()
    }
}

impl HostChannel for FakeHosts {
    fn exchange<'a>(
        &'a self,
        frame: Event,
        host: PublicKey,
        request_id: String,
        _timeout: std::time::Duration,
    ) -> BoxFuture<'a, Result<HostTelemetry, String>> {
        Box::pin(async move {
            let host_keys = self
                .hosts
                .iter()
                .find(|keys| keys.public_key() == host)
                .expect("frame addressed to a known host")
                .clone();
            // The host side: verify tags and decrypt like a real host would.
            let tag = |name: &str| {
                frame
                    .tags
                    .iter()
                    .find(|tag| tag.kind().to_string() == name)
                    .and_then(|tag| tag.content())
                    .unwrap()
                    .to_string()
            };
            assert_eq!(frame.pubkey, self.owner.public_key());
            assert_eq!(tag("p"), host.to_hex());
            assert_eq!(tag("agent"), host.to_hex());
            assert_eq!(tag("frame"), "control");
            let payload: serde_json::Value =
                buzz_core_pkg::observer::decrypt_observer_payload(&host_keys, &frame).unwrap();
            assert_eq!(payload["request_id"], request_id);
            self.wire.lock().unwrap().push(frame.as_json());
            self.received
                .lock()
                .unwrap()
                .push((host.to_hex(), payload.clone()));
            self.frame_arrived.notify_one();

            let reply = self
                .script
                .lock()
                .unwrap()
                .pop_front()
                .expect("scripted reply");
            let ack = match reply {
                Reply::AckOk => {
                    serde_json::json!({"type": "host.ack", "request_id": request_id, "ok": true})
                }
                Reply::AckErr(error) => {
                    serde_json::json!({"type": "host.ack", "request_id": request_id, "ok": false, "error": error})
                }
                Reply::Silent => std::future::pending().await,
                Reply::Gated(gate) => {
                    gate.await.unwrap();
                    serde_json::json!({"type": "host.ack", "request_id": request_id, "ok": true})
                }
            };
            let event = self.telemetry(&host_keys, ack);
            parse_host_telemetry(&self.owner, &host, &event, frames::now_secs())
                .map(|telemetry| telemetry.expect("known telemetry type"))
        })
    }
}

struct Fixture {
    app: tauri::App<MockRuntime>,
    owner: Keys,
    agent_keys: Keys,
    agent_nsec: String,
    host_a: Keys,
    host_b: Keys,
    _data: tempfile::TempDir,
}

impl Fixture {
    fn new() -> Self {
        #[cfg(feature = "system-keyring")]
        keyring::set_default_credential_builder(keyring::mock::default_credential_builder());
        let data = tempfile::tempdir().unwrap();
        let mut context = tauri::test::mock_context(tauri::test::noop_assets());
        context.config_mut().identifier = data.path().to_str().unwrap().to_owned();
        let state = crate::app_state::build_app_state();
        *state.relay_url_override.lock().unwrap() = Some(RELAY.into());
        let owner = state.signing_keys().unwrap();
        let app = tauri::test::mock_builder()
            .manage(state)
            .manage(HostOps::default())
            .build(context)
            .unwrap();
        let agent_keys = Keys::generate();
        let agent_nsec = nostr::ToBech32::to_bech32(agent_keys.secret_key()).unwrap();
        let record: ManagedAgentRecord = serde_json::from_value(serde_json::json!({
            "pubkey": agent_keys.public_key().to_hex(), "name": "Host Test Agent",
            "relay_url": "", "acp_command": "", "agent_command": "", "agent_args": [],
            "mcp_command": "", "turn_timeout_seconds": 0, "system_prompt": null,
            "private_key_nsec": agent_nsec,
            "auth_tag": "[\"auth\",\"owner\",\"\",\"sig\"]",
            "created_at": "2026-01-01T00:00:00Z", "updated_at": "2026-01-01T00:00:00Z",
            "last_started_at": null, "last_stopped_at": null, "last_exit_code": null,
            "last_error": null
        }))
        .unwrap();
        save_managed_agents(app.handle(), &[record]).unwrap();
        let host_a = Keys::generate();
        let host_b = Keys::generate();
        let ops = HostOps::default();
        for (keys, name) in [(&host_a, "alpha"), (&host_b, "bravo")] {
            add_approved_host(
                app.handle(),
                &ops,
                AgentHostRecord {
                    pubkey: keys.public_key().to_hex(),
                    name: name.into(),
                    os: "linux".into(),
                    arch: "x86_64".into(),
                    relay_url: RELAY.into(),
                    added_at: "2026-01-01T00:00:00Z".into(),
                    status: None,
                },
            )
            .unwrap();
        }
        Self {
            app,
            owner,
            agent_keys,
            agent_nsec,
            host_a,
            host_b,
            _data: data,
        }
    }

    fn agent(&self) -> String {
        self.agent_keys.public_key().to_hex()
    }

    fn record(&self) -> ManagedAgentRecord {
        load_managed_agents(self.app.handle())
            .unwrap()
            .into_iter()
            .find(|record| record.pubkey == self.agent())
            .unwrap()
    }

    fn place_on(&self, host: &Keys) {
        let mut records = load_managed_agents(self.app.handle()).unwrap();
        records[0].backend = BackendKind::Host {
            host_pubkey: host.public_key().to_hex(),
            workdir: None,
        };
        records[0].backend_agent_id = Some(host.public_key().to_hex());
        save_managed_agents(self.app.handle(), &records).unwrap();
    }

    fn fake(&self, script: Vec<Reply>) -> FakeHosts {
        FakeHosts::new(
            self.owner.clone(),
            vec![self.host_a.clone(), self.host_b.clone()],
            script,
        )
    }

    /// Stand-in for `build_deploy_payload` (which needs the full persona and
    /// harness catalog). The mock keyring does not persist across entries,
    /// so the key comes from the fixture rather than the hydrated record.
    fn payload(&self) -> impl FnOnce(&ManagedAgentRecord) -> Result<serde_json::Value, String> {
        let nsec = self.agent_nsec.clone();
        move |_record: &ManagedAgentRecord| {
            Ok(serde_json::json!({
                "relay_url": RELAY,
                "private_key_nsec": nsec,
                "env_vars": {"FOO": "bar"},
                "launch": {"command": "goose", "args": ["acp"], "env": {}, "policy_env": {}, "owner_pubkey": "o"},
            }))
        }
    }

    async fn deploy(&self, ops: &HostOps, channel: &FakeHosts, host: &Keys) -> Result<(), String> {
        let state = self.app.state::<AppState>();
        deploy_agent_to_host(
            self.app.handle(),
            &state,
            ops,
            channel,
            &self.agent(),
            &host.public_key().to_hex(),
            RELAY,
            self.payload(),
        )
        .await
    }
}

fn deployed_on(record: &ManagedAgentRecord) -> Option<String> {
    deployed_host(record).map(str::to_string)
}

#[tokio::test]
async fn deploy_persists_only_after_the_host_acks() {
    let fx = Fixture::new();
    let ops = HostOps::default();
    let (release, gate) = oneshot::channel();
    let fake = fx.fake(vec![Reply::Gated(gate)]);

    let deploy = fx.deploy(&ops, &fake, &fx.host_a);
    tokio::pin!(deploy);
    tokio::select! {
        _ = &mut deploy => panic!("deploy finished before the host answered"),
        _ = fake.frame_arrived.notified() => {}
    }
    // Frame is out, no ack yet: the record must not claim the deploy.
    assert_eq!(deployed_on(&fx.record()), None);

    release.send(()).unwrap();
    deploy.await.unwrap();
    let record = fx.record();
    assert_eq!(deployed_on(&record), Some(fx.host_a.public_key().to_hex()));
    assert_eq!(record.last_error, None);

    // The host received exactly the contract's host.deploy shape.
    let received = fake.received.lock().unwrap();
    let (_, frame) = &received[0];
    assert_eq!(frame["type"], "host.deploy");
    assert_eq!(frame["agent_pubkey"], fx.agent());
    assert_eq!(frame["agent_nsec"], fx.agent_nsec);
    assert_eq!(frame["auth_tag"], "[\"auth\",\"owner\",\"\",\"sig\"]");
    assert_eq!(frame["relay_url"], RELAY);
    assert_eq!(frame["env"]["FOO"], "bar");
    assert_eq!(frame["launch"]["command"], "goose");
    assert!(frame["workdir"].is_null());
}

#[tokio::test(start_paused = true)]
async fn deploy_timeout_leaves_the_record_undeployed_with_a_visible_error() {
    let fx = Fixture::new();
    let ops = HostOps::default();
    assert_eq!(ops.ack_timeout, std::time::Duration::from_secs(60));
    let fake = fx.fake(vec![Reply::Silent]);
    let started = tokio::time::Instant::now();

    let error = fx.deploy(&ops, &fake, &fx.host_a).await.unwrap_err();
    assert!(started.elapsed() >= std::time::Duration::from_secs(60));
    assert!(error.contains("did not answer in time"), "{error}");
    let record = fx.record();
    assert_eq!(deployed_on(&record), None);
    assert_eq!(record.last_error.as_deref(), Some(error.as_str()));
}

#[tokio::test]
async fn refused_deploy_is_not_persisted() {
    let fx = Fixture::new();
    let ops = HostOps::default();
    let fake = fx.fake(vec![Reply::AckErr("claude not installed")]);
    let error = fx.deploy(&ops, &fake, &fx.host_a).await.unwrap_err();
    assert!(error.contains("claude not installed"));
    assert_eq!(deployed_on(&fx.record()), None);
}

#[tokio::test]
async fn move_undeploys_the_old_host_before_deploying_to_the_new_one() {
    let fx = Fixture::new();
    fx.place_on(&fx.host_a);
    let ops = HostOps::default();
    let fake = fx.fake(vec![Reply::AckOk, Reply::AckOk]);

    fx.deploy(&ops, &fake, &fx.host_b).await.unwrap();
    assert_eq!(
        fake.types(),
        vec![
            (fx.host_a.public_key().to_hex(), "host.undeploy".to_string()),
            (fx.host_b.public_key().to_hex(), "host.deploy".to_string()),
        ]
    );
    assert_eq!(
        fake.received.lock().unwrap()[0].1["agent_pubkey"],
        fx.agent()
    );
    assert_eq!(
        deployed_on(&fx.record()),
        Some(fx.host_b.public_key().to_hex())
    );
}

#[tokio::test]
async fn move_stops_when_the_old_host_does_not_confirm_undeploy() {
    let fx = Fixture::new();
    fx.place_on(&fx.host_a);
    let ops = HostOps::default();
    let fake = fx.fake(vec![Reply::AckErr("busy")]);

    let error = fx.deploy(&ops, &fake, &fx.host_b).await.unwrap_err();
    assert!(error.contains("current machine"), "{error}");
    // The new host never heard about the agent; it is still on the old one.
    assert_eq!(fake.types().len(), 1);
    assert_eq!(
        deployed_on(&fx.record()),
        Some(fx.host_a.public_key().to_hex())
    );
}

#[tokio::test]
async fn redeploy_to_the_same_host_does_not_undeploy_first() {
    let fx = Fixture::new();
    fx.place_on(&fx.host_a);
    let ops = HostOps::default();
    let fake = fx.fake(vec![Reply::AckOk]);
    fx.deploy(&ops, &fake, &fx.host_a).await.unwrap();
    assert_eq!(fake.types()[0].1, "host.deploy");
    assert_eq!(fake.types().len(), 1);
}

impl Fixture {
    /// Mark a saved `anyone` policy as not yet delivered to the machine.
    fn save_pending_anyone_policy(&self) {
        let mut records = load_managed_agents(self.app.handle()).unwrap();
        records[0].respond_to = crate::managed_agents::RespondTo::Anyone;
        records[0].provider_policy_pending = true;
        save_managed_agents(self.app.handle(), &records).unwrap();
    }

    /// Redeploy to host A with a payload that carries `respond_to`.
    async fn redeploy_with_policy(
        &self,
        ops: &HostOps,
        channel: &FakeHosts,
        respond_to: &str,
    ) -> Result<(), String> {
        let state = self.app.state::<AppState>();
        let nsec = self.agent_nsec.clone();
        let respond_to = respond_to.to_string();
        deploy_agent_to_host(
            self.app.handle(),
            &state,
            ops,
            channel,
            &self.agent(),
            &self.host_a.public_key().to_hex(),
            RELAY,
            move |_record: &ManagedAgentRecord| {
                Ok(serde_json::json!({
                    "relay_url": RELAY,
                    "private_key_nsec": nsec,
                    "launch": {"command": "goose", "args": ["acp"], "env": {}, "policy_env": {}},
                    "respond_to": respond_to,
                    "respond_to_allowlist": [],
                }))
            },
        )
        .await
    }
}

/// A machine restarts a redeployed agent with the frame's policy, so its ack
/// is what delivers a pending access change.
#[tokio::test]
async fn acknowledged_redeploy_delivers_a_pending_access_policy() {
    let fx = Fixture::new();
    fx.place_on(&fx.host_a);
    fx.save_pending_anyone_policy();
    let ops = HostOps::default();
    let fake = fx.fake(vec![Reply::AckOk]);

    fx.redeploy_with_policy(&ops, &fake, "anyone")
        .await
        .unwrap();

    assert_eq!(fake.received.lock().unwrap()[0].1["respond_to"], "anyone");
    assert!(!fx.record().provider_policy_pending);
}

#[tokio::test]
async fn refused_or_stale_redeploys_keep_the_access_policy_pending() {
    let fx = Fixture::new();
    fx.place_on(&fx.host_a);
    fx.save_pending_anyone_policy();
    let ops = HostOps::default();

    let refused = fx.fake(vec![Reply::AckErr("disk full")]);
    let error = fx
        .redeploy_with_policy(&ops, &refused, "anyone")
        .await
        .unwrap_err();
    assert!(error.contains("disk full"), "{error}");
    let record = fx.record();
    assert!(record.provider_policy_pending, "retried later");
    assert_eq!(deployed_on(&record), Some(fx.host_a.public_key().to_hex()));

    // A deploy that delivered an older policy does not acknowledge the newer one.
    let stale = fx.fake(vec![Reply::AckOk]);
    fx.redeploy_with_policy(&ops, &stale, "owner-only")
        .await
        .unwrap();
    assert!(fx.record().provider_policy_pending);
}

#[tokio::test]
async fn forgetting_the_host_mid_deploy_fences_out_the_late_ack() {
    let fx = Fixture::new();
    // The record already targets host A (created with "Where to run" = A).
    {
        let mut records = load_managed_agents(fx.app.handle()).unwrap();
        records[0].backend = BackendKind::Host {
            host_pubkey: fx.host_a.public_key().to_hex(),
            workdir: None,
        };
        save_managed_agents(fx.app.handle(), &records).unwrap();
    }
    let ops = HostOps::default();
    let (release, gate) = oneshot::channel();
    // deploy (gated), then the forget's own ack.
    let fake = fx.fake(vec![Reply::Gated(gate), Reply::AckOk]);

    let deploy = fx.deploy(&ops, &fake, &fx.host_a);
    tokio::pin!(deploy);
    tokio::select! {
        _ = &mut deploy => panic!("deploy finished before the host answered"),
        _ = fake.frame_arrived.notified() => {}
    }
    let state = fx.app.state::<AppState>();
    let outcome = forget_host(
        fx.app.handle(),
        &state,
        &ops,
        &fake,
        RELAY,
        &fx.host_a.public_key().to_hex(),
    )
    .await
    .unwrap();
    assert!(outcome.host_acknowledged);
    assert_eq!(outcome.undeployed_agents, vec![fx.agent()]);

    release.send(()).unwrap();
    assert_eq!(deploy.await.unwrap_err(), SUPERSEDED);
    assert_eq!(deployed_on(&fx.record()), None);
    assert!(list_hosts(fx.app.handle(), &ops, RELAY)
        .unwrap()
        .iter()
        .all(|host| host.pubkey != fx.host_a.public_key().to_hex()));
}

#[tokio::test]
async fn forget_removes_locally_even_when_the_host_is_unreachable() {
    let fx = Fixture::new();
    fx.place_on(&fx.host_a);
    let ops = HostOps::default();
    let fake = fx.fake(vec![Reply::AckErr("gone")]);
    let state = fx.app.state::<AppState>();
    let outcome = forget_host(
        fx.app.handle(),
        &state,
        &ops,
        &fake,
        RELAY,
        &fx.host_a.public_key().to_hex(),
    )
    .await
    .unwrap();
    assert!(!outcome.host_acknowledged);
    assert_eq!(fake.types()[0].1, "host.forget");
    assert_eq!(deployed_on(&fx.record()), None);
    assert_eq!(list_hosts(fx.app.handle(), &ops, RELAY).unwrap().len(), 1);
}

#[tokio::test]
async fn undeploy_requires_the_ack() {
    let fx = Fixture::new();
    fx.place_on(&fx.host_a);
    let ops = HostOps::default();
    let state = fx.app.state::<AppState>();

    let fake = fx.fake(vec![Reply::AckErr("nope")]);
    undeploy_agent_from_host(fx.app.handle(), &state, &ops, &fake, &fx.agent())
        .await
        .unwrap_err();
    assert_eq!(
        deployed_on(&fx.record()),
        Some(fx.host_a.public_key().to_hex())
    );

    let fake = fx.fake(vec![Reply::AckOk]);
    undeploy_agent_from_host(fx.app.handle(), &state, &ops, &fake, &fx.agent())
        .await
        .unwrap();
    assert_eq!(fake.types()[0].1, "host.undeploy");
    assert_eq!(deployed_on(&fx.record()), None);
}

#[tokio::test]
async fn deploy_refuses_a_machine_not_approved_in_this_community() {
    let fx = Fixture::new();
    let ops = HostOps::default();
    let stranger = Keys::generate();
    let fake = FakeHosts::new(fx.owner.clone(), vec![stranger.clone()], vec![]);
    let error = fx.deploy(&ops, &fake, &stranger).await.unwrap_err();
    assert!(error.contains("not approved"));
    assert!(fake.types().is_empty());
}

#[tokio::test(start_paused = true)]
async fn nsec_never_leaves_in_plaintext_or_lands_in_errors() {
    let fx = Fixture::new();
    let ops = HostOps::default();
    let fake = fx.fake(vec![Reply::AckOk, Reply::Silent]);
    fx.deploy(&ops, &fake, &fx.host_a).await.unwrap();
    let error = fx.deploy(&ops, &fake, &fx.host_a).await.unwrap_err();

    let secret_hex = fx.agent_keys.secret_key().to_secret_hex();
    for wire in fake.wire.lock().unwrap().iter() {
        assert!(
            !wire.contains(&fx.agent_nsec),
            "nsec on the wire in plaintext"
        );
        assert!(!wire.contains(&secret_hex));
    }
    assert!(!error.contains(&fx.agent_nsec));
    let record = fx.record();
    assert!(!record
        .last_error
        .unwrap_or_default()
        .contains(&fx.agent_nsec));
    // Debug output of everything the host flow logs or returns is clean too.
    let debug = format!("{:?}", fake.types());
    assert!(!debug.contains(&fx.agent_nsec));
}

#[path = "ops_workdir_tests.rs"]
mod workdir;

#[path = "ops_route_tests.rs"]
mod route;
