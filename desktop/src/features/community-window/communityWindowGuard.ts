import { focusCommunityWindow } from "@/features/community-window/communityWindowApi";
import { isCommunityWindowId } from "@/shared/lib/windowKind";

/**
 * Two-writer guard for main-window community switches.
 *
 * A community runs in at most one window: two windows on the same community
 * would both own its observed-unread store, drafts, and read markers. When
 * the main window is asked to switch to a community that already has its own
 * window, that window is focused instead and the main window stays put.
 *
 * Resolves to true when the switch was handled by focusing the community's
 * window (the caller must then NOT switch). A failed focus resolves to false:
 * the native side reports "no such window" as `false`, so an error means the
 * window is unreachable and the main window may take the community.
 */
export async function focusCommunityWindowInsteadOfSwitch(
  targetCommunityId: string,
  focus: (communityId: string) => Promise<boolean> = focusCommunityWindow,
): Promise<boolean> {
  if (!isCommunityWindowId(targetCommunityId)) return false;
  try {
    return await focus(targetCommunityId);
  } catch (error) {
    console.warn("Could not reach the community's window:", error);
    return false;
  }
}
