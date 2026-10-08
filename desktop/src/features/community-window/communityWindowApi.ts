import { invokeTauri } from "@/shared/api/tauri";

/**
 * Opens `communityId` in its own window (label `community-<id>`), or focuses
 * the window it already has. Resolves to the window label.
 */
export function openCommunityWindow(
  communityId: string,
  title?: string,
): Promise<string> {
  return invokeTauri<string>("open_community_window", {
    communityId,
    title: title ?? null,
  });
}

/**
 * Focuses the community window of `communityId` when one is open. Resolves
 * to whether a window was focused.
 */
export function focusCommunityWindow(communityId: string): Promise<boolean> {
  return invokeTauri<boolean>("focus_community_window", { communityId });
}

/**
 * Binds the calling community window to `relayUrl`. The backend accepts only
 * a relay from this device's saved community list.
 */
export function bindWindowCommunity(relayUrl: string): Promise<void> {
  return invokeTauri<void>("bind_window_community", { relayUrl });
}
