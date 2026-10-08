import assert from "node:assert/strict";
import test from "node:test";

import {
  closePartialMarkdown,
  EMPTY_STREAM_DRAFT_STATE,
  isStreamDraftCompletedBy,
  listStreamDrafts,
  MAX_STREAM_DRAFTS,
  parseStreamDraftFrame,
  selectStreamDraftsForScope,
  selectUnrenderedStreamDrafts,
  STREAM_DRAFT_COMPLETION_GRACE_MS,
  STREAM_DRAFT_TOMBSTONE_MS,
  STREAM_DRAFT_TTL_MS,
  streamDraftReducer,
  timelineRowCompletionMessage,
  toStreamDraftCompletionMessage,
} from "./streamDrafts.ts";

const CHANNEL = "36411e44-0e2d-4cfe-bd6e-567eb169db9f";
const OTHER_CHANNEL = "b0b0b0b0-0e2d-4cfe-bd6e-567eb169db9f";
const AGENT = "A".repeat(64);
const AGENT_LC = AGENT.toLowerCase();
const OTHER_AGENT = "b".repeat(64);
const ROOT = "1".repeat(64);
const PARENT = "2".repeat(64);
const STREAM = "5b0b2c55-0000-4000-8000-000000000001";
const STREAM_2 = "5b0b2c55-0000-4000-8000-000000000002";
const NOW_S = 1_800_000_000;
const NOW_MS = NOW_S * 1_000;

function frame({
  seq,
  status = "writing",
  content = "",
  stream = STREAM,
  pubkey = AGENT,
  channel = CHANNEL,
  createdAt = NOW_S,
  thread = null,
  label,
  extraTags = [],
}) {
  const tags = [["h", channel]];
  if (stream !== null) tags.push(["stream", stream]);
  if (seq !== null) tags.push(["seq", String(seq)]);
  if (thread?.root) tags.push(["e", thread.root, "", "root"]);
  if (thread?.parent) tags.push(["e", thread.parent, "", "reply"]);
  if (status !== null) tags.push(["status", status]);
  if (label) tags.push(["label", label]);
  tags.push(...extraTags);
  return {
    id: `${seq}-${status}-${stream}`,
    pubkey,
    created_at: createdAt,
    kind: 20003,
    tags,
    content,
    sig: "",
  };
}

function apply(state, event, nowMs = NOW_MS) {
  return streamDraftReducer(state, {
    type: "frame",
    event,
    channelId: CHANNEL,
    nowMs,
  });
}

function only(state) {
  const drafts = listStreamDrafts(state);
  assert.equal(drafts.length, 1);
  return drafts[0];
}

function completion({
  pubkey = AGENT_LC,
  createdAt = NOW_S,
  scopeRootId = null,
  streamId = null,
} = {}) {
  return { pubkey, createdAt, scopeRootId, streamId };
}

test("a writing frame opens a ghost keyed by (pubkey, stream) with its snapshot", () => {
  const state = apply(
    EMPTY_STREAM_DRAFT_STATE,
    frame({ seq: 1, content: "Hel" }),
  );
  const draft = only(state);
  assert.equal(draft.key, `${AGENT_LC}:${STREAM}`);
  assert.equal(draft.pubkey, AGENT_LC);
  assert.equal(draft.content, "Hel");
  assert.equal(draft.status, "writing");
  assert.equal(draft.scopeRootId, null);
  assert.equal(draft.parentId, null);
  assert.equal(draft.startedAt, NOW_S);
  assert.equal(draft.completedAtMs, null);
});

test("content is replaced by each newer cumulative snapshot, not appended", () => {
  let state = apply(
    EMPTY_STREAM_DRAFT_STATE,
    frame({ seq: 1, content: "Hel" }),
  );
  state = apply(state, frame({ seq: 2, content: "Hello wor" }));
  state = apply(state, frame({ seq: 3, content: "Hello world" }));
  assert.equal(only(state).content, "Hello world");
  assert.equal(only(state).seq, 3);
});

test("frames with seq <= the last seen seq are dropped (reordering, duplicates)", () => {
  let state = apply(
    EMPTY_STREAM_DRAFT_STATE,
    frame({ seq: 5, content: "newest" }),
  );
  const afterOlder = apply(state, frame({ seq: 4, content: "older" }));
  assert.equal(afterOlder, state, "an older frame is a no-op");
  const afterDuplicate = apply(state, frame({ seq: 5, content: "dupe" }));
  assert.equal(afterDuplicate, state, "a duplicate seq is a no-op");
  state = apply(state, frame({ seq: 6, content: "newer" }));
  assert.equal(only(state).content, "newer");
});

test("seq is tracked per stream: a new stream from the same agent is independent", () => {
  let state = apply(
    EMPTY_STREAM_DRAFT_STATE,
    frame({ seq: 9, content: "first" }),
  );
  state = apply(state, frame({ seq: 1, content: "second", stream: STREAM_2 }));
  assert.equal(listStreamDrafts(state).length, 2);
});

test("status-only frames never wipe the text so far, and carry their label", () => {
  let state = apply(
    EMPTY_STREAM_DRAFT_STATE,
    frame({ seq: 1, status: "thinking" }),
  );
  assert.equal(only(state).status, "thinking");
  assert.equal(only(state).content, "");
  state = apply(state, frame({ seq: 2, content: "Looking" }));
  state = apply(
    state,
    frame({ seq: 3, status: "tool", label: "Read file", content: "" }),
  );
  assert.equal(only(state).status, "tool");
  assert.equal(only(state).label, "Read file");
  assert.equal(only(state).content, "Looking");
  state = apply(state, frame({ seq: 4, content: "Looking good" }));
  assert.equal(only(state).label, null, "label belongs to the tool status");
});

test("abandoned removes the ghost and later frames of that stream are ignored", () => {
  let state = apply(
    EMPTY_STREAM_DRAFT_STATE,
    frame({ seq: 1, content: "partial" }),
  );
  state = apply(state, frame({ seq: 2, status: "abandoned" }));
  assert.equal(listStreamDrafts(state).length, 0);
  state = apply(state, frame({ seq: 3, content: "zombie" }));
  assert.equal(listStreamDrafts(state).length, 0);
});

test("final keeps the text visible as complete until its row renders or grace ends", () => {
  let state = apply(
    EMPTY_STREAM_DRAFT_STATE,
    frame({ seq: 1, content: "Done." }),
  );
  state = apply(state, frame({ seq: 2, status: "final" }), NOW_MS + 100);
  const draft = only(state);
  assert.equal(draft.content, "Done.");
  assert.equal(draft.completedAtMs, NOW_MS + 100);
  // A late frame cannot reopen it.
  assert.equal(apply(state, frame({ seq: 3, content: "x" })), state);
  state = streamDraftReducer(state, {
    type: "prune",
    nowMs: NOW_MS + 100 + STREAM_DRAFT_COMPLETION_GRACE_MS,
  });
  assert.equal(listStreamDrafts(state).length, 0);
});

test("a status-only final for an unseen stream opens nothing", () => {
  const state = apply(
    EMPTY_STREAM_DRAFT_STATE,
    frame({ seq: 1, status: "final" }),
  );
  assert.equal(listStreamDrafts(state).length, 0);
});

test("TTL: a ghost with no new frame for 15 s expires; a fresh frame extends it", () => {
  let state = apply(EMPTY_STREAM_DRAFT_STATE, frame({ seq: 1, content: "a" }));
  state = streamDraftReducer(state, {
    type: "prune",
    nowMs: NOW_MS + STREAM_DRAFT_TTL_MS - 1,
  });
  assert.equal(listStreamDrafts(state).length, 1);
  state = apply(
    state,
    frame({ seq: 2, content: "ab", createdAt: NOW_S + 14 }),
    NOW_MS + 14_000,
  );
  state = streamDraftReducer(state, {
    type: "prune",
    nowMs: NOW_MS + STREAM_DRAFT_TTL_MS + 1_000,
  });
  assert.equal(listStreamDrafts(state).length, 1, "extended by seq 2");
  state = streamDraftReducer(state, {
    type: "prune",
    nowMs: NOW_MS + 14_000 + STREAM_DRAFT_TTL_MS,
  });
  assert.equal(listStreamDrafts(state).length, 0);
});

test("a stale replayed frame (older than the TTL) never opens a ghost", () => {
  const state = apply(
    EMPTY_STREAM_DRAFT_STATE,
    frame({ seq: 1, content: "old", createdAt: NOW_S - 16 }),
  );
  assert.equal(listStreamDrafts(state).length, 0);
});

test("tombstones expire so the store stays bounded", () => {
  let state = apply(
    EMPTY_STREAM_DRAFT_STATE,
    frame({ seq: 1, status: "abandoned" }),
  );
  assert.equal(Object.keys(state.tombstones).length, 1);
  state = streamDraftReducer(state, {
    type: "prune",
    nowMs: NOW_MS + STREAM_DRAFT_TOMBSTONE_MS,
  });
  assert.equal(Object.keys(state.tombstones).length, 0);
});

test("simultaneous ghosts are capped", () => {
  let state = EMPTY_STREAM_DRAFT_STATE;
  for (let index = 0; index < MAX_STREAM_DRAFTS + 3; index += 1) {
    state = apply(
      state,
      frame({ seq: 1, content: "x", stream: `stream-${index}` }),
    );
  }
  assert.equal(listStreamDrafts(state).length, MAX_STREAM_DRAFTS);
});

test("frames for another channel, other kinds or without stream/seq are rejected", () => {
  const rejected = [
    frame({ seq: 1, content: "x", channel: OTHER_CHANNEL }),
    { ...frame({ seq: 1, content: "x" }), kind: 20002 },
    frame({ seq: 1, content: "x", stream: null }),
    frame({ seq: null, content: "x" }),
    frame({ seq: "1.5", content: "x" }),
    frame({ seq: "-1", content: "x" }),
  ];
  for (const event of rejected) {
    assert.equal(
      apply(EMPTY_STREAM_DRAFT_STATE, event),
      EMPTY_STREAM_DRAFT_STATE,
    );
  }
});

test("a missing or unknown status degrades to the content-implied state", () => {
  assert.equal(
    parseStreamDraftFrame(
      frame({ seq: 1, status: null, content: "x" }),
      CHANNEL,
    ).status,
    "writing",
  );
  assert.equal(
    parseStreamDraftFrame(frame({ seq: 1, status: "planning" }), CHANNEL)
      .status,
    "thinking",
  );
});

test("thread tags scope a ghost to its thread root", () => {
  const state = apply(
    EMPTY_STREAM_DRAFT_STATE,
    frame({ seq: 1, content: "x", thread: { root: ROOT, parent: PARENT } }),
  );
  assert.equal(only(state).scopeRootId, ROOT);
  assert.equal(only(state).parentId, PARENT);
  const direct = apply(
    EMPTY_STREAM_DRAFT_STATE,
    frame({ seq: 1, content: "x", thread: { parent: ROOT } }),
  );
  assert.equal(
    only(direct).scopeRootId,
    ROOT,
    "reply-only tag is its own root",
  );
});

test("kind 9 from the same pubkey in the same scope completes the ghost", () => {
  let state = apply(EMPTY_STREAM_DRAFT_STATE, frame({ seq: 1, content: "x" }));
  state = streamDraftReducer(state, {
    type: "messages",
    messages: [completion({ createdAt: NOW_S + 2 })],
    nowMs: NOW_MS + 2_000,
  });
  assert.equal(only(state).completedAtMs, NOW_MS + 2_000);
  assert.equal(
    apply(state, frame({ seq: 2, content: "late" }), NOW_MS + 2_500),
    state,
    "completion tombstones the stream",
  );
});

test("kind 9 in another scope, from another pubkey, or older than the stream does not complete", () => {
  const threadState = apply(
    EMPTY_STREAM_DRAFT_STATE,
    frame({ seq: 1, content: "x", thread: { root: ROOT, parent: ROOT } }),
  );
  const rootState = apply(
    EMPTY_STREAM_DRAFT_STATE,
    frame({ seq: 1, content: "x" }),
  );
  const cases = [
    [threadState, completion({ scopeRootId: null })],
    [rootState, completion({ scopeRootId: ROOT })],
    [threadState, completion({ scopeRootId: PARENT })],
    [rootState, completion({ pubkey: OTHER_AGENT })],
    [rootState, completion({ createdAt: NOW_S - 1 })],
    [rootState, completion({ streamId: STREAM_2 })],
  ];
  for (const [state, message] of cases) {
    const next = streamDraftReducer(state, {
      type: "messages",
      messages: [message],
      nowMs: NOW_MS + 1_000,
    });
    assert.equal(next, state, JSON.stringify(message));
  }
  const sameThread = streamDraftReducer(threadState, {
    type: "messages",
    messages: [completion({ scopeRootId: ROOT })],
    nowMs: NOW_MS + 1_000,
  });
  assert.notEqual(only(sameThread).completedAtMs, null);
});

test("a stream-tagged autoposted reply completes its own stream regardless of time", () => {
  const draft = {
    pubkey: AGENT_LC,
    scopeRootId: null,
    startedAt: NOW_S,
    streamId: STREAM,
  };
  assert.equal(
    isStreamDraftCompletedBy(
      draft,
      completion({ streamId: STREAM, createdAt: NOW_S - 5, scopeRootId: ROOT }),
    ),
    true,
  );
});

test("message projections resolve scope and stream tags", () => {
  const event = {
    id: "m",
    pubkey: AGENT,
    created_at: NOW_S,
    kind: 9,
    tags: [
      ["h", CHANNEL],
      ["e", ROOT, "", "root"],
      ["e", PARENT, "", "reply"],
      ["stream", STREAM],
    ],
    content: "x",
    sig: "",
  };
  assert.deepEqual(toStreamDraftCompletionMessage(event, AGENT), {
    pubkey: AGENT_LC,
    createdAt: NOW_S,
    scopeRootId: ROOT,
    streamId: STREAM,
  });
  assert.equal(
    toStreamDraftCompletionMessage({ ...event, kind: 7 }, AGENT),
    null,
  );
  assert.equal(
    toStreamDraftCompletionMessage({ ...event, kind: 20003 }, AGENT),
    null,
  );
  assert.deepEqual(
    timelineRowCompletionMessage({
      pubkey: AGENT,
      createdAt: NOW_S,
      parentId: PARENT,
      rootId: ROOT,
      tags: event.tags,
    }),
    { pubkey: AGENT_LC, createdAt: NOW_S, scopeRootId: ROOT, streamId: STREAM },
  );
});

test("scope selection: channel timeline gets root ghosts, a thread pane its own", () => {
  let state = apply(
    EMPTY_STREAM_DRAFT_STATE,
    frame({ seq: 1, content: "root" }),
  );
  state = apply(
    state,
    frame({
      seq: 1,
      content: "thread",
      stream: STREAM_2,
      thread: { root: ROOT, parent: PARENT },
    }),
  );
  const drafts = listStreamDrafts(state);
  assert.deepEqual(
    selectStreamDraftsForScope(drafts, null).map((draft) => draft.content),
    ["root"],
  );
  assert.deepEqual(
    selectStreamDraftsForScope(drafts, ROOT).map((draft) => draft.content),
    ["thread"],
  );
  assert.deepEqual(
    selectStreamDraftsForScope(drafts, PARENT).map((draft) => draft.content),
    ["thread"],
    "a nested head shows replies directly under it",
  );
  assert.deepEqual(selectStreamDraftsForScope(drafts, "f".repeat(64)), []);
});

test("rendered-row hand-off hides the ghost in the same pass its reply renders", () => {
  const state = apply(
    EMPTY_STREAM_DRAFT_STATE,
    frame({ seq: 1, content: "x" }),
  );
  const drafts = listStreamDrafts(state);
  assert.equal(
    selectUnrenderedStreamDrafts(drafts, [
      completion({ createdAt: NOW_S - 30 }),
    ]),
    drafts,
    "older rows keep the same array identity",
  );
  assert.deepEqual(
    selectUnrenderedStreamDrafts(drafts, [
      completion({ createdAt: NOW_S + 1 }),
    ]),
    [],
  );
});

test("closePartialMarkdown closes an open fence, inline code and strong runs", () => {
  assert.equal(
    closePartialMarkdown("Here:\n```ts\nconst a = 1;"),
    "Here:\n```ts\nconst a = 1;\n```",
  );
  assert.equal(closePartialMarkdown("Here:\n```ts\n"), "Here:\n```ts\n```");
  assert.equal(
    closePartialMarkdown("```\na\n```\nafter"),
    "```\na\n```\nafter",
    "a closed fence is untouched",
  );
  assert.equal(closePartialMarkdown("run `npm i"), "run `npm i`");
  assert.equal(closePartialMarkdown("this is **bold"), "this is **bold**");
  assert.equal(closePartialMarkdown("**a `b"), "**a `b`**");
  assert.equal(closePartialMarkdown("trailing **"), "trailing ");
  assert.equal(closePartialMarkdown("**done** and `ok`"), "**done** and `ok`");
});
