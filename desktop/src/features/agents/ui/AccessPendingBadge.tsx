import { ShieldAlert } from "lucide-react";
import { toast } from "sonner";

import { useStartManagedAgentMutation } from "@/features/agents/hooks";
import type { ManagedAgent } from "@/shared/api/types";
import { Badge } from "@/shared/ui/badge";
import { Button } from "@/shared/ui/button";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/shared/ui/tooltip";

export const ACCESS_PENDING_BLURB =
  "The new access setting is saved and published, but the deployed agent has not been redeployed with it yet, so it still uses its previous access. Buzz retries the redeploy the next time this community loads.";

/**
 * Shown on a provider or paired-machine agent whose saved access policy has
 * not been delivered by a redeploy yet (`providerPolicyPending`). The badge is
 * a non-interactive status (the row shows the last error); Retry is the only
 * action and owns its own label. Retry redeploys through Start, which
 * acknowledges the policy on success.
 */
export function AccessPendingBadge({
  agent,
}: {
  agent: Pick<ManagedAgent, "pubkey" | "name">;
}) {
  const startMutation = useStartManagedAgentMutation();

  return (
    <div className="flex items-center gap-1.5">
      <Tooltip>
        <TooltipTrigger asChild>
          <Badge
            className="cursor-default gap-1"
            data-testid="agent-access-pending"
            tabIndex={0}
            variant="warning"
          >
            <ShieldAlert aria-hidden="true" className="h-3 w-3" />
            Access change pending
          </Badge>
        </TooltipTrigger>
        <TooltipContent className="max-w-72 text-xs" side="bottom">
          <p>{ACCESS_PENDING_BLURB}</p>
        </TooltipContent>
      </Tooltip>
      <Button
        aria-label={`Retry applying the access change to ${agent.name}`}
        data-testid="agent-access-pending-retry"
        disabled={startMutation.isPending}
        onClick={() => {
          startMutation.mutate(agent.pubkey, {
            onError: (error) => {
              toast.error(
                `Couldn't apply the access change: ${error instanceof Error ? error.message : String(error)}`,
              );
            },
          });
        }}
        size="sm"
        type="button"
        variant="outline"
      >
        {startMutation.isPending ? "Retrying…" : "Retry"}
      </Button>
    </div>
  );
}
