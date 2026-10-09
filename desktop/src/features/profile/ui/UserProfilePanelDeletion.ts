import * as React from "react";

import {
  deleteManagedAgentWithRules,
  type ManagedAgentActionResult,
} from "@/features/agents/lib/managedAgentControlActions";
import { showProgressToast } from "@/features/agents/lib/progressToast";
import {
  type ChannelCleanupReport,
  mergeChannelCleanup,
} from "@/shared/api/channelCleanup";
import type {
  AgentPersona,
  Channel,
  ManagedAgent,
  RelayAgent,
} from "@/shared/api/types";

type DeleteManagedAgentRulesContext = Omit<
  Parameters<typeof deleteManagedAgentWithRules>[0],
  "agent"
>;

type DeleteProfileManagedAgentsForPersonaContext =
  DeleteManagedAgentRulesContext & {
    managedAgents: readonly ManagedAgent[];
    selectedAgent?: ManagedAgent;
  };

type UseProfileAgentDeletionInput = {
  confirm: DeleteManagedAgentRulesContext["confirm"];
  channels?: readonly Channel[];
  deleteManagedAgent: DeleteManagedAgentRulesContext["deleteManagedAgent"];
  managedAgent?: ManagedAgent;
  managedAgents?: readonly ManagedAgent[];
  getAvailability: DeleteManagedAgentRulesContext["getAvailability"];
  relayAgents?: readonly RelayAgent[];
};

/**
 * Profile-panel agent deletion. The backend removes each deleted agent from
 * every channel it is in; the result carries what it could not remove so
 * the caller can say so.
 */
export function useProfileAgentDeletion({
  confirm,
  channels,
  deleteManagedAgent,
  managedAgent,
  managedAgents,
  getAvailability,
  relayAgents,
}: UseProfileAgentDeletionInput) {
  const deleteManagedAgentRecord = React.useCallback(
    (agentToDelete: ManagedAgent) =>
      deleteManagedAgentWithRules({
        showProgress: showProgressToast,
        agent: agentToDelete,
        channels: channels ?? [],
        confirm,
        deleteManagedAgent,
        getAvailability,
        relayAgents: relayAgents ?? [],
        skipRemoteDeleteConfirm: true,
      }),
    [channels, confirm, deleteManagedAgent, getAvailability, relayAgents],
  );

  const deleteManagedAgentsForPersona = React.useCallback(
    (persona: AgentPersona) =>
      deleteProfileManagedAgentsForPersona(persona, {
        showProgress: showProgressToast,
        channels: channels ?? [],
        confirm,
        deleteManagedAgent,
        managedAgents: managedAgents ?? [],
        getAvailability,
        relayAgents: relayAgents ?? [],
        selectedAgent: managedAgent,
      }),
    [
      channels,
      confirm,
      deleteManagedAgent,
      managedAgent,
      managedAgents,
      getAvailability,
      relayAgents,
    ],
  );

  return {
    deleteManagedAgentRecord,
    deleteManagedAgentsForPersona,
  };
}

export async function deleteProfileManagedAgentsForPersona(
  persona: AgentPersona,
  context: DeleteProfileManagedAgentsForPersonaContext,
): Promise<ManagedAgentActionResult> {
  const { managedAgents, selectedAgent, ...deleteContext } = context;
  const agentsByPubkey = new Map<string, ManagedAgent>();

  for (const agent of managedAgents) {
    if (agent.personaId === persona.id) {
      agentsByPubkey.set(agent.pubkey, agent);
    }
  }

  if (selectedAgent?.personaId === persona.id) {
    agentsByPubkey.set(selectedAgent.pubkey, selectedAgent);
  }

  const reports: (ChannelCleanupReport | undefined)[] = [];
  for (const agent of agentsByPubkey.values()) {
    const result = await deleteManagedAgentWithRules({
      agent,
      ...deleteContext,
    });
    if (result.cancelled) return result;
    reports.push(result.channelCleanup);
  }

  const channelCleanup = mergeChannelCleanup(reports);
  return channelCleanup ? { channelCleanup } : {};
}
