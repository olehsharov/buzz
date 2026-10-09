import type { AgentPersona, ManagedAgent } from "@/shared/api/types";
import { remoteAgentLocation } from "../lib/personaRemoteAgents";
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
import { Button } from "@/shared/ui/button";

type PersonaDeleteDialogProps = {
  open: boolean;
  persona: AgentPersona | null;
  /** Number of managed-agent instances backed by this persona. Omit or pass 0 to suppress the instance-count sentence. */
  instanceCount?: number;
  /** The persona's agents deployed elsewhere; confirming removes them from
   * where they run before the persona is deleted. */
  remoteAgents?: readonly ManagedAgent[];
  onConfirm: (persona: AgentPersona) => void;
  onOpenChange: (open: boolean) => void;
};

/**
 * Confirmation copy for deleting a persona. Pure so the cascade archival
 * disclosure stays unit-testable without a renderer: whenever instances are
 * cascade-deleted, each one's identity is also archived on the relay
 * (NIP-IA), and that durable side effect must be disclosed before the
 * destructive confirm — matching the direct agent-delete dialog.
 */
export function personaDeleteDescription(
  persona: AgentPersona | null,
  instanceCount: number,
): string {
  if (!persona) {
    return "Delete this agent.";
  }
  if (instanceCount === 0) {
    return `Delete ${persona.displayName}.`;
  }
  const cascade =
    instanceCount === 1
      ? "Also deletes 1 agent instance and archives its identity on the relay, so it no longer appears in member lists or mention suggestions."
      : `Also deletes ${instanceCount} agent instances and archives their identities on the relay, so they no longer appear in member lists or mention suggestions.`;
  return `Delete ${persona.displayName}. ${cascade}`;
}

export function PersonaDeleteDialog({
  open,
  persona,
  instanceCount = 0,
  remoteAgents = [],
  onConfirm,
  onOpenChange,
}: PersonaDeleteDialogProps) {
  return (
    <AlertDialog onOpenChange={onOpenChange} open={open}>
      <AlertDialogContent data-testid="persona-delete-dialog">
        <AlertDialogHeader>
          <AlertDialogTitle>Delete agent?</AlertDialogTitle>
          <AlertDialogDescription>
            {personaDeleteDescription(persona, instanceCount)}
          </AlertDialogDescription>
        </AlertDialogHeader>
        {remoteAgents.length > 0 ? (
          <div className="space-y-1.5 text-sm text-muted-foreground">
            <p>These agents run elsewhere and are removed from there first:</p>
            <ul className="list-disc space-y-1 pl-5">
              {remoteAgents.map((agent) => (
                <li key={agent.pubkey}>{remoteAgentLocation(agent)}</li>
              ))}
            </ul>
          </div>
        ) : null}
        <AlertDialogFooter>
          <AlertDialogCancel asChild>
            <Button type="button" variant="outline">
              Cancel
            </Button>
          </AlertDialogCancel>
          <AlertDialogAction asChild>
            <Button
              onClick={() => {
                if (persona) {
                  onConfirm(persona);
                }
              }}
              type="button"
              variant="destructive"
            >
              {remoteAgents.length > 0 ? "Remove and delete" : "Delete"}
            </Button>
          </AlertDialogAction>
        </AlertDialogFooter>
      </AlertDialogContent>
    </AlertDialog>
  );
}
