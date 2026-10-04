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
import {
  getAgentIdentityPubkeys,
  getMentionableAgentPubkeys,
} from "../../agents/lib/agentAutocompleteEligibility.ts";
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

const members = (count) =>
  Array.from({ length: count }, (_, index) => ({
    pubkey: index.toString(16).padStart(64, "f"),
  }));

// ── Audience ───────────────────────────────────────────────────────────

const CHANNEL = "chan";
const OTHER_OWNER = "e".repeat(64);
const relayAgent = (pubkey, overrides = {}) => ({
  pubkey,
  ownerPubkey: OTHER_OWNER,
  respondTo: "anyone",
  respondToAllowlist: [],
  channelIds: [CHANNEL],
  ...overrides,
});

// The audience as the composer computes it: agent identity and mention
// eligibility come from the same helpers the picker and send revalidation use.
function audienceFor({
  currentPubkey = ME,
  members: roster,
  managed = [],
  relayAgents = [],
}) {
  const managedAgentPubkeys = new Set(managed);
  return resolveMentionAllAudience({
    admittedAgentPubkeys: getMentionableAgentPubkeys({
      currentPubkey,
      eligibilityScope: { type: "channel", channelId: CHANNEL },
      phase: "publish",
      managedAgentPubkeys,
      relayAgents,
      sharedChannelIds: new Set([CHANNEL]),
    }),
    agentPubkeys: getAgentIdentityPubkeys({
      managedAgentPubkeys,
      relayAgents,
      members: roster ?? [],
      profileIsAgent: () => false,
    }),
    currentPubkey,
    members: roster,
  });
}

test("@all resolves to every member except the sender, deduped and lowercased", () => {
  assert.deepEqual(
    audienceFor({
      currentPubkey: ME.toUpperCase(),
      members: [
        { pubkey: ME },
        { pubkey: ALICE.toUpperCase() },
        { pubkey: ALICE },
        { pubkey: BOB },
      ],
    }),
    { status: "available", recipients: [ALICE, BOB] },
  );
});

test("@all tags every agent the owner manages: owner plus 5 agents", () => {
  const agents = Array.from({ length: 5 }, (_, index) =>
    index.toString(16).padStart(64, "9"),
  );
  assert.deepEqual(
    audienceFor({
      members: [{ pubkey: ME }, ...agents.map((pubkey) => ({ pubkey }))],
      managed: agents,
    }),
    { status: "available", recipients: agents },
  );
});

test("@all includes an eligible relay agent and skips one that would not answer", () => {
  const ineligible = "8".repeat(64);
  const audience = audienceFor({
    members: [
      { pubkey: ME },
      { pubkey: BOB },
      { pubkey: AGENT },
      { pubkey: ineligible, role: "bot" },
    ],
    relayAgents: [
      relayAgent(AGENT),
      relayAgent(ineligible, { respondTo: "owner-only" }),
    ],
  });
  assert.deepEqual(audience, { status: "available", recipients: [BOB, AGENT] });
});

test("@all with only ineligible agents besides the sender is empty, not blocked by them", () => {
  assert.deepEqual(
    audienceFor({
      members: [{ pubkey: ME }, { pubkey: AGENT, role: "bot" }],
      relayAgents: [relayAgent(AGENT, { channelIds: ["elsewhere"] })],
    }),
    { status: "empty" },
  );
});

test("@all is available at the cap and over-cap one past it, never truncated", () => {
  const at = audienceFor({
    members: [{ pubkey: ME }, ...members(MENTION_ALL_RECIPIENT_CAP)],
  });
  assert.equal(at.status, "available");
  assert.equal(at.recipients.length, 50);
  assert.deepEqual(
    audienceFor({
      members: [{ pubkey: ME }, ...members(MENTION_ALL_RECIPIENT_CAP + 1)],
    }),
    { status: "over-cap", count: 51 },
  );
});

test("eligible agents count toward the cap; skipped agents do not", () => {
  const agents = Array.from({ length: 5 }, (_, index) =>
    index.toString(16).padStart(64, "9"),
  );
  const roster = [...members(45), ...agents.map((pubkey) => ({ pubkey }))];
  assert.equal(
    audienceFor({ members: roster, managed: agents }).status,
    "available",
  );
  const extra = "7".repeat(64);
  assert.deepEqual(
    audienceFor({
      members: [...roster, { pubkey: extra }],
      managed: agents,
    }),
    { status: "over-cap", count: 51 },
  );
  assert.equal(
    audienceFor({
      members: [...roster, { pubkey: extra, role: "bot" }],
      managed: agents,
    }).status,
    "available",
  );
});

test("@all with nobody else or an unloaded roster is unavailable", () => {
  assert.deepEqual(
    audienceFor({ members: [{ pubkey: ME }, { pubkey: ME.toUpperCase() }] }),
    { status: "empty" },
  );
  assert.deepEqual(audienceFor({ members: undefined }), { status: "loading" });
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
      message: `${MENTION_ALL_OVER_CAP_REASON}. This channel has 51 besides you.`,
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
  const fifty = members(50).map((member) => member.pubkey);
  assert.match(
    planMentionAllSend({
      otherRecipientPubkeys: [],
      pendingPersonaCount: 1,
      resolution: { status: "resolved", recipients: fifty },
    }).error,
    /would notify 51 recipients/,
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
  assert.equal(over.disabledReason, "@all can notify at most 50 members");
  assert.equal(
    buildMentionAllCandidate({ status: "empty" }).disabledReason,
    MENTION_ALL_EMPTY_REASON,
  );
});

test("a typed prefix of all ranks @all first; a bare @ keeps it after the roster", () => {
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
  const labels = (query) =>
    rankMentionCandidates([outsider, member, group], query).map(
      ({ label }) => label,
    );
  for (const query of ["a", "al", "all", "AL"]) {
    assert.equal(labels(query)[0], "all", `query ${query}`);
  }
  assert.deepEqual(labels("al"), ["all", "Allison", "Alfred"]);
  assert.deepEqual(labels(""), ["Allison", "all", "Alfred"]);
  assert.deepEqual(labels("alli"), ["Allison"]);
});

test("@all survives the suggestion cap in a large channel on @a", () => {
  // Real rosters: nearly every hex key contains "a", so on `@a` every member
  // matches by key and, ranked ahead of the group, pushed it past the cap.
  const roster = Array.from({ length: 60 }, (_, index) => ({
    kind: "identity",
    pubkey: (index + 1).toString(16).padStart(64, "a"),
    displayName: null,
    isAgent: false,
    isMember: true,
  }));
  const group = buildMentionAllCandidate({ status: "over-cap", count: 60 });
  const shown = rankMentionCandidates([...roster, group], "a").slice(0, 50);
  assert.equal(shown[0].candidate.kind, "group");
  assert.equal(shown[0].candidate.disabledReason, MENTION_ALL_OVER_CAP_REASON);
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
