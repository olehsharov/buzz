import * as React from "react";

import { PresenceDot } from "@/features/presence/ui/PresenceBadge";
import { normalizePubkey } from "@/shared/lib/pubkey";
import { Button } from "@/shared/ui/button";
import { PersonaDropdownField } from "../ui/PersonaDropdownField";
import { hostPubkeyFromRunOn } from "../ui/whereToRunIntent";
import {
  buildHostRunOnOptions,
  describeHost,
  describeHostSetupProblem,
  hostAvailability,
} from "./hostRunOptions";
import { HostDeployError, HostRepairNotice } from "./HostRepairNotice";
import { HostWorkdirField } from "./HostWorkdirField";
import {
  useAgentHostsWithPresence,
  useDeployToHostMutation,
  useSetHostAgentWorkdirMutation,
} from "./useAgentHosts";

/** Env var naming an ACP session the agent resumes on start. */
const RESUME_SESSION_ENV = "BUZZ_ACP_RESUME_SESSION";

function errorText(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

/**
 * Edit-dialog "Run on" for an agent on a machine: which machine, its state,
 * the folder it runs in there, and "Move to" another approved machine. A
 * move removes the agent from the current machine first and only then
 * deploys it on the new one; the folder moves with it. Saving a new folder
 * restarts the agent on its machine in that folder.
 */
export function HostRunOnSection({
  agentPubkey,
  hostPubkey,
  workdir,
  envVars,
}: {
  agentPubkey: string | undefined;
  hostPubkey: string;
  /** Saved folder on the machine; null = the machine's default. */
  workdir: string | null;
  envVars?: Record<string, string>;
}) {
  const { hosts, presence, presenceLoaded } = useAgentHostsWithPresence();
  const deploy = useDeployToHostMutation();
  const setWorkdir = useSetHostAgentWorkdirMutation();
  const [target, setTarget] = React.useState("");
  // null = showing the saved folder; a string = the owner's unsaved edit.
  const [folderDraft, setFolderDraft] = React.useState<string | null>(null);
  const now = Date.now();
  const current = hosts.find(
    (host) => normalizePubkey(host.pubkey) === normalizePubkey(hostPubkey),
  );
  const availability = hostAvailability(presence, presenceLoaded, hostPubkey);
  const setupProblem = describeHostSetupProblem(current?.status ?? null);
  const moveOptions = buildHostRunOnOptions(
    hosts.filter((host) => host !== current),
    presence,
    presenceLoaded,
    now,
  );
  const targetPubkey = hostPubkeyFromRunOn(target);
  const busy = deploy.isPending || setWorkdir.isPending;
  const folderValue = folderDraft ?? workdir ?? "";
  const folderChanged = folderValue.trim() !== (workdir ?? "");
  const resumesSession = Boolean(envVars?.[RESUME_SESSION_ENV]?.trim());

  const saveFolder = () => {
    if (!agentPubkey || !folderChanged || busy) return;
    setWorkdir.mutate(
      { pubkey: agentPubkey, workdir: folderValue.trim() || null },
      {
        // On failure the typed value stays for a retry. When only the
        // restart failed the folder is saved, so the refreshed record
        // matches the draft and Save turns off.
        onSuccess: () => setFolderDraft(null),
      },
    );
  };

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
          {setupProblem ? (
            <HostRepairNotice
              message={setupProblem}
              testId="edit-agent-run-on-setup"
            />
          ) : null}
          <p
            className="break-all text-xs text-muted-foreground"
            data-testid="edit-agent-run-on-workdir"
          >
            Folder:{" "}
            {workdir ? (
              <span className="font-mono text-foreground">{workdir}</span>
            ) : (
              "default (~/buzz-agents/…)"
            )}
          </p>
        </div>
      </div>
      {agentPubkey ? (
        <div className="space-y-1.5">
          <HostWorkdirField
            disabled={busy}
            extraHint={
              resumesSession
                ? "This agent resumes a saved session, so the folder must be the folder that session ran in."
                : null
            }
            id="edit-agent-host-workdir"
            machineName={current?.name ?? "the machine"}
            onChange={setFolderDraft}
            onKeyDown={(event) => {
              if (event.key === "Enter" && !event.nativeEvent.isComposing) {
                event.preventDefault();
                saveFolder();
              }
            }}
            testId="edit-agent-host-workdir"
            value={folderValue}
          >
            <Button
              data-testid="edit-agent-host-workdir-submit"
              disabled={!folderChanged || busy}
              onClick={saveFolder}
              type="button"
              variant="outline"
            >
              {setWorkdir.isPending ? "Saving…" : "Save folder"}
            </Button>
          </HostWorkdirField>
          {setWorkdir.error ? (
            <HostDeployError
              message={errorText(setWorkdir.error)}
              testId="edit-agent-host-workdir-error"
            />
          ) : null}
        </div>
      ) : null}
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
                disabled={busy}
                id="edit-agent-move-host"
                onValueChange={setTarget}
                options={moveOptions}
                placeholder="Choose a machine"
                value={target}
              />
            </div>
            <Button
              data-testid="edit-agent-move-host-submit"
              disabled={!targetPubkey || busy}
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
            <HostDeployError
              message={errorText(deploy.error)}
              testId="edit-agent-move-host-error"
            />
          ) : null}
        </div>
      ) : null}
    </div>
  );
}
