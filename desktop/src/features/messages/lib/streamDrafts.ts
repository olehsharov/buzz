/**
 * Pure store for live reply drafts (ephemeral kind 20003 "ghost" messages).
 *
 * Wire contract (producer: buzz-acp):
 * - tags: `h` channel, `stream` uuid (one per reply), `seq` monotonic per
 *   stream, thread `e` root/reply tags, `status`
 *   (`thinking|tool|writing|final|abandoned`), optional `label` (tool title).
 * - content: the CUMULATIVE markdown snapshot of the reply so far — each frame
 *   replaces the previous content. Status-only frames carry empty content.
 * - the final reply is an ordinary kind 9 from the same pubkey; an autoposted
 *   final reply carries `["stream", <same uuid>]`.
 *
 * Client rules implemented here: drop frames whose seq is <= the last seen
 * seq for (pubkey, stream); drop the ghost on `abandoned`; on `final`, or a
 * message from the same pubkey in the same scope (channel root vs. the same
 * thread root), mark it complete so the surface can swap it for the real row
 * without a gap; expire it after {@link STREAM_DRAFT_TTL_MS} with no frames.
 * Ended streams leave a short tombstone so late or reordered frames cannot
 * resurrect a ghost after its reply landed.
 *
 * Drafts never enter message caches, so they are structurally excluded from
 * unread state, notifications, search, the local archive, mention badges and
 * last-message previews — those all read kind-filtered message queries.
 */

import { getThreadReference } from "@/features/messages/lib/threading";
import type { RelayEvent } from "@/shared/api/types";
import {
  KIND_STREAM_DRAFT,
  KIND_STREAM_MESSAGE,
  KIND_STREAM_MESSAGE_DIFF,
  KIND_STREAM_MESSAGE_V2,
} from "@/shared/constants/kinds";

/** Ghost lifetime without a new frame. */
export const STREAM_DRAFT_TTL_MS = 15_000;
/**
 * How long a completed ghost may wait for its real message row to render
 * before it is dropped anyway (the row can be deferred, buffered behind a
 * scrolled-up reader, or never arrive if the producer posted nothing).
 */
export const STREAM_DRAFT_COMPLETION_GRACE_MS = 5_000;
/** How long an ended stream rejects late frames. */
export const STREAM_DRAFT_TOMBSTONE_MS = 60_000;
/** Upper bound on simultaneous ghosts in one channel (defensive). */
export const MAX_STREAM_DRAFTS = 32;
/** Upper bound on retained tombstones (defensive). */
const MAX_STREAM_DRAFT_TOMBSTONES = 256;

export type StreamDraftStatus = "thinking" | "tool" | "writing";
type StreamDraftWireStatus = StreamDraftStatus | "final" | "abandoned";

export type StreamDraft = {
  /** `${pubkey}:${streamId}` — stable React key for the ghost row. */
  key: string;
  channelId: string;
  pubkey: string;
  streamId: string;
  /** Thread root, or null for a top-level (channel) reply. */
  scopeRootId: string | null;
  /** Direct parent for a thread reply, else null. */
  parentId: string | null;
  seq: number;
  status: StreamDraftStatus;
  label: string | null;
  content: string;
  /** created_at (seconds) of the first admitted frame. */
  startedAt: number;
  /** Local receipt time of the newest frame (TTL clock). */
  updatedAtMs: number;
  /** Set once the reply finished; the ghost then only awaits its real row. */
  completedAtMs: number | null;
};

export type StreamDraftState = {
  drafts: Readonly<Record<string, StreamDraft>>;
  /** Ended stream key → local time after which it may be forgotten. */
  tombstones: Readonly<Record<string, number>>;
};

export const EMPTY_STREAM_DRAFT_STATE: StreamDraftState = {
  drafts: {},
  tombstones: {},
};

export type StreamDraftAction =
  | { type: "frame"; event: RelayEvent; channelId: string; nowMs: number }
  | {
      type: "messages";
      messages: readonly StreamDraftCompletionMessage[];
      nowMs: number;
    }
  | { type: "prune"; nowMs: number }
  | { type: "reset" };

/** The fields of a posted message that can complete a ghost. */
export type StreamDraftCompletionMessage = {
  /** Resolved (display) author pubkey. */
  pubkey: string;
  createdAt: number;
  scopeRootId: string | null;
  streamId: string | null;
};

const WIRE_STATUSES = new Set<string>([
  "thinking",
  "tool",
  "writing",
  "final",
  "abandoned",
]);

const COMPLETION_KINDS = new Set<number>([
  KIND_STREAM_MESSAGE,
  KIND_STREAM_MESSAGE_V2,
  KIND_STREAM_MESSAGE_DIFF,
]);

function tagValue(tags: readonly string[][], name: string): string | null {
  const value = tags.find((tag) => tag[0] === name)?.[1];
  return typeof value === "string" && value.length > 0 ? value : null;
}

export function streamDraftKey(pubkey: string, streamId: string) {
  return `${pubkey.toLowerCase()}:${streamId}`;
}

/** Thread scope shared by drafts and messages: the root id, or null. */
export function getStreamScope(tags: readonly string[][]) {
  const reference = getThreadReference(tags as string[][]);
  return reference.parentId
    ? { parentId: reference.parentId, scopeRootId: reference.rootId }
    : { parentId: null, scopeRootId: null };
}

export function isStreamDraftCompletionKind(kind: number) {
  return COMPLETION_KINDS.has(kind);
}

/** Project a posted message event onto the fields that complete a ghost. */
export function toStreamDraftCompletionMessage(
  event: RelayEvent,
  authorPubkey: string,
): StreamDraftCompletionMessage | null {
  if (!isStreamDraftCompletionKind(event.kind)) return null;
  return {
    pubkey: authorPubkey.toLowerCase(),
    createdAt: event.created_at,
    scopeRootId: getStreamScope(event.tags).scopeRootId,
    streamId: tagValue(event.tags, "stream"),
  };
}

type ParsedFrame = {
  key: string;
  pubkey: string;
  streamId: string;
  seq: number;
  status: StreamDraftWireStatus;
  label: string | null;
  content: string;
  createdAt: number;
  parentId: string | null;
  scopeRootId: string | null;
};

/** Parse a kind 20003 frame for `channelId`; null when it is not admissible. */
export function parseStreamDraftFrame(
  event: RelayEvent,
  channelId: string,
): ParsedFrame | null {
  if (event.kind !== KIND_STREAM_DRAFT) return null;
  if (tagValue(event.tags, "h") !== channelId) return null;
  const streamId = tagValue(event.tags, "stream");
  const seqText = tagValue(event.tags, "seq");
  if (!streamId || !seqText || !/^\d{1,15}$/.test(seqText)) return null;
  const rawStatus = tagValue(event.tags, "status");
  const content = typeof event.content === "string" ? event.content : "";
  // An unknown/missing status degrades to the content-implied state rather
  // than dropping the frame, so a newer producer status stays visible.
  const status: StreamDraftWireStatus =
    rawStatus && WIRE_STATUSES.has(rawStatus)
      ? (rawStatus as StreamDraftWireStatus)
      : content.length > 0
        ? "writing"
        : "thinking";
  const pubkey = event.pubkey.toLowerCase();
  return {
    key: streamDraftKey(pubkey, streamId),
    pubkey,
    streamId,
    seq: Number(seqText),
    status,
    label: tagValue(event.tags, "label"),
    content,
    createdAt: event.created_at,
    ...getStreamScope(event.tags),
  };
}

function withTombstone(
  tombstones: Readonly<Record<string, number>>,
  key: string,
  nowMs: number,
) {
  const next = { ...tombstones, [key]: nowMs + STREAM_DRAFT_TOMBSTONE_MS };
  const keys = Object.keys(next);
  if (keys.length > MAX_STREAM_DRAFT_TOMBSTONES) {
    // Evict the soonest-expiring entries first.
    keys
      .sort((left, right) => (next[left] ?? 0) - (next[right] ?? 0))
      .slice(0, keys.length - MAX_STREAM_DRAFT_TOMBSTONES)
      .forEach((stale) => {
        delete next[stale];
      });
  }
  return next;
}

function withoutKey<T>(record: Readonly<Record<string, T>>, key: string) {
  const next = { ...record };
  delete next[key];
  return next;
}

function applyFrame(
  state: StreamDraftState,
  event: RelayEvent,
  channelId: string,
  nowMs: number,
): StreamDraftState {
  const frame = parseStreamDraftFrame(event, channelId);
  if (!frame) return state;
  const tombstoneUntil = state.tombstones[frame.key];
  if (tombstoneUntil !== undefined && tombstoneUntil > nowMs) return state;

  const existing = state.drafts[frame.key];
  if (existing && frame.seq <= existing.seq) return state;
  if (!existing && frame.createdAt * 1_000 + STREAM_DRAFT_TTL_MS <= nowMs) {
    // A stale replay (e.g. the subscription's `since` lookback) never opens
    // a ghost.
    return state;
  }

  if (frame.status === "abandoned") {
    return {
      drafts: existing ? withoutKey(state.drafts, frame.key) : state.drafts,
      tombstones: withTombstone(state.tombstones, frame.key, nowMs),
    };
  }

  // Snapshots are cumulative, so a legitimately non-empty reply never shrinks
  // to empty: an empty frame (thinking/tool status) keeps the text so far.
  const content =
    frame.content.length > 0 ? frame.content : (existing?.content ?? "");

  if (frame.status === "final") {
    if (!existing && content.length === 0) {
      return {
        drafts: state.drafts,
        tombstones: withTombstone(state.tombstones, frame.key, nowMs),
      };
    }
    return {
      drafts: {
        ...state.drafts,
        [frame.key]: {
          ...(existing ?? draftFromFrame(frame, channelId, nowMs)),
          seq: frame.seq,
          content,
          status: "writing",
          label: null,
          updatedAtMs: nowMs,
          completedAtMs: existing?.completedAtMs ?? nowMs,
        },
      },
      tombstones: withTombstone(state.tombstones, frame.key, nowMs),
    };
  }

  if (!existing && Object.keys(state.drafts).length >= MAX_STREAM_DRAFTS) {
    return state;
  }

  const base = existing ?? draftFromFrame(frame, channelId, nowMs);
  return {
    ...state,
    drafts: {
      ...state.drafts,
      [frame.key]: {
        ...base,
        seq: frame.seq,
        status: frame.status,
        label: frame.status === "tool" ? frame.label : null,
        content,
        updatedAtMs: nowMs,
      },
    },
  };
}

function draftFromFrame(
  frame: ParsedFrame,
  channelId: string,
  nowMs: number,
): StreamDraft {
  return {
    key: frame.key,
    channelId,
    pubkey: frame.pubkey,
    streamId: frame.streamId,
    scopeRootId: frame.scopeRootId,
    parentId: frame.parentId,
    seq: frame.seq,
    status:
      frame.status === "tool" || frame.status === "writing"
        ? frame.status
        : "thinking",
    label: null,
    content: "",
    startedAt: frame.createdAt,
    updatedAtMs: nowMs,
    completedAtMs: null,
  };
}

/** True when `message` is the real reply that a ghost stands in for. */
export function isStreamDraftCompletedBy(
  draft: Pick<StreamDraft, "pubkey" | "scopeRootId" | "startedAt" | "streamId">,
  message: StreamDraftCompletionMessage,
) {
  if (message.pubkey.toLowerCase() !== draft.pubkey) return false;
  if (message.streamId !== null) return message.streamId === draft.streamId;
  // Untagged (agent-posted) final reply: same author in the same scope, no
  // older than the stream. Older history rows must not end a live ghost.
  return (
    message.scopeRootId === draft.scopeRootId &&
    message.createdAt >= draft.startedAt
  );
}

function applyMessages(
  state: StreamDraftState,
  messages: readonly StreamDraftCompletionMessage[],
  nowMs: number,
): StreamDraftState {
  let drafts: Record<string, StreamDraft> | null = null;
  let tombstones = state.tombstones;
  for (const draft of Object.values(state.drafts)) {
    if (draft.completedAtMs !== null) continue;
    if (!messages.some((message) => isStreamDraftCompletedBy(draft, message))) {
      continue;
    }
    drafts ??= { ...state.drafts };
    drafts[draft.key] = { ...draft, completedAtMs: nowMs };
    tombstones = withTombstone(tombstones, draft.key, nowMs);
  }
  return drafts ? { drafts, tombstones } : state;
}

function applyPrune(state: StreamDraftState, nowMs: number): StreamDraftState {
  let drafts: Record<string, StreamDraft> | null = null;
  for (const draft of Object.values(state.drafts)) {
    const expired =
      draft.completedAtMs !== null
        ? draft.completedAtMs + STREAM_DRAFT_COMPLETION_GRACE_MS <= nowMs
        : draft.updatedAtMs + STREAM_DRAFT_TTL_MS <= nowMs;
    if (!expired) continue;
    drafts ??= { ...state.drafts };
    delete drafts[draft.key];
  }
  let tombstones: Record<string, number> | null = null;
  for (const [key, until] of Object.entries(state.tombstones)) {
    if (until > nowMs) continue;
    tombstones ??= { ...state.tombstones };
    delete tombstones[key];
  }
  if (!drafts && !tombstones) return state;
  return {
    drafts: drafts ?? state.drafts,
    tombstones: tombstones ?? state.tombstones,
  };
}

export function streamDraftReducer(
  state: StreamDraftState,
  action: StreamDraftAction,
): StreamDraftState {
  switch (action.type) {
    case "frame":
      return applyFrame(state, action.event, action.channelId, action.nowMs);
    case "messages":
      return applyMessages(state, action.messages, action.nowMs);
    case "prune":
      return applyPrune(state, action.nowMs);
    case "reset":
      return state.drafts === EMPTY_STREAM_DRAFT_STATE.drafts &&
        state.tombstones === EMPTY_STREAM_DRAFT_STATE.tombstones
        ? state
        : EMPTY_STREAM_DRAFT_STATE;
  }
}

/** Ghosts in creation order (oldest first). */
export function listStreamDrafts(state: StreamDraftState): StreamDraft[] {
  return Object.values(state.drafts).sort(
    (left, right) =>
      left.startedAt - right.startedAt || left.key.localeCompare(right.key),
  );
}

/**
 * Drafts for one surface: the channel timeline (`threadHeadId === null`,
 * top-level replies only) or a thread pane headed by `threadHeadId` (replies
 * in that thread root, or directly under that head when it is nested).
 */
export function selectStreamDraftsForScope(
  drafts: readonly StreamDraft[],
  threadHeadId: string | null,
): StreamDraft[] {
  return drafts.filter((draft) =>
    threadHeadId === null
      ? draft.scopeRootId === null
      : draft.scopeRootId === threadHeadId || draft.parentId === threadHeadId,
  );
}

/**
 * Drop ghosts whose real reply is already among the RENDERED rows, so the
 * row and its ghost swap in the same commit (no duplicate, no gap). Rows are
 * scanned from the newest end; `maxScan` bounds the walk on long timelines.
 */
export function selectUnrenderedStreamDrafts(
  drafts: readonly StreamDraft[],
  renderedMessages: readonly StreamDraftCompletionMessage[],
  maxScan = 200,
): StreamDraft[] {
  if (drafts.length === 0) return drafts as StreamDraft[];
  const tail = renderedMessages.slice(-maxScan);
  const visible = drafts.filter(
    (draft) =>
      !tail.some((message) => isStreamDraftCompletedBy(draft, message)),
  );
  return visible.length === drafts.length ? (drafts as StreamDraft[]) : visible;
}

/** Projection of a rendered timeline row for {@link selectUnrenderedStreamDrafts}. */
export function timelineRowCompletionMessage(message: {
  pubkey?: string;
  signerPubkey?: string;
  createdAt: number;
  parentId?: string | null;
  rootId?: string | null;
  tags?: string[][];
}): StreamDraftCompletionMessage | null {
  const pubkey = message.pubkey ?? message.signerPubkey;
  if (!pubkey) return null;
  return {
    pubkey: pubkey.toLowerCase(),
    createdAt: message.createdAt,
    scopeRootId: message.parentId ? (message.rootId ?? message.parentId) : null,
    streamId: message.tags ? tagValue(message.tags, "stream") : null,
  };
}

/**
 * Make a partial markdown snapshot safe to render: close an unterminated code
 * fence and, outside fences, an unterminated inline code span or strong
 * emphasis on the trailing line. Unclosed links/emphasis otherwise render as
 * literal text, which is acceptable for a draft.
 */
export function closePartialMarkdown(content: string): string {
  const lines = content.split("\n");
  let openFence: string | null = null;
  for (const line of lines) {
    const match = /^ {0,3}(`{3,}|~{3,})/.exec(line);
    if (!match?.[1]) continue;
    const fence = match[1];
    if (openFence === null) {
      openFence = fence;
    } else if (
      fence[0] === openFence[0] &&
      fence.length >= openFence.length &&
      line.trim() === fence
    ) {
      openFence = null;
    }
  }
  if (openFence !== null) {
    return `${content}${content.endsWith("\n") ? "" : "\n"}${openFence}`;
  }
  const lastLine = lines[lines.length - 1] ?? "";
  let suffix = "";
  const backticks = lastLine.match(/`/g)?.length ?? 0;
  if (backticks % 2 === 1) suffix += "`";
  const outsideCode =
    backticks % 2 === 1
      ? lastLine.slice(0, lastLine.lastIndexOf("`"))
      : lastLine;
  const strongMarkers =
    outsideCode.replace(/`[^`]*`/g, "").match(/\*\*/g)?.length ?? 0;
  if (strongMarkers % 2 === 1) {
    // A dangling `**` with nothing after it renders as literal asterisks;
    // drop it instead of producing an empty strong node.
    if (/\*\*\s*$/.test(lastLine) && suffix === "") {
      return content.replace(/\*\*\s*$/, "");
    }
    // Close the inner code span first, then the enclosing strong run.
    suffix = `${suffix}**`;
  }
  return `${content}${suffix}`;
}
