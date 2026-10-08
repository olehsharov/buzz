import * as React from "react";

import { useManagedAgentsQuery } from "@/features/agents/hooks";
import { PresenceDot } from "@/features/presence/ui/PresenceBadge";
import {
  SettingsOptionGroup,
  SettingsOptionRow,
} from "@/features/settings/ui/SettingsOptionGroup";
import type { AgentHost } from "@/shared/api/agentHosts";
import { normalizePubkey } from "@/shared/lib/pubkey";
import {
  AlertDialog,
  AlertDialogAction,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
} from "@/shared/ui/alert-dialog";
import { Button, buttonVariants } from "@/shared/ui/button";
import { AddMachineDialog } from "./AddMachineDialog";
import {
  describeHost,
  describeHostClaude,
  hostAvailability,
} from "./hostRunOptions";
import {
  useAgentHostsWithPresence,
  useForgetHostMutation,
} from "./useAgentHosts";

/** Settings → Agents → Machines: approved machines in this community. */
export function MachinesSettingsGroup() {
  const { hosts, hostsQuery, presence, presenceLoaded } =
    useAgentHostsWithPresence();
  const agents = useManagedAgentsQuery().data ?? [];
  const [addOpen, setAddOpen] = React.useState(false);
  const [forgetting, setForgetting] = React.useState<AgentHost | null>(null);
  const [notice, setNotice] = React.useState<string | null>(null);
  const forget = useForgetHostMutation();
  const now = Date.now();

  const agentsOn = (host: AgentHost) =>
    agents.filter(
      (agent) =>
        agent.backend.type === "host" &&
        normalizePubkey(agent.backend.host_pubkey) ===
          normalizePubkey(host.pubkey),
    );

  const confirmForget = () => {
    const host = forgetting;
    if (!host) return;
    forget.mutate(host.pubkey, {
      onSuccess: (outcome) => {
        setNotice(
          outcome.hostAcknowledged
            ? `${host.name} was forgotten.`
            : `${host.name} was removed here, but it did not confirm (${outcome.hostError ?? "no answer"}). Run "buzz host forget" on it to stop its agents.`,
        );
      },
      onError: (error) => {
        setNotice(error instanceof Error ? error.message : String(error));
      },
      onSettled: () => setForgetting(null),
    });
  };

  return (
    <SettingsOptionGroup
      data-testid="settings-machines"
      description="Computers you approved to run your agents in this community."
      headerAction={
        <Button
          data-testid="settings-add-machine"
          onClick={() => setAddOpen(true)}
          size="sm"
          type="button"
          variant="outline"
        >
          Add machine…
        </Button>
      }
      title="Machines"
    >
      {hostsQuery.isError ? (
        <SettingsOptionRow>
          <p className="text-sm text-destructive" role="alert">
            Could not load machines.
          </p>
        </SettingsOptionRow>
      ) : hosts.length === 0 ? (
        <SettingsOptionRow>
          <p className="text-sm text-muted-foreground">
            No machines yet. Add one to run agents while this computer sleeps.
          </p>
        </SettingsOptionRow>
      ) : (
        <ul aria-label="Machines" className="divide-y divide-border/60">
          {hosts.map((host) => {
            const availability = hostAvailability(
              presence,
              presenceLoaded,
              host.pubkey,
            );
            const onHost = agentsOn(host);
            return (
              <li data-testid={`machine-row-${host.pubkey}`} key={host.pubkey}>
                <SettingsOptionRow>
                  <div className="flex min-w-0 items-start gap-3">
                    <PresenceDot
                      className="mt-1.5"
                      status={
                        availability === "unknown" ? "offline" : availability
                      }
                    />
                    <div className="min-w-0">
                      <p className="truncate font-medium text-foreground">
                        {host.name}
                      </p>
                      <p className="text-sm text-muted-foreground">
                        {describeHost(host, availability, now)}
                      </p>
                      <p className="text-xs text-muted-foreground">
                        {describeHostClaude(host.status)}
                      </p>
                      <p className="text-xs text-muted-foreground">
                        {onHost.length === 0
                          ? "No agents"
                          : `Agents: ${onHost.map((agent) => agent.name).join(", ")}`}
                      </p>
                    </div>
                  </div>
                  <Button
                    aria-label={`Forget machine ${host.name}`}
                    data-testid={`machine-forget-${host.pubkey}`}
                    onClick={() => setForgetting(host)}
                    size="sm"
                    type="button"
                    variant="outline"
                  >
                    Forget machine
                  </Button>
                </SettingsOptionRow>
              </li>
            );
          })}
        </ul>
      )}
      {notice ? (
        <SettingsOptionRow>
          <p
            className="text-sm text-muted-foreground"
            data-testid="settings-machines-notice"
            role="status"
          >
            {notice}
          </p>
        </SettingsOptionRow>
      ) : null}

      <AddMachineDialog onOpenChange={setAddOpen} open={addOpen} />

      <AlertDialog
        onOpenChange={(open) => {
          if (!open && !forget.isPending) setForgetting(null);
        }}
        open={forgetting !== null}
      >
        <AlertDialogContent data-testid="forget-machine-dialog">
          <AlertDialogHeader>
            <AlertDialogTitle>Forget {forgetting?.name}?</AlertDialogTitle>
            <AlertDialogDescription>
              The machine stops every agent you run on it and deletes their
              keys. Agents stay in Buzz, undeployed, so you can deploy them
              somewhere else. To use this machine again, add it again.
            </AlertDialogDescription>
          </AlertDialogHeader>
          <AlertDialogFooter>
            <AlertDialogCancel disabled={forget.isPending}>
              Cancel
            </AlertDialogCancel>
            <AlertDialogAction
              className={buttonVariants({ variant: "destructive" })}
              data-testid="forget-machine-confirm"
              disabled={forget.isPending}
              onClick={(event) => {
                event.preventDefault();
                confirmForget();
              }}
            >
              {forget.isPending ? "Forgetting…" : "Forget machine"}
            </AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>
    </SettingsOptionGroup>
  );
}
