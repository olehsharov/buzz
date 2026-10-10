import type { AgentHost } from "@/shared/api/agentHosts";
import type { PresenceLookup, PresenceStatus } from "@/shared/api/types";
import { normalizePubkey } from "@/shared/lib/pubkey";
import { hostRunOnValue } from "../ui/whereToRunIntent";

/**
 * A machine's availability for deploys. `unknown` = presence has not loaded
 * (or failed); treated like offline for deploys, but labelled honestly.
 */
export type HostAvailability = PresenceStatus | "unknown";

export function hostAvailability(
  lookup: PresenceLookup | undefined,
  presenceLoaded: boolean,
  hostPubkey: string,
): HostAvailability {
  if (!presenceLoaded || !lookup) return "unknown";
  return lookup[normalizePubkey(hostPubkey)] ?? "offline";
}

/** Online and away hosts can take deploys; offline and unknown cannot. */
export function hostAcceptsDeploys(availability: HostAvailability): boolean {
  return availability === "online" || availability === "away";
}

export function hostOsLabel(os: string): string {
  switch (os.toLowerCase()) {
    case "linux":
      return "Linux";
    case "macos":
    case "darwin":
      return "macOS";
    default:
      return os;
  }
}

/** "last seen 5 min ago" from the newest status frame, or null when never. */
export function formatHostLastSeen(
  receivedAtSecs: number | null | undefined,
  nowMs: number,
): string | null {
  if (!receivedAtSecs) return null;
  const elapsed = Math.max(0, Math.floor(nowMs / 1000) - receivedAtSecs);
  if (elapsed < 60) return "last seen just now";
  const minutes = Math.floor(elapsed / 60);
  if (minutes < 60) return `last seen ${minutes} min ago`;
  const hours = Math.floor(minutes / 60);
  if (hours < 24) return `last seen ${hours} h ago`;
  const days = Math.floor(hours / 24);
  return `last seen ${days} d ago`;
}

/** One line describing a machine's state, used as the option's second line
 * and as the visible status in the Machines list. */
export function describeHost(
  host: Pick<AgentHost, "os" | "arch" | "status">,
  availability: HostAvailability,
  nowMs: number,
): string {
  const os = `${hostOsLabel(host.os)} · ${host.arch}`;
  switch (availability) {
    case "online":
      return `${os} · Online`;
    case "away":
      return `${os} · Away`;
    case "unknown":
      return `${os} · Checking…`;
    default: {
      const lastSeen = formatHostLastSeen(host.status?.receivedAt, nowMs);
      return lastSeen ? `${os} · Offline, ${lastSeen}` : `${os} · Offline`;
    }
  }
}

export type HostRunOnOption = {
  value: string;
  label: string;
  description: string;
  disabled: boolean;
  presence: PresenceStatus;
};

/** "Where to run" options for approved machines, in name order. Offline (or
 * not-yet-known) machines are listed but cannot be picked. */
export function buildHostRunOnOptions(
  hosts: readonly AgentHost[],
  lookup: PresenceLookup | undefined,
  presenceLoaded: boolean,
  nowMs: number,
): HostRunOnOption[] {
  return [...hosts]
    .sort((a, b) => a.name.localeCompare(b.name))
    .map((host) => {
      const availability = hostAvailability(
        lookup,
        presenceLoaded,
        host.pubkey,
      );
      const problem = describeHostSetupProblem(host.status);
      const state = describeHost(host, availability, nowMs);
      return {
        value: hostRunOnValue(host.pubkey),
        label: host.name,
        description: problem ? `${state} · ${problem}` : state,
        disabled: !hostAcceptsDeploys(availability),
        presence: availability === "unknown" ? "offline" : availability,
      };
    });
}

/** Drop approved machines (host keys) from a list of relay agents. */
export function excludeMachineAgents<T extends { pubkey: string }>(
  agents: readonly T[],
  hosts: readonly Pick<AgentHost, "pubkey">[],
): T[] {
  if (hosts.length === 0) return [...agents];
  const machines = new Set(hosts.map((host) => normalizePubkey(host.pubkey)));
  return agents.filter((agent) => !machines.has(normalizePubkey(agent.pubkey)));
}

/** "Claude: ready / not signed in / unknown / not installed" — the host
 * cannot verify sign-in yet (`auth_ok` is null), so unknown is not a check. */
export function describeHostClaude(
  status: Pick<NonNullable<AgentHost["status"]>, "claude"> | null,
): string {
  if (!status) return "Claude: unknown";
  if (!status.claude.installed) return "Claude: not installed";
  if (status.claude.authOk === true) return "Claude: ready";
  if (status.claude.authOk === false) return "Claude: not signed in";
  return "Claude: installed, sign-in unknown";
}

/** What to do about anything a machine is missing. */
export const HOST_REPAIR_HINT = "re-run the install snippet";

/**
 * What a machine reports missing for Claude agents, from its last status:
 * "Missing Claude adapter — re-run the install snippet". Null when it has
 * everything, or before it has reported (nothing to go on yet).
 */
export function describeHostSetupProblem(
  status: Pick<NonNullable<AgentHost["status"]>, "claude" | "tools"> | null,
): string | null {
  if (!status) return null;
  const missing: string[] = [];
  if (!status.tools.claudeAgentAcp) missing.push("Claude adapter");
  if (!status.claude.installed) missing.push("Claude Code");
  if (!status.tools.node) missing.push("Node.js");
  if (!status.tools.buzzAcp) missing.push("buzz-acp");
  if (missing.length === 0) return null;
  return `Missing ${missing.join(", ")} — ${HOST_REPAIR_HINT}`;
}

/** A deploy the machine refused because a tool is not installed there
 * (`buzz-host` env.rs: "… is not installed on this host …"). */
export function isHostToolMissingError(message: string): boolean {
  return message.includes("is not installed on this host");
}

/** The machine an agent runs on, for "Runs on <machine>". */
export function hostForAgent(
  hosts: readonly AgentHost[] | undefined,
  backend: { type: string; host_pubkey?: string },
): AgentHost | null {
  if (backend.type !== "host" || !backend.host_pubkey) return null;
  const key = normalizePubkey(backend.host_pubkey);
  return hosts?.find((host) => normalizePubkey(host.pubkey) === key) ?? null;
}
