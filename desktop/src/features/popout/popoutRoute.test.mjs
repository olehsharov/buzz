import assert from "node:assert/strict";
import test from "node:test";

import {
  buildPopoutRoute,
  normalizePopoutRoute,
  parsePopoutRoute,
  popoutDestinationFromLocation,
} from "./popoutRoute.ts";

const CHANNEL = "8b7c2d1e-0000-4000-8000-000000000001";
const EVENT = "a".repeat(64);
const ROOT = "b".repeat(64);
const PUBKEY = "c".repeat(64);

test("buildPopoutRoute builds every MVP destination", () => {
  assert.equal(
    buildPopoutRoute({ kind: "channel", channelId: CHANNEL }),
    `/channels/${CHANNEL}`,
  );
  assert.equal(
    buildPopoutRoute({ kind: "thread", channelId: CHANNEL, threadRootId: ROOT }),
    `/channels/${CHANNEL}?thread=${ROOT}`,
  );
  assert.equal(
    buildPopoutRoute({
      kind: "channel",
      channelId: CHANNEL,
      messageId: EVENT,
      threadRootId: ROOT,
    }),
    `/channels/${CHANNEL}?messageId=${EVENT}&threadRootId=${ROOT}`,
  );
  assert.equal(
    buildPopoutRoute({
      kind: "forum-post",
      channelId: CHANNEL,
      postId: EVENT,
    }),
    `/channels/${CHANNEL}/posts/${EVENT}`,
  );
  assert.equal(
    buildPopoutRoute({ kind: "profile", pubkey: PUBKEY }),
    `/pulse?profile=${PUBKEY}`,
  );
});

test("buildPopoutRoute drops a threadRootId without a message target", () => {
  assert.equal(
    buildPopoutRoute({
      kind: "channel",
      channelId: CHANNEL,
      messageId: null,
      threadRootId: ROOT,
    }),
    `/channels/${CHANNEL}`,
  );
});

test("buildPopoutRoute rejects ids that could escape the route", () => {
  for (const channelId of ["", "../settings", "a/b", "a?b=1", "a#b", "a&b"]) {
    assert.equal(buildPopoutRoute({ kind: "channel", channelId }), null);
  }
  assert.equal(buildPopoutRoute({ kind: "profile", pubkey: "x/y" }), null);
  assert.equal(
    buildPopoutRoute({ kind: "thread", channelId: CHANNEL, threadRootId: "" }),
    null,
  );
});

test("buildPopoutRoute rejects non-MVP destinations", () => {
  assert.equal(buildPopoutRoute({ kind: "project", projectId: "p" }), null);
  assert.equal(buildPopoutRoute({ kind: "settings" }), null);
});

test("numeric-looking ids round-trip as strings", () => {
  // TanStack's search codec JSON-parses values; "1e5" must stay a string id.
  const route = buildPopoutRoute({
    kind: "thread",
    channelId: CHANNEL,
    threadRootId: "1e5",
  });
  assert.deepEqual(parsePopoutRoute(route), {
    kind: "thread",
    channelId: CHANNEL,
    threadRootId: "1e5",
  });
});

test("parsePopoutRoute round-trips every built route", () => {
  const destinations = [
    {
      kind: "channel",
      channelId: CHANNEL,
      messageId: null,
      threadRootId: null,
    },
    {
      kind: "channel",
      channelId: CHANNEL,
      messageId: EVENT,
      threadRootId: ROOT,
    },
    { kind: "thread", channelId: CHANNEL, threadRootId: ROOT },
    { kind: "forum-post", channelId: CHANNEL, postId: EVENT, replyId: ROOT },
    { kind: "profile", pubkey: PUBKEY },
  ];
  for (const destination of destinations) {
    assert.deepEqual(
      parsePopoutRoute(buildPopoutRoute(destination)),
      destination,
    );
  }
});

test("parsePopoutRoute rejects non-destination routes and extra keys", () => {
  for (const route of [
    null,
    42,
    "",
    "/",
    "/settings",
    "/projects/abc",
    "/workflows/abc",
    "/pulse",
    `/channels/${CHANNEL}?autoSend=draft`,
    `/channels/${CHANNEL}?thread=${ROOT}&messageId=${EVENT}`,
    `/channels/${CHANNEL}?threadRootId=${ROOT}`,
    `/channels/${CHANNEL}/posts/${EVENT}?thread=${ROOT}`,
    `https://evil.example/channels/${CHANNEL}`,
    `/channels/${CHANNEL}/../settings`,
    "x".repeat(2000),
  ]) {
    assert.equal(parsePopoutRoute(route), null, String(route));
  }
});

test("normalizePopoutRoute canonicalizes valid routes only", () => {
  assert.equal(
    normalizePopoutRoute(`/channels/${CHANNEL}`),
    `/channels/${CHANNEL}`,
  );
  assert.equal(normalizePopoutRoute("/settings"), null);
});

test("popoutDestinationFromLocation ignores unrelated search keys", () => {
  assert.deepEqual(
    popoutDestinationFromLocation(`/channels/${CHANNEL}`, {
      thread: ROOT,
      profile: PUBKEY,
      agentSession: "x",
    }),
    { kind: "thread", channelId: CHANNEL, threadRootId: ROOT },
  );
  assert.deepEqual(popoutDestinationFromLocation("/pulse", { profile: PUBKEY }), {
    kind: "profile",
    pubkey: PUBKEY,
  });
  assert.equal(popoutDestinationFromLocation("/", {}), null);
  assert.equal(popoutDestinationFromLocation("/projects", {}), null);
});
