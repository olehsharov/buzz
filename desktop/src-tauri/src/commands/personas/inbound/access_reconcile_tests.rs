//! Production-seam regression: a cross-device access edit (kind:30177) for a
//! deployed provider agent, driven through the real inbound entrypoint
//! `reconcile_inbound_persona_event_blocking`, is saved with the redeploy
//! marked pending in the same write and hands back a provider redeploy.

use nostr::JsonUtil;

use super::{reconcile_inbound_persona_event_blocking, InboundRuntimeRefresh};
use crate::commands::agents::{
    access_transition::RemoteAccessRedeploy,
    scripted_provider_fixture::{ScriptedProvider, FIXTURE_RELAY},
};
use crate::managed_agents::{agent_events::agent_event_content, RespondTo};

#[test]
fn inbound_access_edit_of_a_deployed_provider_agent_is_pending_and_redeployed() {
    if crate::managed_agents::owner_only_access_build() {
        // Owner-only builds never change the projected policy.
        return;
    }
    let fixture = ScriptedProvider::new(r#"{"ok":true,"agent_id":"unused"}"#);
    let owner = nostr::Keys::generate();
    *fixture.state().keys.lock().unwrap() = owner.clone();
    let (agent, nsec) = fixture.deployed_agent();
    assert_eq!(agent.respond_to, RespondTo::OwnerOnly);
    let pubkey = agent.pubkey.clone();
    fixture.save(&[(agent.clone(), nsec)]);

    // Another device of the same owner opened the agent to anyone.
    let mut edited = agent;
    edited.respond_to = RespondTo::Anyone;
    let event = nostr::EventBuilder::new(
        nostr::Kind::Custom(30177),
        serde_json::to_string(&agent_event_content(&edited)).unwrap(),
    )
    .tags(vec![nostr::Tag::parse(["d", pubkey.as_str()]).unwrap()])
    .sign_with_keys(&owner)
    .unwrap();

    let refresh = reconcile_inbound_persona_event_blocking(
        event.as_json(),
        FIXTURE_RELAY.to_string(),
        FIXTURE_RELAY.to_string(),
        fixture.app.handle().clone(),
    )
    .expect("the inbound access edit applies");

    match refresh {
        Some(InboundRuntimeRefresh::Remote {
            pubkey: refreshed,
            target: RemoteAccessRedeploy::Provider,
        }) => assert_eq!(refreshed, pubkey),
        other => panic!("a deployed provider agent must be redeployed, got {other:?}"),
    }
    let saved = fixture.load(&pubkey);
    assert_eq!(saved.respond_to, RespondTo::Anyone);
    assert!(
        saved.provider_policy_pending,
        "pending is saved with the inbound policy"
    );
}
