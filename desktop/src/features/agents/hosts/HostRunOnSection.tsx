import * as React from "react";

import { PresenceDot } from "@/features/presence/ui/PresenceBadge";
import { normalizePubkey } from "@/shared/lib/pubkey";
import { Button } from "@/shared/ui/button";
import { PersonaDropdownField } from "../ui/PersonaDropdownField";
import { hostPubkeyFromRunOn } from "../ui/whereToRunIntent";
import {
  buildHostRunOnOptions,
  describeHost,
  hostAvailability,
} from "./hostRunOptions";
import {
  useAgentHostsWithPresence,
  useDeployToHostMutation,
} from "./useAgentHosts";

/**
 * Edit-dialog "Run on" for an agent on a machine: which machine, its state,
 * and "Move to" another approved machine. A move removes the agent from the
 * current machine first and only then deploys it on the new one.
 */
export function HostRunOnSection({
  agentPubkey,
  hostPubkey,
}: {
  agentPubkey: string | undefined;
  hostPubkey: string;
}) {
  const { hosts, presence, presenceLoaded } = useAgentHostsWithPresence();
  const deploy = useDeployToHostMutation();
  const [target, setTarget] = React.useState("");
  const now = Date.now();
  const current = hosts.find(
    (host) => normalizePubkey(host.pubkey) === normalizePubkey(hostPubkey),
  );
  const availability = hostAvailability(presence, presenceLoaded, hostPubkey);
  const moveOptions = buildHostRunOnOptions(
    hosts.filter((host) => host !== current),
    presence,
    presenceLoaded,
    now,
  );
  const targetPubkey = hostPubkeyFromRunOn(target);

  return (
    <div className="space-y-2" data-testid="edit-agent-run-on">
      <span className="text-sm font-medium text-foreground">Run on</span>
      <div className="flex items-center gap-2 rounded-2xl border border-border bg-muted/30 px-4 py-3 text-sm">
        <PresenceDot
          status={availability === "unknown" ? "offline" : availability}
        />
        <div className="min-w-0">
          <p className="font-medium" data-testid="edit-agent-run-on-location">
            {current?.name ?? "A machine that is no longer approved"}
          </p>
          {current ? (
            <p className="text-xs text-muted-foreground">
              {describeHost(current, availability, now)}
            </p>
          ) : null}
        </div>
      </div>
      {agentPubkey && moveOptions.length > 0 ? (
        <div className="space-y-1.5">
          <label
            className="text-xs text-muted-foreground"
            htmlFor="edit-agent-move-host"
          >
            Move to another machine
          </label>
          <div className="flex gap-2">
            <div className="min-w-0 flex-1">
              <PersonaDropdownField
                disabled={deploy.isPending}
                id="edit-agent-move-host"
                onValueChange={setTarget}
                options={moveOptions}
                placeholder="Choose a machine"
                value={target}
              />
            </div>
            <Button
              data-testid="edit-agent-move-host-submit"
              disabled={!targetPubkey || deploy.isPending}
              onClick={() => {
                if (!targetPubkey) return;
                deploy.mutate({
                  pubkey: agentPubkey,
                  hostPubkey: targetPubkey,
                });
              }}
              type="button"
              variant="outline"
            >
              {deploy.isPending ? "Moving…" : "Move"}
            </Button>
          </div>
          {deploy.error ? (
            <p className="text-xs text-destructive" role="alert">
              {deploy.error instanceof Error
                ? deploy.error.message
                : String(deploy.error)}
            </p>
          ) : null}
        </div>
      ) : null}
    </div>
  );
}
