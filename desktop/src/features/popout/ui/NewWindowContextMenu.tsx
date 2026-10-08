import type * as React from "react";

import {
  buildPopoutRoute,
  type PopoutDestination,
} from "@/features/popout/popoutRoute";
import { OpenInNewWindowMenuItem } from "@/features/popout/ui/OpenInNewWindowMenuItem";
import {
  ContextMenu,
  ContextMenuContent,
  ContextMenuTrigger,
} from "@/shared/ui/context-menu";

/**
 * Gives an opener that has no context menu of its own a right-click menu with
 * a single "Open in new window" row. The child stays the only focusable /
 * labelled element (the trigger is `asChild`), so no extra screen-reader stop
 * is introduced. Inner menus win over outer ones: Radix skips an ancestor
 * trigger once this one has handled the `contextmenu` event.
 */
export function NewWindowContextMenu({
  children,
  destination,
}: {
  children: React.ReactElement;
  destination: PopoutDestination | null | undefined;
}) {
  if (!destination || buildPopoutRoute(destination) === null) {
    return children;
  }
  return (
    <ContextMenu>
      <ContextMenuTrigger asChild>{children}</ContextMenuTrigger>
      <ContextMenuContent>
        <OpenInNewWindowMenuItem
          destination={destination}
          withIconSlot={false}
        />
      </ContextMenuContent>
    </ContextMenu>
  );
}
