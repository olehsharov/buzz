import {
  type ChannelCleanupReport,
  mergeChannelCleanup,
} from "@/shared/api/channelCleanup";
import type { AgentPersona, ManagedAgent } from "@/shared/api/types";
import type { ManagedAgentActionResult } from "./managedAgentControlActions";

/** A persona's agents that are deployed somewhere else (a paired machine or
 * a provider). The backend refuses to delete the persona while any exist,
 * because deleting their records would orphan the deployments. */
export function remoteAgentsOfPersona(
  managedAgents: readonly ManagedAgent[],
  personaId: string,
): ManagedAgent[] {
  return managedAgents.filter(
    (agent) =>
      agent.personaId === personaId &&
      agent.backend.type !== "local" &&
      agent.backendAgentId !== null,
  );
}

/** "Infra is running on workstation" — where a remote agent runs. */
export function remoteAgentLocation(agent: ManagedAgent): string {
  if (agent.backend.type === "provider") {
    return `${agent.name} is deployed through ${agent.backend.id}`;
  }
  return `${agent.name} is running on ${agent.hostName ?? "a paired machine"}`;
}

/**
 * Delete a persona whose agents may run elsewhere, after one confirmation:
 * each remote agent is deleted first through the normal agent delete (a
 * machine is asked to remove it and must confirm, or the user is asked
 * about deleting it anyway), then the persona with its remaining agents.
 * A cancelled or failed agent delete stops here and leaves the persona.
 */
export async function deletePersonaAfterRemoteAgents({
  persona,
  managedAgents,
  deleteAgent,
  deletePersona,
}: {
  persona: AgentPersona;
  managedAgents: readonly ManagedAgent[];
  deleteAgent: (agent: ManagedAgent) => Promise<ManagedAgentActionResult>;
  deletePersona: (id: string) => Promise<ChannelCleanupReport>;
}): Promise<{
  cancelled?: boolean;
  channelCleanup?: ChannelCleanupReport;
  /** When the cascade stopped early: the agents already deleted. */
  deletedAgents?: string[];
}> {
  const reports: (ChannelCleanupReport | undefined)[] = [];
  const deletedAgents: string[] = [];
  for (const agent of remoteAgentsOfPersona(managedAgents, persona.id)) {
    const result = await deleteAgent(agent);
    if (result.cancelled) {
      return {
        cancelled: true,
        channelCleanup: mergeChannelCleanup(reports),
        deletedAgents,
      };
    }
    reports.push(result.channelCleanup);
    deletedAgents.push(agent.name);
  }
  reports.push(await deletePersona(persona.id));
  return { channelCleanup: mergeChannelCleanup(reports) };
}

/** The notice for a cascade the user stopped part-way, or null. */
export function partialPersonaDeleteNotice(
  persona: AgentPersona,
  deletedAgents: readonly string[] | undefined,
): string | null {
  if (!deletedAgents || deletedAgents.length === 0) return null;
  return (
    `Deleted ${deletedAgents.join(", ")}, but kept ${persona.displayName} ` +
    "and its other agents because an agent was not removed from where it runs."
  );
}
