import type { ConfirmRequest } from "@/shared/ui/useConfirmDialog";

/** Confirmation before replacing a lost identity with a new one. */
export const ABANDON_PREVIOUS_IDENTITY: ConfirmRequest = {
  title: "Create a new identity?",
  description:
    "This will create a new identity and abandon your previous key. This cannot be undone.",
  confirmLabel: "Create new identity",
  destructive: true,
};
