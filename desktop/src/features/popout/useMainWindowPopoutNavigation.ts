import { listen } from "@tauri-apps/api/event";
import { getCurrentWebviewWindow } from "@tauri-apps/api/webviewWindow";
import * as React from "react";

import { useAppNavigation } from "@/app/navigation/useAppNavigation";
import { POPOUT_NAVIGATE_MAIN_EVENT } from "@/features/popout/popoutApi";
import { parsePopoutRoute } from "@/features/popout/popoutRoute";

/**
 * Main window only: follows "Open in main window" requests from pop-outs.
 * The native side has already focused this window; the route is validated
 * (MVP destinations only) and committed through the normal navigation choke
 * point. Anything else ("/" from a paused pop-out) is a focus-only request.
 */
export function useMainWindowPopoutNavigation(
  enabled: boolean,
  /**
   * A community window follows its own pop-outs. It listens on its own window
   * target only: a global listener would also follow navigations the native
   * side addresses to the main window.
   */
  windowScoped = false,
): void {
  const { goChannel, goForumPost, goProfile } = useAppNavigation();

  const navigateTo = React.useEffectEvent((route: unknown) => {
    const destination = parsePopoutRoute(route);
    if (!destination) return;
    switch (destination.kind) {
      case "channel":
        void goChannel(destination.channelId, {
          messageId: destination.messageId ?? undefined,
          threadRootId: destination.threadRootId,
        });
        return;
      case "thread":
        void goChannel(destination.channelId, {
          thread: destination.threadRootId,
        });
        return;
      case "forum-post":
        void goForumPost(destination.channelId, destination.postId, {
          replyId: destination.replyId ?? undefined,
        });
        return;
      case "profile":
        void goProfile(destination.pubkey);
        return;
    }
  });

  React.useEffect(() => {
    if (!enabled) return;
    let cancelled = false;
    let unlisten: (() => void) | null = null;
    const subscribe = windowScoped
      ? getCurrentWebviewWindow().listen.bind(getCurrentWebviewWindow())
      : listen;
    void subscribe<{ route?: unknown }>(POPOUT_NAVIGATE_MAIN_EVENT, (event) => {
      if (!cancelled) navigateTo(event.payload?.route);
    })
      .then((cleanup) => {
        if (cancelled) cleanup();
        else unlisten = cleanup;
      })
      .catch((error: unknown) => {
        console.error("Failed to listen for pop-out navigation:", error);
      });
    return () => {
      cancelled = true;
      unlisten?.();
    };
  }, [enabled, windowScoped]);
}
