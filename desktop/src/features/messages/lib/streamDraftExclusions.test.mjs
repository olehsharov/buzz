import assert from "node:assert/strict";
import test from "node:test";

import { isDmNotifiableKind } from "@/features/channels/isDmNotifiableKind";
import { isChannelUnreadTriggerKind } from "@/features/channels/useLiveChannelUpdates";
import { isTimelineContentEvent } from "@/features/messages/lib/formatTimelineMessages";
import {
  CHANNEL_AUX_EVENT_KINDS,
  CHANNEL_EVENT_KINDS,
  CHANNEL_MESSAGE_EVENT_KINDS,
  CHANNEL_TIMELINE_CONTENT_KINDS,
  HOME_MENTION_EVENT_KINDS,
  KIND_STREAM_DRAFT,
} from "@/shared/constants/kinds";
import {
  buildChannelFilter,
  buildChannelHistoryFilter,
  buildGlobalStreamFilter,
} from "@/shared/api/relayChannelFilters";

// Live reply ghosts must stay out of every durable/attention surface: unread
// badges, OS notifications, mention badges (Home feed), search and the local
// archive (both fed by the channel message kinds), and "last message"
// previews (timeline rows). Those surfaces all key off these kind sets.

test("kind 20003 is never an unread or notification trigger", () => {
  assert.equal(KIND_STREAM_DRAFT, 20003);
  assert.equal(isChannelUnreadTriggerKind(KIND_STREAM_DRAFT, false), false);
  assert.equal(isChannelUnreadTriggerKind(KIND_STREAM_DRAFT, true), false);
  assert.equal(isDmNotifiableKind(KIND_STREAM_DRAFT), false);
});

test("kind 20003 is in no message, mention, timeline or aux kind set", () => {
  for (const kinds of [
    CHANNEL_MESSAGE_EVENT_KINDS,
    HOME_MENTION_EVENT_KINDS,
    CHANNEL_EVENT_KINDS,
    CHANNEL_TIMELINE_CONTENT_KINDS,
    CHANNEL_AUX_EVENT_KINDS,
  ]) {
    assert.equal(kinds.includes(KIND_STREAM_DRAFT), false);
  }
});

test("channel history/live/global filters never request kind 20003", () => {
  const channelId = "36411e44-0e2d-4cfe-bd6e-567eb169db9f";
  for (const filter of [
    buildChannelFilter(channelId, 50),
    buildChannelHistoryFilter(channelId, 50),
    buildGlobalStreamFilter(50),
  ]) {
    assert.equal(filter.kinds?.includes(KIND_STREAM_DRAFT), false);
  }
});

test("a kind 20003 event never formats as a timeline row", () => {
  assert.equal(
    isTimelineContentEvent({
      id: "a".repeat(64),
      pubkey: "b".repeat(64),
      created_at: 1,
      kind: KIND_STREAM_DRAFT,
      tags: [["h", "c"]],
      content: "draft",
      sig: "",
    }),
    false,
  );
});
