//! Session loss and restart recovery in `run_prompt_task`, driven by a scripted
//! ACP agent: a dead provider session fails fast and stays dead until
//! `!rotate`; a journaled turn survives a shutdown and resumes in its own
//! session on the next start.
use super::control_cancel_tests::{
    cancel_test_agent, cancel_test_ctx, captured_requests, conv, prompt_request_text, requests_for,
    single_event_batch,
};
use super::*;
use crate::acp::AcpClient;
use crate::turn_journal::{TurnJournal, TurnJournalStore, TurnRecord};

/// How the scripted agent answers `session/prompt`.
#[derive(Clone, Copy)]
enum PromptReply {
    EndTurn,
    Cancelled,
    /// Never answer (the turn stays in flight).
    Hang,
}

/// Scripted ACP agent. `initialize` advertises `caps` as
/// `sessionCapabilities`. `session/new` mints `new-<id>`. `session/resume`
/// echoes the session unless it is `fail_resume`, which answers
/// `RESOURCE_NOT_FOUND`. `session/prompt` to `dead` answers claude-agent-acp's
/// "Session not found"; any other prompt answers per `reply`. Every request
/// line is appended to the returned capture file.
async fn spawn_recovery_acp(
    caps: &str,
    dead: &str,
    fail_resume: &str,
    reply: PromptReply,
) -> (AcpClient, std::path::PathBuf) {
    let capture = std::env::temp_dir().join(format!(
        "buzz-acp-session-recovery-{}.ndjson",
        Uuid::new_v4()
    ));
    let quoted_capture = capture.to_string_lossy().replace('\'', "'\\''");
    let prompt_body = match reply {
        PromptReply::EndTurn => {
            r#"printf '%s\n' '{"jsonrpc":"2.0","id":'"$id"',"result":{"stopReason":"end_turn"}}'"#
        }
        PromptReply::Cancelled => {
            r#"printf '%s\n' '{"jsonrpc":"2.0","id":'"$id"',"result":{"stopReason":"cancelled"}}'"#
        }
        PromptReply::Hang => ":",
    };
    let script = format!(
        r#"while IFS= read -r line; do
  printf '%s\n' "$line" >> '{quoted_capture}'
  id=$(printf '%s' "$line" | sed -n 's/.*"id":\([0-9][0-9]*\).*/\1/p')
  sid=$(printf '%s' "$line" | sed -n 's/.*"sessionId":"\([^"]*\)".*/\1/p')
  case "$line" in
    *'"method":"initialize"'*)
      printf '%s\n' '{{"jsonrpc":"2.0","id":'"$id"',"result":{{"protocolVersion":1,"agentCapabilities":{{"sessionCapabilities":{caps}}}}}}}' ;;
    *'"method":"session/new"'*)
      printf '%s\n' '{{"jsonrpc":"2.0","id":'"$id"',"result":{{"sessionId":"new-'"$id"'"}}}}' ;;
    *'"method":"session/resume"'*)
      if [ "$sid" = '{fail_resume}' ]; then
        printf '%s\n' '{{"jsonrpc":"2.0","id":'"$id"',"error":{{"code":-32002,"message":"Resource not found: '"$sid"'"}}}}'
      else
        printf '%s\n' '{{"jsonrpc":"2.0","id":'"$id"',"result":{{"sessionId":"'"$sid"'"}}}}'
      fi ;;
    *'"method":"session/prompt"'*)
      if [ "$sid" = '{dead}' ]; then
        printf '%s\n' '{{"jsonrpc":"2.0","id":'"$id"',"error":{{"code":-32603,"message":"Internal error","data":{{"details":"Session not found"}}}}}}'
      else
        {prompt_body}
      fi ;;
    *) [ -n "$id" ] && printf '%s\n' '{{"jsonrpc":"2.0","id":'"$id"',"result":{{}}}}' ;;
  esac
done"#
    );
    let mut acp = AcpClient::spawn("bash", &["-c".to_string(), script], &[], false)
        .await
        .expect("spawn recovery ACP script");
    acp.initialize().await.expect("initialize");
    (acp, capture)
}

/// Variant name of an outcome, for assertion messages.
fn shape(outcome: &PromptOutcome) -> String {
    match outcome {
        PromptOutcome::SessionDead(reason) => format!("SessionDead({reason})"),
        PromptOutcome::Error(error) => format!("Error({error})"),
        PromptOutcome::Ok(stop) => format!("Ok({stop:?})"),
        PromptOutcome::Cancelled => "Cancelled".into(),
        PromptOutcome::AgentExited => "AgentExited".into(),
        PromptOutcome::Timeout(_) => "Timeout".into(),
        PromptOutcome::CancelDrainTimeout(_) => "CancelDrainTimeout".into(),
        PromptOutcome::ProjectContextIndeterminate(reason) => {
            format!("ProjectContextIndeterminate({reason})")
        }
    }
}

const RESUME_ONLY: &str = r#"{"resume":{}}"#;

async fn run_turn(agent: OwnedAgent, batch: FlushBatch, ctx: &Arc<PromptContext>) -> PromptResult {
    let (result_tx, mut result_rx) = mpsc::unbounded_channel();
    run_prompt_task(
        agent,
        Some(batch),
        None,
        Arc::clone(ctx),
        result_tx,
        None,
        "recovery-turn".into(),
    )
    .await;
    result_rx.recv().await.expect("prompt result")
}

fn journal_records(store: &TurnJournalStore) -> Vec<TurnRecord> {
    store
        .load_all()
        .expect("list journal")
        .into_iter()
        .map(|(_, record)| record.expect("readable record"))
        .collect()
}

#[test]
fn dead_session_errors_are_classified() {
    let agent_error = |code, message: &str| AcpError::AgentError {
        code,
        message: message.into(),
    };
    let cases = [
        (
            agent_error(-32603, "Internal error: Session not found"),
            true,
        ),
        (
            agent_error(
                -32603,
                "Internal error: The Claude Agent session has ended. Please start a new session.",
            ),
            true,
        ),
        (agent_error(-32002, "Resource not found: abc"), true),
        (
            agent_error(
                -32603,
                "Internal error: No conversation found with session ID: abc",
            ),
            true,
        ),
        (agent_error(-32002, "llm model not found: gpt-x"), false),
        (agent_error(-32603, "Internal error: rate limited"), false),
        (agent_error(-32000, "Authentication required"), false),
        (AcpError::Protocol("Session not found".into()), false),
        (AcpError::AgentExited, false),
    ];
    for (error, dead) in cases {
        assert_eq!(is_dead_session_error(&error), dead, "{error:?}");
    }
}

/// A prompt to a session the adapter no longer has fails fast: the outcome
/// is `SessionDead` with the adapter's code, message and the session id, the
/// batch comes back for dead-lettering (not a retry), no new session is
/// created, and the next message for the scope fails the same way without
/// touching the agent — until `!rotate` clears it.
#[tokio::test]
async fn dead_session_fails_fast_and_stays_dead_until_rotate() {
    let (acp, capture) =
        spawn_recovery_acp(RESUME_ONLY, "live-1", "none", PromptReply::EndTurn).await;
    let mut agent = cancel_test_agent(acp);
    let channel_id = Uuid::new_v4();
    let scope = conv(channel_id);
    agent.state.sessions.insert(scope.clone(), "live-1".into());
    agent
        .state
        .deliveries
        .insert(scope.clone(), ChannelDeliveryState::default());
    let relay = crate::stream_draft::test_relay::FakeRelay::spawn().await;
    let ctx = Arc::new(cancel_test_ctx(&relay, channel_id));

    let result = run_turn(agent, single_event_batch(channel_id, "first"), &ctx).await;
    let PromptOutcome::SessionDead(reason) = &result.outcome else {
        panic!("expected SessionDead, got {}", shape(&result.outcome));
    };
    assert!(
        reason.contains("live-1")
            && reason.contains("-32603")
            && reason.contains("Session not found"),
        "the reason names the session, code and adapter message: {reason}"
    );
    assert!(reason.contains("!rotate"), "{reason}");
    assert!(
        result.batch.is_some(),
        "the batch is handed back to be dead-lettered"
    );
    assert!(!result.agent.state.sessions.contains_key(&scope));
    assert_eq!(
        ctx.dead_sessions.reason(&scope).as_deref(),
        Some(reason.as_str())
    );
    let first_reason = reason.clone();

    let result = run_turn(result.agent, single_event_batch(channel_id, "second"), &ctx).await;
    assert!(
        matches!(&result.outcome, PromptOutcome::SessionDead(r) if *r == first_reason),
        "the next message fails with the same reason: {}",
        shape(&result.outcome)
    );
    let requests = captured_requests(&capture);
    assert_eq!(
        requests_for(&requests, "session/prompt").len(),
        1,
        "no retry of the dead session"
    );
    assert!(
        requests_for(&requests, "session/new").is_empty(),
        "never a silent new session"
    );

    // `!rotate` is the explicit way to start fresh.
    assert!(ctx.dead_sessions.clear(&scope));
    let mut result = run_turn(result.agent, single_event_batch(channel_id, "third"), &ctx).await;
    assert!(matches!(
        result.outcome,
        PromptOutcome::Ok(StopReason::EndTurn)
    ));
    result.agent.acp.shutdown().await;
    let requests = captured_requests(&capture);
    let _ = std::fs::remove_file(&capture);
    assert_eq!(requests_for(&requests, "session/new").len(), 1);
}

/// A turn whose prompt ends `cancelled` while the harness is shutting down
/// keeps its journal record (for resume on restart) and is not marked
/// delivered. A turn that completes during shutdown, or ends `cancelled`
/// while the harness keeps running, leaves no record.
#[tokio::test]
async fn shutdown_cancelled_turn_keeps_its_journal_record() {
    for (label, reply, shutting_down, kept) in [
        ("cancelled in shutdown", PromptReply::Cancelled, true, true),
        ("completed in shutdown", PromptReply::EndTurn, true, false),
        (
            "cancelled while running",
            PromptReply::Cancelled,
            false,
            false,
        ),
    ] {
        let (acp, capture) = spawn_recovery_acp(RESUME_ONLY, "none", "none", reply).await;
        let mut agent = cancel_test_agent(acp);
        let channel_id = Uuid::new_v4();
        let scope = conv(channel_id);
        agent.state.sessions.insert(scope.clone(), "live-1".into());
        agent
            .state
            .deliveries
            .insert(scope.clone(), ChannelDeliveryState::default());
        let relay = crate::stream_draft::test_relay::FakeRelay::spawn().await;
        let state = tempfile::tempdir().expect("state dir");
        let store = TurnJournalStore::new(state.path(), "agent-hex");
        let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(());
        let mut ctx = cancel_test_ctx(&relay, channel_id);
        ctx.turn_journal = TurnJournal::new(store.clone(), shutdown_rx);
        let ctx = Arc::new(ctx);
        if shutting_down {
            shutdown_tx.send(()).expect("shutdown receiver alive");
        }

        let batch = single_event_batch(channel_id, "work");
        let event_id = batch.events[0].event.id.to_hex();
        let mut result = run_turn(agent, batch, &ctx).await;
        result.agent.acp.shutdown().await;
        let _ = std::fs::remove_file(&capture);
        let records = journal_records(&store);
        assert_eq!(records.len(), usize::from(kept), "{label}: {records:?}");
        if kept {
            assert_eq!(records[0].session_id, "live-1", "{label}");
            assert_eq!(records[0].resume_attempts, 0, "{label}");
            assert!(
                !result.agent.state.deliveries[&scope]
                    .delivered_event_ids
                    .contains(&event_id),
                "{label}: an interrupted turn's events are not marked delivered"
            );
        }
    }
}

/// A turn still in flight when the harness gives up waiting (its task is
/// aborted after the shutdown grace) keeps its record.
#[tokio::test]
async fn aborted_turn_during_shutdown_keeps_its_journal_record() {
    let (acp, capture) = spawn_recovery_acp(RESUME_ONLY, "none", "none", PromptReply::Hang).await;
    let mut agent = cancel_test_agent(acp);
    let channel_id = Uuid::new_v4();
    let scope = conv(channel_id);
    agent.state.sessions.insert(scope.clone(), "live-1".into());
    let relay = crate::stream_draft::test_relay::FakeRelay::spawn().await;
    let state = tempfile::tempdir().expect("state dir");
    let store = TurnJournalStore::new(state.path(), "agent-hex");
    let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(());
    let mut ctx = cancel_test_ctx(&relay, channel_id);
    ctx.turn_journal = TurnJournal::new(store.clone(), shutdown_rx);
    let ctx = Arc::new(ctx);

    let (result_tx, _result_rx) = mpsc::unbounded_channel();
    let task = tokio::spawn(run_prompt_task(
        agent,
        Some(single_event_batch(channel_id, "long work")),
        None,
        Arc::clone(&ctx),
        result_tx,
        None,
        "aborted-turn".into(),
    ));
    tokio::time::timeout(Duration::from_secs(10), async {
        while requests_for(&captured_requests(&capture), "session/prompt").is_empty() {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("prompt reached the agent");
    shutdown_tx.send(()).expect("shutdown receiver alive");
    task.abort();
    let _ = task.await;
    let _ = std::fs::remove_file(&capture);
    let records = journal_records(&store);
    assert_eq!(records.len(), 1, "{records:?}");
    assert_eq!(records[0].scope(), scope);
}

/// A restored turn resumes the recorded session (no `session/new`), is framed
/// as an interrupted turn to continue, and its record is gone once the
/// resumed turn completes.
#[tokio::test]
async fn restored_turn_resumes_its_own_session() {
    let (acp, capture) =
        spawn_recovery_acp(RESUME_ONLY, "none", "none", PromptReply::EndTurn).await;
    let agent = cancel_test_agent(acp);
    let channel_id = Uuid::new_v4();
    let scope = conv(channel_id);
    let relay = crate::stream_draft::test_relay::FakeRelay::spawn().await;
    let state = tempfile::tempdir().expect("state dir");
    let store = TurnJournalStore::new(state.path(), "agent-hex");
    let (_shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(());
    let mut ctx = cancel_test_ctx(&relay, channel_id);
    ctx.turn_journal = TurnJournal::new(store.clone(), shutdown_rx);
    let interrupted = single_event_batch(channel_id, "the interrupted request");
    let record = TurnRecord::for_turn(&interrupted, "old-session", &ctx.cwd, 1);
    store.save(&record).expect("seed record");
    ctx.turn_journal
        .register_restart(scope.clone(), "old-session".into());
    let ctx = Arc::new(ctx);

    let mut result = run_turn(agent, record.to_batch(), &ctx).await;
    assert!(matches!(
        result.outcome,
        PromptOutcome::Ok(StopReason::EndTurn)
    ));
    assert_eq!(
        result.agent.state.sessions.get(&scope).map(String::as_str),
        Some("old-session")
    );
    result.agent.acp.shutdown().await;
    let requests = captured_requests(&capture);
    let _ = std::fs::remove_file(&capture);
    assert!(
        requests_for(&requests, "session/new").is_empty(),
        "no new session"
    );
    let resumes = requests_for(&requests, "session/resume");
    assert_eq!(resumes.len(), 1);
    assert_eq!(resumes[0]["params"]["sessionId"], "old-session");
    let prompts = requests_for(&requests, "session/prompt");
    assert_eq!(prompts[0]["params"]["sessionId"], "old-session");
    let text = prompt_request_text(prompts[0]);
    assert!(
        text.contains("<your-interrupted-turn-was-handling>")
            && text.contains("the harness restarted")
            && text.contains("the interrupted request"),
        "the restored prompt says continue the interrupted turn: {text}"
    );
    assert_eq!(ctx.turn_journal.restart_session(&scope), None);
    assert!(
        journal_records(&store).is_empty(),
        "the record is gone after the resumed turn"
    );
}

/// Resuming a restored turn's session that is gone fails fast like any dead
/// session: `SessionDead`, no `session/new`, record dropped, scope dead. An
/// agent that cannot resume at all fails the same way.
#[tokio::test]
async fn restored_turn_resume_failure_fails_fast() {
    for (label, caps) in [
        ("session gone", RESUME_ONLY),
        ("no resume capability", "{}"),
    ] {
        let (acp, capture) =
            spawn_recovery_acp(caps, "none", "old-session", PromptReply::EndTurn).await;
        let agent = cancel_test_agent(acp);
        let channel_id = Uuid::new_v4();
        let scope = conv(channel_id);
        let relay = crate::stream_draft::test_relay::FakeRelay::spawn().await;
        let state = tempfile::tempdir().expect("state dir");
        let store = TurnJournalStore::new(state.path(), "agent-hex");
        let (_shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(());
        let mut ctx = cancel_test_ctx(&relay, channel_id);
        ctx.turn_journal = TurnJournal::new(store.clone(), shutdown_rx);
        let record = TurnRecord::for_turn(
            &single_event_batch(channel_id, "the interrupted request"),
            "old-session",
            &ctx.cwd,
            1,
        );
        store.save(&record).expect("seed record");
        ctx.turn_journal
            .register_restart(scope.clone(), "old-session".into());
        let ctx = Arc::new(ctx);

        let mut result = run_turn(agent, record.to_batch(), &ctx).await;
        assert!(
            matches!(&result.outcome, PromptOutcome::SessionDead(r) if r.contains("old-session")),
            "{label}: {}",
            shape(&result.outcome)
        );
        assert!(result.batch.is_some(), "{label}");
        result.agent.acp.shutdown().await;
        let requests = captured_requests(&capture);
        let _ = std::fs::remove_file(&capture);
        assert!(
            requests_for(&requests, "session/new").is_empty(),
            "{label}: no new session"
        );
        assert!(
            requests_for(&requests, "session/prompt").is_empty(),
            "{label}"
        );
        assert!(ctx.dead_sessions.reason(&scope).is_some(), "{label}");
        assert_eq!(ctx.turn_journal.restart_session(&scope), None, "{label}");
        assert!(
            journal_records(&store).is_empty(),
            "{label}: the record is dropped"
        );
    }
}
