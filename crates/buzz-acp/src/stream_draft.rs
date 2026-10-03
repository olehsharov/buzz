//! Live reply streaming: NIP-SD `kind:20003` ghost drafts (`docs/nips/NIP-SD.md`).
//!
//! ```text
//! AcpClient read loop ──on_text/on_thought/on_tool──► StreamSink (bounded state)
//!                                                         │ notify
//!                                                         ▼
//!                              publisher task ── throttle + FrameBudget ──► relay (ephemeral)
//! run_prompt_task ──ReplyStream::finish(stop)──► [autopost kind:9] ──► final/abandoned frame
//! ```
//!
//! The agent's `agent_message_chunk` text is accumulated per *segment*: every
//! `tool_call` starts a new segment, so the draft shows the text written since
//! the last tool call (pre-tool text is narration; the answer comes last). The
//! autopost text is the last non-empty segment, so an answer followed by a
//! trailing bookkeeping tool call is not lost.
//!
//! Every exit path is covered: dropping a [`ReplyStream`] without
//! [`finish`](ReplyStream::finish) abandons the stream, and the publisher task
//! sends the terminal frame on its own (no async work in `Drop`).

use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use buzz_sdk::{StreamDraft, StreamDraftStatus, ThreadRef};
use nostr::{Alphabet, EventId, Keys, Kind, SingleLetterTag, Timestamp};
use tokio::sync::{oneshot, Notify};
use tokio::time::Instant;
use uuid::Uuid;

use crate::acp::StopReason;
use crate::relay::{RelayEventPublisher, RestClient};

/// `--stream` / `BUZZ_ACP_STREAM`: whether the harness streams reply drafts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, clap::ValueEnum)]
pub enum StreamMode {
    /// No drafts (default). The agent replies only via `buzz messages send`.
    #[default]
    Off,
    /// Stream ghost drafts of the agent's response text into the trigger's
    /// reply destination. Nothing is posted by the harness.
    Draft,
    /// `draft`, plus: at end of turn, post the response text as the reply
    /// (kind:9 with the `stream` tag) unless the agent already replied in that
    /// scope via the CLI during the turn.
    #[value(name = "draft+autopost")]
    DraftAutopost,
}

impl StreamMode {
    /// Whether the harness publishes drafts at all.
    pub fn is_enabled(self) -> bool {
        self != Self::Off
    }
}

impl std::fmt::Display for StreamMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Off => "off",
            Self::Draft => "draft",
            Self::DraftAutopost => "draft+autopost",
        })
    }
}

/// Minimum spacing between two frames of one stream.
pub(crate) const MIN_FRAME_INTERVAL: Duration = Duration::from_millis(200);
/// New characters that justify a text frame (status changes always do).
pub(crate) const MIN_NEW_CHARS: usize = 24;
/// Upper bound on how long fewer than [`MIN_NEW_CHARS`] new characters wait.
pub(crate) const IDLE_FLUSH_AFTER: Duration = Duration::from_secs(1);
/// Re-send the current frame this often so clients (15 s TTL) keep the ghost
/// alive through long tool calls and silent model phases.
pub(crate) const KEEPALIVE_INTERVAL: Duration = Duration::from_secs(5);
/// Harness-wide sustained frame rate, across every concurrent stream. Keeps
/// drafts plus typing well inside the relay's ~10 frames/s per-pubkey budget.
pub(crate) const FRAMES_PER_SEC: f64 = 6.0;
/// Harness-wide burst allowance on top of [`FRAMES_PER_SEC`].
pub(crate) const FRAME_BURST: f64 = 3.0;
/// Bound on waiting for the publisher to emit the end-of-turn flush.
const FLUSH_TIMEOUT: Duration = Duration::from_secs(2);
/// Bound on waiting for the publisher to send the terminal frame.
const TERMINAL_TIMEOUT: Duration = Duration::from_secs(2);
/// Bound on each relay HTTP call made while finalizing.
const FINALIZE_HTTP_TIMEOUT: Duration = Duration::from_secs(15);
/// Own kind:9 events inspected for the "already replied" check.
const SELF_REPLY_QUERY_LIMIT: usize = 100;

// ── Throttle policy (pure) ───────────────────────────────────────────────────

/// Live (non-terminal) status of a stream.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LiveStatus {
    Thinking,
    Tool,
    Writing,
}

impl LiveStatus {
    fn wire(self) -> StreamDraftStatus {
        match self {
            Self::Thinking => StreamDraftStatus::Thinking,
            Self::Tool => StreamDraftStatus::Tool,
            Self::Writing => StreamDraftStatus::Writing,
        }
    }
}

/// What the throttle compares between the current state and the last frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct FrameState {
    pub status: LiveStatus,
    /// Incremented on every tool call (text resets there).
    pub segment: u64,
    /// Incremented whenever the tool label changes within a segment.
    pub label_rev: u64,
    /// Characters in the current segment's text.
    pub chars: usize,
}

/// The last frame actually handed to the relay.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Emitted {
    pub at: Instant,
    pub state: FrameState,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ThrottleAction {
    Emit,
    WaitUntil(Instant),
}

/// Decide whether to emit a frame now.
///
/// - Nothing observed yet → `None` (no frame, wait for an update).
/// - First frame → emit immediately.
/// - Status/segment/label change or ≥ [`MIN_NEW_CHARS`] new characters →
///   emit once [`MIN_FRAME_INTERVAL`] has passed since the last frame.
/// - Fewer new characters → emit after [`IDLE_FLUSH_AFTER`].
/// - No change → keepalive after [`KEEPALIVE_INTERVAL`].
pub(crate) fn throttle(
    last: Option<&Emitted>,
    current: Option<&FrameState>,
    now: Instant,
) -> Option<ThrottleAction> {
    let current = current?;
    let Some(last) = last else {
        return Some(ThrottleAction::Emit);
    };
    let changed = current.status != last.state.status
        || current.segment != last.state.segment
        || current.label_rev != last.state.label_rev;
    let new_chars = current.chars.saturating_sub(last.state.chars);
    let due = if changed || new_chars >= MIN_NEW_CHARS {
        last.at + MIN_FRAME_INTERVAL
    } else if new_chars > 0 {
        last.at + IDLE_FLUSH_AFTER
    } else {
        last.at + KEEPALIVE_INTERVAL
    };
    Some(if now >= due {
        ThrottleAction::Emit
    } else {
        ThrottleAction::WaitUntil(due)
    })
}

/// Harness-wide token bucket shared by every concurrent stream.
#[derive(Debug)]
pub(crate) struct FrameBudget {
    capacity: f64,
    per_sec: f64,
    tokens: f64,
    refilled_at: Instant,
}

impl FrameBudget {
    pub(crate) fn new(capacity: f64, per_sec: f64, now: Instant) -> Self {
        Self {
            capacity,
            per_sec,
            tokens: capacity,
            refilled_at: now,
        }
    }

    /// Take one token, or report how long until one is available.
    pub(crate) fn try_take(&mut self, now: Instant) -> Result<(), Duration> {
        let elapsed = now
            .saturating_duration_since(self.refilled_at)
            .as_secs_f64();
        self.tokens = (self.tokens + elapsed * self.per_sec).min(self.capacity);
        self.refilled_at = now;
        if self.tokens >= 1.0 {
            self.tokens -= 1.0;
            Ok(())
        } else {
            Err(Duration::from_secs_f64((1.0 - self.tokens) / self.per_sec))
        }
    }
}

// ── Reply accumulation (pure) ────────────────────────────────────────────────

/// Bounded per-turn reply state fed by ACP session updates.
#[derive(Debug, Default)]
pub(crate) struct ReplyAccumulator {
    status: Option<LiveStatus>,
    segment: u64,
    label_rev: u64,
    /// Text since the last tool call, capped just above the snapshot limit.
    text: String,
    chars: usize,
    /// Last non-empty segment before the current one (autopost fallback).
    previous: String,
    tool_call_id: Option<String>,
    label: Option<String>,
}

/// Retain a little past the cap so the snapshot still gets its `…` marker.
const TEXT_RETAIN_BYTES: usize = buzz_sdk::MAX_STREAM_DRAFT_CONTENT_BYTES + 4;

impl ReplyAccumulator {
    pub(crate) fn on_text(&mut self, chunk: &str) {
        self.status = Some(LiveStatus::Writing);
        let room = TEXT_RETAIN_BYTES.saturating_sub(self.text.len());
        if room == 0 || chunk.is_empty() {
            return;
        }
        let mut end = chunk.len().min(room);
        while !chunk.is_char_boundary(end) {
            end -= 1;
        }
        let kept = &chunk[..end];
        self.chars += kept.chars().count();
        self.text.push_str(kept);
    }

    /// Reasoning changes status only; its text is never retained.
    pub(crate) fn on_thought(&mut self) {
        self.status = Some(LiveStatus::Thinking);
    }

    pub(crate) fn on_tool_call(&mut self, tool_call_id: Option<&str>, title: Option<&str>) {
        if !self.text.trim().is_empty() {
            self.previous = std::mem::take(&mut self.text);
        }
        self.text.clear();
        self.chars = 0;
        self.segment += 1;
        self.status = Some(LiveStatus::Tool);
        self.tool_call_id = tool_call_id.map(str::to_owned);
        self.label = clean_label(title);
    }

    /// A refined title for the running tool (e.g. the actual command).
    pub(crate) fn on_tool_title(&mut self, tool_call_id: Option<&str>, title: Option<&str>) {
        if self.status != Some(LiveStatus::Tool)
            || tool_call_id.is_none()
            || tool_call_id != self.tool_call_id.as_deref()
        {
            return;
        }
        let label = clean_label(title);
        if label.is_some() && label != self.label {
            self.label = label;
            self.label_rev += 1;
        }
    }

    pub(crate) fn frame_state(&self) -> Option<FrameState> {
        Some(FrameState {
            status: self.status?,
            segment: self.segment,
            label_rev: self.label_rev,
            chars: self.chars,
        })
    }

    /// The draft content for the current state (current segment, capped).
    pub(crate) fn snapshot(&self) -> String {
        buzz_sdk::truncate_stream_content(&self.text).into_owned()
    }

    pub(crate) fn label(&self) -> Option<&str> {
        match self.status {
            Some(LiveStatus::Tool) => self.label.as_deref(),
            _ => None,
        }
    }

    /// The reply text to autopost: the last non-empty segment, capped.
    pub(crate) fn final_text(&self) -> String {
        let text = if self.text.trim().is_empty() {
            &self.previous
        } else {
            &self.text
        };
        buzz_sdk::truncate_stream_content(text).into_owned()
    }
}

fn clean_label(title: Option<&str>) -> Option<String> {
    let title = title?.trim();
    (!title.is_empty()).then(|| {
        title
            .chars()
            .take(buzz_sdk::MAX_STREAM_DRAFT_LABEL_CHARS)
            .collect()
    })
}

// ── Finalization policy (pure) ───────────────────────────────────────────────

/// What the end of a turn does with the stream.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Finalization {
    /// Send `abandoned`; nothing is posted.
    Abandon,
    /// Send `final`; nothing is posted (draft-only mode, or nothing to post).
    Final,
    /// Post the text as the reply unless the agent already replied in scope.
    AutopostUnlessReplied,
}

pub(crate) fn finalization(mode: StreamMode, stop: &StopReason, final_text: &str) -> Finalization {
    match stop {
        StopReason::EndTurn => {}
        StopReason::Cancelled
        | StopReason::MaxTokens
        | StopReason::MaxTurnRequests
        | StopReason::Refusal => return Finalization::Abandon,
    }
    match mode {
        StreamMode::DraftAutopost if !final_text.trim().is_empty() => {
            Finalization::AutopostUnlessReplied
        }
        StreamMode::Off | StreamMode::Draft | StreamMode::DraftAutopost => Finalization::Final,
    }
}

// ── Destination ──────────────────────────────────────────────────────────────

/// Where this turn's reply lands: the same destination the reply instruction
/// names (see `queue::turn_reply_thread`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ReplyTarget {
    pub channel_id: Uuid,
    /// `(root, parent)` event ids when the reply goes into a thread.
    pub thread: Option<(String, String)>,
}

impl ReplyTarget {
    fn thread_ref(&self) -> Option<ThreadRef> {
        let (root, parent) = self.thread.as_ref()?;
        Some(ThreadRef {
            root_event_id: EventId::from_hex(root).ok()?,
            parent_event_id: EventId::from_hex(parent).ok()?,
        })
    }

    fn root(&self) -> Option<&str> {
        self.thread.as_ref().map(|(root, _)| root.as_str())
    }
}

/// Whether an own kind:9 (relay JSON) is a reply landing in `target`'s scope:
/// same thread root, or both top-level. Our own autopost (same stream tag) is
/// never evidence that the agent replied by other means.
pub(crate) fn lands_in_scope(
    event: &serde_json::Value,
    target: &ReplyTarget,
    stream_id: &str,
) -> bool {
    let Some(tags) = event.get("tags").and_then(serde_json::Value::as_array) else {
        return false;
    };
    let parts: Vec<Vec<&str>> = tags
        .iter()
        .filter_map(|tag| {
            tag.as_array()
                .map(|t| t.iter().filter_map(serde_json::Value::as_str).collect())
        })
        .collect();
    let h_matches = parts.iter().any(|t| {
        t.first() == Some(&"h") && t.get(1).copied() == Some(target.channel_id.to_string().as_str())
    });
    let ours = parts
        .iter()
        .any(|t| t.first() == Some(&buzz_sdk::STREAM_TAG) && t.get(1) == Some(&stream_id));
    if !h_matches || ours {
        return false;
    }
    let root = buzz_core::nip10::parse_thread_markers_from_parts(parts.iter().map(Vec::as_slice))
        .resolve()
        .map(|(root, _)| root);
    root.as_deref() == target.root()
}

/// `p` tags for autoposted text, resolved the way `buzz messages send` does
/// (code regions ignored; `nostr:npub` URIs; `@Name` against current channel
/// members' display names) — except that an unresolved or ambiguous name is
/// left as plain text instead of failing the send, and never fans out.
pub(crate) fn resolve_reply_mentions(
    content: &str,
    member_pubkeys: &[String],
    member_profiles: &[(String, String)],
    author_hex: &str,
) -> Vec<String> {
    use buzz_sdk::mentions::{
        extract_at_mentions_with_known, extract_nostr_uris, match_names_to_profiles,
        normalize_mention_pubkeys, strip_code_regions, MentionProfile, MENTION_CAP,
    };
    let stripped = strip_code_regions(content);
    let mut pubkeys = extract_nostr_uris(&stripped);
    if stripped.contains('@') {
        let profiles: Vec<MentionProfile<'_>> = member_profiles
            .iter()
            .filter(|(pubkey, _)| member_pubkeys.contains(pubkey))
            .map(|(pubkey, content_json)| MentionProfile {
                pubkey,
                content_json,
            })
            .collect();
        let display_names: Vec<String> = member_profiles
            .iter()
            .filter_map(|(_, content_json)| {
                let value: serde_json::Value = serde_json::from_str(content_json).ok()?;
                value
                    .get("display_name")
                    .or_else(|| value.get("name"))
                    .and_then(serde_json::Value::as_str)
                    .filter(|name| !name.is_empty())
                    .map(str::to_owned)
            })
            .collect();
        let known: Vec<&str> = display_names.iter().map(String::as_str).collect();
        for name in extract_at_mentions_with_known(&stripped, &known) {
            if let [only] =
                match_names_to_profiles(std::slice::from_ref(&name), &profiles).as_slice()
            {
                pubkeys.push(only.clone());
            }
        }
    }
    let mut pubkeys = normalize_mention_pubkeys(&pubkeys, Some(author_hex));
    pubkeys.truncate(MENTION_CAP);
    pubkeys
}

// ── Runtime ──────────────────────────────────────────────────────────────────

/// Harness-wide streaming resources, shared (cloned) into every turn.
#[derive(Clone)]
pub struct StreamRuntime {
    mode: StreamMode,
    publisher: RelayEventPublisher,
    keys: Keys,
    budget: Arc<Mutex<FrameBudget>>,
}

impl StreamRuntime {
    /// `None` when streaming is off.
    pub fn new(mode: StreamMode, publisher: RelayEventPublisher, keys: Keys) -> Option<Self> {
        mode.is_enabled().then(|| Self {
            mode,
            publisher,
            keys,
            budget: Arc::new(Mutex::new(FrameBudget::new(
                FRAME_BURST,
                FRAMES_PER_SEC,
                Instant::now(),
            ))),
        })
    }

    async fn take_budget(&self) {
        loop {
            let wait = match lock(&self.budget).try_take(Instant::now()) {
                Ok(()) => return,
                Err(wait) => wait,
            };
            tokio::time::sleep(wait).await;
        }
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

#[derive(Default)]
struct SinkState {
    acc: ReplyAccumulator,
    terminal: Option<StreamDraftStatus>,
    flush: Option<oneshot::Sender<()>>,
}

/// Write side handed to the ACP read loop. All methods are synchronous,
/// non-blocking, and memory-bounded.
#[derive(Default)]
pub struct StreamSink {
    state: Mutex<SinkState>,
    notify: Notify,
}

impl StreamSink {
    fn update(&self, f: impl FnOnce(&mut ReplyAccumulator)) {
        {
            let mut state = lock(&self.state);
            if state.terminal.is_some() {
                return;
            }
            f(&mut state.acc);
        }
        self.notify.notify_one();
    }

    /// `agent_message_chunk` text.
    pub fn on_text(&self, chunk: &str) {
        self.update(|acc| acc.on_text(chunk));
    }

    /// `agent_thought_chunk` (status only).
    pub fn on_thought(&self) {
        self.update(ReplyAccumulator::on_thought);
    }

    /// `tool_call` start.
    pub fn on_tool_call(&self, tool_call_id: Option<&str>, title: Option<&str>) {
        self.update(|acc| acc.on_tool_call(tool_call_id, title));
    }

    /// `tool_call_update` that may carry a refined title.
    pub fn on_tool_title(&self, tool_call_id: Option<&str>, title: Option<&str>) {
        self.update(|acc| acc.on_tool_title(tool_call_id, title));
    }

    fn terminate(&self, status: StreamDraftStatus) {
        {
            let mut state = lock(&self.state);
            if state.terminal.is_none() {
                state.terminal = Some(status);
            }
        }
        self.notify.notify_one();
    }
}

/// What end-of-turn autopost did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum AutopostOutcome {
    /// The response text was posted as this kind:9 event id.
    Posted(String),
    /// The agent replied in scope itself; the draft was discarded.
    AlreadyReplied,
    /// Nothing was posted and the reply text is lost (logged and surfaced).
    Failed(String),
}

impl AutopostOutcome {
    fn terminal(&self) -> StreamDraftStatus {
        match self {
            Self::Posted(_) | Self::AlreadyReplied => StreamDraftStatus::Final,
            Self::Failed(_) => StreamDraftStatus::Abandoned,
        }
    }

    /// Observer payload for the turn's activity feed.
    pub(crate) fn observer_payload(&self) -> serde_json::Value {
        match self {
            Self::Posted(event_id) => serde_json::json!({"outcome": "posted", "eventId": event_id}),
            Self::AlreadyReplied => serde_json::json!({"outcome": "already_replied"}),
            Self::Failed(error) => serde_json::json!({"outcome": "failed", "error": error}),
        }
    }
}

/// One reply stream, owned by the turn. Drop without [`finish`](Self::finish)
/// abandons it.
pub struct ReplyStream {
    runtime: StreamRuntime,
    target: ReplyTarget,
    stream_id: Uuid,
    started_at: Timestamp,
    sink: Arc<StreamSink>,
    task: Option<tokio::task::JoinHandle<()>>,
}

impl ReplyStream {
    /// Start a stream and its publisher task.
    pub(crate) fn start(runtime: &StreamRuntime, target: ReplyTarget) -> Self {
        let stream_id = Uuid::new_v4();
        let sink = Arc::new(StreamSink::default());
        let task = tokio::spawn(publish_loop(
            runtime.clone(),
            target.clone(),
            stream_id,
            Arc::clone(&sink),
        ));
        Self {
            runtime: runtime.clone(),
            target,
            stream_id,
            started_at: Timestamp::now(),
            sink,
            task: Some(task),
        }
    }

    /// The sink the ACP client feeds for this turn.
    pub fn sink(&self) -> Arc<StreamSink> {
        Arc::clone(&self.sink)
    }

    /// End the stream for a turn that returned `stop`. In
    /// `draft+autopost` mode this may post the reply (kind:9 + `stream` tag).
    /// Returns the autopost outcome, if one was attempted, for the turn's
    /// observer feed.
    pub(crate) async fn finish(
        mut self,
        stop: &StopReason,
        rest: &RestClient,
    ) -> Option<AutopostOutcome> {
        let text = lock(&self.sink.state).acc.final_text();
        let (terminal, outcome) = match finalization(self.runtime.mode, stop, &text) {
            Finalization::Abandon => (StreamDraftStatus::Abandoned, None),
            Finalization::Final => {
                self.flush().await;
                (StreamDraftStatus::Final, None)
            }
            Finalization::AutopostUnlessReplied => {
                self.flush().await;
                let outcome = self.autopost(rest, &text).await;
                (outcome.terminal(), Some(outcome))
            }
        };
        self.sink.terminate(terminal);
        if let Some(task) = self.task.take() {
            if tokio::time::timeout(TERMINAL_TIMEOUT, task).await.is_err() {
                tracing::warn!(target: "stream_draft", "terminal draft frame timed out");
            }
        }
        outcome
    }

    /// Ask the publisher to emit any unsent snapshot now; bounded wait.
    async fn flush(&self) {
        let (tx, rx) = oneshot::channel();
        lock(&self.sink.state).flush = Some(tx);
        self.sink.notify.notify_one();
        let _ = tokio::time::timeout(FLUSH_TIMEOUT, rx).await;
    }

    /// Post `text` unless the agent already replied in scope.
    async fn autopost(&self, rest: &RestClient, text: &str) -> AutopostOutcome {
        match self.agent_replied(rest).await {
            Ok(true) => {
                tracing::info!(
                    target: "stream_draft",
                    stream = %self.stream_id,
                    "agent replied via CLI during the turn — draft discarded, not autoposting"
                );
                return AutopostOutcome::AlreadyReplied;
            }
            Ok(false) => {}
            Err(error) => {
                // Unknown whether a reply exists: posting could duplicate it,
                // so surface the failure and abandon visibly instead.
                tracing::warn!(
                    target: "stream_draft",
                    stream = %self.stream_id,
                    "self-reply check failed, not autoposting: {error}"
                );
                return AutopostOutcome::Failed(format!("self-reply check failed: {error}"));
            }
        }
        match self.post_reply(rest, text).await {
            Ok(event_id) => {
                tracing::info!(
                    target: "stream_draft",
                    stream = %self.stream_id,
                    event_id = %event_id,
                    "autoposted turn response as reply"
                );
                AutopostOutcome::Posted(event_id)
            }
            Err(error) => {
                tracing::error!(
                    target: "stream_draft",
                    stream = %self.stream_id,
                    "autopost failed, reply text not delivered: {error}"
                );
                AutopostOutcome::Failed(error)
            }
        }
    }

    async fn agent_replied(&self, rest: &RestClient) -> Result<bool, String> {
        let filter = nostr::Filter::new()
            .kind(Kind::Custom(buzz_core::kind::KIND_STREAM_MESSAGE as u16))
            .author(self.runtime.keys.public_key())
            .custom_tag(
                SingleLetterTag::lowercase(Alphabet::H),
                self.target.channel_id.to_string(),
            )
            .since(self.started_at)
            .limit(SELF_REPLY_QUERY_LIMIT);
        let response = tokio::time::timeout(FINALIZE_HTTP_TIMEOUT, rest.query(&[filter]))
            .await
            .map_err(|_| "query timed out".to_string())?
            .map_err(|e| e.to_string())?;
        let events = response
            .as_array()
            .ok_or_else(|| "query response is not an array".to_string())?;
        let stream_id = self.stream_id.to_string();
        Ok(events
            .iter()
            .any(|event| lands_in_scope(event, &self.target, &stream_id)))
    }

    async fn post_reply(&self, rest: &RestClient, text: &str) -> Result<String, String> {
        let author = self.runtime.keys.public_key().to_hex();
        let mentions = match self.member_profiles(rest, text).await {
            Ok((members, profiles)) => resolve_reply_mentions(text, &members, &profiles, &author),
            Err(error) => {
                // Degrade to explicit `nostr:npub` mentions rather than drop
                // the reply; @Name text stays readable but does not notify.
                tracing::warn!(
                    target: "stream_draft",
                    stream = %self.stream_id,
                    "mention lookup failed, posting without @Name p-tags: {error}"
                );
                resolve_reply_mentions(text, &[], &[], &author)
            }
        };
        let mention_refs: Vec<&str> = mentions.iter().map(String::as_str).collect();
        let thread_ref = self.target.thread_ref();
        if self.target.thread.is_some() && thread_ref.is_none() {
            // Never post a thread reply top-level.
            return Err(format!("invalid reply thread ids {:?}", self.target.thread));
        }
        let stream_tag = buzz_sdk::stream_tag(self.stream_id).map_err(|e| e.to_string())?;
        let event = buzz_sdk::build_message(
            self.target.channel_id,
            text,
            thread_ref.as_ref(),
            &mention_refs,
            false,
            &[],
            &[],
        )
        .map_err(|e| e.to_string())?
        .tag(stream_tag)
        .sign_with_keys(&self.runtime.keys)
        .map_err(|e| e.to_string())?;
        let response = tokio::time::timeout(FINALIZE_HTTP_TIMEOUT, rest.submit_event(&event))
            .await
            .map_err(|_| "submit timed out".to_string())?
            .map_err(|e| e.to_string())?;
        if response
            .get("accepted")
            .and_then(serde_json::Value::as_bool)
            == Some(false)
        {
            return Err(format!(
                "relay rejected reply: {}",
                response
                    .get("message")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("")
            ));
        }
        Ok(event.id.to_hex())
    }

    /// Current channel members and their kind:0 profile content, only when
    /// the text could carry an `@Name`.
    async fn member_profiles(
        &self,
        rest: &RestClient,
        text: &str,
    ) -> Result<(Vec<String>, Vec<(String, String)>), String> {
        if !buzz_sdk::mentions::strip_code_regions(text).contains('@') {
            return Ok((Vec::new(), Vec::new()));
        }
        let members_filter = nostr::Filter::new()
            .kind(Kind::Custom(
                buzz_core::kind::KIND_NIP29_GROUP_MEMBERS as u16,
            ))
            .custom_tag(
                SingleLetterTag::lowercase(Alphabet::D),
                self.target.channel_id.to_string(),
            )
            .limit(1);
        let members = query_events(rest, members_filter).await?;
        let member_pubkeys: Vec<String> = members
            .first()
            .and_then(|event| event.get("tags"))
            .and_then(serde_json::Value::as_array)
            .map(|tags| {
                tags.iter()
                    .filter_map(|tag| {
                        let tag = tag.as_array()?;
                        (tag.first()?.as_str()? == "p")
                            .then(|| nostr::PublicKey::from_hex(tag.get(1)?.as_str()?).ok())
                            .flatten()
                            .map(|pk| pk.to_hex())
                    })
                    .collect()
            })
            .unwrap_or_default();
        if member_pubkeys.is_empty() {
            return Ok((member_pubkeys, Vec::new()));
        }
        let authors: Vec<nostr::PublicKey> = member_pubkeys
            .iter()
            .filter_map(|pk| nostr::PublicKey::from_hex(pk).ok())
            .collect();
        let profiles_filter = nostr::Filter::new()
            .kind(Kind::Metadata)
            .authors(authors)
            .limit(member_pubkeys.len());
        let profiles = query_events(rest, profiles_filter)
            .await?
            .iter()
            .filter_map(|event| {
                Some((
                    event.get("pubkey")?.as_str()?.to_ascii_lowercase(),
                    event.get("content")?.as_str()?.to_owned(),
                ))
            })
            .collect();
        Ok((member_pubkeys, profiles))
    }
}

impl Drop for ReplyStream {
    fn drop(&mut self) {
        // No-op after `finish` (terminal already set); otherwise the turn
        // ended on an error/cancel path and the publisher sends `abandoned`.
        self.sink.terminate(StreamDraftStatus::Abandoned);
    }
}

async fn query_events(
    rest: &RestClient,
    filter: nostr::Filter,
) -> Result<Vec<serde_json::Value>, String> {
    let response = tokio::time::timeout(FINALIZE_HTTP_TIMEOUT, rest.query(&[filter]))
        .await
        .map_err(|_| "query timed out".to_string())?
        .map_err(|e| e.to_string())?;
    response
        .as_array()
        .cloned()
        .ok_or_else(|| "query response is not an array".to_string())
}

/// Read-and-reset view of the sink for one publisher iteration.
struct Pending {
    state: Option<FrameState>,
    terminal: Option<StreamDraftStatus>,
    flush: Option<oneshot::Sender<()>>,
}

fn take_pending(sink: &StreamSink) -> Pending {
    let mut state = lock(&sink.state);
    Pending {
        state: state.acc.frame_state(),
        terminal: state.terminal,
        flush: state.flush.take(),
    }
}

/// Publisher task: emits throttled frames until a terminal status is set,
/// then sends the terminal frame (only if the stream ever emitted one).
async fn publish_loop(
    runtime: StreamRuntime,
    target: ReplyTarget,
    stream_id: Uuid,
    sink: Arc<StreamSink>,
) {
    let thread_ref = target.thread_ref();
    let mut seq: u64 = 0;
    let mut last: Option<Emitted> = None;
    loop {
        let notified = sink.notify.notified();
        tokio::pin!(notified);
        notified.as_mut().enable();

        let pending = take_pending(&sink);
        if let Some(terminal) = pending.terminal {
            if seq > 0 {
                runtime.take_budget().await;
                seq += 1;
                publish_frame(
                    &runtime,
                    &target,
                    thread_ref.as_ref(),
                    stream_id,
                    seq,
                    terminal,
                    None,
                    "",
                );
            }
            return;
        }
        if let Some(ack) = pending.flush {
            let unsent = pending
                .state
                .is_some_and(|state| last.is_none_or(|last| last.state != state));
            if unsent {
                runtime.take_budget().await;
                if let Some(emitted) = emit_current(
                    &runtime,
                    &target,
                    thread_ref.as_ref(),
                    stream_id,
                    &sink,
                    &mut seq,
                ) {
                    last = Some(emitted);
                }
            }
            let _ = ack.send(());
            continue;
        }
        match throttle(last.as_ref(), pending.state.as_ref(), Instant::now()) {
            None => notified.await,
            Some(ThrottleAction::Emit) => {
                runtime.take_budget().await;
                if let Some(emitted) = emit_current(
                    &runtime,
                    &target,
                    thread_ref.as_ref(),
                    stream_id,
                    &sink,
                    &mut seq,
                ) {
                    last = Some(emitted);
                }
            }
            Some(ThrottleAction::WaitUntil(at)) => {
                tokio::select! {
                    _ = tokio::time::sleep_until(at) => {}
                    _ = notified => {}
                }
            }
        }
    }
}

/// Build and publish a frame from the sink's state at emission time.
fn emit_current(
    runtime: &StreamRuntime,
    target: &ReplyTarget,
    thread_ref: Option<&ThreadRef>,
    stream_id: Uuid,
    sink: &StreamSink,
    seq: &mut u64,
) -> Option<Emitted> {
    let (state, content, label) = {
        let guard = lock(&sink.state);
        (
            guard.acc.frame_state()?,
            guard.acc.snapshot(),
            guard.acc.label().map(str::to_owned),
        )
    };
    *seq += 1;
    publish_frame(
        runtime,
        target,
        thread_ref,
        stream_id,
        *seq,
        state.status.wire(),
        label.as_deref(),
        &content,
    );
    Some(Emitted {
        at: Instant::now(),
        state,
    })
}

#[allow(clippy::too_many_arguments)]
fn publish_frame(
    runtime: &StreamRuntime,
    target: &ReplyTarget,
    thread_ref: Option<&ThreadRef>,
    stream_id: Uuid,
    seq: u64,
    status: StreamDraftStatus,
    label: Option<&str>,
    content: &str,
) {
    let draft = StreamDraft {
        channel_id: target.channel_id,
        stream_id,
        seq,
        status,
        thread_ref,
        label,
        content,
    };
    let event = match buzz_sdk::build_stream_draft(&draft)
        .map_err(|e| e.to_string())
        .and_then(|b| b.sign_with_keys(&runtime.keys).map_err(|e| e.to_string()))
    {
        Ok(event) => event,
        Err(error) => {
            tracing::warn!(target: "stream_draft", %stream_id, seq, "draft frame build failed: {error}");
            return;
        }
    };
    // Fire-and-forget like typing indicators: a dropped frame is healed by the
    // next cumulative snapshot (or by the client's TTL for the terminal one).
    if let Err(error) = runtime.publisher.try_publish_event(event) {
        tracing::debug!(target: "stream_draft", %stream_id, seq, "draft frame dropped: {error}");
    }
}

/// Minimal fake relay HTTP bridge for finalization tests (`/query`, `/events`).
#[cfg(test)]
pub(crate) mod test_relay {
    use std::sync::{Arc, Mutex};

    use axum::{body::Bytes, http::StatusCode, routing::post, Json, Router};
    use serde_json::{json, Value};

    /// What the fake bridge answers and what it saw.
    #[derive(Clone, Default)]
    pub(crate) struct FakeRelay {
        pub base_url: String,
        /// Answer for the agent's own kind:9 lookup (`authors` + `kinds:[9]`).
        pub self_events: Arc<Mutex<Vec<Value>>>,
        /// Fail the own-kind:9 lookup with HTTP 500.
        pub fail_self_query: Arc<Mutex<bool>>,
        /// kind:39002 members event and kind:0 profiles for mention lookup.
        pub members: Arc<Mutex<Vec<Value>>>,
        pub profiles: Arc<Mutex<Vec<Value>>>,
        /// Every submitted event (`POST /events`).
        pub submitted: Arc<Mutex<Vec<Value>>>,
        /// Every `/query` body.
        pub queries: Arc<Mutex<Vec<Value>>>,
    }

    impl FakeRelay {
        pub(crate) async fn spawn() -> Self {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
                .await
                .expect("bind fake relay");
            let relay = Self {
                base_url: format!("http://{}", listener.local_addr().expect("addr")),
                ..Self::default()
            };
            let state = relay.clone();
            let query_state = state.clone();
            let app = Router::new()
                .route(
                    "/query",
                    post(move |body: Bytes| {
                        let state = query_state.clone();
                        async move { state.answer_query(&body) }
                    }),
                )
                .route(
                    "/events",
                    post(move |body: Bytes| {
                        let state = state.clone();
                        async move {
                            let event: Value = serde_json::from_slice(&body).unwrap_or(Value::Null);
                            let id = event["id"].clone();
                            state.submitted.lock().unwrap().push(event);
                            Json(json!({"event_id": id, "accepted": true, "message": ""}))
                        }
                    }),
                );
            tokio::spawn(async move {
                let _ = axum::serve(listener, app).await;
            });
            relay
        }

        fn answer_query(&self, body: &[u8]) -> (StatusCode, Json<Value>) {
            let filters: Value = serde_json::from_slice(body).unwrap_or(Value::Null);
            self.queries.lock().unwrap().push(filters.clone());
            let filter = &filters[0];
            let kinds: Vec<u64> = filter["kinds"]
                .as_array()
                .map(|k| k.iter().filter_map(Value::as_u64).collect())
                .unwrap_or_default();
            if filter.get("authors").is_some() && kinds == [9] {
                if *self.fail_self_query.lock().unwrap() {
                    return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({})));
                }
                return (
                    StatusCode::OK,
                    Json(Value::Array(self.self_events.lock().unwrap().clone())),
                );
            }
            if kinds == [39002] {
                return (
                    StatusCode::OK,
                    Json(Value::Array(self.members.lock().unwrap().clone())),
                );
            }
            if kinds == [0] {
                return (
                    StatusCode::OK,
                    Json(Value::Array(self.profiles.lock().unwrap().clone())),
                );
            }
            (StatusCode::OK, Json(json!([])))
        }

        pub(crate) fn rest(&self, keys: &nostr::Keys) -> crate::relay::RestClient {
            crate::relay::RestClient {
                http: reqwest::Client::new(),
                base_url: self.base_url.clone(),
                keys: keys.clone(),
                auth_tag_json: None,
            }
        }

        /// Submitted kind:9 events (ignores reactions and other kinds).
        pub(crate) fn posted_messages(&self) -> Vec<Value> {
            self.submitted
                .lock()
                .unwrap()
                .iter()
                .filter(|e| e["kind"] == 9)
                .cloned()
                .collect()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::relay::RelayEventPublisher;
    use serde_json::json;
    use test_relay::FakeRelay;

    fn state(status: LiveStatus, segment: u64, label_rev: u64, chars: usize) -> FrameState {
        FrameState {
            status,
            segment,
            label_rev,
            chars,
        }
    }

    // ── throttle table ───────────────────────────────────────────────────

    #[test]
    fn throttle_policy_table() {
        let t0 = Instant::now();
        let ms = |n: u64| t0 + Duration::from_millis(n);
        let writing = |chars| state(LiveStatus::Writing, 0, 0, chars);
        let last = Emitted {
            at: t0,
            state: writing(10),
        };
        type Case = (
            &'static str,
            Option<Emitted>,
            Option<FrameState>,
            Instant,
            Option<ThrottleAction>,
        );
        let cases: Vec<Case> = vec![
            ("nothing observed", None, None, t0, None),
            (
                "first frame is immediate",
                None,
                Some(writing(1)),
                t0,
                Some(ThrottleAction::Emit),
            ),
            (
                "first status frame is immediate",
                None,
                Some(state(LiveStatus::Thinking, 0, 0, 0)),
                t0,
                Some(ThrottleAction::Emit),
            ),
            (
                "24 new chars inside 200ms waits for the interval",
                Some(last),
                Some(writing(34)),
                ms(50),
                Some(ThrottleAction::WaitUntil(ms(200))),
            ),
            (
                "24 new chars after 200ms emits",
                Some(last),
                Some(writing(34)),
                ms(200),
                Some(ThrottleAction::Emit),
            ),
            (
                "23 new chars after 200ms waits for the idle flush",
                Some(last),
                Some(writing(33)),
                ms(250),
                Some(ThrottleAction::WaitUntil(ms(1000))),
            ),
            (
                "23 new chars after idle flush emits",
                Some(last),
                Some(writing(33)),
                ms(1000),
                Some(ThrottleAction::Emit),
            ),
            (
                "status change bypasses the char threshold",
                Some(last),
                Some(state(LiveStatus::Thinking, 0, 0, 10)),
                ms(200),
                Some(ThrottleAction::Emit),
            ),
            (
                "status change still honors the interval",
                Some(last),
                Some(state(LiveStatus::Tool, 1, 0, 0)),
                ms(10),
                Some(ThrottleAction::WaitUntil(ms(200))),
            ),
            (
                "new segment (tool call) counts as a change",
                Some(Emitted {
                    at: t0,
                    state: state(LiveStatus::Tool, 1, 0, 0),
                }),
                Some(state(LiveStatus::Tool, 2, 0, 0)),
                ms(200),
                Some(ThrottleAction::Emit),
            ),
            (
                "refined tool label counts as a change",
                Some(Emitted {
                    at: t0,
                    state: state(LiveStatus::Tool, 1, 0, 0),
                }),
                Some(state(LiveStatus::Tool, 1, 1, 0)),
                ms(200),
                Some(ThrottleAction::Emit),
            ),
            (
                "unchanged state waits for keepalive",
                Some(last),
                Some(writing(10)),
                ms(1000),
                Some(ThrottleAction::WaitUntil(ms(5000))),
            ),
            (
                "unchanged state re-sends at keepalive",
                Some(last),
                Some(writing(10)),
                ms(5000),
                Some(ThrottleAction::Emit),
            ),
        ];
        for (name, last, current, now, expected) in cases {
            assert_eq!(
                throttle(last.as_ref(), current.as_ref(), now),
                expected,
                "{name}"
            );
        }
    }

    #[test]
    fn frame_budget_caps_burst_and_rate() {
        let t0 = Instant::now();
        let mut budget = FrameBudget::new(FRAME_BURST, FRAMES_PER_SEC, t0);
        for _ in 0..FRAME_BURST as usize {
            assert!(budget.try_take(t0).is_ok());
        }
        let wait = budget.try_take(t0).expect_err("burst exhausted");
        assert!(wait > Duration::ZERO && wait <= Duration::from_secs_f64(1.0 / FRAMES_PER_SEC));
        // Over one second at most FRAMES_PER_SEC more frames are admitted.
        let mut admitted = 0;
        for step in 1..=100u64 {
            if budget
                .try_take(t0 + Duration::from_millis(step * 10))
                .is_ok()
            {
                admitted += 1;
            }
        }
        assert_eq!(admitted, FRAMES_PER_SEC as usize);
    }

    // ── segmentation ─────────────────────────────────────────────────────

    #[test]
    fn segmentation_resets_at_tool_calls_and_keeps_last_answer() {
        let mut acc = ReplyAccumulator::default();
        assert!(acc.frame_state().is_none());
        acc.on_thought();
        assert_eq!(acc.frame_state().unwrap().status, LiveStatus::Thinking);
        acc.on_text("Let me check the file.");
        assert_eq!(acc.snapshot(), "Let me check the file.");
        acc.on_tool_call(Some("t1"), Some("Read src/lib.rs"));
        let tool = acc.frame_state().unwrap();
        assert_eq!(
            (tool.status, tool.segment, tool.chars),
            (LiveStatus::Tool, 1, 0)
        );
        assert_eq!(acc.snapshot(), "", "tool frames carry no narration");
        assert_eq!(acc.label(), Some("Read src/lib.rs"));
        acc.on_text("The answer ");
        acc.on_text("is 42.");
        assert_eq!(acc.snapshot(), "The answer is 42.");
        assert_eq!(acc.label(), None, "labels ride only on tool frames");
        assert_eq!(acc.final_text(), "The answer is 42.");

        // A trailing bookkeeping tool call must not lose the answer.
        acc.on_tool_call(Some("t2"), Some("buzz mem set"));
        assert_eq!(acc.snapshot(), "");
        assert_eq!(acc.final_text(), "The answer is 42.");

        // A whitespace-only trailing segment also falls back.
        acc.on_text("\n  ");
        assert_eq!(acc.final_text(), "The answer is 42.");
    }

    #[test]
    fn thoughts_never_enter_content() {
        let mut acc = ReplyAccumulator::default();
        acc.on_text("visible");
        acc.on_thought();
        let s = acc.frame_state().unwrap();
        assert_eq!(s.status, LiveStatus::Thinking);
        assert_eq!(acc.snapshot(), "visible");
        assert_eq!(s.chars, 7);
    }

    #[test]
    fn tool_title_refinement_only_for_the_running_tool() {
        let mut acc = ReplyAccumulator::default();
        acc.on_tool_call(Some("t1"), Some("Terminal"));
        acc.on_tool_title(Some("other"), Some("ls"));
        assert_eq!(acc.label(), Some("Terminal"));
        acc.on_tool_title(Some("t1"), None);
        assert_eq!(acc.frame_state().unwrap().label_rev, 0);
        acc.on_tool_title(Some("t1"), Some("cargo test -p buzz-acp"));
        assert_eq!(acc.label(), Some("cargo test -p buzz-acp"));
        assert_eq!(acc.frame_state().unwrap().label_rev, 1);
        acc.on_text("done");
        acc.on_tool_title(Some("t1"), Some("late"));
        assert_eq!(
            acc.frame_state().unwrap().label_rev,
            1,
            "ignored once writing"
        );
    }

    #[test]
    fn accumulated_text_is_bounded_and_truncated() {
        let mut acc = ReplyAccumulator::default();
        let chunk = "ß".repeat(4096);
        for _ in 0..64 {
            acc.on_text(&chunk);
        }
        assert!(acc.text.len() <= TEXT_RETAIN_BYTES);
        let snap = acc.snapshot();
        assert!(snap.len() <= buzz_sdk::MAX_STREAM_DRAFT_CONTENT_BYTES);
        assert!(snap.ends_with('…'));
        assert_eq!(acc.final_text(), snap);
    }

    // ── finalization table ───────────────────────────────────────────────

    #[test]
    fn finalization_table() {
        use Finalization::*;
        let cases = [
            (StreamMode::Draft, StopReason::EndTurn, "answer", Final),
            (StreamMode::Draft, StopReason::Cancelled, "answer", Abandon),
            (
                StreamMode::DraftAutopost,
                StopReason::EndTurn,
                "answer",
                AutopostUnlessReplied,
            ),
            (
                StreamMode::DraftAutopost,
                StopReason::EndTurn,
                "  \n",
                Final,
            ),
            (StreamMode::DraftAutopost, StopReason::EndTurn, "", Final),
            (
                StreamMode::DraftAutopost,
                StopReason::Cancelled,
                "answer",
                Abandon,
            ),
            (
                StreamMode::DraftAutopost,
                StopReason::MaxTokens,
                "answer",
                Abandon,
            ),
            (
                StreamMode::DraftAutopost,
                StopReason::MaxTurnRequests,
                "answer",
                Abandon,
            ),
            (
                StreamMode::DraftAutopost,
                StopReason::Refusal,
                "answer",
                Abandon,
            ),
        ];
        for (mode, stop, text, expected) in cases {
            assert_eq!(
                finalization(mode, &stop, text),
                expected,
                "{mode} {stop:?} {text:?}"
            );
        }
    }

    // ── scope matching ───────────────────────────────────────────────────

    fn own_kind9(channel: Uuid, extra: serde_json::Value) -> serde_json::Value {
        let mut tags = vec![json!(["h", channel.to_string()])];
        tags.extend(extra.as_array().cloned().unwrap_or_default());
        json!({"kind": 9, "tags": tags})
    }

    #[test]
    fn lands_in_scope_matches_destination_thread_only() {
        let channel = Uuid::new_v4();
        let root = "a".repeat(64);
        let other_root = "b".repeat(64);
        let nested_parent = "c".repeat(64);
        let stream = Uuid::new_v4().to_string();
        let thread = ReplyTarget {
            channel_id: channel,
            thread: Some((root.clone(), root.clone())),
        };
        let top = ReplyTarget {
            channel_id: channel,
            thread: None,
        };
        let direct = own_kind9(channel, json!([["e", root, "", "reply"]]));
        let nested = own_kind9(
            channel,
            json!([["e", root, "", "root"], ["e", nested_parent, "", "reply"]]),
        );
        let elsewhere = own_kind9(channel, json!([["e", other_root, "", "reply"]]));
        let top_level = own_kind9(channel, json!([]));
        let other_channel = own_kind9(Uuid::new_v4(), json!([["e", root, "", "reply"]]));
        let our_autopost = own_kind9(
            channel,
            json!([["e", root, "", "reply"], ["stream", stream]]),
        );

        assert!(lands_in_scope(&direct, &thread, &stream));
        assert!(lands_in_scope(&nested, &thread, &stream));
        assert!(!lands_in_scope(&elsewhere, &thread, &stream));
        assert!(!lands_in_scope(&top_level, &thread, &stream));
        assert!(!lands_in_scope(&other_channel, &thread, &stream));
        assert!(!lands_in_scope(&our_autopost, &thread, &stream));
        assert!(lands_in_scope(&top_level, &top, &stream));
        assert!(!lands_in_scope(&direct, &top, &stream));
    }

    // ── mentions ─────────────────────────────────────────────────────────

    #[test]
    fn reply_mentions_resolve_unique_members_only() {
        let alice = "a".repeat(64);
        let bob1 = "b".repeat(64);
        let bob2 = "c".repeat(64);
        let outsider = "d".repeat(64);
        let me = "e".repeat(64);
        let profile = |name: &str| json!({ "display_name": name }).to_string();
        let members = vec![alice.clone(), bob1.clone(), bob2.clone(), me.clone()];
        let profiles = vec![
            (alice.clone(), profile("Alice Smith")),
            (bob1.clone(), profile("Bob")),
            (bob2.clone(), profile("Bob")),
            (outsider.clone(), profile("Carol")),
            (me.clone(), profile("Agent")),
        ];
        let text = "Done, @Alice Smith. @Bob @Carol @Agent `@Alice Smith` @nobody";
        assert_eq!(
            resolve_reply_mentions(text, &members, &profiles, &me),
            vec![alice.clone()],
            "unique member resolves; ambiguous, non-member, self, code and unknown do not"
        );

        let npub = nostr::PublicKey::from_hex(&outsider).unwrap();
        let npub = nostr::ToBech32::to_bech32(&npub).unwrap();
        assert_eq!(
            resolve_reply_mentions(&format!("cc nostr:{npub}"), &[], &[], &me),
            vec![outsider],
            "explicit nostr:npub URIs are kept even without member data"
        );
    }

    // ── config ───────────────────────────────────────────────────────────

    #[test]
    fn stream_mode_parses_wire_values() {
        use clap::ValueEnum;
        for (raw, mode) in [
            ("off", StreamMode::Off),
            ("draft", StreamMode::Draft),
            ("draft+autopost", StreamMode::DraftAutopost),
        ] {
            assert_eq!(StreamMode::from_str(raw, false), Ok(mode));
            assert_eq!(mode.to_string(), raw);
        }
        assert!(StreamMode::from_str("autopost", false).is_err());
        assert!(!StreamMode::Off.is_enabled());
        assert!(StreamMode::Draft.is_enabled());
        assert!(StreamMode::DraftAutopost.is_enabled());
    }

    // ── publisher + finalization (production seams) ──────────────────────

    struct Frame {
        seq: u64,
        status: String,
        content: String,
        label: Option<String>,
        event: nostr::Event,
    }

    fn tag(event: &nostr::Event, name: &str) -> Option<String> {
        event.tags.iter().find_map(|t| {
            let s = t.as_slice();
            (s.first().map(String::as_str) == Some(name))
                .then(|| s.get(1).cloned())
                .flatten()
        })
    }

    fn frame(event: nostr::Event) -> Frame {
        Frame {
            seq: tag(&event, "seq").unwrap().parse().unwrap(),
            status: tag(&event, "status").unwrap(),
            content: event.content.clone(),
            label: tag(&event, "label"),
            event,
        }
    }

    async fn drain(rx: &mut tokio::sync::mpsc::Receiver<nostr::Event>) -> Vec<Frame> {
        let mut frames = Vec::new();
        while let Ok(Some(event)) =
            tokio::time::timeout(Duration::from_millis(300), rx.recv()).await
        {
            frames.push(frame(event));
        }
        frames
    }

    fn runtime(
        mode: StreamMode,
        keys: &Keys,
    ) -> (StreamRuntime, tokio::sync::mpsc::Receiver<nostr::Event>) {
        let (publisher, rx) = RelayEventPublisher::test_pair();
        (
            StreamRuntime::new(mode, publisher, keys.clone()).expect("enabled"),
            rx,
        )
    }

    fn thread_target(channel: Uuid, root: &str) -> ReplyTarget {
        ReplyTarget {
            channel_id: channel,
            thread: Some((root.to_string(), root.to_string())),
        }
    }

    fn assert_stream_invariants(frames: &[Frame], channel: Uuid, keys: &Keys) -> String {
        let stream = tag(&frames[0].event, "stream").unwrap();
        for (i, f) in frames.iter().enumerate() {
            assert_eq!(
                f.event.kind.as_u16() as u32,
                buzz_core::kind::KIND_STREAM_DRAFT
            );
            assert_eq!(f.event.pubkey, keys.public_key());
            assert_eq!(tag(&f.event, "h"), Some(channel.to_string()));
            assert_eq!(tag(&f.event, "stream").as_ref(), Some(&stream));
            assert_eq!(
                f.seq,
                i as u64 + 1,
                "seq is 1-based and gapless at the source"
            );
        }
        stream
    }

    #[tokio::test]
    async fn draft_mode_streams_status_text_and_final() {
        let keys = Keys::generate();
        let channel = Uuid::new_v4();
        let root = "f".repeat(64);
        let (runtime, mut rx) = runtime(StreamMode::Draft, &keys);
        let relay = FakeRelay::spawn().await;
        let stream = ReplyStream::start(&runtime, thread_target(channel, &root));
        let sink = stream.sink();

        sink.on_thought();
        tokio::time::sleep(Duration::from_millis(250)).await;
        sink.on_text("Checking.");
        tokio::time::sleep(Duration::from_millis(250)).await;
        sink.on_tool_call(Some("t1"), Some("Run tests"));
        tokio::time::sleep(Duration::from_millis(250)).await;
        // A post-tool burst arrives at once right before end_turn.
        for word in ["All ", "tests ", "pass: ", "1248 ", "passed, ", "0 failed."] {
            sink.on_text(word);
        }
        stream
            .finish(&StopReason::EndTurn, &relay.rest(&keys))
            .await;

        let frames = drain(&mut rx).await;
        assert_stream_invariants(&frames, channel, &keys);
        let statuses: Vec<&str> = frames.iter().map(|f| f.status.as_str()).collect();
        assert_eq!(statuses.first(), Some(&"thinking"));
        assert!(statuses.contains(&"tool"));
        assert_eq!(statuses.last(), Some(&"final"));
        let tool = frames.iter().find(|f| f.status == "tool").unwrap();
        assert_eq!(tool.label.as_deref(), Some("Run tests"));
        assert_eq!(tool.content, "");
        let writing: Vec<&Frame> = frames.iter().filter(|f| f.status == "writing").collect();
        assert_eq!(
            writing.last().unwrap().content,
            "All tests pass: 1248 passed, 0 failed.",
            "the end-of-turn flush publishes the complete snapshot"
        );
        let post_tool = frames
            .iter()
            .skip_while(|f| f.status != "tool")
            .filter(|f| f.status == "writing")
            .count();
        assert!(
            post_tool <= 2,
            "a burst yields at most two frames, got {post_tool}"
        );
        assert!(frames.last().unwrap().content.is_empty());
        for f in &frames {
            assert_eq!(
                tag(&f.event, "e"),
                Some(root.clone()),
                "every frame targets the reply thread"
            );
        }
        assert!(relay.posted_messages().is_empty(), "draft mode never posts");
    }

    #[tokio::test]
    async fn keepalive_refreshes_a_long_tool_call() {
        let keys = Keys::generate();
        let (runtime, mut rx) = runtime(StreamMode::Draft, &keys);
        let stream = ReplyStream::start(&runtime, thread_target(Uuid::new_v4(), &"f".repeat(64)));
        stream.sink().on_tool_call(Some("t1"), Some("cargo build"));
        tokio::time::sleep(KEEPALIVE_INTERVAL + Duration::from_millis(500)).await;
        drop(stream);
        let frames = drain(&mut rx).await;
        let tools: Vec<&Frame> = frames.iter().filter(|f| f.status == "tool").collect();
        assert_eq!(tools.len(), 2, "initial frame plus one keepalive");
        assert!(tools
            .iter()
            .all(|f| f.label.as_deref() == Some("cargo build")));
        assert_eq!(frames.last().unwrap().status, "abandoned");
    }

    #[tokio::test]
    async fn dropped_stream_abandons_and_silent_stream_sends_nothing() {
        let keys = Keys::generate();
        let (runtime, mut rx) = runtime(StreamMode::DraftAutopost, &keys);
        let target = thread_target(Uuid::new_v4(), &"f".repeat(64));

        // Never emitted a frame → no terminal frame either.
        drop(ReplyStream::start(&runtime, target.clone()));
        assert!(drain(&mut rx).await.is_empty());

        let stream = ReplyStream::start(&runtime, target);
        stream.sink().on_text("partial answer that will be cut");
        tokio::time::sleep(Duration::from_millis(100)).await;
        drop(stream);
        let frames = drain(&mut rx).await;
        assert_eq!(frames.last().unwrap().status, "abandoned");
        assert!(frames.last().unwrap().content.is_empty());
    }

    async fn autopost_case(
        relay: &FakeRelay,
        stop: StopReason,
        text: &str,
    ) -> (Vec<Frame>, Keys, Uuid, String, Option<AutopostOutcome>) {
        let keys = Keys::generate();
        let channel = Uuid::new_v4();
        let root = "f".repeat(64);
        let (runtime, mut rx) = runtime(StreamMode::DraftAutopost, &keys);
        let stream = ReplyStream::start(&runtime, thread_target(channel, &root));
        stream.sink().on_tool_call(Some("t1"), Some("Search"));
        stream.sink().on_text(text);
        // Let the live frames go out before the turn ends.
        tokio::time::sleep(Duration::from_millis(50)).await;
        let outcome = stream.finish(&stop, &relay.rest(&keys)).await;
        (drain(&mut rx).await, keys, channel, root, outcome)
    }

    #[tokio::test]
    async fn autopost_posts_kind9_with_stream_tag_when_agent_did_not_reply() {
        let relay = FakeRelay::spawn().await;
        let (frames, keys, channel, root, outcome) =
            autopost_case(&relay, StopReason::EndTurn, "Here is the answer.").await;
        let stream = assert_stream_invariants(&frames, channel, &keys);
        assert_eq!(frames.last().unwrap().status, "final");
        let posted_id = relay.posted_messages()[0]["id"]
            .as_str()
            .unwrap()
            .to_string();
        assert_eq!(outcome, Some(AutopostOutcome::Posted(posted_id)));

        let posted = relay.posted_messages();
        assert_eq!(posted.len(), 1, "exactly one autoposted reply");
        let reply = &posted[0];
        assert_eq!(reply["content"], "Here is the answer.");
        assert_eq!(reply["pubkey"], keys.public_key().to_hex());
        let tags = reply["tags"].as_array().unwrap();
        assert!(tags.contains(&json!(["h", channel.to_string()])));
        assert!(tags.contains(&json!(["e", root, "", "reply"])));
        assert!(tags.contains(&json!(["stream", stream])));

        // The self-reply check is scoped to this agent, channel and turn.
        let queries = relay.queries.lock().unwrap().clone();
        let check = queries
            .iter()
            .map(|q| &q[0])
            .find(|f| f.get("authors").is_some())
            .expect("self-reply query");
        assert_eq!(check["authors"], json!([keys.public_key().to_hex()]));
        assert_eq!(check["kinds"], json!([9]));
        assert_eq!(check["#h"], json!([channel.to_string()]));
        assert!(check["since"].as_u64().is_some());
    }

    #[tokio::test]
    async fn autopost_discards_when_agent_replied_in_scope() {
        let relay = FakeRelay::spawn().await;
        let keys = Keys::generate();
        let channel = Uuid::new_v4();
        let root = "f".repeat(64);
        relay.self_events.lock().unwrap().push(json!({
            "kind": 9,
            "tags": [["h", channel.to_string()], ["e", root, "", "reply"]],
        }));
        let (runtime, mut rx) = runtime(StreamMode::DraftAutopost, &keys);
        let stream = ReplyStream::start(&runtime, thread_target(channel, &root));
        stream.sink().on_text("Duplicate of what I already sent.");
        let outcome = stream
            .finish(&StopReason::EndTurn, &relay.rest(&keys))
            .await;
        assert_eq!(outcome, Some(AutopostOutcome::AlreadyReplied));

        let frames = drain(&mut rx).await;
        assert_eq!(frames.last().unwrap().status, "final");
        assert!(relay.posted_messages().is_empty(), "agent's CLI reply wins");
    }

    #[tokio::test]
    async fn autopost_ignores_own_messages_in_other_threads() {
        let relay = FakeRelay::spawn().await;
        let keys = Keys::generate();
        let channel = Uuid::new_v4();
        let root = "f".repeat(64);
        relay.self_events.lock().unwrap().push(json!({
            "kind": 9,
            "tags": [["h", channel.to_string()], ["e", "1".repeat(64), "", "reply"]],
        }));
        let (runtime, _rx) = runtime(StreamMode::DraftAutopost, &keys);
        let stream = ReplyStream::start(&runtime, thread_target(channel, &root));
        stream.sink().on_text("The real answer.");
        stream
            .finish(&StopReason::EndTurn, &relay.rest(&keys))
            .await;
        assert_eq!(relay.posted_messages().len(), 1);
    }

    #[tokio::test]
    async fn autopost_abandons_when_self_reply_check_fails() {
        let relay = FakeRelay::spawn().await;
        *relay.fail_self_query.lock().unwrap() = true;
        let (frames, .., outcome) = autopost_case(&relay, StopReason::EndTurn, "answer").await;
        assert_eq!(frames.last().unwrap().status, "abandoned");
        assert!(relay.posted_messages().is_empty(), "never risk a duplicate");
        assert!(
            matches!(outcome, Some(AutopostOutcome::Failed(_))),
            "the lost reply is surfaced: {outcome:?}"
        );
    }

    #[tokio::test]
    async fn autopost_skips_cancelled_and_truncated_turns() {
        for stop in [
            StopReason::Cancelled,
            StopReason::MaxTokens,
            StopReason::MaxTurnRequests,
            StopReason::Refusal,
        ] {
            let relay = FakeRelay::spawn().await;
            let (frames, .., outcome) = autopost_case(&relay, stop.clone(), "partial").await;
            assert_eq!(outcome, None, "{stop:?}: no autopost attempted");
            assert_eq!(frames.last().unwrap().status, "abandoned", "{stop:?}");
            assert!(relay.posted_messages().is_empty(), "{stop:?}");
            assert!(
                relay.queries.lock().unwrap().is_empty(),
                "{stop:?}: no lookups"
            );
        }
    }

    #[tokio::test]
    async fn autopost_skips_whitespace_text() {
        let relay = FakeRelay::spawn().await;
        let (frames, .., outcome) = autopost_case(&relay, StopReason::EndTurn, " \n\t").await;
        assert_eq!(outcome, None);
        assert_eq!(frames.last().unwrap().status, "final");
        assert!(relay.posted_messages().is_empty());
    }

    #[tokio::test]
    async fn autopost_resolves_member_mentions() {
        let relay = FakeRelay::spawn().await;
        let owner = Keys::generate().public_key().to_hex();
        relay.members.lock().unwrap().push(json!({
            "kind": 39002,
            "tags": [["d", "x"], ["p", owner]],
        }));
        relay.profiles.lock().unwrap().push(json!({
            "kind": 0,
            "pubkey": owner,
            "content": json!({"display_name": "Alice Example"}).to_string(),
        }));
        autopost_case(
            &relay,
            StopReason::EndTurn,
            "Done — @Alice Example please review.",
        )
        .await;
        let posted = relay.posted_messages();
        assert_eq!(posted.len(), 1);
        assert!(posted[0]["tags"]
            .as_array()
            .unwrap()
            .contains(&json!(["p", owner])));
    }
}
