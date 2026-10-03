//! `@all` send-path tests: drive `cmd_send_message` through a fake relay
//! that answers `/query` by filter kind and captures `/events`.

use std::sync::{Arc, Mutex};

use axum::body::Bytes;
use axum::extract::State;
use axum::routing::post;
use axum::Router;
use nostr::{EventBuilder, Keys, Kind, Tag};
use serde_json::{json, Value};
use tokio::net::TcpListener;

use super::{cmd_send_message, SendMessageParams};
use crate::client::BuzzClient;
use crate::error::{exit_code, CliError};

const CHANNEL: &str = "123e4567-e89b-12d3-a456-426614174000";

#[derive(Default)]
struct Relay {
    roster: Value,
    profiles: Vec<Value>,
    metadata: Value,
    /// First `kinds` entry of every `/query` filter, in call order.
    queried_kinds: Vec<u64>,
    submitted: Option<Value>,
}

type Shared = Arc<Mutex<Relay>>;

async fn serve(relay: Relay) -> (String, Shared) {
    let shared: Shared = Arc::new(Mutex::new(relay));
    let app = Router::new()
        .route(
            "/query",
            post(|State(s): State<Shared>, body: Bytes| async move {
                let filters: Vec<Value> = serde_json::from_slice(&body).unwrap();
                let kind = filters[0]["kinds"][0].as_u64().unwrap();
                let mut relay = s.lock().unwrap();
                relay.queried_kinds.push(kind);
                let out = match kind {
                    39002 => json!([relay.roster.clone()]),
                    39000 => json!([relay.metadata.clone()]),
                    0 => json!(relay.profiles.clone()),
                    _ => json!([]),
                };
                ([("content-type", "application/json")], out.to_string())
            }),
        )
        .route(
            "/events",
            post(|State(s): State<Shared>, body: Bytes| async move {
                s.lock().unwrap().submitted = Some(serde_json::from_slice(&body).unwrap());
                (
                    [("content-type", "application/json")],
                    r#"{"event_id":"fake","accepted":true}"#,
                )
            }),
        )
        .with_state(shared.clone());
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (format!("http://{addr}"), shared)
}

fn profile(keys: &Keys, name: &str, tags: Vec<Tag>) -> Value {
    let event = EventBuilder::new(Kind::Metadata, json!({ "name": name }).to_string())
        .tags(tags)
        .sign_with_keys(keys)
        .unwrap();
    serde_json::to_value(event).unwrap()
}

fn auth_tag_for(agent: &Keys) -> Tag {
    let json =
        buzz_sdk::nip_oa::compute_auth_tag(&Keys::generate(), &agent.public_key(), "").unwrap();
    Tag::parse(serde_json::from_str::<Vec<String>>(&json).unwrap()).unwrap()
}

fn metadata(channel_type: &str) -> Value {
    json!({ "kind": 39000, "tags": [["d", CHANNEL], ["t", channel_type]] })
}

/// Channel with: sender (owner), alice + bob (humans), a `bot`-role member
/// whose profile is literally named "all", and an attested agent with the
/// plain `member` role.
struct Fixture {
    sender: Keys,
    alice: String,
    bob: String,
    bot_named_all: String,
    agent: String,
}

fn fixture(channel_type: &str) -> (Fixture, Relay) {
    let (sender, alice, bob, bot, agent) = (
        Keys::generate(),
        Keys::generate(),
        Keys::generate(),
        Keys::generate(),
        Keys::generate(),
    );
    let hex = |k: &Keys| k.public_key().to_hex();
    let relay = Relay {
        roster: json!({ "kind": 39002, "tags": [
            ["d", CHANNEL],
            ["p", hex(&sender), "", "owner"],
            ["p", hex(&alice), "", "member"],
            ["p", hex(&bob)],
            ["p", hex(&bot), "", "bot"],
            ["p", hex(&agent), "", "member"],
        ]}),
        profiles: vec![
            profile(&sender, "me", vec![]),
            profile(&alice, "alice", vec![]),
            profile(&bob, "bob", vec![]),
            profile(&bot, "all", vec![]),
            profile(&agent, "helper", vec![auth_tag_for(&agent)]),
        ],
        metadata: metadata(channel_type),
        ..Relay::default()
    };
    (
        Fixture {
            alice: hex(&alice),
            bob: hex(&bob),
            bot_named_all: hex(&bot),
            agent: hex(&agent),
            sender,
        },
        relay,
    )
}

fn params(content: &str) -> SendMessageParams {
    SendMessageParams {
        channel_id: CHANNEL.into(),
        content: content.into(),
        kind: None,
        reply_to: None,
        broadcast: false,
        files: vec![],
        mentions: vec![],
        mention_all: false,
    }
}

fn tag_values(event: &Value, name: &str) -> Vec<Vec<String>> {
    event["tags"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| {
            t.as_array()
                .unwrap()
                .iter()
                .map(|v| v.as_str().unwrap().to_string())
                .collect::<Vec<_>>()
        })
        .filter(|t| t[0] == name)
        .collect()
}

fn p_tags(event: &Value) -> Vec<String> {
    tag_values(event, "p")
        .into_iter()
        .map(|t| t[1].clone())
        .collect()
}

fn has_marker(event: &Value) -> bool {
    tag_values(event, "buzz:mention-group") == vec![vec!["buzz:mention-group", "all"]]
}

async fn send(fx: &Fixture, relay: Relay, p: SendMessageParams) -> (Result<(), CliError>, Shared) {
    let (url, shared) = serve(relay).await;
    let client = BuzzClient::new(url, fx.sender.clone(), None, None).unwrap();
    (cmd_send_message(&client, p).await, shared)
}

#[tokio::test]
async fn at_all_token_mentions_humans_only_with_marker() {
    let (fx, relay) = fixture("stream");
    let (res, shared) = send(&fx, relay, params("@all standup in 5")).await;
    res.unwrap();
    let event = shared.lock().unwrap().submitted.clone().unwrap();
    assert_eq!(p_tags(&event), vec![fx.alice.clone(), fx.bob.clone()]);
    assert!(has_marker(&event));
    assert_eq!(event["content"], "@all standup in 5");
    // Neither the bot literally named "all" nor the attested agent is tagged.
    assert!(!p_tags(&event).contains(&fx.bot_named_all));
    assert!(!p_tags(&event).contains(&fx.agent));
}

#[tokio::test]
async fn at_all_tokenizes_with_trailing_punctuation_and_case() {
    let (fx, mut relay) = fixture("forum");
    // Drop the member literally named "all" so tokenization relies on the
    // reserved token alone (fallback tokenization would read "all.").
    relay.profiles.remove(3);
    let (res, shared) = send(&fx, relay, params("Heads up, please read @ALL.")).await;
    res.unwrap();
    let event = shared.lock().unwrap().submitted.clone().unwrap();
    assert_eq!(p_tags(&event), vec![fx.alice.clone(), fx.bob.clone()]);
    assert!(has_marker(&event));
}

#[tokio::test]
async fn at_all_dedupes_with_explicit_mentions_and_named_mentions() {
    let (fx, relay) = fixture("stream");
    let mut p = params("@bob and @all");
    p.mentions = vec![fx.alice.clone(), fx.agent.clone()];
    let (res, shared) = send(&fx, relay, p).await;
    res.unwrap();
    let event = shared.lock().unwrap().submitted.clone().unwrap();
    // Explicit first (an explicit agent stays), then name-resolved, then @all.
    assert_eq!(
        p_tags(&event),
        vec![fx.alice.clone(), fx.agent.clone(), fx.bob.clone()]
    );
    assert!(has_marker(&event));
}

#[tokio::test]
async fn mention_all_flag_works_without_token_and_keeps_content() {
    let (fx, relay) = fixture("stream");
    let mut p = params("standup in 5");
    p.mention_all = true;
    let (res, shared) = send(&fx, relay, p).await;
    res.unwrap();
    let event = shared.lock().unwrap().submitted.clone().unwrap();
    assert_eq!(p_tags(&event), vec![fx.alice.clone(), fx.bob.clone()]);
    assert!(has_marker(&event));
    assert_eq!(event["content"], "standup in 5");
}

#[tokio::test]
async fn at_all_in_dm_is_rejected_before_submit() {
    for flag in [false, true] {
        let (fx, relay) = fixture("dm");
        let mut p = params(if flag { "hi" } else { "@all hi" });
        p.mention_all = flag;
        let (res, shared) = send(&fx, relay, p).await;
        let err = res.unwrap_err();
        assert_eq!(exit_code(&err), 1, "{err:?}");
        assert!(err.to_string().contains("not supported in DMs"), "{err}");
        assert!(shared.lock().unwrap().submitted.is_none());
    }
}

#[tokio::test]
async fn at_all_inside_code_is_plain_text() {
    let (fx, relay) = fixture("dm");
    let (res, shared) = send(&fx, relay, params("run `notify @all` or\n```\n@all\n```")).await;
    res.unwrap();
    let relay = shared.lock().unwrap();
    let event = relay.submitted.clone().unwrap();
    assert!(p_tags(&event).is_empty());
    assert!(!has_marker(&event));
    // No metadata lookup either: the DM fixture would otherwise reject.
    assert!(!relay.queried_kinds.contains(&39000));
}

#[tokio::test]
async fn named_mention_without_at_all_has_no_marker() {
    let (fx, relay) = fixture("stream");
    let (res, shared) = send(&fx, relay, params("@alice ping; mail team@all.example")).await;
    res.unwrap();
    let event = shared.lock().unwrap().submitted.clone().unwrap();
    assert_eq!(p_tags(&event), vec![fx.alice.clone()]);
    assert!(!has_marker(&event));
}

#[tokio::test]
async fn at_all_prefix_is_an_ordinary_unknown_name() {
    // `@all-hands` is not the group token; it follows the normal
    // unknown-@Name rule and fails visibly instead of notifying everyone.
    let (fx, relay) = fixture("stream");
    let (res, shared) = send(&fx, relay, params("@all-hands later")).await;
    let err = res.unwrap_err();
    assert!(
        err.to_string().contains("'@all-hands' does not match"),
        "{err}"
    );
    assert!(shared.lock().unwrap().submitted.is_none());
}

#[tokio::test]
async fn more_than_cap_humans_fails_with_count() {
    let sender = Keys::generate();
    let mut tags = vec![
        json!(["d", CHANNEL]),
        json!(["p", sender.public_key().to_hex()]),
    ];
    for _ in 0..51 {
        tags.push(json!(["p", Keys::generate().public_key().to_hex()]));
    }
    let relay = Relay {
        roster: json!({ "kind": 39002, "tags": tags }),
        metadata: metadata("stream"),
        ..Relay::default()
    };
    let fx = Fixture {
        sender,
        alice: String::new(),
        bob: String::new(),
        bot_named_all: String::new(),
        agent: String::new(),
    };
    let (res, shared) = send(&fx, relay, params("@all hello")).await;
    let err = res.unwrap_err();
    assert_eq!(exit_code(&err), 1);
    assert!(err.to_string().contains("51 human members"), "{err}");
    assert!(err.to_string().contains("limit of 50"), "{err}");
    assert!(shared.lock().unwrap().submitted.is_none());
}

#[tokio::test]
async fn at_all_applies_to_forum_posts() {
    let (fx, relay) = fixture("forum");
    let mut p = params("@all new RFC");
    p.kind = Some(45001);
    let (res, shared) = send(&fx, relay, p).await;
    res.unwrap();
    let event = shared.lock().unwrap().submitted.clone().unwrap();
    assert_eq!(event["kind"], 45001);
    assert_eq!(p_tags(&event), vec![fx.alice.clone(), fx.bob.clone()]);
    assert!(has_marker(&event));
}
