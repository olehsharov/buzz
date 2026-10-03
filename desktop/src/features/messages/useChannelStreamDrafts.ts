import * as React from "react";

import {
  EMPTY_STREAM_DRAFT_STATE,
  isStreamDraftCompletionKind,
  listStreamDrafts,
  type StreamDraft,
  type StreamDraftCompletionMessage,
  streamDraftReducer,
  toStreamDraftCompletionMessage,
} from "@/features/messages/lib/streamDrafts";
import { relayClient } from "@/shared/api/relayClient";
import type { Channel, RelayEvent } from "@/shared/api/types";
import { KIND_STREAM_DRAFT } from "@/shared/constants/kinds";
import { resolveEventAuthorPubkey } from "@/shared/lib/authors";

const STREAM_DRAFT_PRUNE_INTERVAL_MS = 1_000;
/** Newest rows per source inspected for a completing reply. */
const COMPLETION_SCAN_LIMIT = 50;

const EMPTY_DRAFTS: StreamDraft[] = [];

function collectCompletionMessages(
  sources: ReadonlyArray<readonly RelayEvent[]>,
  relaySelfPubkey: string | null | undefined,
) {
  const messages: StreamDraftCompletionMessage[] = [];
  for (const events of sources) {
    for (const event of events.slice(-COMPLETION_SCAN_LIMIT)) {
      if (!isStreamDraftCompletionKind(event.kind)) continue;
      const author = resolveEventAuthorPubkey({
        event,
        preferActorTag: true,
        relaySelfPubkey,
        requireChannelTagForPTags: true,
      });
      const message = toStreamDraftCompletionMessage(event, author);
      if (message) messages.push(message);
    }
  }
  return messages;
}

/**
 * Live reply drafts (kind 20003) for the open channel. State is component
 * scoped (no module singleton): a channel switch resets it, and a community
 * switch remounts the whole tree, so nothing joins `resetCommunityState()`.
 *
 * `channelEvents` / `threadEvents` are the channel's live message lists
 * (timeline + open thread); a reply landing there marks the matching ghost
 * complete.
 */
export function useChannelStreamDrafts(
  channel: Channel | null,
  channelEvents: readonly RelayEvent[],
  threadEvents: readonly RelayEvent[],
  relaySelfPubkey?: string | null,
): StreamDraft[] {
  const channelId = channel?.id ?? null;
  const channelType = channel?.channelType ?? null;
  const [state, dispatch] = React.useReducer(
    streamDraftReducer,
    EMPTY_STREAM_DRAFT_STATE,
  );

  // biome-ignore lint/correctness/useExhaustiveDependencies: a channel change must drop the previous channel's ghosts
  React.useEffect(() => {
    dispatch({ type: "reset" });
  }, [channelId]);

  React.useEffect(() => {
    if (!channelId || channelType === "forum") return;
    let isDisposed = false;
    let cleanup: (() => Promise<void>) | undefined;
    relayClient
      .subscribeToChannelEphemeral(KIND_STREAM_DRAFT, channelId, (event) => {
        if (!isDisposed) {
          dispatch({ type: "frame", event, channelId, nowMs: Date.now() });
        }
      })
      .then((dispose) => {
        if (isDisposed) {
          void dispose();
          return;
        }
        cleanup = dispose;
      })
      .catch((error) => {
        console.error("Failed to subscribe to stream drafts", channelId, error);
      });
    return () => {
      isDisposed = true;
      if (cleanup) void cleanup();
    };
  }, [channelId, channelType]);

  const hasDrafts = Object.keys(state.drafts).length > 0;
  const hasState = hasDrafts || Object.keys(state.tombstones).length > 0;

  // Completion only matters while a ghost is live; skip the scan otherwise.
  React.useEffect(() => {
    if (!hasDrafts) return;
    const messages = collectCompletionMessages(
      [channelEvents, threadEvents],
      relaySelfPubkey,
    );
    if (messages.length === 0) return;
    dispatch({ type: "messages", messages, nowMs: Date.now() });
  }, [channelEvents, hasDrafts, relaySelfPubkey, threadEvents]);

  React.useEffect(() => {
    if (!hasState) return;
    const interval = window.setInterval(() => {
      dispatch({ type: "prune", nowMs: Date.now() });
    }, STREAM_DRAFT_PRUNE_INTERVAL_MS);
    return () => window.clearInterval(interval);
  }, [hasState]);

  const drafts = state.drafts;
  return React.useMemo(() => {
    // The channel filter also covers the one render between a channel switch
    // and the reset effect.
    const list = listStreamDrafts({ drafts, tombstones: {} }).filter(
      (draft) => draft.channelId === channelId,
    );
    return list.length === 0 ? EMPTY_DRAFTS : list;
  }, [channelId, drafts]);
}
