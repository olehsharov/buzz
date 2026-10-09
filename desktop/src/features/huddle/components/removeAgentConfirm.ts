import type { ConfirmRequest } from "@/shared/ui/useConfirmDialog";

/** Confirmation shown before removing an agent from a huddle. */
export const REMOVE_AGENT_FROM_HUDDLE: ConfirmRequest = {
  title: "Remove this agent from the huddle?",
  description: "The agent stops listening and speaking in this huddle.",
  confirmLabel: "Remove agent",
  destructive: true,
};
