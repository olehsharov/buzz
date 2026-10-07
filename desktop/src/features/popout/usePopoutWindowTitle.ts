import { useLocation } from "@tanstack/react-router";
import { isTauri } from "@tauri-apps/api/core";
import { getCurrentWindow } from "@tauri-apps/api/window";
import * as React from "react";

import {
  type PopoutDestination,
  popoutDestinationFromLocation,
} from "@/features/popout/popoutRoute";
import { popoutWindowTitle } from "@/features/popout/popoutWindowTitle";
import {
  useUserProfileQuery,
  useUsersBatchQuery,
} from "@/features/profile/hooks";
import { resolveChannelDisplayLabel } from "@/features/sidebar/lib/channelLabels";
import type { Channel } from "@/shared/api/types";

const EMPTY_PUBKEYS: string[] = [];

/** The pop-out destination the router is currently showing, if any. */
export function usePopoutLocationDestination(): PopoutDestination | null {
  const location = useLocation();
  return React.useMemo(
    () =>
      popoutDestinationFromLocation(
        location.pathname,
        location.search as Record<string, unknown>,
      ),
    [location.pathname, location.search],
  );
}

/**
 * Keeps a pop-out's window title on its destination (channel / person name),
 * updating on in-window navigation. Returns the title for the header.
 */
export function usePopoutWindowTitle(
  destination: PopoutDestination | null,
  channels: readonly Channel[],
  currentPubkey: string | undefined,
): string {
  const channel =
    destination && "channelId" in destination
      ? (channels.find((entry) => entry.id === destination.channelId) ?? null)
      : null;
  const dmPubkeys =
    channel?.channelType === "dm" ? channel.participantPubkeys : EMPTY_PUBKEYS;
  const profiles = useUsersBatchQuery(dmPubkeys, {
    enabled: dmPubkeys.length > 0,
  }).data?.profiles;
  const profilePubkey =
    destination?.kind === "profile" ? destination.pubkey : undefined;
  const profileName = useUserProfileQuery(profilePubkey).data?.displayName;

  const title = popoutWindowTitle({
    destination,
    channelLabel: channel
      ? resolveChannelDisplayLabel(channel, currentPubkey, profiles)
      : null,
    channelIsDm: channel?.channelType === "dm",
    profileName,
  });

  React.useEffect(() => {
    document.title = title;
    if (!isTauri()) return;
    void getCurrentWindow()
      .setTitle(title)
      .catch((error: unknown) => {
        console.warn("Failed to set the pop-out window title:", error);
      });
  }, [title]);

  return title;
}
