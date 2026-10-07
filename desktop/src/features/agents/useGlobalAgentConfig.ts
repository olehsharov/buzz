/**
 * React hook: load the agent defaults of this window's community.
 *
 * Defaults are per community. The cache needs no community in its key: the
 * QueryClient itself is per community (`CommunityQueryProvider` is keyed on
 * the community and remounts on every switch, and each community window has
 * its own), so a switch always refetches the new community's defaults.
 *
 * Backed by TanStack Query with a stable query key so the config is fetched
 * once per QueryClient lifetime and shared across all callers — dialogs always
 * receive the already-populated value on first render, eliminating the
 * per-mount IPC race that caused required-env-key rows to be missing on open.
 *
 * On fetch error the query falls back to EMPTY_CONFIG (safe — the absence of
 * a global config is never an error state for callers).
 */
import { useQuery } from "@tanstack/react-query";

import { getGlobalAgentConfig } from "@/shared/api/tauriGlobalAgentConfig";
import type { GlobalAgentConfig } from "@/shared/api/types";

const EMPTY_CONFIG: GlobalAgentConfig = {
  env_vars: {},
  provider: null,
  model: null,
  preferred_runtime: null,
};

export const globalAgentConfigQueryKey = ["globalAgentConfig"] as const;

export function useGlobalAgentConfig(): {
  globalConfig: GlobalAgentConfig;
  isLoading: boolean;
} {
  const { data, isPending } = useQuery({
    queryKey: globalAgentConfigQueryKey,
    queryFn: getGlobalAgentConfig,
    // Config is only mutated via setGlobalAgentConfig — treat as stable until
    // explicitly invalidated by AgentDefaultsSettingsCard after a save.
    staleTime: Number.POSITIVE_INFINITY,
    // Never show a stale empty flash while a background refetch runs.
    placeholderData: EMPTY_CONFIG,
  });

  return {
    globalConfig: data ?? EMPTY_CONFIG,
    isLoading: isPending,
  };
}
