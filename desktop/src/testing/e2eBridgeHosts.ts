/**
 * Mock agent hosts (`buzz host` machines) for the E2E bridge: the hosts
 * store, the host-approval pairing flow, deploy/forget, and presence. A test
 * double of the desktop backend built from the hosts contract v1; the real
 * frame encryption, ack waiting and grant signing are covered by Rust tests.
 */

import type { PresenceStatus } from "@/shared/api/types";

export type MockAgentHostConfig = {
  pubkey: string;
  name: string;
  os?: string;
  arch?: string;
  online?: boolean;
  /** Seconds since the last status frame (drives "last seen"). */
  lastSeenSecsAgo?: number;
};

type RawHost = {
  pubkey: string;
  name: string;
  os: string;
  arch: string;
  relay_url: string;
  added_at: string;
  status: {
    version: string | null;
    agents: { agent_pubkey: string; state: string }[];
    claude: { installed: boolean; auth_ok: boolean | null };
    tools: { node: boolean; claude_agent_acp: boolean; buzz_acp: boolean };
    received_at: number;
  } | null;
};

type MockHello = {
  host_pubkey: string;
  name: string;
  os: string;
  arch: string;
};

type MockAgentLike = {
  pubkey: string;
  status: string;
  backend: { type: string; [key: string]: unknown };
  backend_agent_id: string | null;
};

type HostContext = {
  emit: (event: string, payload: unknown) => Promise<void>;
  setPresence: (pubkey: string, status: PresenceStatus) => void;
  agents: () => MockAgentLike[];
  relayUrl: string;
};

let hosts: RawHost[] = [];
let pairingMode: "host" | null = null;
let pendingHello: MockHello | null = null;

function toRaw(config: MockAgentHostConfig, relayUrl: string): RawHost {
  const now = Math.floor(Date.now() / 1000);
  return {
    pubkey: config.pubkey,
    name: config.name,
    os: config.os ?? "linux",
    arch: config.arch ?? "x86_64",
    relay_url: relayUrl,
    added_at: new Date().toISOString(),
    status:
      config.lastSeenSecsAgo === undefined
        ? null
        : {
            version: "0.1.0",
            agents: [],
            claude: { installed: true, auth_ok: true },
            tools: { node: true, claude_agent_acp: true, buzz_acp: true },
            received_at: now - config.lastSeenSecsAgo,
          },
  };
}

export function initMockAgentHosts(
  configs: MockAgentHostConfig[] | undefined,
  ctx: Pick<HostContext, "setPresence" | "relayUrl">,
) {
  hosts = (configs ?? []).map((config) => toRaw(config, ctx.relayUrl));
  for (const config of configs ?? []) {
    ctx.setPresence(config.pubkey, config.online ? "online" : "offline");
  }
  pairingMode = null;
  pendingHello = null;
}

/** Test hook: the machine scanned the code and shows `sas`. */
export async function mockHostPairingOffer(
  ctx: Pick<HostContext, "emit">,
  sas: string,
  hello: MockHello,
) {
  pendingHello = hello;
  await ctx.emit("pairing-sas-received", { sas });
}

/** Returns `{ handled: false }` for commands this module does not own. */
export async function handleMockHostCommand(
  command: string,
  payload: unknown,
  ctx: HostContext,
): Promise<{ handled: boolean; value?: unknown }> {
  const args = (payload ?? {}) as Record<string, unknown>;
  switch (command) {
    case "get_host_install_info":
      return {
        handled: true,
        value: {
          install_url: "https://example.invalid/buzz-host/install.sh",
          command: `curl -fsSL 'https://example.invalid/buzz-host/install.sh' | sh -s -- --relay '${ctx.relayUrl}'`,
          relay_url: ctx.relayUrl,
        },
      };
    case "list_agent_hosts":
      return { handled: true, value: structuredClone(hosts) };
    case "start_host_pairing":
      pairingMode = "host";
      pendingHello = null;
      return {
        handled: true,
        value:
          "nostrpair://8f4b8db31967ce14fef970a1ff1e8eecf19a430aa1c83875e2f5be68dcac0f1a?relay=wss%3A%2F%2Frelay.example.com&secret=87d5a8cfd5807a0cb44f728b67d88d6dcb8daf99be137c158f21a50c1e913c0a&v=1",
      };
    case "confirm_pairing_sas": {
      if (pairingMode !== "host") return { handled: false };
      // Approve = SAS confirm; the machine's hello arrives, the backend sends
      // the grant, and the machine completes.
      const hello = pendingHello;
      if (!hello) throw new Error("no active pairing session");
      window.__BUZZ_E2E_HOST_GRANTS__ = [
        ...(window.__BUZZ_E2E_HOST_GRANTS__ ?? []),
        { host_pubkey: hello.host_pubkey, relay_url: ctx.relayUrl },
      ];
      await ctx.emit("host-pairing-hello", hello);
      hosts = [
        ...hosts.filter((host) => host.pubkey !== hello.host_pubkey),
        toRaw(
          {
            pubkey: hello.host_pubkey,
            name: hello.name,
            os: hello.os,
            arch: hello.arch,
          },
          ctx.relayUrl,
        ),
      ];
      ctx.setPresence(hello.host_pubkey, "online");
      pairingMode = null;
      await ctx.emit("pairing-complete", { host_pubkey: hello.host_pubkey });
      return { handled: true, value: null };
    }
    case "cancel_pairing":
      pairingMode = null;
      pendingHello = null;
      return { handled: false };
    case "request_host_status":
      throw new Error("The machine did not answer in time.");
    case "ingest_host_telemetry":
      return { handled: true, value: null };
    case "forget_host": {
      const hostPubkey = String(args.hostPubkey ?? "");
      hosts = hosts.filter((host) => host.pubkey !== hostPubkey);
      const undeployed: string[] = [];
      for (const agent of ctx.agents()) {
        if (
          agent.backend.type === "host" &&
          agent.backend.host_pubkey === hostPubkey
        ) {
          agent.backend_agent_id = null;
          agent.status = "not_deployed";
          undeployed.push(agent.pubkey);
        }
      }
      return {
        handled: true,
        value: {
          host_acknowledged: true,
          host_error: null,
          undeployed_agents: undeployed,
        },
      };
    }
    default:
      return { handled: false };
  }
}

/** Deploy bookkeeping shared by create, start and deploy_to_host mocks. */
export function markMockAgentOnHost(agent: MockAgentLike, hostPubkey: string) {
  agent.backend = { type: "host", host_pubkey: hostPubkey };
  agent.backend_agent_id = hostPubkey;
  agent.status = "deployed";
}

declare global {
  interface Window {
    __BUZZ_E2E_HOST_GRANTS__?: { host_pubkey: string; relay_url: string }[];
    __BUZZ_E2E_HOST_PAIRING_OFFER__?: (
      sas: string,
      hello: MockHello,
    ) => Promise<void>;
  }
}
