import { invokeTauri } from "@/shared/api/tauri";

/**
 * Emitted by the native side to the MAIN window only, after
 * `focus_main_window_route`, carrying `{ route }`.
 */
export const POPOUT_NAVIGATE_MAIN_EVENT = "popout:navigate-main";

/**
 * Identifies the community a pop-out belongs to. Only the stored community id
 * travels through the native side — never relay tokens or keys; the pop-out
 * resolves the rest from the shared `buzz-communities` storage.
 */
export type PopoutCommunityRef = { id: string };

export type PopoutLaunchPayload = {
  route: string;
  community: unknown;
};

/** Opens a pop-out window for `route`; resolves to the new window label. */
export function openPopoutWindow(
  route: string,
  community: PopoutCommunityRef,
): Promise<string> {
  return invokeTauri<string>("open_popout_window", { route, community });
}

/** One-time launch payload for the calling pop-out; null once consumed. */
export function takePopoutLaunch(): Promise<PopoutLaunchPayload | null> {
  return invokeTauri<PopoutLaunchPayload | null>("take_popout_launch");
}

/** Focuses the main window and asks it to navigate to `route`. */
export function focusMainWindowRoute(route: string): Promise<void> {
  return invokeTauri<void>("focus_main_window_route", { route });
}

/** Validates the opaque `community` field of a launch payload. */
export function parsePopoutCommunityRef(
  value: unknown,
): PopoutCommunityRef | null {
  if (!value || typeof value !== "object") return null;
  const id = (value as Record<string, unknown>).id;
  return typeof id === "string" && id.length > 0 && id.length <= 200
    ? { id }
    : null;
}
