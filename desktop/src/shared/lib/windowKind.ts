import { isTauri } from "@tauri-apps/api/core";
import { getCurrentWindow } from "@tauri-apps/api/window";

/**
 * Which role this webview plays. Every Buzz window loads the same React tree;
 * only the main window owns app-global work (unread tracking, notifications,
 * dock badge, deep links, archive sync, settings).
 *
 * - `main`: the primary app window.
 * - `huddle`: the dedicated huddle companion (`huddle-<channel uuid>`).
 * - `popout`: a "open in new window" view (`popout-<uuid>`) showing one
 *   destination of one community.
 */
export type WindowKind = "main" | "huddle" | "popout";

export const HUDDLE_WINDOW_LABEL_PREFIX = "huddle-";
export const POPOUT_WINDOW_LABEL_PREFIX = "popout-";

const UUID_PATTERN =
  /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i;

/** Pure label classifier. Unknown or malformed labels are the main window. */
export function windowKindFromLabel(label: string | null): WindowKind {
  if (!label) return "main";
  if (
    label.startsWith(HUDDLE_WINDOW_LABEL_PREFIX) &&
    UUID_PATTERN.test(label.slice(HUDDLE_WINDOW_LABEL_PREFIX.length))
  ) {
    return "huddle";
  }
  if (
    label.startsWith(POPOUT_WINDOW_LABEL_PREFIX) &&
    UUID_PATTERN.test(label.slice(POPOUT_WINDOW_LABEL_PREFIX.length))
  ) {
    return "popout";
  }
  return "main";
}

/** The current Tauri window label, or null outside Tauri / without metadata. */
export function currentWindowLabel(): string | null {
  if (!isTauri()) return null;
  try {
    return getCurrentWindow().label;
  } catch {
    // Browser previews can expose the Tauri IPC mock without window metadata.
    return null;
  }
}

/** The role of the current window. A window's label never changes. */
export function currentWindowKind(): WindowKind {
  return windowKindFromLabel(currentWindowLabel());
}

/** True only for the primary window that owns app-global side effects. */
export function isMainWindow(): boolean {
  return currentWindowKind() === "main";
}

/** True for an "open in new window" pop-out. */
export function isPopoutWindow(): boolean {
  return currentWindowKind() === "popout";
}
