//! The agent's folder on its machine: carried in `host.deploy`, kept on a
//! move, and changed for an existing agent with one save plus a redeploy.

use super::*;

const FOLDER: &str = "~/projects/agent-work";

impl Fixture {
    fn place_on_with_folder(&self, host: &Keys, workdir: Option<&str>) {
        let mut records = load_managed_agents(self.app.handle()).unwrap();
        records[0].backend = BackendKind::Host {
            host_pubkey: host.public_key().to_hex(),
            workdir: workdir.map(str::to_string),
        };
        records[0].backend_agent_id = Some(host.public_key().to_hex());
        save_managed_agents(self.app.handle(), &records).unwrap();
    }

    /// Like [`Fixture::payload`], but carrying the record's own access
    /// policy, as `build_deploy_payload` does.
    fn policy_payload(
        &self,
    ) -> impl FnOnce(&ManagedAgentRecord) -> Result<serde_json::Value, String> {
        let nsec = self.agent_nsec.clone();
        move |record: &ManagedAgentRecord| {
            Ok(serde_json::json!({
                "relay_url": RELAY,
                "private_key_nsec": nsec,
                "launch": {"command": "goose", "args": ["acp"], "env": {}, "policy_env": {}},
                "respond_to": record.respond_to.as_str(),
                "respond_to_allowlist": record.respond_to_allowlist,
            }))
        }
    }

    async fn set_folder(
        &self,
        ops: &HostOps,
        channel: &FakeHosts,
        workdir: Option<&str>,
    ) -> Result<(), String> {
        let state = self.app.state::<AppState>();
        set_host_agent_workdir(
            self.app.handle(),
            &state,
            ops,
            channel,
            &self.agent(),
            workdir,
            RELAY,
            self.policy_payload(),
        )
        .await
    }
}

fn saved_folder(record: &ManagedAgentRecord) -> Option<String> {
    host_workdir(record).map(str::to_string)
}

#[tokio::test]
async fn deploy_frame_carries_the_saved_folder() {
    let fx = Fixture::new();
    fx.place_on_with_folder(&fx.host_a, Some(FOLDER));
    let ops = HostOps::default();
    let fake = fx.fake(vec![Reply::AckOk]);

    fx.deploy(&ops, &fake, &fx.host_a).await.unwrap();

    assert_eq!(fake.received.lock().unwrap()[0].1["workdir"], FOLDER);
    // The acknowledged deploy keeps the folder on the record.
    assert_eq!(saved_folder(&fx.record()).as_deref(), Some(FOLDER));
}

#[tokio::test]
async fn move_to_another_machine_carries_the_folder() {
    let fx = Fixture::new();
    fx.place_on_with_folder(&fx.host_a, Some(FOLDER));
    let ops = HostOps::default();
    let fake = fx.fake(vec![Reply::AckOk, Reply::AckOk]);

    fx.deploy(&ops, &fake, &fx.host_b).await.unwrap();

    let received = fake.received.lock().unwrap();
    assert_eq!(received[0].1["type"], "host.undeploy");
    assert_eq!(received[1].0, fx.host_b.public_key().to_hex());
    assert_eq!(received[1].1["workdir"], FOLDER);
    let record = fx.record();
    assert_eq!(deployed_on(&record), Some(fx.host_b.public_key().to_hex()));
    assert_eq!(saved_folder(&record).as_deref(), Some(FOLDER));
}

#[tokio::test]
async fn changing_the_folder_saves_it_and_redeploys_into_it() {
    let fx = Fixture::new();
    fx.place_on_with_folder(&fx.host_a, None);
    let ops = HostOps::default();
    let (release, gate) = oneshot::channel();
    let fake = fx.fake(vec![Reply::Gated(gate)]);

    let padded = format!("  {FOLDER} ");
    let change = fx.set_folder(&ops, &fake, Some(&padded));
    tokio::pin!(change);
    tokio::select! {
        _ = &mut change => panic!("change finished before the machine answered"),
        _ = fake.frame_arrived.notified() => {}
    }
    // Saved (and marked for redeploy) before the machine answers.
    let record = fx.record();
    assert_eq!(saved_folder(&record).as_deref(), Some(FOLDER));
    assert!(record.provider_policy_pending);

    release.send(()).unwrap();
    change.await.unwrap();
    let received = fake.received.lock().unwrap();
    assert_eq!(received.len(), 1);
    assert_eq!(received[0].0, fx.host_a.public_key().to_hex());
    assert_eq!(received[0].1["type"], "host.deploy");
    assert_eq!(received[0].1["workdir"], FOLDER);
    let record = fx.record();
    assert_eq!(saved_folder(&record).as_deref(), Some(FOLDER));
    assert!(!record.provider_policy_pending, "the ack delivered it");
    assert_eq!(record.last_error, None);
}

#[tokio::test]
async fn clearing_the_folder_redeploys_with_the_machine_default() {
    let fx = Fixture::new();
    fx.place_on_with_folder(&fx.host_a, Some(FOLDER));
    let ops = HostOps::default();
    let fake = fx.fake(vec![Reply::AckOk]);

    fx.set_folder(&ops, &fake, Some("   ")).await.unwrap();

    assert!(fake.received.lock().unwrap()[0].1["workdir"].is_null());
    assert_eq!(saved_folder(&fx.record()), None);
}

#[tokio::test]
async fn a_refused_redeploy_keeps_the_new_folder_pending_with_the_error() {
    let fx = Fixture::new();
    fx.place_on_with_folder(&fx.host_a, None);
    let ops = HostOps::default();
    let fake = fx.fake(vec![Reply::AckErr("permission denied")]);

    let error = fx.set_folder(&ops, &fake, Some(FOLDER)).await.unwrap_err();

    assert!(error.contains("The folder was saved"), "{error}");
    assert!(error.contains("permission denied"), "{error}");
    let record = fx.record();
    assert_eq!(saved_folder(&record).as_deref(), Some(FOLDER));
    assert!(record.provider_policy_pending, "retried on the next load");
    assert!(record
        .last_error
        .as_deref()
        .is_some_and(|last| last.contains("permission denied")));
}

/// A redeploy that fails before any frame goes out (here: the payload
/// cannot be built) is still recorded on the agent.
#[tokio::test]
async fn a_redeploy_that_fails_before_sending_records_the_error() {
    let fx = Fixture::new();
    fx.place_on_with_folder(&fx.host_a, None);
    let ops = HostOps::default();
    let fake = fx.fake(vec![]);
    let state = fx.app.state::<AppState>();

    let error = set_host_agent_workdir(
        fx.app.handle(),
        &state,
        &ops,
        &fake,
        &fx.agent(),
        Some(FOLDER),
        RELAY,
        |_record: &ManagedAgentRecord| Err("harness catalog unavailable".to_string()),
    )
    .await
    .unwrap_err();

    assert!(error.contains("harness catalog unavailable"), "{error}");
    assert!(fake.received.lock().unwrap().is_empty());
    let record = fx.record();
    assert_eq!(saved_folder(&record).as_deref(), Some(FOLDER));
    assert!(record.provider_policy_pending);
    assert_eq!(
        record.last_error.as_deref(),
        Some("harness catalog unavailable")
    );
}

#[tokio::test]
async fn an_undeployed_agent_saves_the_folder_without_contacting_a_machine() {
    let fx = Fixture::new();
    fx.place_on_with_folder(&fx.host_a, None);
    {
        let mut records = load_managed_agents(fx.app.handle()).unwrap();
        records[0].backend_agent_id = None;
        save_managed_agents(fx.app.handle(), &records).unwrap();
    }
    let ops = HostOps::default();
    let fake = fx.fake(vec![]);

    fx.set_folder(&ops, &fake, Some(FOLDER)).await.unwrap();

    assert!(fake.received.lock().unwrap().is_empty());
    let record = fx.record();
    assert_eq!(saved_folder(&record).as_deref(), Some(FOLDER));
    assert!(!record.provider_policy_pending);
}

#[tokio::test]
async fn saving_the_same_folder_or_a_bad_one_changes_nothing() {
    let fx = Fixture::new();
    fx.place_on_with_folder(&fx.host_a, Some(FOLDER));
    let ops = HostOps::default();
    let fake = fx.fake(vec![]);

    fx.set_folder(&ops, &fake, Some(FOLDER)).await.unwrap();
    assert!(fx.set_folder(&ops, &fake, Some("a\nb")).await.is_err());

    assert!(fake.received.lock().unwrap().is_empty());
    let record = fx.record();
    assert_eq!(saved_folder(&record).as_deref(), Some(FOLDER));
    assert!(!record.provider_policy_pending);
}

#[tokio::test]
async fn only_agents_on_a_machine_have_a_machine_folder() {
    let fx = Fixture::new();
    let ops = HostOps::default();
    let fake = fx.fake(vec![]);

    let error = fx.set_folder(&ops, &fake, Some(FOLDER)).await.unwrap_err();

    assert!(error.contains("Only an agent on a machine"), "{error}");
    assert_eq!(fx.record().backend, BackendKind::Local);
}

/// A folder saved while an older deploy is in flight is neither overwritten
/// nor acknowledged by that deploy's ack: it stays pending for its own
/// redeploy.
#[tokio::test]
async fn an_in_flight_deploy_does_not_clobber_a_newer_folder() {
    let fx = Fixture::new();
    fx.place_on_with_folder(&fx.host_a, None);
    let ops = HostOps::default();
    let (release, gate) = oneshot::channel();
    let fake = fx.fake(vec![Reply::Gated(gate)]);
    let state = fx.app.state::<AppState>();
    let agent = fx.agent();
    let host = fx.host_a.public_key().to_hex();

    let deploy = deploy_agent_to_host(
        fx.app.handle(),
        &state,
        &ops,
        &fake,
        &agent,
        &host,
        RELAY,
        fx.policy_payload(),
    );
    tokio::pin!(deploy);
    tokio::select! {
        _ = &mut deploy => panic!("deploy finished before the machine answered"),
        _ = fake.frame_arrived.notified() => {}
    }
    // The owner saves a new folder while the old frame is in flight.
    fx.place_on_with_folder(&fx.host_a, Some(FOLDER));
    {
        let mut records = load_managed_agents(fx.app.handle()).unwrap();
        records[0].provider_policy_pending = true;
        save_managed_agents(fx.app.handle(), &records).unwrap();
    }

    release.send(()).unwrap();
    deploy.await.unwrap();
    assert!(fake.received.lock().unwrap()[0].1["workdir"].is_null());
    let record = fx.record();
    assert_eq!(saved_folder(&record).as_deref(), Some(FOLDER));
    assert!(
        record.provider_policy_pending,
        "the new folder is undelivered"
    );
}
