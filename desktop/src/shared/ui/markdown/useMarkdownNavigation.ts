import * as React from "react";

import { useAppNavigation } from "@/app/navigation/useAppNavigation";
import type { ParsedMessageLink } from "@/features/messages/lib/messageLink";

/** Stable navigation callbacks the Markdown runtime hands to its chips. */
export function useMarkdownNavigation() {
  const { goChannel, goAgents } = useAppNavigation();
  const onOpenChannel = React.useCallback(
    (channelId: string) => {
      void goChannel(channelId);
    },
    [goChannel],
  );
  const onOpenMessageLink = React.useCallback(
    (link: ParsedMessageLink) => {
      // Always route through `goChannel` with `messageId` set: the navigation
      // boundary guards every message-targeting caller before URL mutation.
      // `useAnchoredScroll` + `getEventById` backfill, and works for
      void goChannel(link.channelId, {
        messageId: link.messageId,
        threadRootId: link.threadRootId,
      });
    },
    [goChannel],
  );
  return { goAgents, onOpenChannel, onOpenMessageLink };
}
