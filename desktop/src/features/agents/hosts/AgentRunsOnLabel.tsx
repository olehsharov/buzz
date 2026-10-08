import type { ManagedAgentBackend } from "@/shared/api/types";
import { cn } from "@/shared/lib/cn";
import { hostForAgent } from "./hostRunOptions";
import { useAgentHostsQuery } from "./useAgentHosts";

/** "Runs on <machine>" for agents on an approved machine; nothing otherwise.
 * Tolerates records without a backend (pre-backend fixtures). */
export function AgentRunsOnLabel({
  backend,
  className,
}: {
  backend: ManagedAgentBackend | undefined;
  className?: string;
}) {
  const text = useAgentRunsOnText(backend);
  if (!text) return null;
  return (
    <span
      className={cn(
        "min-w-0 truncate text-xs text-muted-foreground",
        className,
      )}
      data-testid="agent-runs-on"
    >
      {text}
    </span>
  );
}

/** Plain-text variant for places that render strings. */
export function useAgentRunsOnText(
  backend: ManagedAgentBackend | undefined,
): string | null {
  const hosts = useAgentHostsQuery().data;
  if (backend?.type !== "host") return null;
  return `Runs on ${hostForAgent(hosts, backend)?.name ?? "a forgotten machine"}`;
}
