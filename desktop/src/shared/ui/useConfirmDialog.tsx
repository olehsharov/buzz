import * as React from "react";

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

/** One in-app confirmation. */
export type ConfirmRequest = {
  title: string;
  description: string;
  /** Extra lines shown under the description (e.g. affected items). */
  details?: readonly string[];
  confirmLabel: string;
  cancelLabel?: string;
  destructive?: boolean;
};

/**
 * Ask the user to confirm; resolves `true` only when they press the confirm
 * action. Cancel, Escape, clicking away, a newer request and unmounting all
 * resolve `false`.
 *
 * This replaces `window.confirm`, which the Tauri dialog plugin turns into
 * an async function: `!window.confirm(...)` is then always false, so every
 * guarded action ran without asking.
 */
export type ConfirmFn = (request: ConfirmRequest) => Promise<boolean>;

type Pending = ConfirmRequest & { resolve: (confirmed: boolean) => void };

/** `confirm` plus the dialog element the caller must render. */
export function useConfirmDialog(): {
  confirm: ConfirmFn;
  confirmDialog: React.ReactNode;
} {
  const [pending, setPending] = React.useState<Pending | null>(null);
  const pendingRef = React.useRef<Pending | null>(null);

  const settle = React.useCallback((confirmed: boolean) => {
    const current = pendingRef.current;
    pendingRef.current = null;
    setPending(null);
    current?.resolve(confirmed);
  }, []);

  React.useEffect(() => () => settle(false), [settle]);

  const confirm = React.useCallback<ConfirmFn>(
    (request) =>
      new Promise<boolean>((resolve) => {
        pendingRef.current?.resolve(false);
        const next = { ...request, resolve };
        pendingRef.current = next;
        setPending(next);
      }),
    [],
  );

  const confirmDialog = (
    <AlertDialog
      onOpenChange={(open) => {
        if (!open) settle(false);
      }}
      open={pending !== null}
    >
      {pending ? (
        <AlertDialogContent data-testid="confirm-dialog">
          <AlertDialogHeader>
            <AlertDialogTitle>{pending.title}</AlertDialogTitle>
            {/* A div, so the details list is part of the accessible
                description (aria-describedby) without nesting a list in a
                paragraph. */}
            <AlertDialogDescription asChild>
              <div className="space-y-2">
                <p>{pending.description}</p>
                {pending.details && pending.details.length > 0 ? (
                  <ul className="list-disc space-y-1.5 pl-5">
                    {pending.details.map((detail) => (
                      <li key={detail}>{detail}</li>
                    ))}
                  </ul>
                ) : null}
              </div>
            </AlertDialogDescription>
          </AlertDialogHeader>
          <AlertDialogFooter>
            <AlertDialogCancel asChild>
              <Button
                data-testid="confirm-dialog-cancel"
                type="button"
                variant="outline"
              >
                {pending.cancelLabel ?? "Cancel"}
              </Button>
            </AlertDialogCancel>
            <AlertDialogAction
              className={buttonVariants({
                variant: pending.destructive ? "destructive" : "default",
              })}
              data-testid="confirm-dialog-action"
              onClick={() => settle(true)}
            >
              {pending.confirmLabel}
            </AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      ) : null}
    </AlertDialog>
  );

  return { confirm, confirmDialog };
}
