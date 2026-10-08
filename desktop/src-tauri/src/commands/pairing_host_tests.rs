//! Host approval over a real NIP-AB session pair: the desktop (source) runs
//! the production `host_hello_reply`, the test plays the machine (target).

use super::*;
use buzz_core_pkg::pairing::session::SessionState;

fn approved_pair() -> (PairingSession, PairingSession) {
    let (mut source, qr) = PairingSession::new_source("wss://pair.example".into());
    let (mut target, offer) = PairingSession::new_target(&qr).unwrap();
    source.handle_offer(&offer).unwrap();
    let sas_confirm = source.confirm_sas().unwrap();
    target.handle_sas_confirm(&sas_confirm).unwrap();
    target.confirm_target_sas().unwrap();
    (source, target)
}

fn hello(host: &nostr::PublicKey) -> String {
    serde_json::json!({
        "type": "buzz-host-hello", "v": 1, "host_pubkey": host.to_hex(),
        "name": "devbox", "os": "linux", "arch": "x86_64", "version": "0.1.0"
    })
    .to_string()
}

#[test]
fn hello_is_answered_with_a_grant_the_machine_can_verify() {
    let owner = nostr::Keys::generate();
    let host = nostr::Keys::generate();
    let (mut source, mut target) = approved_pair();

    let hello_event = target
        .send_return_payload(
            PayloadType::Custom,
            Zeroizing::new(hello(&host.public_key())),
        )
        .unwrap();
    let (payload_type, payload) = source.handle_return_payload(&hello_event).unwrap();
    let (parsed, reply) = host_hello_reply(
        &mut source,
        payload_type,
        &payload,
        &owner,
        "wss://community.example",
    )
    .unwrap();
    assert_eq!(parsed.name, "devbox");
    assert_eq!(parsed.host_pubkey, host.public_key().to_hex());

    // The machine receives and validates the grant.
    let (grant_type, grant) = target.handle_payload(&reply).unwrap();
    assert_eq!(grant_type, PayloadType::Custom);
    let grant: serde_json::Value = serde_json::from_str(&grant).unwrap();
    assert_eq!(grant["type"], "buzz-host-grant");
    assert_eq!(grant["v"], 1);
    assert_eq!(grant["owner_pubkey"], owner.public_key().to_hex());
    assert_eq!(grant["relay_url"], "wss://community.example");
    let verified = buzz_sdk_pkg::nip_oa::verify_auth_tag(
        grant["auth_tag"].as_str().unwrap(),
        &host.public_key(),
    )
    .unwrap();
    assert_eq!(verified, owner.public_key());

    // The machine completes; the desktop's session accepts it.
    let complete = target.send_complete().unwrap();
    source.handle_complete(&complete).unwrap();
    assert_eq!(source.state(), SessionState::Completed);

    let record = approved_host_record(&parsed, "wss://community.example");
    assert_eq!(record.pubkey, host.public_key().to_hex());
    assert_eq!(record.relay_url, "wss://community.example");
    assert_eq!(
        (record.os.as_str(), record.arch.as_str()),
        ("linux", "x86_64")
    );
}

#[test]
fn non_custom_or_malformed_hello_gets_no_grant() {
    let owner = nostr::Keys::generate();
    let host = nostr::Keys::generate();

    let (mut source, mut target) = approved_pair();
    let event = target
        .send_return_payload(PayloadType::Nsec, Zeroizing::new(hello(&host.public_key())))
        .unwrap();
    let (payload_type, payload) = source.handle_return_payload(&event).unwrap();
    assert!(host_hello_reply(&mut source, payload_type, &payload, &owner, "wss://c").is_err());

    let (mut source, mut target) = approved_pair();
    let event = target
        .send_return_payload(
            PayloadType::Custom,
            Zeroizing::new("{\"type\":\"other\"}".into()),
        )
        .unwrap();
    let (payload_type, payload) = source.handle_return_payload(&event).unwrap();
    assert!(host_hello_reply(&mut source, payload_type, &payload, &owner, "wss://c").is_err());
}
