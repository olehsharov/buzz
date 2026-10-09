import * as React from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";

import { usePresenceQuery } from "@/features/presence/hooks";
import {
  type AgentHost,
  deployToHost,
  forgetHost,
  ingestHostTelemetry,
  listAgentHosts,
  requestHostStatus,
  setHostAgentWorkdir,
} from "@/shared/api/agentHosts";
import { relayClient } from "@/shared/api/relayClient";
import { useIdentityQuery } from "@/shared/api/hooks";
import { KIND_AGENT_OBSERVER_FRAME } from "@/shared/constants/kinds";
import type { RelayEvent } from "@/shared/api/types";
import { normalizePubkey } from "@/shared/lib/pubkey";

/** The query client is keyed per community, so this cache is too. */
export const agentHostsQueryKey = ["agent-hosts"] as const;

export function useAgentHostsQuery() {
  return useQuery({
    queryKey: agentHostsQueryKey,
    queryFn: listAgentHosts,
    staleTime: 30_000,
  });
}

/** Approved machines plus their kind:20001 presence. */
export function useAgentHostsWithPresence() {
  const hostsQuery = useAgentHostsQuery();
  const hosts = hostsQuery.data ?? [];
  const pubkeys = React.useMemo(
    () => hosts.map((host) => host.pubkey),
    [hosts],
  );
  const presenceQuery = usePresenceQuery(pubkeys);
  return {
    hosts,
    hostsQuery,
    presence: presenceQuery.data,
    presenceLoaded: presenceQuery.isSuccess,
  };
}

function replaceHost(
  hosts: AgentHost[] | undefined,
  updated: AgentHost,
): AgentHost[] | undefined {
  if (!hosts) return hosts;
  return hosts.map((host) =>
    normalizePubkey(host.pubkey) === normalizePubkey(updated.pubkey)
      ? updated
      : host,
  );
}

export function useForgetHostMutation() {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: (hostPubkey: string) => forgetHost(hostPubkey),
    onSettled: async () => {
      await Promise.all([
        queryClient.invalidateQueries({ queryKey: agentHostsQueryKey }),
        queryClient.invalidateQueries({ queryKey: ["managed-agents"] }),
      ]);
    },
  });
}

export function useDeployToHostMutation() {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: ({
      pubkey,
      hostPubkey,
    }: {
      pubkey: string;
      hostPubkey: string;
    }) => deployToHost(pubkey, hostPubkey),
    onSettled: async () => {
      await queryClient.invalidateQueries({ queryKey: ["managed-agents"] });
    },
  });
}

export function useSetHostAgentWorkdirMutation() {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: ({
      pubkey,
      workdir,
    }: {
      pubkey: string;
      workdir: string | null;
    }) => setHostAgentWorkdir(pubkey, workdir),
    onSettled: async () => {
      await queryClient.invalidateQueries({ queryKey: ["managed-agents"] });
    },
  });
}

/**
 * Keep machine details fresh: ask each approved machine for `host.status`
 * when the app opens (best effort), and fold the periodic status frames the
 * machines publish into the cache. Frames are verified by the backend; this
 * hook only carries them. Mounted once in the app shell.
 */
export function useAgentHostTelemetry() {
  const queryClient = useQueryClient();
  const identity = useIdentityQuery().data?.pubkey;
  const hostsQuery = useAgentHostsQuery();
  const hostKey = (hostsQuery.data ?? [])
    .map((host) => normalizePubkey(host.pubkey))
    .sort()
    .join(",");

  React.useEffect(() => {
    if (!identity || !hostKey) return;
    const authors = hostKey.split(",");
    let cancelled = false;

    const apply = (updated: AgentHost | null) => {
      if (cancelled || !updated) return;
      queryClient.setQueryData<AgentHost[]>(agentHostsQueryKey, (hosts) =>
        replaceHost(hosts, updated),
      );
    };

    for (const host of authors) {
      requestHostStatus(host).then(apply, () => {
        // Offline machines simply do not answer; presence already says so.
      });
    }

    let unsubscribe: (() => void) | null = null;
    void relayClient
      .subscribeInteractive(
        {
          kinds: [KIND_AGENT_OBSERVER_FRAME],
          "#p": [identity],
          authors,
          since: Math.floor(Date.now() / 1000) - 600,
          limit: 100,
        },
        (event: RelayEvent) => {
          ingestHostTelemetry(event).then(apply, () => {});
        },
      )
      .then((close) => {
        if (cancelled) close();
        else unsubscribe = close;
      })
      .catch(() => {});

    return () => {
      cancelled = true;
      unsubscribe?.();
    };
  }, [identity, hostKey, queryClient]);
}
