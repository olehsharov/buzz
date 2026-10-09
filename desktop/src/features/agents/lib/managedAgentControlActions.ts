import { captureRelayRemovals } from "@/features/agents/managedAgentRelayCleanup";
import { HOST_UNDEPLOY_FAILED_PREFIX } from "@/shared/api/agentHosts";
import type { ChannelCleanupReport } from "@/shared/api/channelCleanup";
import { sendChannelMessage } from "@/shared/api/tauri";
import type { Channel, ManagedAgent, RelayAgent } from "@/shared/api/types";
import type { ConfirmFn } from "@/shared/ui/useConfirmDialog";
import type { AgentAvailabilityReader } from "./useAgentAvailability";
import { normalizePubkey } from "@/shared/lib/pubkey";

type DeleteManagedAgentInput = {
  pubkey: string;
  forceRemoteDelete?: boolean;
};

type StartManagedAgent = (pubkey: string) => Promise<unknown>;
type StopManagedAgent = (pubkey: string) => Promise<unknown>;
type DeleteManagedAgent = (
  input: DeleteManagedAgentInput,
) => Promise<ChannelCleanupReport | undefined>;

type ManagedAgentChannelContext = {
  channels: readonly Channel[];
  preferredChannelId?: string | null;
  relayAgents: readonly RelayAgent[];
};

type ManagedAgentActionContext = ManagedAgentChannelContext & {
  getAvailability: AgentAvailabilityReader;
};

export type ManagedAgentActionResult = {
  cancelled?: boolean;
  noticeMessage?: string;
  /** Set by a completed delete: the channels the agent left or kept. */
  channelCleanup?: ChannelCleanupReport;
};

/** Agents that run somewhere else: a provider deployment or an approved
 * machine. Both deploy instead of starting. A provider agent stops via
 * `!shutdown` in a channel; a machine agent is removed from its machine. */
export function isRemoteManagedAgent(agent: Pick<ManagedAgent, "backend">) {
  return agent.backend.type === "provider" || agent.backend.type === "host";
}

/** Lifecycle action routing only; deployed is a retained receipt, not presence. */
export function isManagedAgentActive(agent: Pick<ManagedAgent, "status">) {
  return agent.status === "running" || agent.status === "deployed";
}

export function getManagedAgentPrimaryActionLabel(agent: ManagedAgent) {
  if (isRemoteManagedAgent(agent)) {
    return isManagedAgentActive(agent) ? "Shutdown" : "Deploy";
  }

  if (isManagedAgentActive(agent)) {
    return "Stop";
  }

  return "Start agent";
}

/**
 * Label for the secondary lifecycle action, or null when none applies.
 *
 * Local agents restart a live process. A provider agent with a deployment
 * record can always be redeployed: its record stays "deployed" after
 * `!shutdown` (so the primary action stays "Shutdown"), and configuration
 * edits never reach a deployed remote agent on their own — redeploy is the
 * only way back from either. Provider deploy is idempotent
 * (docs/remote-agents.md), so it cannot create a second live instance.
 */
export function getManagedAgentRestartLabel(
  agent: Pick<ManagedAgent, "backend" | "status">,
): string | null {
  if (isRemoteManagedAgent(agent)) {
    return agent.status === "deployed" ? "Redeploy agent" : null;
  }
  return isManagedAgentActive(agent) ? "Restart agent" : null;
}

export function resolveManagedAgentChannelId(
  agent: Pick<ManagedAgent, "pubkey">,
  context: ManagedAgentChannelContext,
) {
  if (context.preferredChannelId) {
    return context.preferredChannelId;
  }

  const relayAgent = context.relayAgents.find(
    (candidate) =>
      normalizePubkey(candidate.pubkey) === normalizePubkey(agent.pubkey),
  );

  if (relayAgent?.channelIds?.length) {
    return relayAgent.channelIds[0];
  }

  const channelName = relayAgent?.channels?.[0];
  if (!channelName) {
    return null;
  }

  const matches = context.channels.filter(
    (channel) => channel.name === channelName,
  );
  return matches.length === 1 ? matches[0].id : null;
}

export async function startManagedAgentWithRules({
  agent,
  startManagedAgent,
}: {
  agent: ManagedAgent;
  startManagedAgent: StartManagedAgent;
}) {
  // Relay-mesh agents are no longer blocked here: the backend start preflight
  // (ensure_relay_mesh_for_record) re-resolves a live serve target and dials
  // it, failing with an actionable error when no peer serves the model.
  await startManagedAgent(agent.pubkey);
}

export async function respawnManagedAgentWithRules({
  agent,
  relayUrl,
  startManagedAgent,
  stopManagedAgent,
  onStopped,
}: {
  agent: ManagedAgent;
  /** The active community's relay: the workspace pair the native stop and
   * start target. A removal of it during the stop cancels the start. */
  relayUrl: string | undefined;
  startManagedAgent: StartManagedAgent;
  stopManagedAgent: StopManagedAgent;
  /** Called after a successful stop and before start begins — use this to
   * clear stale working badges at the right boundary. */
  onStopped?: () => void;
}) {
  if (agent.backend.type === "local" && isManagedAgentActive(agent)) {
    const assertRelayNotRemoved = relayUrl
      ? captureRelayRemovals(relayUrl)
      : () => {};
    await stopManagedAgent(agent.pubkey);
    onStopped?.();
    assertRelayNotRemoved();
  }

  await startManagedAgent(agent.pubkey);
}

export async function stopManagedAgentWithRules({
  agent,
  channels,
  preferredChannelId,
  relayAgents,
  stopManagedAgent,
}: {
  agent: ManagedAgent;
  stopManagedAgent: StopManagedAgent;
} & ManagedAgentChannelContext): Promise<ManagedAgentActionResult> {
  if (agent.backend.type === "host") {
    // The backend removes the agent from its machine and waits for the
    // machine to confirm; no channel is involved. Deploy brings it back.
    await stopManagedAgent(agent.pubkey);
    const machine = agent.hostName ?? "its machine";
    return {
      noticeMessage: `Stopped ${agent.name} on ${machine}. Deploy starts it there again.`,
    };
  }
  if (isRemoteManagedAgent(agent)) {
    const channelId = resolveManagedAgentChannelId(agent, {
      channels,
      preferredChannelId,
      relayAgents,
    });
    if (!channelId) {
      throw new Error("Cannot stop: agent is not in any channel");
    }

    await sendChannelMessage(channelId, "!shutdown", undefined, undefined, [
      agent.pubkey,
    ]);
    return {
      noticeMessage:
        "Shutdown requested. This does not confirm the agent has stopped.",
    };
  }

  await stopManagedAgent(agent.pubkey);
  return {};
}

export async function deleteManagedAgentWithRules({
  agent,
  channels,
  confirm,
  deleteManagedAgent,
  preferredChannelId,
  getAvailability,
  relayAgents,
  skipRemoteDeleteConfirm = false,
}: {
  agent: ManagedAgent;
  /** In-app confirmation (see `useConfirmDialog`). */
  confirm: ConfirmFn;
  deleteManagedAgent: DeleteManagedAgent;
  skipRemoteDeleteConfirm?: boolean;
} & ManagedAgentActionContext): Promise<ManagedAgentActionResult> {
  if (agent.backend.type === "host") {
    return deleteHostAgent(
      agent,
      deleteManagedAgent,
      confirm,
      skipRemoteDeleteConfirm,
    );
  }
  if (agent.backend.type === "provider" && agent.backendAgentId) {
    const availability = getAvailability(agent.pubkey);
    const channelId = resolveManagedAgentChannelId(agent, {
      channels,
      preferredChannelId,
      relayAgents,
    });

    let warning: string;
    if (channelId) {
      // Only established Offline preserves the intentional no-request path.
      // Unknown is not evidence that shutdown can safely be skipped.
      if (availability !== "offline") {
        await sendChannelMessage(channelId, "!shutdown", undefined, undefined, [
          agent.pubkey,
        ]);
        warning =
          (availability === undefined
            ? "This agent’s availability is unknown. "
            : "") +
          "Shutdown requested, but the agent may still be running. " +
          "Deleting now removes the local record — the remote deployment " +
          "will be orphaned if shutdown hasn't completed.";
      } else {
        warning =
          "This agent is offline but the remote deployment may still exist. " +
          "Deleting removes the local management record.";
      }
    } else {
      warning =
        "This agent is deployed but not in any channel. " +
        "Deleting removes the local management record; the remote deployment may still be running.";
    }
    if (
      !skipRemoteDeleteConfirm &&
      !(await confirm({
        title: `Delete ${agent.name}?`,
        description: warning,
        confirmLabel: "Delete agent",
        destructive: true,
      }))
    ) {
      return { cancelled: true };
    }
  }

  const isDeployedRemote =
    agent.backend.type === "provider" && agent.backendAgentId;
  return deleted(
    await deleteManagedAgent({
      pubkey: agent.pubkey,
      forceRemoteDelete: isDeployedRemote ? true : undefined,
    }),
  );
}

function deleted(
  channelCleanup: ChannelCleanupReport | undefined,
): ManagedAgentActionResult {
  return channelCleanup ? { channelCleanup } : {};
}

/**
 * A machine agent is removed from its machine first; the backend requires
 * the machine's acknowledgement. When the machine cannot be reached the user
 * can still delete the record (the agent may keep running there until the
 * machine is forgotten), so a dead machine never strands the agent.
 */
async function deleteHostAgent(
  agent: ManagedAgent,
  deleteManagedAgent: DeleteManagedAgent,
  confirm: ConfirmFn,
  skipConfirm: boolean,
): Promise<ManagedAgentActionResult> {
  try {
    return deleted(await deleteManagedAgent({ pubkey: agent.pubkey }));
  } catch (error) {
    const message = error instanceof Error ? error.message : String(error);
    if (!message.startsWith(HOST_UNDEPLOY_FAILED_PREFIX)) throw error;
    const reason = message.slice(HOST_UNDEPLOY_FAILED_PREFIX.length);
    if (
      !skipConfirm &&
      !(await confirm({
        title: `Delete ${agent.name} anyway?`,
        description:
          `The machine did not confirm removing this agent (${reason}). ` +
          "Delete it here anyway? It may keep running on the machine until " +
          "you forget the machine.",
        confirmLabel: "Delete anyway",
        destructive: true,
      }))
    ) {
      return { cancelled: true };
    }
    return deleted(
      await deleteManagedAgent({
        pubkey: agent.pubkey,
        forceRemoteDelete: true,
      }),
    );
  }
}
