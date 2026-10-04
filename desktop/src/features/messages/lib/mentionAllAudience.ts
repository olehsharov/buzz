import { filterAdmittedMentionPubkeys } from "@/features/agents/lib/agentAutocompleteEligibility";
import { MENTION_ALL_RECIPIENT_CAP } from "@/shared/lib/mentionGroup";
import { normalizePubkey } from "@/shared/lib/pubkey";

/** Who `@all` would notify in the current channel, or why it cannot. */
export type MentionAllAudience =
  | { status: "loading" }
  | { status: "empty" }
  | { status: "over-cap"; count: number }
  | { status: "available"; recipients: string[] };

/** Outcome of the send-time `@all` re-check. */
export type MentionAllSendResolution =
  | { status: "none" }
  | { status: "resolved"; recipients: string[] }
  | { status: "blocked"; message: string };

export const MENTION_ALL_OVER_CAP_REASON = `@all can notify at most ${MENTION_ALL_RECIPIENT_CAP} members`;
export const MENTION_ALL_EMPTY_REASON = "No one else in this channel to notify";
export const MENTION_ALL_LOADING_REASON = "Loading channel members…";
export const MENTION_ALL_ROSTER_ERROR =
  "Couldn't load this channel's members for @all. Try again.";
export const MENTION_ALL_AGENT_DIRECTORY_ERROR =
  "Couldn't verify which agents @all can notify. Try again.";
export const MENTION_ALL_AMBIGUOUS_ERROR =
  "The mention @all is ambiguous: a member is named “all”. Choose a recipient from the mention picker.";

/**
 * Resolve `@all` to every channel member except the sender: every person, plus
 * every agent the sender may mention here. A member in `agentPubkeys` that is
 * not in `admittedAgentPubkeys` (the same mention eligibility the picker and
 * send-time revalidation apply) is skipped, never tagged. More recipients than
 * the cap is reported, never truncated.
 */
export function resolveMentionAllAudience({
  admittedAgentPubkeys,
  agentPubkeys,
  currentPubkey,
  members,
}: {
  admittedAgentPubkeys: ReadonlySet<string>;
  agentPubkeys: ReadonlySet<string>;
  currentPubkey: string | null;
  members: readonly { pubkey: string }[] | undefined;
}): MentionAllAudience {
  if (!members || !currentPubkey) return { status: "loading" };
  const sender = normalizePubkey(currentPubkey);
  const recipients = filterAdmittedMentionPubkeys(
    [
      ...new Set(members.map((member) => normalizePubkey(member.pubkey))),
    ].filter((pubkey) => pubkey !== sender),
    agentPubkeys,
    admittedAgentPubkeys,
  );
  if (recipients.length === 0) return { status: "empty" };
  if (recipients.length > MENTION_ALL_RECIPIENT_CAP) {
    return { status: "over-cap", count: recipients.length };
  }
  return { status: "available", recipients };
}

/** The reason `@all` cannot be picked, or null when it can. */
export function mentionAllDisabledReason(
  audience: MentionAllAudience,
): string | null {
  switch (audience.status) {
    case "available":
      return null;
    case "over-cap":
      return MENTION_ALL_OVER_CAP_REASON;
    case "empty":
      return MENTION_ALL_EMPTY_REASON;
    case "loading":
      return MENTION_ALL_LOADING_REASON;
  }
}

/**
 * Translate a fresh audience into the send decision. Every non-available
 * state blocks the send with a visible message; the caller keeps the draft.
 */
export function mentionAllSendResolution(
  audience: MentionAllAudience,
): MentionAllSendResolution {
  switch (audience.status) {
    case "available":
      return { status: "resolved", recipients: audience.recipients };
    case "over-cap":
      return {
        status: "blocked",
        message: `${MENTION_ALL_OVER_CAP_REASON}. This channel has ${audience.count} besides you.`,
      };
    case "empty":
      return { status: "blocked", message: `${MENTION_ALL_EMPTY_REASON}.` };
    case "loading":
      return { status: "blocked", message: MENTION_ALL_ROSTER_ERROR };
  }
}

/**
 * A send that adds `@all` must still fit the event builder's mention cap once
 * explicit and automatically addressed recipients are merged in.
 */
export function mentionAllCombinedCapError(recipientCount: number) {
  return recipientCount > MENTION_ALL_RECIPIENT_CAP
    ? `@all plus the other mentions would notify ${recipientCount} recipients; the limit is ${MENTION_ALL_RECIPIENT_CAP}.`
    : null;
}
