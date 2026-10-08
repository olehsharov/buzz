import assert from "node:assert/strict";
import test from "node:test";

import { openSearchResultInNewWindow } from "./searchResultNewWindow.ts";

function harness(destination) {
  const opened = [];
  const dms = [];
  return {
    opened,
    dms,
    deps: {
      openInNewWindow: (target) => {
        opened.push(target);
        return true;
      },
      openDm: async (input) => {
        dms.push(input);
        return { id: "dm-1" };
      },
      resolveDestination: async () => destination,
    },
  };
}

test("channel results open the channel", async () => {
  const { deps, opened } = harness(null);
  await openSearchResultInNewWindow(
    { kind: "channel", channel: { id: "c1" } },
    deps,
  );
  assert.deepEqual(opened, [{ kind: "channel", channelId: "c1" }]);
});

test("people open their DM, like a plain open", async () => {
  const { deps, opened, dms } = harness(null);
  await openSearchResultInNewWindow(
    { kind: "user", user: { pubkey: "p1" } },
    deps,
  );
  assert.deepEqual(dms, [{ pubkeys: ["p1"] }]);
  assert.deepEqual(opened, [{ kind: "channel", channelId: "dm-1" }]);
});

test("message hits open the resolved message or forum post", async () => {
  const thread = harness({
    kind: "channel",
    channelId: "c1",
    messageId: "m1",
    threadRootId: "r1",
  });
  await openSearchResultInNewWindow(
    { kind: "message", hit: { eventId: "m1" } },
    thread.deps,
  );
  assert.deepEqual(thread.opened, [
    { kind: "channel", channelId: "c1", messageId: "m1", threadRootId: "r1" },
  ]);

  const forum = harness({
    kind: "forum-post",
    channelId: "f1",
    postId: "p1",
    replyId: "x1",
  });
  await openSearchResultInNewWindow(
    { kind: "message", hit: { eventId: "x1" } },
    forum.deps,
  );
  assert.deepEqual(forum.opened, [
    { kind: "forum-post", channelId: "f1", postId: "p1", replyId: "x1" },
  ]);
});

test("unresolvable hits and actions open nothing", async () => {
  const { deps, opened } = harness(null);
  assert.equal(
    await openSearchResultInNewWindow(
      { kind: "message", hit: { eventId: "m1" } },
      deps,
    ),
    false,
  );
  assert.equal(
    await openSearchResultInNewWindow(
      { kind: "action", action: { id: "create-channel", title: "x" } },
      deps,
    ),
    false,
  );
  assert.deepEqual(opened, []);
});
