import { AlertTriangle } from "lucide-react";
import * as React from "react";

import { useBackendProvidersQuery } from "@/features/agents/hooks";
import { AddMachineDialog } from "@/features/agents/hosts/AddMachineDialog";
import { buildHostRunOnOptions } from "@/features/agents/hosts/hostRunOptions";
import { HostWorkdirField } from "@/features/agents/hosts/HostWorkdirField";
import { useAgentHostsWithPresence } from "@/features/agents/hosts/useAgentHosts";
import { probeBackendProvider } from "@/shared/api/tauri";
import { normalizePubkey } from "@/shared/lib/pubkey";

import { ProviderConfigFields } from "./ProviderConfigFields";
import { PersonaDropdownField } from "./PersonaDropdownField";
import {
  applyProbeResult,
  emptyWhereToRunDraft,
  hostPubkeyFromRunOn,
  type WhereToRunDraft,
} from "./whereToRunIntent";

/**
 * "Where to run": this computer, the user's approved machines (with
 * presence; offline ones listed but disabled), then provider scripts.
 * Buzz shared compute is an LLM provider, not a run destination.
 */
export function WhereToRunSection({
  draft,
  isPending,
  onDraftChange,
}: {
  draft: WhereToRunDraft;
  isPending: boolean;
  onDraftChange: (next: WhereToRunDraft) => void;
}) {
  const backendProviders = useBackendProvidersQuery().data ?? [];
  const { hosts, presence, presenceLoaded } = useAgentHostsWithPresence();
  const [addMachineOpen, setAddMachineOpen] = React.useState(false);
  const [probeError, setProbeError] = React.useState<string | null>(null);
  const runOnOptions = React.useMemo(
    () => [
      { label: "This computer", value: "local" },
      ...buildHostRunOnOptions(hosts, presence, presenceLoaded, Date.now()),
      ...backendProviders.map((provider) => ({
        label: provider.id,
        value: provider.id,
      })),
    ],
    [backendProviders, hosts, presence, presenceLoaded],
  );
  const selectedHostPubkey = hostPubkeyFromRunOn(draft.runOn);
  const selectedHost = selectedHostPubkey
    ? (hosts.find(
        (host) =>
          normalizePubkey(host.pubkey) === normalizePubkey(selectedHostPubkey),
      ) ?? null)
    : null;
  const isProviderMode = draft.runOn !== "local" && !selectedHostPubkey;
  const selectedBackendProvider = React.useMemo(
    () =>
      backendProviders.find((provider) => provider.id === draft.runOn) ?? null,
    [backendProviders, draft.runOn],
  );

  // Latest-state seam for probe resolution: an Effect Event always sees the
  // draft as it is *now*. Without this, the probe promise closes over the
  // draft from probe start, and anything typed while the probe was in flight
  // gets thrown away when it resolves (a second, subtler Typewriter Eraser).
  const applyProbe = React.useEffectEvent(
    (result: Awaited<ReturnType<typeof probeBackendProvider>>) => {
      onDraftChange(applyProbeResult(draft, result));
    },
  );

  // Probe once per provider *selection*, keyed on the provider's stable
  // path — never on the draft. Depending on the draft made every keystroke
  // refire the probe, and each resolution reset providerConfig to schema
  // defaults, which erased what the user was typing (the Typewriter Eraser)
  // and spawned the provider binary in a loop for as long as the dialog was
  // open. Keying on the path (not the provider object) also keeps a
  // providers-query refresh from reprobing an unchanged selection.
  const selectedBinaryPath = isProviderMode
    ? (selectedBackendProvider?.binaryPath ?? null)
    : null;
  React.useEffect(() => {
    if (!selectedBinaryPath || draft.probedProvider) {
      setProbeError(null);
      return;
    }
    let cancelled = false;
    setProbeError(null);
    void probeBackendProvider(selectedBinaryPath)
      .then((result) => {
        if (cancelled) return;
        applyProbe(result);
      })
      .catch((error: unknown) => {
        if (!cancelled) {
          setProbeError(error instanceof Error ? error.message : String(error));
        }
      });
    return () => {
      cancelled = true;
    };
  }, [selectedBinaryPath, draft.probedProvider]);

  return (
    <div className="space-y-4">
      <div className="space-y-1.5">
        <label className="text-sm font-medium" htmlFor="agent-run-on">
          Run on
        </label>
        <PersonaDropdownField
          disabled={isPending}
          id="agent-run-on"
          onValueChange={(runOn) =>
            onDraftChange({
              ...emptyWhereToRunDraft,
              runOn,
            })
          }
          footerAction={{
            label: "Add machine…",
            onSelect: () => setAddMachineOpen(true),
            testId: "where-to-run-add-machine",
          }}
          options={runOnOptions}
          placeholder="Choose where to run"
          value={draft.runOn}
        />
      </div>
      <AddMachineDialog
        onOpenChange={setAddMachineOpen}
        open={addMachineOpen}
      />

      {selectedHost ? (
        <>
          <p
            className="rounded-2xl border border-border bg-muted/30 px-4 py-3 text-sm text-muted-foreground"
            data-testid="where-to-run-host-note"
          >
            Runs on{" "}
            <span className="font-medium text-foreground">
              {selectedHost.name}
            </span>
            . Buzz sends this agent&apos;s key to that machine, encrypted, when
            it deploys, and waits for the machine to confirm.
          </p>
          <HostWorkdirField
            disabled={isPending}
            id="where-to-run-host-workdir"
            machineName={selectedHost.name}
            onChange={(hostWorkdir) => onDraftChange({ ...draft, hostWorkdir })}
            testId="where-to-run-host-workdir"
            value={draft.hostWorkdir ?? ""}
          />
        </>
      ) : null}

      {isProviderMode && selectedBackendProvider ? (
        <div className="space-y-4">
          <div className="flex gap-3 rounded-2xl border border-warning/30 bg-warning-bg px-4 py-3">
            <AlertTriangle className="mt-0.5 h-4 w-4 shrink-0 text-warning" />
            <p className="text-sm text-warning">
              This provider at{" "}
              <span className="font-mono font-medium">
                {selectedBackendProvider.binaryPath}
              </span>{" "}
              will receive your agent&apos;s private key. Only use providers
              from trusted sources.
            </p>
          </div>
          {probeError ? (
            <p className="rounded-2xl border border-destructive/30 bg-destructive/10 px-4 py-3 text-sm text-destructive">
              Could not probe provider: {probeError}
            </p>
          ) : null}
          {draft.probedProvider?.config_schema ? (
            <ProviderConfigFields
              config={draft.providerConfig}
              onChange={(providerConfig) =>
                onDraftChange({ ...draft, providerConfig })
              }
              schema={draft.probedProvider.config_schema}
            />
          ) : null}
        </div>
      ) : null}
    </div>
  );
}
