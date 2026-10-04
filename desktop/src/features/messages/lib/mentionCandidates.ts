import { resolveTeamPersonas } from "@/features/agents/lib/teamPersonas";
import type {
  AgentPersona,
  AgentTeam,
  ChannelRole,
  UserSearchResult,
} from "@/shared/api/types";
import { MENTION_GROUP_ALL } from "@/shared/lib/mentionGroup";
import { truncateNpub } from "@/shared/lib/pubkey";
import {
  type MentionAllAudience,
  mentionAllDisabledReason,
} from "./mentionAllAudience";

export function formatSearchUserDisplayName(user: UserSearchResult) {
  return user.displayName?.trim() || user.nip05Handle?.trim() || null;
}

export function formatSearchUserSecondaryLabel(user: UserSearchResult) {
  const displayName = user.displayName?.trim();
  const nip05Handle = user.nip05Handle?.trim();
  return displayName && nip05Handle ? nip05Handle : null;
}

export function appendUniqueName(current: string[], name: string): string[] {
  return current.some(
    (candidate) => candidate.toLowerCase() === name.toLowerCase(),
  )
    ? current
    : [...current, name];
}

export type TeamMentionMember = {
  displayName: string;
  kind: "identity" | "persona";
  personaId?: string;
  pubkey?: string;
};

export type MentionCandidate = {
  kind: "identity" | "persona" | "team" | "group";
  pubkey?: string;
  personaId?: string;
  teamId?: string;
  teamMembers?: TeamMentionMember[];
  displayName: string | null;
  avatarUrl?: string | null;
  isMember: boolean;
  role?: ChannelRole | null;
  personaName?: string | null;
  secondaryLabel?: string | null;
  ownerPubkey?: string | null;
  isAgent: boolean;
  isActiveAgent?: boolean;
  isManagedAgent?: boolean;
  isGlobalSearchResult?: boolean;
  /** Group mentions only: why the entry is shown but cannot be picked. */
  disabledReason?: string | null;
  /** Group mentions only: how many members selecting it would notify. */
  groupRecipientCount?: number;
};

export function mentionCandidateLabel(candidate: MentionCandidate) {
  return (
    candidate.displayName ??
    (candidate.pubkey ? truncateNpub(candidate.pubkey) : "agent")
  );
}

export function globalSearchIdentityKey(candidate: MentionCandidate) {
  if (
    !candidate.isGlobalSearchResult ||
    candidate.isMember ||
    candidate.isAgent
  ) {
    return null;
  }

  const label = candidate.displayName?.trim().toLowerCase();
  if (!label) return null;

  const secondaryLabel = candidate.secondaryLabel?.trim().toLowerCase() ?? "";
  return `global-person:${label}:${secondaryLabel}`;
}

function findTeamMemberTarget(
  persona: AgentPersona,
  candidates: readonly MentionCandidate[],
): TeamMentionMember | null {
  const linked = candidates
    .filter(
      (candidate) =>
        candidate.kind !== "team" &&
        candidate.kind !== "group" &&
        candidate.personaId === persona.id,
    )
    .sort((left, right) => {
      const rank = (candidate: MentionCandidate) => {
        if (candidate.kind === "identity" && candidate.isMember) return 0;
        if (candidate.kind === "identity" && candidate.isManagedAgent) return 1;
        if (candidate.kind === "identity") return 2;
        return 3;
      };
      return rank(left) - rank(right);
    })[0];

  if (linked) {
    return {
      displayName: linked.displayName?.trim() || persona.displayName,
      kind: linked.kind === "identity" ? "identity" : "persona",
      personaId: linked.personaId,
      pubkey: linked.pubkey,
    };
  }

  return persona.isActive
    ? {
        displayName: persona.displayName,
        kind: "persona",
        personaId: persona.id,
      }
    : null;
}

/** Build autocomplete entries for editable, locally owned teams. */
export function buildTeamMentionCandidates(
  teams: readonly AgentTeam[],
  personas: AgentPersona[],
  candidates: readonly MentionCandidate[],
): MentionCandidate[] {
  return teams.flatMap((team) => {
    if (team.isBuiltin || !team.name.trim()) return [];

    const resolution = resolveTeamPersonas(team, personas);
    if (!resolution.isUsable) return [];

    const teamMembers = resolution.resolvedPersonas
      .map((persona) => findTeamMemberTarget(persona, candidates))
      .filter((member): member is TeamMentionMember => member !== null);
    if (teamMembers.length !== resolution.resolvedPersonas.length) return [];

    const mentionNames = new Map<string, TeamMentionMember>();
    for (const member of teamMembers) {
      const mentionName = member.displayName.trim().toLowerCase();
      const previous = mentionNames.get(mentionName);
      // Exact-key members can reserve distinct labels at selection time. A
      // persona without a key cannot yet be disambiguated that way.
      if (previous && (!previous.pubkey || !member.pubkey)) return [];
      mentionNames.set(mentionName, member);
    }

    return [
      {
        kind: "team" as const,
        teamId: team.id,
        teamMembers,
        displayName: team.name.trim(),
        isMember: false,
        isAgent: true,
      },
    ];
  });
}

export function formatTeamMention(
  teamName: string,
  members: readonly TeamMentionMember[],
) {
  return `${teamName}(${members.map((member) => `@${member.displayName}`).join(" ")}) `;
}

/**
 * The `@all` autocomplete entry. It is always listed in a channel composer so
 * an unavailable state (too many members, nobody to notify, roster loading) is
 * explained rather than silently missing.
 */
export function buildMentionAllCandidate(
  audience: MentionAllAudience,
): MentionCandidate {
  return {
    kind: "group",
    displayName: MENTION_GROUP_ALL,
    isMember: false,
    isAgent: false,
    disabledReason: mentionAllDisabledReason(audience),
    groupRecipientCount:
      audience.status === "available"
        ? audience.recipients.length
        : audience.status === "over-cap"
          ? audience.count
          : 0,
  };
}
