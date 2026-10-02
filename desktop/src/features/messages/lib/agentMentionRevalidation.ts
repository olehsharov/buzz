import {
  getAgentMentionAdmission,
  getMentionableAgentPubkeys,
  type AgentEligibilityScope,
} from "@/features/agents/lib/agentAutocompleteEligibility";
import { getChannelMembers } from "@/shared/api/tauriChannels";
import { revalidateRelayAgents } from "@/shared/api/tauriRelayAgents";
import type { ManagedAgent, RelayAgent } from "@/shared/api/types";
import { normalizePubkey } from "@/shared/lib/pubkey";
import * as React from "react";

export type MentionRevalidationOptions = {
  phase?: "prepare" | "publish";
  intendedAgentPubkeys?: readonly string[];
};

export class AgentMentionAuthorizationError extends Error {
  constructor() {
    super(
      "Could not authorize a mentioned agent. Check its access and channel membership, then retry or remove the mention.",
    );
    this.name = "AgentMentionAuthorizationError";
  }
}

/** A mentioned person is not a member of the destination at publication. */
export class MentionMembershipChangedError extends Error {
  constructor(unverified = false) {
    super(
      unverified
        ? "Could not check that the people you mentioned are still in this channel. Retry, or remove the mentions."
        : "A mentioned person is no longer in this channel. Invite them or remove the mention, then retry.",
    );
    this.name = "MentionMembershipChangedError";
  }
}

/** Errors whose message is safe and actionable for the composer to show. */
export function isMentionAuthorizationError(error: unknown): error is Error {
  return (
    error instanceof AgentMentionAuthorizationError ||
    error instanceof MentionMembershipChangedError
  );
}

/**
 * Publication signs a `p` tag for every person in `pubkeys`. Selection and
 * the Invite prompt prove membership only at their own time, so read the
 * destination's member list fresh and fail closed for any person who is not
 * a member now (from the writer: a lagging replica can still list someone
 * just removed). Send without inviting has already moved declined people to
 * reference tags, and invited people are members by this point.
 */
export async function revalidateHumanMentionMembership({
  pubkeys,
  agentPubkeys,
  channelId,
  fetchMembers = (id) => getChannelMembers(id, { readYourWrites: true }),
}: {
  pubkeys: readonly string[];
  agentPubkeys: ReadonlySet<string>;
  channelId: string;
  fetchMembers?: (channelId: string) => Promise<{ pubkey: string }[]>;
}) {
  const humans = [...new Set(pubkeys.map(normalizePubkey))].filter(
    (pubkey) => !agentPubkeys.has(pubkey),
  );
  if (humans.length === 0) return;
  const members = await fetchMembers(channelId).catch(() => null);
  if (!members) throw new MentionMembershipChangedError(true);
  const memberPubkeys = new Set(
    members.map((member) => normalizePubkey(member.pubkey)),
  );
  if (humans.some((pubkey) => !memberPubkeys.has(pubkey)))
    throw new MentionMembershipChangedError();
}

type DirectoryResult<T> = {
  data: T | undefined;
  error: Error | null;
};

export async function revalidateAgentMentionPubkeys({
  pubkeys,
  agentPubkeys,
  currentPubkey,
  eligibilityScope,
  sharedChannelIds,
  refetchManagedAgents,
  fetchRelayAgents,
  phase = "publish",
}: {
  phase?: "prepare" | "publish";
  pubkeys: readonly string[];
  agentPubkeys: ReadonlySet<string>;
  currentPubkey: string | null;
  eligibilityScope: AgentEligibilityScope;
  sharedChannelIds: ReadonlySet<string>;
  refetchManagedAgents: () => Promise<DirectoryResult<ManagedAgent[]>>;
  fetchRelayAgents: (pubkeys: string[]) => Promise<RelayAgent[]>;
}) {
  const requestedAgentPubkeys = new Set(
    pubkeys.map(normalizePubkey).filter((pubkey) => agentPubkeys.has(pubkey)),
  );
  if (requestedAgentPubkeys.size === 0) {
    return [...pubkeys];
  }

  const [managedResult, relayAgents] = await Promise.all([
    refetchManagedAgents().catch(() => null),
    fetchRelayAgents([...requestedAgentPubkeys]).catch(() => null),
  ]);
  const relayDirectoryReady = relayAgents !== null;
  // Each directory proves only its own identities. A failed local runtime
  // query must neither veto fresh relay evidence nor admit stale local data.
  const managedPubkeys = new Set(
    (managedResult?.error === null ? (managedResult.data ?? []) : []).map(
      (agent) => normalizePubkey(agent.pubkey),
    ),
  );
  const mentionablePubkeys = getMentionableAgentPubkeys({
    currentPubkey,
    eligibilityScope,
    phase,
    managedAgentPubkeys: managedPubkeys,
    relayAgents: relayDirectoryReady ? relayAgents : [],
    sharedChannelIds,
  });
  const admittedPubkeys = new Set(
    [...agentPubkeys].filter((pubkey) => {
      const isManagedAgent = managedPubkeys.has(normalizePubkey(pubkey));
      const directoryReady = isManagedAgent || relayDirectoryReady;
      return (
        getAgentMentionAdmission({
          isAgent: true,
          pubkey,
          mentionableAgentPubkeys: mentionablePubkeys,
          directoryReady,
        }) === "allow"
      );
    }),
  );
  if (
    [...requestedAgentPubkeys].some((pubkey) => !admittedPubkeys.has(pubkey))
  ) {
    throw new AgentMentionAuthorizationError();
  }
  return [...pubkeys];
}

export function useAgentMentionRevalidation({
  agentPubkeys,
  getSelectedAgentPubkeys,
  channelType,
  currentPubkey,
  eligibilityScope,
  sharedChannelIds,
  refetchManagedAgents,
}: {
  agentPubkeys: ReadonlySet<string>;
  channelType?: string | null;
  getSelectedAgentPubkeys: () => ReadonlySet<string>;
  currentPubkey: string | null;
  eligibilityScope: AgentEligibilityScope;
  sharedChannelIds: ReadonlySet<string>;
  refetchManagedAgents: () => Promise<DirectoryResult<ManagedAgent[]>>;
}) {
  return React.useCallback(
    (
      pubkeys: readonly string[],
      destinationChannelId?: string | null,
      options: MentionRevalidationOptions = {},
    ) => {
      // A new DM can acquire its channel during preparation. Validate the
      // actual destination at publication, not the composer's original null id.
      const scope: AgentEligibilityScope = destinationChannelId
        ? {
            type: eligibilityScope.type === "owned" ? "owned" : "channel",
            channelId: destinationChannelId,
          }
        : eligibilityScope;
      const knownAgentPubkeys = new Set([
        ...agentPubkeys,
        ...getSelectedAgentPubkeys(),
        ...(options.intendedAgentPubkeys ?? []).map(normalizePubkey),
      ]);
      // Only the explicit publish pass signs new recipients into a channel.
      // DMs have fixed participants and no member list to change.
      const humanMembership =
        options.phase === "publish" &&
        destinationChannelId &&
        channelType !== "dm"
          ? revalidateHumanMentionMembership({
              pubkeys,
              agentPubkeys: knownAgentPubkeys,
              channelId: destinationChannelId,
            })
          : Promise.resolve();
      const agents = revalidateAgentMentionPubkeys({
        pubkeys,
        agentPubkeys: knownAgentPubkeys,
        phase: options.phase,
        currentPubkey,
        eligibilityScope: scope,
        sharedChannelIds,
        refetchManagedAgents,
        fetchRelayAgents: (requestedPubkeys) =>
          revalidateRelayAgents(
            requestedPubkeys,
            "channelId" in scope ? (scope.channelId ?? undefined) : undefined,
          ),
      });
      return Promise.all([agents, humanMembership]).then(([result]) => result);
    },
    [
      agentPubkeys,
      channelType,
      currentPubkey,
      eligibilityScope,
      getSelectedAgentPubkeys,
      refetchManagedAgents,
      sharedChannelIds,
    ],
  );
}
