//! Control-signal cancels in `run_prompt_task`, driven by a scripted ACP agent:
//! which cancels keep the scope's provider session, and what the re-prompt of a
//! kept session looks like.
use super::*;
use crate::acp::AcpClient;
use nostr::{EventBuilder, Keys, Kind};
use tests::make_prompt_context_no_owner;

fn conv(channel_id: Uuid) -> SessionScope {
    SessionScope::Conversation { channel_id }
}

/// Scripted ACP for control-cancel tests. `session/new` mints
/// `new-<id>`. The first `session/prompt` streams one text chunk and stays
/// in flight; every later prompt ends its turn. `session/cancel` runs
/// `on_cancel` with `$pending` set to the in-flight prompt's id. Every
/// request line is appended to the returned capture file.
async fn spawn_cancel_acp(on_cancel: &str) -> (AcpClient, std::path::PathBuf) {
    let capture =
        std::env::temp_dir().join(format!("buzz-acp-control-cancel-{}.ndjson", Uuid::new_v4()));
    let quoted_capture = capture.to_string_lossy().replace('\'', "'\\''");
    let script = format!(
        r#"started=""
pending=""
sid=""
while IFS= read -r line; do
  printf '%s\n' "$line" >> '{quoted_capture}'
  id=$(printf '%s' "$line" | sed -n 's/.*"id":\([0-9][0-9]*\).*/\1/p')
  case "$line" in
*'"method":"session/new"'*)
  sid="new-$id"
  printf '%s\n' '{{"jsonrpc":"2.0","id":'"$id"',"result":{{"sessionId":"'"$sid"'"}}}}' ;;
*'"method":"session/prompt"'*)
  if [ -z "$started" ]; then
    started=1
    pending=$id
    printf '%s\n' '{{"jsonrpc":"2.0","method":"session/update","params":{{"sessionId":"'"$sid"'","update":{{"sessionUpdate":"agent_message_chunk","content":{{"type":"text","text":"partial answer"}}}}}}}}'
  else
    printf '%s\n' '{{"jsonrpc":"2.0","id":'"$id"',"result":{{"stopReason":"end_turn"}}}}'
  fi ;;
*'"method":"session/cancel"'*) {on_cancel} ;;
  esac
done"#
    );
    let acp = AcpClient::spawn("bash", &["-c".to_string(), script], &[], false)
        .await
        .expect("spawn control-cancel ACP script");
    (acp, capture)
}

const CANCEL_CLEAN: &str =
    r#"printf '%s\n' '{"jsonrpc":"2.0","id":'"$pending"',"result":{"stopReason":"cancelled"}}'"#;

fn captured_requests(capture: &std::path::Path) -> Vec<serde_json::Value> {
    std::fs::read_to_string(capture)
        .unwrap_or_default()
        .lines()
        .map(|line| serde_json::from_str(line).expect("captured request is JSON"))
        .collect()
}

fn requests_for<'a>(requests: &'a [serde_json::Value], method: &str) -> Vec<&'a serde_json::Value> {
    requests.iter().filter(|r| r["method"] == method).collect()
}

fn prompt_request_text(request: &serde_json::Value) -> String {
    request["params"]["prompt"]
        .as_array()
        .expect("prompt blocks")
        .iter()
        .filter_map(|block| block["text"].as_str())
        .collect::<Vec<_>>()
        .join("\n\n")
}

fn cancel_test_agent(acp: AcpClient) -> OwnedAgent {
    OwnedAgent {
        index: 0,
        acp,
        state: SessionState::default(),
        model_capabilities: None,
        desired_model: None,
        model_overridden: false,
        desired_model_request_id: None,
        desired_model_pending_ack: false,
        startup_effort: None,
        agent_name: "legacy-test-agent".into(),
        goose_system_prompt_supported: None,
        protocol_version: 1,
    }
}

fn single_event_batch(channel_id: Uuid, content: &str) -> FlushBatch {
    FlushBatch {
        channel_id,
        scope: conv(channel_id),
        events: vec![crate::queue::BatchEvent {
            edit: None,
            event: EventBuilder::new(Kind::Custom(9), content)
                .sign_with_keys(&Keys::generate())
                .unwrap(),
            prompt_tag: "@mention".into(),
            received_at: std::time::Instant::now(),
        }],
        cancelled_events: vec![],
        cancel_reason: None,
    }
}

/// Prompt context whose relay is `relay` and that already knows
/// `channel_id` as a stream channel, so turns reach the agent promptly.
fn cancel_test_ctx(
    relay: &crate::stream_draft::test_relay::FakeRelay,
    channel_id: Uuid,
) -> PromptContext {
    let keys = Keys::generate();
    let mut ctx = make_prompt_context_no_owner();
    ctx.dedup_mode = DedupMode::Queue;
    ctx.rest_client.base_url = relay.base_url.clone();
    ctx.channel_info = ChannelInfoResolver::new(
        HashMap::from([(
            channel_id,
            crate::relay::ChannelInfo {
                name: "cancels".into(),
                channel_type: "stream".into(),
                description: None,
            },
        )]),
        relay.rest(&keys),
    );
    ctx
}

/// Run one controllable turn, send `signal` once its prompt is in flight,
/// and return the prompt result.
async fn run_turn_and_signal(
    agent: OwnedAgent,
    batch: FlushBatch,
    ctx: &Arc<PromptContext>,
    capture: &std::path::Path,
    signal: ControlSignal,
) -> PromptResult {
    let (result_tx, mut result_rx) = mpsc::unbounded_channel();
    let (control_tx, control_rx) = tokio::sync::oneshot::channel();
    let task = tokio::spawn(run_prompt_task(
        agent,
        Some(batch),
        None,
        Arc::clone(ctx),
        result_tx,
        Some(control_rx),
        "cancelled-turn".into(),
    ));
    tokio::time::timeout(Duration::from_secs(10), async {
        while requests_for(&captured_requests(capture), "session/prompt").is_empty() {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("prompt reached the agent");
    // Let the streamed chunk reach the reply draft before cancelling.
    tokio::time::sleep(Duration::from_millis(300)).await;
    control_tx.send(signal).expect("turn still listening");
    task.await.expect("prompt task completed");
    result_rx.recv().await.expect("prompt result")
}

/// A steer that the adapter cancels cleanly keeps the scope's session:
/// the merged re-prompt goes to the same session (no `session/new`),
/// without resending standing context, framed as an interrupted turn.
/// The cancelled turn's reply draft ends abandoned, and nothing posts.
#[tokio::test]
async fn clean_steer_cancel_keeps_session_and_reprompts_it() {
    let (acp, capture) = spawn_cancel_acp(CANCEL_CLEAN).await;
    let agent = cancel_test_agent(acp);
    let channel_id = Uuid::new_v4();
    let scope = conv(channel_id);
    let relay = crate::stream_draft::test_relay::FakeRelay::spawn().await;
    let (publisher, mut frames_rx) = crate::relay::RelayEventPublisher::test_pair();
    let mut ctx = cancel_test_ctx(&relay, channel_id);
    ctx.base_prompt = Some("standing-once".into());
    ctx.stream = crate::stream_draft::StreamRuntime::new(
        crate::stream_draft::StreamMode::DraftAutopost,
        publisher,
        Keys::generate(),
    );
    let ctx = Arc::new(ctx);

    let original = single_event_batch(channel_id, "the original task");
    let original_id = original.events[0].event.id.to_hex();
    let result = run_turn_and_signal(agent, original, &ctx, &capture, ControlSignal::Steer).await;
    assert!(matches!(result.outcome, PromptOutcome::Cancelled));
    let mut agent = result.agent;
    assert_eq!(
        agent.state.sessions.get(&scope).map(String::as_str),
        Some("new-0"),
        "a clean steer cancel keeps the session"
    );
    let delivery = &agent.state.deliveries[&scope];
    assert!(
        delivery.standing_context_sent && delivery.delivered_event_ids.contains(&original_id),
        "the cancelled prompt reached the kept session, so its delivery commits"
    );
    let retry = result.batch.expect("steer requeues the batch");
    assert_eq!(retry.cancel_reason, Some(crate::queue::CancelReason::Steer));

    // The queue merges the requeued batch into the next flush.
    let mut merged = single_event_batch(channel_id, "the new message");
    merged.cancelled_events = retry.events;
    merged.cancel_reason = retry.cancel_reason;
    let (result_tx, mut result_rx) = mpsc::unbounded_channel();
    let (_control_tx, control_rx) = tokio::sync::oneshot::channel();
    run_prompt_task(
        agent,
        Some(merged),
        None,
        Arc::clone(&ctx),
        result_tx,
        Some(control_rx),
        "reprompt-turn".into(),
    )
    .await;
    let result = result_rx.recv().await.expect("prompt result");
    assert!(matches!(
        result.outcome,
        PromptOutcome::Ok(StopReason::EndTurn)
    ));
    agent = result.agent;
    agent.acp.shutdown().await;

    let requests = captured_requests(&capture);
    std::fs::remove_file(&capture).expect("remove ACP capture");
    assert_eq!(
        requests_for(&requests, "session/new").len(),
        1,
        "the re-prompt must not create a new session"
    );
    let prompts = requests_for(&requests, "session/prompt");
    assert_eq!(prompts.len(), 2);
    assert_eq!(prompts[1]["params"]["sessionId"], "new-0");
    let first = prompt_request_text(prompts[0]);
    let reprompt = prompt_request_text(prompts[1]);
    assert!(first.contains("standing-once"));
    assert!(
        !reprompt.contains("standing-once"),
        "standing context is not resent to the kept session: {reprompt}"
    );
    assert!(
        reprompt.contains("<your-interrupted-turn-was-handling>")
            && reprompt.contains("Your previous turn was interrupted")
            && !reprompt.contains("<what-you-were-working-on>"),
        "the re-prompt says the turn was interrupted: {reprompt}"
    );
    assert!(reprompt.contains("the new message"));

    let mut statuses: HashMap<String, Vec<String>> = HashMap::new();
    while let Ok(Some(frame)) =
        tokio::time::timeout(Duration::from_millis(300), frames_rx.recv()).await
    {
        let tag = |name: &str| {
            frame
                .tags
                .iter()
                .map(|t| t.as_slice())
                .find(|t| t.first().map(String::as_str) == Some(name))
                .and_then(|t| t.get(1).cloned())
                .unwrap_or_default()
        };
        statuses
            .entry(tag("stream"))
            .or_default()
            .push(tag("status"));
    }
    assert_eq!(
        statuses.len(),
        1,
        "only the cancelled turn streamed text: {statuses:?}"
    );
    let cancelled = statuses.values().next().expect("cancelled turn stream");
    assert_eq!(
        cancelled.last().map(String::as_str),
        Some("abandoned"),
        "{cancelled:?}"
    );
    assert!(
        !cancelled.iter().any(|s| s == "final"),
        "the cancelled draft never finalizes: {cancelled:?}"
    );
    assert!(
        relay.posted_messages().is_empty(),
        "the cancelled turn's partial text is never posted"
    );
}

/// Every cancel outcome other than a clean `cancelled` answer to a steer
/// or interrupt still drops the session, as before.
#[tokio::test]
async fn control_cancel_invalidates_session_unless_clean_mid_turn_cancel() {
    const CANCEL_ENDS_TURN: &str =
        r#"printf '%s\n' '{"jsonrpc":"2.0","id":'"$pending"',"result":{"stopReason":"end_turn"}}'"#;
    let cases: [(&str, ControlSignal, &str); 4] = [
        ("non-clean stop", ControlSignal::Steer, CANCEL_ENDS_TURN),
        ("agent exit", ControlSignal::Steer, "exit 0"),
        ("explicit stop", ControlSignal::Cancel, CANCEL_CLEAN),
        ("rotate", ControlSignal::Rotate, CANCEL_CLEAN),
    ];
    for (label, signal, on_cancel) in cases {
        let (acp, capture) = spawn_cancel_acp(on_cancel).await;
        let channel_id = Uuid::new_v4();
        let relay = crate::stream_draft::test_relay::FakeRelay::spawn().await;
        let ctx = Arc::new(cancel_test_ctx(&relay, channel_id));
        let result = run_turn_and_signal(
            cancel_test_agent(acp),
            single_event_batch(channel_id, "the original task"),
            &ctx,
            &capture,
            signal,
        )
        .await;
        let _ = std::fs::remove_file(&capture);
        assert!(
            !result.agent.state.sessions.contains_key(&conv(channel_id)),
            "{label}: the session must be invalidated"
        );
        assert!(
            !result.agent.state.has_channel_state(&channel_id),
            "{label}: no per-scope state survives"
        );
        let mut agent = result.agent;
        agent.acp.shutdown().await;
    }
}

#[test]
fn control_cancel_keeps_session_only_for_clean_mid_turn_cancels() {
    let channel = PromptSource::Channel(conv(Uuid::new_v4()));
    let switch = ControlSignal::SwitchModel {
        model_id: "m".into(),
        request_id: None,
    };
    for (signal, stop, source, keeps) in [
        (ControlSignal::Steer, StopReason::Cancelled, &channel, true),
        (
            ControlSignal::Interrupt,
            StopReason::Cancelled,
            &channel,
            true,
        ),
        (ControlSignal::Steer, StopReason::EndTurn, &channel, false),
        (
            ControlSignal::Interrupt,
            StopReason::MaxTokens,
            &channel,
            false,
        ),
        (
            ControlSignal::Cancel,
            StopReason::Cancelled,
            &channel,
            false,
        ),
        (
            ControlSignal::Rotate,
            StopReason::Cancelled,
            &channel,
            false,
        ),
        (switch, StopReason::Cancelled, &channel, false),
        (
            ControlSignal::Steer,
            StopReason::Cancelled,
            &PromptSource::Heartbeat,
            false,
        ),
    ] {
        assert_eq!(
            control_cancel_keeps_session(&signal, &stop, source),
            keeps,
            "{signal:?} / {stop:?} / {source:?}"
        );
    }
}

/// A turn-limit rotation of the scope that owns the resume binding releases
/// it, so the next session for that scope starts fresh instead of resuming
/// the fork whose context triggered the rotation.
#[tokio::test]
async fn limit_rotation_releases_resume_binding() {
    let script = r#"while IFS= read -r line; do
  id=$(printf '%s' "$line" | sed -n 's/.*"id":\([0-9][0-9]*\).*/\1/p')
  [ -z "$id" ] && continue
  printf '%s\n' '{"jsonrpc":"2.0","id":'"$id"',"result":{"stopReason":"end_turn"}}'
done"#;
    let acp = AcpClient::spawn("bash", &["-c".to_string(), script.to_string()], &[], false)
        .await
        .expect("spawn rotation ACP script");
    let mut agent = cancel_test_agent(acp);
    let channel_id = Uuid::new_v4();
    let scope = conv(channel_id);
    agent.state.sessions.insert(scope.clone(), "fork-1".into());
    agent
        .state
        .deliveries
        .insert(scope.clone(), ChannelDeliveryState::default());

    let relay = crate::stream_draft::test_relay::FakeRelay::spawn().await;
    let state = tempfile::tempdir().expect("state dir");
    let mut ctx = cancel_test_ctx(&relay, channel_id);
    ctx.max_turns_per_session = 1;
    ctx.resume_session = ResumeSessionSlot::new(Some(PendingResume {
        source: "src-1".into(),
        store: crate::resume_store::ResumeForkStore::new(state.path(), "agent-hex"),
    }));
    let claim = ctx
        .resume_session
        .claim(&scope, agent.index)
        .expect("claims the pending resume");
    ctx.resume_session.settle(claim, &scope, agent.index, true);
    assert!(ctx.resume_session.bound_owner().is_some());
    let ctx = Arc::new(ctx);

    let (result_tx, mut result_rx) = mpsc::unbounded_channel();
    run_prompt_task(
        agent,
        Some(single_event_batch(channel_id, "hello")),
        None,
        Arc::clone(&ctx),
        result_tx,
        None,
        "rotating-turn".into(),
    )
    .await;
    let mut result = result_rx.recv().await.expect("prompt result");
    assert!(matches!(
        result.outcome,
        PromptOutcome::Ok(StopReason::EndTurn)
    ));
    assert!(
        !result.agent.state.sessions.contains_key(&scope),
        "the turn limit rotated the session"
    );
    assert_eq!(
        ctx.resume_session.bound_owner(),
        None,
        "a deliberate rotation releases the binding"
    );
    assert!(ctx.resume_session.claim(&scope, 0).is_none());
    result.agent.acp.shutdown().await;
}
