import * as React from "react";
import { toast } from "sonner";

import { useOptionalCommunities } from "@/features/communities/useCommunities";
import {
  isNewWindowKeyEvent,
  isNewWindowPointerEvent,
} from "@/features/popout/newWindowGesture";
import { openPopoutWindow } from "@/features/popout/popoutApi";
import {
  buildPopoutRoute,
  type PopoutDestination,
} from "@/features/popout/popoutRoute";

/**
 * Returns `openInNewWindow(destination)`: validates the destination (MVP
 * kinds only), builds its route, and asks the native side for a pop-out in
 * this window's community. Returns false when nothing was opened.
 */
export function useOpenInNewWindow(): (
  destination: PopoutDestination,
) => boolean {
  const communityId = useOptionalCommunities()?.activeCommunity?.id ?? null;
  return React.useCallback(
    (destination: PopoutDestination) => {
      const route = buildPopoutRoute(destination);
      if (!route || !communityId) return false;
      void openPopoutWindow(route, { id: communityId }).catch(
        (error: unknown) => {
          console.error("Failed to open a new window:", error);
          toast.error("Couldn't open a new window");
        },
      );
      return true;
    },
    [communityId],
  );
}

type GestureEvent = {
  button: number;
  metaKey: boolean;
  ctrlKey: boolean;
  preventDefault: () => void;
  stopPropagation: () => void;
};

type KeyGestureEvent = {
  key: string;
  metaKey: boolean;
  ctrlKey: boolean;
  nativeEvent?: { isComposing?: boolean };
  preventDefault: () => void;
  stopPropagation: () => void;
};

export type NewWindowGestures = {
  /** Call first in `onClick`; true when the click opened a new window. */
  handleClick: (event: GestureEvent) => boolean;
  /** Call first in `onKeyDown`; true when Cmd/Ctrl+Enter opened a window. */
  handleKeyDown: (event: KeyGestureEvent) => boolean;
  /** Spread as element props: middle click opens a window, no autoscroll. */
  pointerProps: {
    onAuxClick: (event: GestureEvent) => void;
    onMouseDown: (event: GestureEvent) => void;
  };
  /** Open the destination now (context-menu item). */
  open: () => boolean;
  /** Whether this opener has an openable destination. */
  enabled: boolean;
};

/**
 * Input-modality wiring for one "open in new window" opener: Cmd-click
 * (macOS) / Ctrl-click (Windows/Linux), middle click (auxclick, with the
 * autoscroll cursor suppressed), and Cmd/Ctrl+Enter. Plain activation is
 * left to the caller. A null destination disables every gesture.
 */
export function useNewWindowGestures(
  destination: PopoutDestination | null | undefined,
): NewWindowGestures {
  const openInNewWindow = useOpenInNewWindow();
  const route = destination ? buildPopoutRoute(destination) : null;
  // Key on the built route so inline destination objects stay referentially
  // cheap for memoized callers.
  const destinationRef = React.useRef(destination);
  destinationRef.current = destination;

  return React.useMemo(() => {
    const enabled = route !== null;
    const open = () => {
      const current = destinationRef.current;
      return enabled && current ? openInNewWindow(current) : false;
    };
    const consume = (event: {
      preventDefault: () => void;
      stopPropagation: () => void;
    }) => {
      event.preventDefault();
      event.stopPropagation();
    };
    return {
      enabled,
      open,
      handleClick: (event) => {
        if (!enabled || !isNewWindowPointerEvent(event)) return false;
        consume(event);
        open();
        return true;
      },
      handleKeyDown: (event) => {
        if (!enabled || !isNewWindowKeyEvent(event)) return false;
        consume(event);
        open();
        return true;
      },
      pointerProps: {
        onAuxClick: (event) => {
          if (!enabled || event.button !== 1) return;
          consume(event);
          open();
        },
        onMouseDown: (event) => {
          // Middle-button mousedown starts the platform autoscroll cursor.
          if (enabled && event.button === 1) event.preventDefault();
        },
      },
    };
  }, [openInNewWindow, route]);
}
