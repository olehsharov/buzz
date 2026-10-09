//! Removing an agent from its machine: the frame goes to the relay the
//! machine listens on, and every failure is left on the agent record.

use super::*;
use crate::commands::{undeploy_before_delete, HOST_UNDEPLOY_FAILED_PREFIX};

const OTHER_COMMUNITY: &str = "wss://other-community.example";
const SECOND_APPROVAL: &str = "wss://second-community.example";

impl Fixture {
    fn approve_host_a_also_on(&self, relay: &str) {
        add_approved_host(
            self.app.handle(),
            &HostOps::default(),
            AgentHostRecord {
                pubkey: self.host_a.public_key().to_hex(),
                name: "alpha-second".into(),
                os: "linux".into(),
                arch: "x86_64".into(),
                relay_url: relay.into(),
                added_at: "2026-01-01T00:00:00Z".into(),
                status: None,
            },
        )
        .unwrap();
    }

    fn set_agent_relay(&self, relay: &str) {
        let mut records = load_managed_agents(self.app.handle()).unwrap();
        records[0].relay_url = relay.into();
        save_managed_agents(self.app.handle(), &records).unwrap();
    }
}

#[tokio::test]
async fn undeploy_targets_the_relay_the_machine_was_approved_on() {
    let fx = Fixture::new();
    fx.place_on(&fx.host_a);
    let state = fx.app.state::<AppState>();
    let ops = HostOps::default();
    let fake = fx.fake(vec![Reply::AckOk]);
    let mut dialed = Vec::new();

    // The command runs in a window showing another community.
    undeploy_agent_via_its_host(
        fx.app.handle(),
        &state,
        &ops,
        OTHER_COMMUNITY,
        &fx.agent(),
        |route| {
            dialed.push(route.relay_url.clone());
            Ok(&fake)
        },
    )
    .await
    .unwrap();

    assert_eq!(dialed, vec![RELAY.to_string()]);
    assert_eq!(fake.types()[0].1, "host.undeploy");
    assert_eq!(deployed_on(&fx.record()), None);
}

#[tokio::test]
async fn a_machine_approved_twice_is_reached_through_the_agents_community() {
    let fx = Fixture::new();
    fx.approve_host_a_also_on(SECOND_APPROVAL);
    fx.place_on(&fx.host_a);
    fx.set_agent_relay(SECOND_APPROVAL);
    let route = deployed_host_route(
        fx.app.handle(),
        &HostOps::default(),
        &fx.record(),
        OTHER_COMMUNITY,
    )
    .unwrap()
    .unwrap();
    assert_eq!(route.relay_url, SECOND_APPROVAL);
    assert_eq!(route.host_name, "alpha-second");
}

#[tokio::test(start_paused = true)]
async fn a_silent_machine_keeps_the_agent_and_records_why() {
    let fx = Fixture::new();
    fx.place_on(&fx.host_a);
    let fake = fx.fake(vec![Reply::Silent]);

    // The production delete seam: the error carries the prefix the UI keys
    // its "delete anyway" offer on, and names the machine.
    let error = undeploy_before_delete(fx.app.handle(), RELAY, &fx.agent(), |_| Ok(&fake))
        .await
        .unwrap_err();
    assert!(error.starts_with(HOST_UNDEPLOY_FAILED_PREFIX), "{error}");
    assert!(error.contains("alpha did not confirm"), "{error}");

    let record = fx.record();
    assert_eq!(deployed_on(&record), Some(fx.host_a.public_key().to_hex()));
    let last_error = record.last_error.expect("failure recorded on the agent");
    assert!(
        last_error.starts_with("Removing the agent from its machine failed"),
        "{last_error}"
    );
}

#[tokio::test]
async fn a_forgotten_machine_fails_fast_and_is_recorded() {
    let fx = Fixture::new();
    fx.place_on(&fx.host_a);
    {
        let ops = HostOps::default();
        let _guard = ops.store_lock.lock().unwrap();
        let dir = hosts_dir(fx.app.handle()).unwrap();
        let mut hosts = store::load_hosts(&dir).unwrap();
        store::remove_host(&mut hosts, RELAY, &fx.host_a.public_key().to_hex());
        store::save_hosts(&dir, &hosts).unwrap();
    }
    let fake = fx.fake(vec![]);
    let error = undeploy_before_delete(fx.app.handle(), RELAY, &fx.agent(), |_| Ok(&fake))
        .await
        .unwrap_err();
    assert!(error.starts_with(HOST_UNDEPLOY_FAILED_PREFIX), "{error}");
    assert!(error.contains("no longer approved"), "{error}");
    assert!(fake.types().is_empty());
    assert!(fx
        .record()
        .last_error
        .unwrap_or_default()
        .contains("no longer approved"));
}
