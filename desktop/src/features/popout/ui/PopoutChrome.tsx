import { AppWindowMac } from "lucide-react";
import * as React from "react";
import { toast } from "sonner";

import { focusMainWindowRoute } from "@/features/popout/popoutApi";
import {
  buildPopoutRoute,
  type PopoutDestination,
} from "@/features/popout/popoutRoute";
import { getPopoutSession } from "@/features/popout/popoutSession";
import {
  usePopoutLocationDestination,
  usePopoutWindowTitle,
} from "@/features/popout/usePopoutWindowTitle";
import type { Channel } from "@/shared/api/types";
import { cn } from "@/shared/lib/cn";
import { isMacPlatform } from "@/shared/lib/platform";
import { useIsFullscreen } from "@/shared/lib/useIsFullscreen";
import { Button } from "@/shared/ui/button";

export const OPEN_IN_MAIN_WINDOW_LABEL = "Open in main window";

/**
 * Hands `destination` (or, failing that, the window's launch route) to the
 * main window. The native side focuses it and emits `popout:navigate-main`.
 * "/" means "just focus": the main window ignores non-destination routes.
 */
export function openInMainWindow(
  destination: PopoutDestination | null,
  focusOnly = false,
) {
  const route = focusOnly
    ? "/"
    : ((destination ? buildPopoutRoute(destination) : null) ??
      getPopoutSession()?.initialRoute ??
      "/");
  void focusMainWindowRoute(route).catch((error: unknown) => {
    console.error("Failed to open in the main window:", error);
    toast.error("Couldn't open the main window");
  });
}

export function OpenInMainWindowButton({
  destination,
  focusOnly = false,
}: {
  destination: PopoutDestination | null;
  focusOnly?: boolean;
}) {
  return (
    <Button
      data-testid="popout-open-in-main-window"
      onClick={() => openInMainWindow(destination, focusOnly)}
      size="sm"
      type="button"
      variant="ghost"
    >
      <AppWindowMac aria-hidden="true" />
      {OPEN_IN_MAIN_WINDOW_LABEL}
    </Button>
  );
}

/**
 * Pop-out window chrome: a draggable title strip that names the destination
 * and offers "Open in main window". Replaces the main window's sidebar,
 * community rail, and history controls.
 */
export function PopoutHeader({
  channels,
  currentPubkey,
}: {
  channels: readonly Channel[];
  currentPubkey: string | undefined;
}) {
  const destination = usePopoutLocationDestination();
  const title = usePopoutWindowTitle(destination, channels, currentPubkey);
  const isFullscreen = useIsFullscreen();
  // Fixed px on purpose: clears the native macOS traffic lights, which do not
  // scale with the app's rem zoom (same exception as AppTopChrome).
  const macChrome = isMacPlatform() && !isFullscreen;

  return (
    <header
      className={cn(
        "relative z-45 flex h-(--buzz-top-chrome-height,40px) shrink-0 cursor-default select-none items-center gap-2 border-b border-border/60 bg-sidebar pr-2 text-sidebar-foreground",
        macChrome ? "pl-[80px]" : "pl-3",
      )}
      data-tauri-drag-region
      data-testid="popout-header"
    >
      <h1
        className="min-w-0 flex-1 truncate text-sm font-semibold"
        data-tauri-drag-region
        data-testid="popout-title"
      >
        {title}
      </h1>
      <OpenInMainWindowButton destination={destination} />
    </header>
  );
}

/**
 * Shown instead of a destination when a pop-out cannot render one: nothing
 * to show (e.g. reloaded without a route), its community was removed, or the
 * main window is on a different community (see popoutCommunityGate).
 */
export function PopoutUnavailableState({
  kind,
  communityName,
}: {
  kind: "empty" | "missing" | "paused";
  communityName?: string | null;
}) {
  const copy = React.useMemo(() => {
    switch (kind) {
      case "paused":
        return {
          title: "This window is paused",
          body: `It shows ${
            communityName ? `the ${communityName} community` : "a community"
          }, but the main window has switched to another community. Switch the main window back to continue here.`,
        };
      case "missing":
        return {
          title: "Community unavailable",
          body: "The community this window belonged to is no longer on this device.",
        };
      default:
        return {
          title: "Nothing to show",
          body: "This window has no conversation open.",
        };
    }
  }, [communityName, kind]);

  return (
    <div
      className="flex min-h-dvh flex-col items-center justify-center gap-3 bg-background px-6 text-center"
      data-popout-state={kind}
      data-testid="popout-unavailable"
      role="status"
    >
      <div
        aria-hidden="true"
        className="fixed inset-x-0 top-0 h-(--buzz-top-chrome-height,40px)"
        data-tauri-drag-region
      />
      <h1 className="text-base font-semibold text-foreground">{copy.title}</h1>
      <p className="max-w-sm text-sm text-muted-foreground">{copy.body}</p>
      {/* Focus only: this window's destination is not valid in the main
          window's current community, so no route is handed over. */}
      <OpenInMainWindowButton destination={null} focusOnly />
    </div>
  );
}
