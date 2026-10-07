import * as React from "react";
import { toast } from "sonner";

import { useOptionalCommunities } from "@/features/communities/useCommunities";
import {
  isNewWindowKeyEvent,
  isNewWindowPointerEvent,
} from "@/features/popout/newWindowGesture";
import { openCommunityWindow } from "@/features/community-window/communityWindowApi";
import { openPopoutWindow } from "@/features/popout/popoutApi";
import {
  buildPopoutRoute,
  type PopoutDestination,
} from "@/features/popout/popoutRoute";
import { isCommunityWindow } from "@/shared/lib/windowKind";

/**
 * Whether this window can open `destination` in a new window. A community
 * window opens no pop-outs: a pop-out runs on the main window's community
 * (see popoutCommunityGate), so one opened from another community would only
 * ever show "paused". Communities open their own window from the main window's
 * rail.
 */
export function canOpenInNewWindow(
  destination: PopoutDestination,
  inCommunityWindow: boolean = isCommunityWindow(),
): boolean {
  if (inCommunityWindow) return false;
  return buildPopoutRoute(destination) !== null;
}

/**
 * Returns `openInNewWindow(destination)`: validates the destination (MVP
 * kinds only), builds its route, and asks the native side for a pop-out in
 * this window's community — or, for a `community` destination, for that
 * community's own window (one per community; an open one is focused).
 * Returns false when nothing was opened.
 */
export function useOpenInNewWindow(): (
  destination: PopoutDestination,
) => boolean {
  const communitiesHook = useOptionalCommunities();
  const communityId = communitiesHook?.activeCommunity?.id ?? null;
  const communities = communitiesHook?.communities;
  return React.useCallback(
    (destination: PopoutDestination) => {
      if (!canOpenInNewWindow(destination)) return false;
      const onError = (error: unknown) => {
        console.error("Failed to open a new window:", error);
        toast.error("Couldn't open a new window");
      };
      if (destination.kind === "community") {
        const target = communities?.find(
          (community) => community.id === destination.communityId,
        );
        // The active community already runs here; a second window for it
        // would make two windows write the same community's read state.
        if (!target || target.id === communityId) return false;
        void openCommunityWindow(target.id, target.name).catch(onError);
        return true;
      }
      const route = buildPopoutRoute(destination);
      if (!route || !communityId) return false;
      void openPopoutWindow(route, { id: communityId }).catch(onError);
      return true;
    },
    [communities, communityId],
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
  const route =
    destination && canOpenInNewWindow(destination)
      ? buildPopoutRoute(destination)
      : null;
  // Key on the built route so inline destination objects stay referentially
  // cheap for memoized callers.
  const destinationRef = React.useRef(destination);
  destinationRef.current = destination;

  return React.useMemo(
    () =>
      createNewWindowGestures(route !== null, () => {
        const current = destinationRef.current;
        return current ? openInNewWindow(current) : false;
      }),
    [openInNewWindow, route],
  );
}

/**
 * The gesture handlers behind [`useNewWindowGestures`], free of React so
 * every input modality can be exercised directly. `enabled: false` makes
 * every handler a no-op that leaves the event to the caller.
 */
export function createNewWindowGestures(
  enabled: boolean,
  openDestination: () => boolean,
  mac?: boolean,
): NewWindowGestures {
  const open = () => (enabled ? openDestination() : false);
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
      if (!enabled || !isNewWindowPointerEvent(event, mac)) return false;
      consume(event);
      open();
      return true;
    },
    handleKeyDown: (event) => {
      if (!enabled || !isNewWindowKeyEvent(event, mac)) return false;
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
}
