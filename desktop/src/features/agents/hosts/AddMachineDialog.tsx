import * as React from "react";
import { listen } from "@tauri-apps/api/event";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { LoaderCircle } from "lucide-react";

import { getHostInstallInfo, startHostPairing } from "@/shared/api/agentHosts";
import { cancelPairing, confirmPairingSas } from "@/shared/api/tauri";
import { Button } from "@/shared/ui/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/shared/ui/dialog";
import { CopyButton } from "../ui/CopyButton";
import { hostOsLabel } from "./hostRunOptions";
import { agentHostsQueryKey, useForgetHostMutation } from "./useAgentHosts";

type MachineHello = {
  host_pubkey: string;
  name: string;
  os: string;
  arch: string;
};

type Step =
  | { kind: "starting" }
  | { kind: "waiting"; uri: string; startedAt: number }
  | { kind: "sas"; uri: string; startedAt: number; sas: string }
  | { kind: "approving"; hello: MachineHello | null }
  | { kind: "done"; hello: MachineHello }
  | { kind: "error"; message: string };

function formatSas(sas: string) {
  return sas.length === 6 ? `${sas.slice(0, 3)} ${sas.slice(3)}` : sas;
}

function formatRemaining(secs: number) {
  const s = Math.max(0, Math.ceil(secs));
  return `${Math.floor(s / 60)}:${String(s % 60).padStart(2, "0")}`;
}

/** Seconds left on the pairing code, ticking once a second. */
function useSecondsLeft(startedAt: number | null, ttlSecs: number | null) {
  const [now, setNow] = React.useState(() => Date.now());
  React.useEffect(() => {
    if (startedAt === null) return;
    setNow(Date.now());
    const timer = window.setInterval(() => setNow(Date.now()), 1000);
    return () => window.clearInterval(timer);
  }, [startedAt]);
  if (startedAt === null || ttlSecs === null) return null;
  return Math.max(0, ttlSecs - (now - startedAt) / 1000);
}

/**
 * "Add machine": one command installs `buzz host` from the community relay
 * (`<relay>/host/install.sh`) and pairs the machine once over
 * NIP-AB, confirm the six-digit code, and the machine is approved for this
 * community. Approve confirms the code; the machine's details arrive next
 * and the grant is sent automatically.
 */
export function AddMachineDialog({
  open,
  onOpenChange,
  onAdded,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  onAdded?: (hostPubkey: string) => void;
}) {
  const queryClient = useQueryClient();
  const forget = useForgetHostMutation();
  const [step, setStep] = React.useState<Step>({ kind: "starting" });
  const [attempt, setAttempt] = React.useState(0);
  const stepRef = React.useRef(step);
  stepRef.current = step;
  const helloRef = React.useRef<MachineHello | null>(null);
  const notifyAdded = React.useEffectEvent((hostPubkey: string) => {
    onAdded?.(hostPubkey);
  });
  const pairingUri =
    step.kind === "waiting" || step.kind === "sas" ? step.uri : null;
  const startedAt =
    step.kind === "waiting" || step.kind === "sas" ? step.startedAt : null;
  const installQuery = useQuery({
    queryKey: ["agent-host-install-info", pairingUri],
    queryFn: () => getHostInstallInfo(pairingUri),
    enabled: open && pairingUri !== null,
  });
  const install = pairingUri ? installQuery.data : undefined;
  const secondsLeft = useSecondsLeft(
    startedAt,
    install?.sessionTtlSecs ?? null,
  );
  const expired = secondsLeft !== null && secondsLeft <= 0;

  // biome-ignore lint/correctness/useExhaustiveDependencies: `attempt` restarts the session on retry.
  React.useEffect(() => {
    if (!open) return;
    let cancelled = false;
    const unlisteners: (() => void)[] = [];
    const track = (promise: Promise<() => void>) => {
      promise.then((fn) => {
        if (cancelled) fn();
        else unlisteners.push(fn);
      });
    };
    helloRef.current = null;
    setStep({ kind: "starting" });

    track(
      listen<{ sas: string }>("pairing-sas-received", (event) => {
        if (cancelled) return;
        setStep((current) =>
          current.kind === "waiting"
            ? { ...current, kind: "sas", sas: event.payload.sas }
            : current,
        );
      }),
    );
    track(
      listen<MachineHello>("host-pairing-hello", (event) => {
        if (cancelled) return;
        helloRef.current = event.payload;
        setStep({ kind: "approving", hello: event.payload });
      }),
    );
    track(
      listen<{ host_pubkey?: string }>("pairing-complete", (event) => {
        if (cancelled || !helloRef.current) return;
        if (
          event.payload?.host_pubkey &&
          event.payload.host_pubkey !== helloRef.current.host_pubkey
        ) {
          return;
        }
        const hello = helloRef.current;
        setStep({ kind: "done", hello });
        void queryClient.invalidateQueries({ queryKey: agentHostsQueryKey });
        notifyAdded(hello.host_pubkey);
      }),
    );
    track(
      listen<{ reason: string }>("pairing-aborted", (event) => {
        if (cancelled) return;
        setStep({
          kind: "error",
          message: `The machine stopped pairing (${event.payload.reason}).`,
        });
      }),
    );
    track(
      listen<{ message: string }>("pairing-error", (event) => {
        if (cancelled) return;
        setStep({ kind: "error", message: event.payload.message });
      }),
    );

    startHostPairing().then(
      (uri) => {
        if (!cancelled) {
          setStep({ kind: "waiting", uri, startedAt: Date.now() });
        }
      },
      (error: unknown) => {
        if (!cancelled) {
          setStep({
            kind: "error",
            message: error instanceof Error ? error.message : String(error),
          });
        }
      },
    );

    return () => {
      cancelled = true;
      for (const fn of unlisteners) fn();
      const kind = stepRef.current.kind;
      if (kind !== "done" && kind !== "error") {
        cancelPairing().catch(() => {});
      }
    };
  }, [open, attempt, queryClient]);

  const approve = () => {
    setStep({ kind: "approving", hello: null });
    confirmPairingSas().catch((error: unknown) => {
      setStep({
        kind: "error",
        message: error instanceof Error ? error.message : String(error),
      });
    });
  };

  const decline = () => {
    cancelPairing().catch(() => {});
    onOpenChange(false);
  };

  const forgetWrongMachine = (hello: MachineHello) => {
    forget.mutate(hello.host_pubkey, {
      onSettled: () => onOpenChange(false),
    });
  };

  const newCode = () => setAttempt((value) => value + 1);

  return (
    <Dialog onOpenChange={onOpenChange} open={open}>
      <DialogContent className="max-w-xl" data-testid="add-machine-dialog">
        <DialogHeader>
          <DialogTitle>Add machine</DialogTitle>
          <DialogDescription>
            Run agents on another computer you own. Approve it once; it then
            shows up under &ldquo;Where to run&rdquo;.
          </DialogDescription>
        </DialogHeader>

        {/* One live region owns the step announcements. */}
        <p
          aria-live="polite"
          className="sr-only"
          data-testid="add-machine-status"
        >
          {step.kind === "sas"
            ? `Code ${formatSas(step.sas)} shown. Check it matches the machine, then approve.`
            : step.kind === "done"
              ? `${step.hello.name} added.`
              : step.kind === "error"
                ? step.message
                : ""}
        </p>

        {step.kind === "starting" ||
        step.kind === "waiting" ||
        step.kind === "sas" ? (
          <ol className="space-y-4 text-sm">
            <li className="space-y-2">
              <p className="font-medium">1. On the machine, run</p>
              {install ? (
                <>
                  <div className="flex items-start gap-2">
                    <code
                      className="min-w-0 flex-1 break-all rounded-xl bg-muted px-3 py-2 font-mono text-xs"
                      data-testid="add-machine-install-command"
                    >
                      {install.command}
                    </code>
                    <CopyButton
                      iconOnly
                      label="Copy install command"
                      value={install.command}
                    />
                  </div>
                  <p className="text-xs text-muted-foreground">
                    Linux or macOS. It installs everything agents need (Node,
                    Claude Code), pairs, and starts in the background. Safe to
                    re-run on a machine that already has it.
                  </p>
                </>
              ) : (
                <p className="flex items-center gap-2 text-muted-foreground">
                  <LoaderCircle
                    aria-hidden="true"
                    className="h-4 w-4 animate-spin"
                  />
                  Creating a pairing code…
                </p>
              )}
              {secondsLeft !== null ? (
                <div className="flex items-center gap-2 text-xs text-muted-foreground">
                  <span data-testid="add-machine-expiry">
                    {expired
                      ? "This code has expired."
                      : `Code expires in ${formatRemaining(secondsLeft)}.`}
                  </span>
                  <Button
                    data-testid="add-machine-new-code"
                    onClick={newCode}
                    size="sm"
                    type="button"
                    variant="outline"
                  >
                    New code
                  </Button>
                </div>
              ) : null}
            </li>
            <li className="space-y-2">
              <p className="font-medium">2. Check the code matches</p>
              {step.kind === "sas" ? (
                <p
                  className="font-mono text-2xl font-semibold tracking-widest"
                  data-testid="add-machine-sas"
                >
                  {formatSas(step.sas)}
                </p>
              ) : (
                <p className="text-muted-foreground">
                  Waiting for the machine…
                </p>
              )}
            </li>
          </ol>
        ) : null}

        {step.kind === "approving" ? (
          <p className="flex items-center gap-2 text-sm text-muted-foreground">
            <LoaderCircle aria-hidden="true" className="h-4 w-4 animate-spin" />
            {step.hello
              ? `Approving ${step.hello.name}…`
              : "Waiting for the machine to introduce itself…"}
          </p>
        ) : null}

        {step.kind === "done" ? (
          <div
            className="space-y-1 rounded-2xl border border-border bg-muted/30 px-4 py-3 text-sm"
            data-testid="add-machine-done"
          >
            <p className="font-medium">Machine added</p>
            <p>
              {step.hello.name} · {hostOsLabel(step.hello.os)} ·{" "}
              {step.hello.arch}
            </p>
            <p className="text-xs text-muted-foreground">
              Not the machine you expected? Forget it now.
            </p>
          </div>
        ) : null}

        {step.kind === "error" ? (
          <p
            className="rounded-2xl border border-destructive/30 bg-destructive/10 px-4 py-3 text-sm text-destructive"
            data-testid="add-machine-error"
            role="alert"
          >
            {step.message}
          </p>
        ) : null}

        <DialogFooter>
          {step.kind === "sas" ? (
            <>
              <Button
                data-testid="add-machine-decline"
                onClick={decline}
                type="button"
                variant="outline"
              >
                Decline
              </Button>
              <Button
                data-testid="add-machine-approve"
                onClick={approve}
                type="button"
              >
                Approve
              </Button>
            </>
          ) : step.kind === "done" ? (
            <>
              <Button
                data-testid="add-machine-forget-wrong"
                disabled={forget.isPending}
                onClick={() => forgetWrongMachine(step.hello)}
                type="button"
                variant="outline"
              >
                Forget this machine
              </Button>
              <Button
                data-testid="add-machine-close"
                onClick={() => onOpenChange(false)}
                type="button"
              >
                Done
              </Button>
            </>
          ) : step.kind === "error" ? (
            <>
              <Button
                onClick={() => onOpenChange(false)}
                type="button"
                variant="outline"
              >
                Close
              </Button>
              <Button
                data-testid="add-machine-retry"
                onClick={newCode}
                type="button"
              >
                Try again
              </Button>
            </>
          ) : (
            <Button onClick={decline} type="button" variant="outline">
              Cancel
            </Button>
          )}
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
