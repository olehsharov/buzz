import assert from "node:assert/strict";
import { test } from "node:test";
import { renderHook } from "@testing-library/react";
import { useAgentMentionRevalidation } from "../lib/agentMentionRevalidation.ts";
import { setup, deferred, KEY } from "./useMentionSendFlow.test-support.mjs";

// The send flow runs against the production publish revalidation. Only the
// Tauri bridge is replaced: it serves the live member list, which can drift
// from the roster cached when the person was selected.
const HUMAN = "d".repeat(64);
const TEXT = "@Pat @RemoteScout hello";
const REMOVED =
  "A mentioned person is no longer in this channel. Invite them or remove the mention, then retry.";

async function membershipSetup(delay) {
  const live = {
    members: [{ pubkey: HUMAN, role: "member", is_agent: false }],
    gate: null,
  };
  window.__TAURI_INTERNALS__ = {
    invoke: async (command) => {
      if (command === "get_channel_members") {
        if (live.gate) await live.gate.promise;
        return { members: live.members };
      }
      throw new Error(`unmocked ${command}`);
    },
  };
  const { result: revalidate } = renderHook(() =>
    useAgentMentionRevalidation({
      agentPubkeys: new Set([KEY]),
      getSelectedAgentPubkeys: () => new Set(),
      channelType: "stream",
      currentPubkey: "a".repeat(64),
      eligibilityScope: { type: "channel", channelId: "general" },
      sharedChannelIds: new Set(),
      refetchManagedAgents: async () => ({
        data: [{ pubkey: KEY }],
        error: null,
      }),
    }),
  );
  const s = await setup();
  s.dismiss();
  const mentions = s.options.mentions;
  mentions.extractMentionPubkeys = () => [HUMAN, KEY];
  // Pat was a member when selected. The agent is not, so Send opens the
  // Invite prompt, and the publish continues after the invite.
  mentions.memberPubkeys = new Set([HUMAN]);
  mentions.revalidateMentionPubkeys = async (keys, channel, options) => {
    s.calls.push([options.phase, channel]);
    return revalidate.current(keys, channel, options);
  };
  if (delay === "upload")
    s.control.attachments = [{ id: "file", file: {}, spoilered: false }];
  s.rerender();
  return { s, live };
}

for (const delay of ["upload", "invite"]) {
  for (const removed of [false, true]) {
    test(`chat publish ${removed ? "refuses" : "keeps"} a person ${removed ? "removed" : "still present"} during the ${delay} delay`, async () => {
      const { s, live } = await membershipSetup(delay);
      if (delay === "invite") s.control.add = deferred();
      await s.prompt(TEXT);
      await s.invite();
      assert.equal(s.events("add").length, 1);
      if (delay === "upload") {
        assert.ok(s.control.uploadCallbacks);
        assert.equal(s.options.contentRef.current, "", "optimistic clear");
      }
      if (removed) live.members = [];
      if (delay === "invite") await s.finish(s.control.add);
      else
        await s.act(async () =>
          s.control.uploadCallbacks.onComplete(
            [],
            new AbortController().signal,
          ),
        );
      await s.flush();
      const sends = s.events("SEND");
      if (!removed) {
        assert.equal(sends.length, 1);
        assert.deepEqual(sends[0][2], [HUMAN, KEY]);
        return;
      }
      assert.equal(sends.length, 0);
      assert.deepEqual(
        s.events("error").map(([, message]) => message),
        [REMOVED],
      );
      assert.equal(
        s.options.contentRef.current,
        TEXT,
        "the draft is kept for retry",
      );
    });
  }
}

test("Send without inviting still publishes: a declined person becomes a reference", async () => {
  const DECLINED = "e".repeat(64);
  const { s } = await membershipSetup("decline");
  s.options.mentions.extractMentionPubkeys = () => [HUMAN, DECLINED];
  s.options.mentions.isAgentPubkey = () => false;
  s.rerender();
  await s.prompt("@Pat @Sam hello");
  assert.equal(s.result.current.nonMemberPromptProps.open, true);
  await s.act(async () => s.result.current.nonMemberPromptProps.onDoNothing());
  await s.flush();
  const sends = s.events("SEND");
  assert.equal(sends.length, 1);
  assert.deepEqual(sends[0][2], [HUMAN]);
  assert.equal(s.events("error").length, 0);
});
