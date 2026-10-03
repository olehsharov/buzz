//! `run_prompt_task` ↔ NIP-SD stream drafts, driven by a scripted ACP agent.
use super::*;
use crate::acp::AcpClient;
use crate::relay::RelayEventPublisher;
use crate::stream_draft::{test_relay::FakeRelay, StreamMode, StreamRuntime};
use nostr::{EventBuilder, Keys, Kind};
use serde_json::json;

fn update(update: serde_json::Value) -> String {
    json!({
        "jsonrpc": "2.0",
        "method": "session/update",
        "params": {"sessionId": "live-session", "update": update},
    })
    .to_string()
}

/// A bash ACP agent that answers its first request (the prompt) with the
/// given session updates, pausing `pause` seconds between them, then
/// `stop_reason`.
fn scripted_agent(updates: &[String], pause: &str, stop_reason: &str) -> String {
    let mut body = String::new();
    for line in updates {
        // JSON lines contain no single quotes, so single-quoting is exact.
        body.push_str(&format!("  printf '%s\\n' '{line}'\n  sleep {pause}\n"));
    }
    format!(
        "count=0\nwhile IFS= read -r line; do\n{body}  printf '%s\\n' \"{{\\\"jsonrpc\\\":\\\"2.0\\\",\\\"id\\\":$count,\\\"result\\\":{{\\\"stopReason\\\":\\\"{stop_reason}\\\"}}}}\"\n  count=$((count + 1))\ndone"
    )
}

async fn run_streamed_turn(
    mode: StreamMode,
    stop_reason: &str,
    relay: &FakeRelay,
    agent_keys: &Keys,
) -> (PromptResult, Vec<nostr::Event>, Uuid, String) {
    let channel_id = Uuid::new_v4();
    let human = Keys::generate();
    let trigger = EventBuilder::new(Kind::Custom(9), "@agent what is the answer?")
        .tags([nostr::Tag::parse(["h", channel_id.to_string().as_str()]).unwrap()])
        .sign_with_keys(&human)
        .unwrap();
    let trigger_id = trigger.id.to_hex();

    let script = scripted_agent(
        &[
            update(json!({"sessionUpdate": "agent_thought_chunk",
                "content": {"type": "text", "text": "SECRET-REASONING"}})),
            update(json!({"sessionUpdate": "agent_message_chunk",
                "content": {"type": "text", "text": "Let me look it up."}})),
            update(json!({"sessionUpdate": "tool_call", "toolCallId": "t1",
                "title": "Read answers.md", "kind": "read", "status": "pending"})),
            update(
                json!({"sessionUpdate": "tool_call_update", "toolCallId": "t1",
                "status": "completed"}),
            ),
            update(json!({"sessionUpdate": "agent_message_chunk",
                "content": {"type": "text", "text": "The answer "}})),
            update(json!({"sessionUpdate": "agent_message_chunk",
                "content": {"type": "text", "text": "is 42."}})),
        ],
        "0.3",
        stop_reason,
    );
    let acp = AcpClient::spawn("bash", &["-c".into(), script], &[], false)
        .await
        .expect("spawn scripted ACP agent");
    let scope = SessionScope::Conversation { channel_id };
    let mut agent = OwnedAgent {
        index: 0,
        acp,
        state: SessionState::default(),
        model_capabilities: None,
        desired_model: None,
        model_overridden: false,
        desired_model_request_id: None,
        desired_model_pending_ack: false,
        startup_effort: None,
        agent_name: "stream-test-agent".into(),
        goose_system_prompt_supported: None,
        protocol_version: 1,
    };
    agent
        .state
        .sessions
        .insert(scope.clone(), "live-session".into());
    agent
        .state
        .deliveries
        .insert(scope.clone(), ChannelDeliveryState::default());

    let (publisher, mut frames_rx) = RelayEventPublisher::test_pair();
    let mut ctx = super::tests::make_prompt_context_no_owner();
    ctx.rest_client.base_url = relay.base_url.clone();
    ctx.channel_info = ChannelInfoResolver::new(
        HashMap::from([(
            channel_id,
            crate::relay::ChannelInfo {
                name: "streams".into(),
                channel_type: "stream".into(),
                description: None,
            },
        )]),
        relay.rest(agent_keys),
    );
    ctx.stream = StreamRuntime::new(mode, publisher, agent_keys.clone());

    let batch = FlushBatch {
        channel_id,
        scope,
        events: vec![crate::queue::BatchEvent {
            edit: None,
            event: trigger,
            prompt_tag: "@mention".into(),
            received_at: std::time::Instant::now(),
        }],
        cancelled_events: vec![],
        cancel_reason: None,
    };
    let (result_tx, mut result_rx) = mpsc::unbounded_channel();
    run_prompt_task(
        agent,
        Some(batch),
        None,
        Arc::new(ctx),
        result_tx,
        None,
        "stream-turn".into(),
    )
    .await;
    let result = result_rx.recv().await.expect("prompt result");

    let mut frames = Vec::new();
    while let Ok(Some(event)) =
        tokio::time::timeout(Duration::from_millis(300), frames_rx.recv()).await
    {
        frames.push(event);
    }
    (result, frames, channel_id, trigger_id)
}

fn tag(event: &nostr::Event, name: &str) -> Option<String> {
    event.tags.iter().find_map(|t| {
        let s = t.as_slice();
        (s.first().map(String::as_str) == Some(name))
            .then(|| s.get(1).cloned())
            .flatten()
    })
}

#[tokio::test]
async fn autopost_turn_streams_drafts_then_posts_the_answer() {
    let relay = FakeRelay::spawn().await;
    let agent_keys = Keys::generate();
    let (mut result, frames, channel_id, trigger_id) =
        run_streamed_turn(StreamMode::DraftAutopost, "end_turn", &relay, &agent_keys).await;
    assert!(matches!(
        result.outcome,
        PromptOutcome::Ok(StopReason::EndTurn)
    ));

    assert!(!frames.is_empty(), "drafts were published");
    let statuses: Vec<String> = frames.iter().filter_map(|f| tag(f, "status")).collect();
    let seqs: Vec<u64> = frames
        .iter()
        .filter_map(|f| tag(f, "seq")?.parse().ok())
        .collect();
    assert_eq!(seqs, (1..=frames.len() as u64).collect::<Vec<_>>());
    assert_eq!(statuses.first().map(String::as_str), Some("thinking"));
    assert_eq!(statuses.last().map(String::as_str), Some("final"));
    let tool = frames
        .iter()
        .find(|f| tag(f, "status").as_deref() == Some("tool"))
        .expect("tool frame");
    assert_eq!(tag(tool, "label").as_deref(), Some("Read answers.md"));
    assert!(frames.iter().any(|f| f.content == "Let me look it up."));
    assert!(
        frames
            .iter()
            .all(|f| !f.content.contains("SECRET-REASONING")),
        "thoughts never reach draft content"
    );
    let stream_id = tag(&frames[0], "stream").expect("stream tag");
    for frame in &frames {
        assert_eq!(
            frame.kind.as_u16() as u32,
            buzz_core::kind::KIND_STREAM_DRAFT
        );
        assert_eq!(tag(frame, "h"), Some(channel_id.to_string()));
        // Human top-level mention → the reply opens a thread on the trigger.
        assert_eq!(tag(frame, "e"), Some(trigger_id.clone()));
        assert_eq!(tag(frame, "stream").as_ref(), Some(&stream_id));
    }

    let posted = relay.posted_messages();
    assert_eq!(posted.len(), 1, "one autoposted reply: {posted:?}");
    let reply = &posted[0];
    assert_eq!(
        reply["content"], "The answer is 42.",
        "text after the last tool call"
    );
    assert_eq!(reply["pubkey"], agent_keys.public_key().to_hex());
    let tags = reply["tags"].as_array().unwrap();
    assert!(tags.contains(&json!(["h", channel_id.to_string()])));
    assert!(tags.contains(&json!(["e", trigger_id, "", "reply"])));
    assert!(tags.contains(&json!(["stream", stream_id])));

    assert!(
        result.agent.acp.stream_sink_is_none(),
        "the returned agent never keeps feeding an ended stream"
    );
    result.agent.acp.shutdown().await;
}

#[tokio::test]
async fn draft_turn_streams_without_posting() {
    let relay = FakeRelay::spawn().await;
    let agent_keys = Keys::generate();
    let (mut result, frames, ..) =
        run_streamed_turn(StreamMode::Draft, "end_turn", &relay, &agent_keys).await;
    assert!(matches!(
        result.outcome,
        PromptOutcome::Ok(StopReason::EndTurn)
    ));
    assert_eq!(
        frames.last().and_then(|f| tag(f, "status")).as_deref(),
        Some("final")
    );
    assert!(frames.iter().any(|f| f.content == "The answer is 42."));
    assert!(relay.posted_messages().is_empty(), "draft mode never posts");
    assert!(
        relay
            .queries
            .lock()
            .unwrap()
            .iter()
            .all(|q| q[0]["kinds"] != json!([9]) || q[0].get("authors").is_none()),
        "no self-reply lookup in draft mode"
    );
    assert!(
        result.agent.acp.stream_sink_is_none(),
        "the returned agent never keeps feeding an ended stream"
    );
    result.agent.acp.shutdown().await;
}

#[tokio::test]
async fn cancelled_turn_abandons_the_draft_without_posting() {
    let relay = FakeRelay::spawn().await;
    let agent_keys = Keys::generate();
    let (mut result, frames, ..) =
        run_streamed_turn(StreamMode::DraftAutopost, "cancelled", &relay, &agent_keys).await;
    assert!(matches!(
        result.outcome,
        PromptOutcome::Ok(StopReason::Cancelled)
    ));
    let last = frames.last().expect("frames");
    assert_eq!(tag(last, "status").as_deref(), Some("abandoned"));
    assert!(last.content.is_empty());
    assert!(
        relay.posted_messages().is_empty(),
        "cancelled turns never autopost"
    );
    result.agent.acp.shutdown().await;
}
