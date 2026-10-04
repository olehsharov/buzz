import assert from "node:assert/strict";
import { test } from "node:test";
import { setup } from "./useMentionSendFlow.test-support.mjs";

const ALICE = "c".repeat(64);
const BOB = "d".repeat(64);
const MARKER = ["buzz:mention-group", "all"];
const TEXT = "@all standup moved";

async function withMentionAll(mentionAll) {
  const s = await setup();
  s.dismiss();
  s.calls.length = 0;
  s.options.mentions.extractMentionPubkeys = () => [];
  s.control.mentionAll = mentionAll;
  s.rerender();
  return s;
}

test("@all send publishes every resolved recipient and the group marker", async () => {
  const s = await withMentionAll({
    status: "resolved",
    recipients: [ALICE, BOB],
  });
  await s.prompt(TEXT);
  const sends = s.events("SEND");
  assert.equal(sends.length, 1);
  const [, content, pubkeys, tags] = sends[0];
  assert.equal(content, TEXT);
  assert.deepEqual(pubkeys, [ALICE, BOB]);
  // The hook runs in a vm realm; compare tag values across realms.
  assert.deepEqual(
    JSON.parse(JSON.stringify(tags.filter((tag) => tag[0] === MARKER[0]))),
    [MARKER],
  );
});

test("a send without @all carries no group marker", async () => {
  const s = await withMentionAll({ status: "none" });
  await s.prompt("plain hello");
  const [, , pubkeys, tags] = s.events("SEND")[0];
  assert.deepEqual(pubkeys, []);
  assert.equal(
    tags.some((tag) => tag[0] === MARKER[0]),
    false,
  );
});

test("@all blocked at send time keeps the draft and has no side effects", async () => {
  const message =
    "@all can notify at most 50 members. This channel has 51 besides you.";
  const s = await withMentionAll({ status: "blocked", message });
  await s.prompt(TEXT);
  assert.equal(s.events("SEND").length, 0);
  // Nothing was revalidated or prepared: the block precedes every side effect.
  assert.equal(s.events("prepare").length, 0);
  assert.deepEqual(
    s.events("error").map((call) => call[1]),
    [message],
  );
  assert.equal(s.result.current.nonMemberPromptProps.error, message);
  assert.equal(s.options.contentRef.current, TEXT);
});

test("@all plus other mentions over the cap blocks instead of truncating", async () => {
  const fifty = Array.from({ length: 50 }, (_, index) =>
    index.toString(16).padStart(64, "e"),
  );
  const s = await withMentionAll({ status: "resolved", recipients: fifty });
  s.options.mentions.extractMentionPubkeys = () => [ALICE];
  s.options.mentions.memberPubkeys = new Set([ALICE, ...fifty]);
  s.rerender();
  await s.prompt(`${TEXT} @Alice`);
  assert.equal(s.events("SEND").length, 0);
  assert.match(String(s.events("error")[0]?.[1]), /would notify 51 recipients/);
  assert.equal(s.options.contentRef.current, `${TEXT} @Alice`);
});
