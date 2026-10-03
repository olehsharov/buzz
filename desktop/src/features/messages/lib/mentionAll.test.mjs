import assert from "node:assert/strict";
import test from "node:test";

import {
  containsMentionAllToken,
  hasMentionAllMarker,
  MENTION_ALL_RECIPIENT_CAP,
} from "../../../shared/lib/mentionGroup.ts";
import { resolveMentionProps } from "../../../shared/lib/resolveMentionNames.ts";
import { splitOutgoingTags } from "./imetaMediaMarkdown.ts";
import {
  MENTION_ALL_EMPTY_REASON,
  MENTION_ALL_OVER_CAP_REASON,
  mentionAllSendResolution,
  resolveMentionAllAudience,
} from "./mentionAllAudience.ts";
import { buildMentionAllCandidate } from "./mentionCandidates.ts";
import { rankMentionCandidates } from "./mentionRanking.ts";
import { mapMentionCandidateToSuggestion } from "./mentionSuggestionMapping.ts";
import { getSendToChannelSemantics } from "./sendToChannelSemantics.ts";
import { planMentionAllSend } from "../ui/useMentionSendFlow.helpers.ts";

const ME = "a".repeat(64);
const ALICE = "b".repeat(64);
const BOB = "c".repeat(64);
const AGENT = "d".repeat(64);
const MARKER = ["buzz:mention-group", "all"];

const humans = (count) =>
  Array.from({ length: count }, (_, index) => ({
    pubkey: index.toString(16).padStart(64, "f"),
  }));

// ── Audience ───────────────────────────────────────────────────────────

test("@all resolves to every human member except the sender, deduped and lowercased", () => {
  const audience = resolveMentionAllAudience({
    agentPubkeys: new Set([AGENT]),
    currentPubkey: ME.toUpperCase(),
    members: [
      { pubkey: ME },
      { pubkey: ALICE.toUpperCase() },
      { pubkey: ALICE },
      { pubkey: BOB },
      { pubkey: AGENT },
    ],
  });
  assert.deepEqual(audience, { status: "available", recipients: [ALICE, BOB] });
});

test("@all is available at the cap and over-cap one past it, never truncated", () => {
  const at = resolveMentionAllAudience({
    agentPubkeys: new Set(),
    currentPubkey: ME,
    members: [{ pubkey: ME }, ...humans(MENTION_ALL_RECIPIENT_CAP)],
  });
  assert.equal(at.status, "available");
  assert.equal(at.recipients.length, 50);
  const over = resolveMentionAllAudience({
    agentPubkeys: new Set(),
    currentPubkey: ME,
    members: [{ pubkey: ME }, ...humans(MENTION_ALL_RECIPIENT_CAP + 1)],
  });
  assert.deepEqual(over, { status: "over-cap", count: 51 });
});

test("agents do not count toward the cap and never become recipients", () => {
  const agents = Array.from({ length: 10 }, (_, index) => ({
    pubkey: index.toString(16).padStart(64, "9"),
  }));
  const audience = resolveMentionAllAudience({
    agentPubkeys: new Set(agents.map((agent) => agent.pubkey)),
    currentPubkey: ME,
    members: [...humans(45), ...agents],
  });
  assert.equal(audience.status, "available");
  assert.equal(audience.recipients.length, 45);
  assert.equal(
    audience.recipients.some((pubkey) => pubkey.startsWith("9")),
    false,
  );
});

test("@all with nobody else or an unloaded roster is unavailable", () => {
  assert.deepEqual(
    resolveMentionAllAudience({
      agentPubkeys: new Set([AGENT]),
      currentPubkey: ME,
      members: [{ pubkey: ME }, { pubkey: AGENT }],
    }),
    { status: "empty" },
  );
  assert.deepEqual(
    resolveMentionAllAudience({
      agentPubkeys: new Set(),
      currentPubkey: ME,
      members: undefined,
    }),
    { status: "loading" },
  );
});

test("send resolution blocks every non-available audience with a reason", () => {
  assert.deepEqual(
    mentionAllSendResolution({ status: "available", recipients: [ALICE] }),
    { status: "resolved", recipients: [ALICE] },
  );
  assert.deepEqual(
    mentionAllSendResolution({ status: "over-cap", count: 51 }),
    {
      status: "blocked",
      message: `${MENTION_ALL_OVER_CAP_REASON}. This channel has 51.`,
    },
  );
  assert.equal(mentionAllSendResolution({ status: "empty" }).status, "blocked");
  assert.equal(
    mentionAllSendResolution({ status: "loading" }).status,
    "blocked",
  );
});

test("send plan adds the marker and enforces the combined cap", () => {
  assert.deepEqual(
    planMentionAllSend({
      otherRecipientPubkeys: [ALICE],
      pendingPersonaCount: 0,
      resolution: {
        status: "resolved",
        recipients: [ALICE, BOB.toUpperCase()],
      },
    }),
    { recipients: [ALICE, BOB], tags: [MARKER] },
  );
  assert.deepEqual(
    planMentionAllSend({
      otherRecipientPubkeys: [ALICE],
      pendingPersonaCount: 0,
      resolution: { status: "none" },
    }),
    { recipients: [], tags: [] },
  );
  const fifty = humans(50).map((member) => member.pubkey);
  assert.match(
    planMentionAllSend({
      otherRecipientPubkeys: [],
      pendingPersonaCount: 1,
      resolution: { status: "resolved", recipients: fifty },
    }).error,
    /would notify 51 people/,
  );
  // Recipients already in the group do not double-count.
  assert.equal(
    "error" in
      planMentionAllSend({
        otherRecipientPubkeys: [fifty[0]],
        pendingPersonaCount: 0,
        resolution: { status: "resolved", recipients: fifty },
      }),
    false,
  );
});

// ── Token grammar ──────────────────────────────────────────────────────

test("the @all token follows mention boundaries, case and code masking", () => {
  for (const text of ["@all", "hey @all!", "(@all)", "**@all**", "@all, ship"])
    assert.equal(containsMentionAllToken(text), true, text);
  for (const text of [
    "@All",
    "@allison",
    "mail@all.example",
    "`@all`",
    "```\n@all\n```",
    "all hands",
  ])
    assert.equal(containsMentionAllToken(text), false, text);
});

test("a longer competing label owns its literal range", () => {
  assert.equal(containsMentionAllToken("@all hands now", ["all hands"]), false);
  assert.equal(containsMentionAllToken("@all handsome", ["all hands"]), true);
});

// ── Autocomplete entry ─────────────────────────────────────────────────

test("@all candidate carries count, or a disabled reason over the cap", () => {
  const available = mapMentionCandidateToSuggestion({
    agentProvenanceReady: true,
    candidate: buildMentionAllCandidate({
      status: "available",
      recipients: [ALICE, BOB],
    }),
    channelType: "stream",
    label: "all",
  });
  assert.equal(available.kind, "group");
  assert.equal(available.disabledReason, null);
  assert.equal(available.groupRecipientCount, 2);
  assert.equal(available.notInChannel, false);
  assert.equal(available.pubkey, undefined);

  const over = mapMentionCandidateToSuggestion({
    agentProvenanceReady: true,
    candidate: buildMentionAllCandidate({ status: "over-cap", count: 51 }),
    channelType: "stream",
    label: "all",
  });
  assert.equal(
    over.disabledReason,
    "@all is limited to channels with up to 50 people",
  );
  assert.equal(
    buildMentionAllCandidate({ status: "empty" }).disabledReason,
    MENTION_ALL_EMPTY_REASON,
  );
});

test("@all ranks after roster members and before other groups", () => {
  const group = buildMentionAllCandidate({ status: "empty" });
  const member = {
    kind: "identity",
    pubkey: ALICE,
    displayName: "Allison",
    isAgent: false,
    isMember: true,
  };
  const outsider = {
    kind: "identity",
    pubkey: BOB,
    displayName: "Alfred",
    isAgent: false,
    isMember: false,
  };
  assert.deepEqual(
    rankMentionCandidates([outsider, group, member], "al").map(
      ({ label }) => label,
    ),
    ["Allison", "all", "Alfred"],
  );
});

// ── Wire routing ───────────────────────────────────────────────────────

test("the marker rides the validated reference-mention arg, never imeta", () => {
  const imeta = ["imeta", "url https://relay.example/a.png"];
  const mention = ["mention", ALICE];
  assert.deepEqual(splitOutgoingTags([imeta, MARKER, mention]), {
    mediaTags: [imeta],
    emojiTags: [],
    mentionTags: [MARKER, mention],
    linkPreviewTags: [],
  });
});

// ── Rendering decision ─────────────────────────────────────────────────

test("a marked event renders its owned @all token as the group", () => {
  const props = resolveMentionProps(
    [["p", ALICE], ["p", BOB], MARKER],
    {},
    "@all standup moved",
  );
  assert.equal(props.mentionAll, true);
  assert.deepEqual(props.mentionNames, ["all"]);
  assert.equal(props.mentionPubkeysByName?.all, undefined);
});

test("without the marker, @all text and recipients render as today", () => {
  for (const tags of [
    [["p", ALICE]],
    [
      ["p", ALICE],
      ["buzz:mention-group", "here"],
    ],
    [
      ["p", ALICE],
      ["mention", "all"],
    ],
  ]) {
    const props = resolveMentionProps(tags, {}, "@all standup moved");
    assert.equal(props.mentionAll, false);
    assert.equal(props.mentionNames, undefined);
  }
  assert.equal(hasMentionAllMarker([MARKER]), true);
});

test("a marker with no owned token (edited away or in code) is not a pill", () => {
  assert.equal(
    resolveMentionProps([MARKER], {}, "standup moved").mentionAll,
    false,
  );
  assert.equal(resolveMentionProps([MARKER], {}, "`@all`").mentionAll, false);
});

test("the marker outranks a tagged member whose alias is all", () => {
  const props = resolveMentionProps(
    [["p", ALICE], MARKER],
    { [ALICE]: { displayName: "all" } },
    "@all hi",
  );
  assert.equal(props.mentionAll, true);
  assert.equal(props.mentionPubkeysByName?.all, undefined);
  const legacy = resolveMentionProps(
    [["p", ALICE]],
    { [ALICE]: { displayName: "all" } },
    "@all hi",
  );
  assert.equal(legacy.mentionAll, false);
  assert.equal(legacy.mentionPubkeysByName?.all, ALICE);
});

// ── Edit / forward ─────────────────────────────────────────────────────

test("send-to-channel forwards the marker only with the original audience", () => {
  const base = {
    pubkey: ME,
    body: "@all standup moved",
    tags: [["p", ME], ["p", ALICE], ["p", BOB], MARKER],
  };
  const forwarded = getSendToChannelSemantics(base, {});
  assert.deepEqual(forwarded.mentionPubkeys, [ALICE, BOB]);
  assert.deepEqual(forwarded.semanticTags, [MARKER]);
  const snapshotEdit = getSendToChannelSemantics(
    { ...base, edited: true, tags: [...base.tags, ["buzz:mention-snapshot"]] },
    {},
  );
  assert.deepEqual(snapshotEdit.mentionPubkeys, []);
  assert.deepEqual(snapshotEdit.semanticTags, []);
});
