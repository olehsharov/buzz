import { useQuery } from "@tanstack/react-query";

import { getHostInstallInfo } from "@/shared/api/agentHosts";
import { CopyButton } from "../ui/CopyButton";
import { HOST_REPAIR_HINT, isHostToolMissingError } from "./hostRunOptions";

/**
 * A deploy error. When the machine refused because a tool is not installed
 * there, the fix (the install snippet) comes with it.
 */
export function HostDeployError({
  message,
  className = "text-xs text-destructive",
  testId,
}: {
  message: string;
  className?: string;
  testId: string;
}) {
  return (
    <div className="space-y-1.5">
      <p className={className} role="alert">
        {message}
      </p>
      {isHostToolMissingError(message) ? (
        <HostRepairNotice
          message={`The machine is missing something agents need — ${HOST_REPAIR_HINT}.`}
          testId={`${testId}-repair`}
        />
      ) : null}
    </div>
  );
}

/**
 * A machine is missing something agents need: say what, and hand over the
 * one line that fixes it (the install snippet without a pairing code, which
 * upgrades a paired machine, installs the tools and restarts its daemon).
 */
export function HostRepairNotice({
  message,
  testId,
}: {
  message: string;
  testId: string;
}) {
  const repair = useQuery({
    queryKey: ["agent-host-install-info", null],
    // Cheap local command; refetched on mount so a community switch never
    // shows another relay's line.
    queryFn: () => getHostInstallInfo(null),
  });
  const command = repair.data?.command ?? null;
  return (
    <div
      className="space-y-1.5 rounded-xl border border-warning/30 bg-warning-bg px-3 py-2 text-xs text-warning"
      data-testid={testId}
    >
      <p>{message}</p>
      {command ? (
        <>
          <p className="text-muted-foreground">
            Run this on the machine; nothing else to answer:
          </p>
          <div className="flex items-start gap-2">
            <code
              className="min-w-0 flex-1 break-all rounded-lg bg-muted px-2 py-1 font-mono text-xs text-foreground"
              data-testid={`${testId}-command`}
            >
              {command}
            </code>
            <CopyButton iconOnly label="Copy install snippet" value={command} />
          </div>
        </>
      ) : null}
    </div>
  );
}
