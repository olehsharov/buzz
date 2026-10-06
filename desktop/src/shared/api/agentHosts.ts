import {
  fromRawManagedAgent,
  invokeTauri,
  type RawManagedAgent,
} from "@/shared/api/tauri";
import type { ManagedAgent, RelayEvent } from "@/shared/api/types";

/** One agent's state as last reported by its machine. */
export type AgentHostAgentState = {
  agentPubkey: string;
  state: string;
};

/** An approved machine (`buzz host`) in the active community. */
export type AgentHost = {
  pubkey: string;
  name: string;
  os: string;
  arch: string;
  relayUrl: string;
  addedAt: string;
  /** Last `host.status` the desktop accepted, if any. */
  status: {
    version: string | null;
    agents: AgentHostAgentState[];
    claude: { installed: boolean; authOk: boolean | null };
    tools: { node: boolean; claudeAgentAcp: boolean; buzzAcp: boolean };
    /** Unix seconds. */
    receivedAt: number;
  } | null;
};

type RawAgentHost = {
  pubkey: string;
  name: string;
  os: string;
  arch: string;
  relay_url: string;
  added_at: string;
  status?: {
    version?: string | null;
    agents?: { agent_pubkey: string; state: string }[];
    claude?: { installed?: boolean; auth_ok?: boolean | null };
    tools?: { node?: boolean; claude_agent_acp?: boolean; buzz_acp?: boolean };
    received_at: number;
  } | null;
};

export function fromRawAgentHost(raw: RawAgentHost): AgentHost {
  return {
    pubkey: raw.pubkey,
    name: raw.name,
    os: raw.os,
    arch: raw.arch,
    relayUrl: raw.relay_url,
    addedAt: raw.added_at,
    status: raw.status
      ? {
          version: raw.status.version ?? null,
          agents: (raw.status.agents ?? []).map((agent) => ({
            agentPubkey: agent.agent_pubkey,
            state: agent.state,
          })),
          claude: {
            installed: raw.status.claude?.installed ?? false,
            authOk: raw.status.claude?.auth_ok ?? null,
          },
          tools: {
            node: raw.status.tools?.node ?? false,
            claudeAgentAcp: raw.status.tools?.claude_agent_acp ?? false,
            buzzAcp: raw.status.tools?.buzz_acp ?? false,
          },
          receivedAt: raw.status.received_at,
        }
      : null,
  };
}

/** The "Add machine" commands for one pairing session. */
export type HostInstallInfo = {
  /** `https://<community relay>/host`, where the relay serves the installer. */
  baseUrl: string;
  /** Install from the relay and pair, in one `curl … | bash` line. */
  command: string;
  /** For machines that already have `buzz host`. */
  upCommand: string;
  /** How long the pairing code stays valid. */
  sessionTtlSecs: number;
};

export async function getHostInstallInfo(
  pairingUri: string,
): Promise<HostInstallInfo> {
  const raw = await invokeTauri<{
    base_url: string;
    command: string;
    up_command: string;
    session_ttl_secs: number;
  }>("get_host_install_info", { pairingUri });
  return {
    baseUrl: raw.base_url,
    command: raw.command,
    upCommand: raw.up_command,
    sessionTtlSecs: raw.session_ttl_secs,
  };
}

export async function listAgentHosts(): Promise<AgentHost[]> {
  const raw = await invokeTauri<RawAgentHost[]>("list_agent_hosts");
  return raw.map(fromRawAgentHost);
}

/** Start NIP-AB "approve a machine". Resolves to the pairing URI. */
export async function startHostPairing(): Promise<string> {
  return invokeTauri<string>("start_host_pairing");
}

/** Deploy, redeploy or move an agent to a machine; waits for its ack. */
export async function deployToHost(
  pubkey: string,
  hostPubkey: string,
): Promise<ManagedAgent> {
  const raw = await invokeTauri<RawManagedAgent>("deploy_to_host", {
    pubkey,
    hostPubkey,
  });
  return fromRawManagedAgent(raw);
}

export async function undeployFromHost(pubkey: string): Promise<ManagedAgent> {
  const raw = await invokeTauri<RawManagedAgent>("undeploy_from_host", {
    pubkey,
  });
  return fromRawManagedAgent(raw);
}

export async function requestHostStatus(
  hostPubkey: string,
): Promise<AgentHost> {
  return fromRawAgentHost(
    await invokeTauri<RawAgentHost>("request_host_status", { hostPubkey }),
  );
}

export type ForgetHostOutcome = {
  hostAcknowledged: boolean;
  hostError: string | null;
  undeployedAgents: string[];
};

export async function forgetHost(
  hostPubkey: string,
): Promise<ForgetHostOutcome> {
  const raw = await invokeTauri<{
    host_acknowledged: boolean;
    host_error: string | null;
    undeployed_agents: string[];
  }>("forget_host", { hostPubkey });
  return {
    hostAcknowledged: raw.host_acknowledged,
    hostError: raw.host_error,
    undeployedAgents: raw.undeployed_agents,
  };
}

/** Hand a telemetry frame from the observer subscription to the backend,
 * which verifies it. Resolves to the updated host for accepted status. */
export async function ingestHostTelemetry(
  event: RelayEvent,
): Promise<AgentHost | null> {
  const raw = await invokeTauri<RawAgentHost | null>("ingest_host_telemetry", {
    eventJson: JSON.stringify(event),
  });
  return raw ? fromRawAgentHost(raw) : null;
}

/** Prefix of a delete error when the agent's machine did not confirm the
 * undeploy (mirrors `HOST_UNDEPLOY_FAILED_PREFIX` in Rust). */
export const HOST_UNDEPLOY_FAILED_PREFIX = "host-undeploy-failed: ";
