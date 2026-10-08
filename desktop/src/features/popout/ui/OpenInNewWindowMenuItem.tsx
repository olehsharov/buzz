import { AppWindow } from "lucide-react";

import type { PopoutDestination } from "@/features/popout/popoutRoute";
import { useNewWindowGestures } from "@/features/popout/useOpenInNewWindow";
import { ContextMenuItem } from "@/shared/ui/context-menu";

export const OPEN_IN_NEW_WINDOW_LABEL = "Open in new window";

/**
 * Radix context-menu row that opens `destination` in a pop-out. Renders
 * nothing when the destination is not openable, so menus never offer a dead
 * item. The visible text is the row's only accessible name (the icon is
 * decorative), giving screen readers exactly one stop per row.
 */
export function OpenInNewWindowMenuItem({
  destination,
  withIconSlot = true,
}: {
  destination: PopoutDestination | null | undefined;
  /** Match menus whose rows reserve a fixed leading icon column. */
  withIconSlot?: boolean;
}) {
  const gestures = useNewWindowGestures(destination);
  if (!gestures.enabled) return null;
  return (
    <ContextMenuItem
      data-testid="open-in-new-window"
      onSelect={() => {
        gestures.open();
      }}
    >
      {withIconSlot ? (
        <span
          aria-hidden="true"
          className="flex h-4 w-4 shrink-0 items-center justify-center"
        >
          <AppWindow className="h-4 w-4" />
        </span>
      ) : (
        <AppWindow aria-hidden="true" className="h-4 w-4" />
      )}
      <span>{OPEN_IN_NEW_WINDOW_LABEL}</span>
    </ContextMenuItem>
  );
}
