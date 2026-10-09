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
}): Promise<{ cancelled?: boolean; channelCleanup?: ChannelCleanupReport }> {
  const reports: (ChannelCleanupReport | undefined)[] = [];
  for (const agent of remoteAgentsOfPersona(managedAgents, persona.id)) {
    const result = await deleteAgent(agent);
    if (result.cancelled) return { cancelled: true };
    reports.push(result.channelCleanup);
  }
  reports.push(await deletePersona(persona.id));
  return { channelCleanup: mergeChannelCleanup(reports) };
}
