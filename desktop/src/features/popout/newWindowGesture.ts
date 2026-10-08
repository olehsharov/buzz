import { isMacPlatform } from "@/shared/lib/platform";

type PointerLike = {
  button: number;
  metaKey: boolean;
  ctrlKey: boolean;
};

type KeyLike = {
  key: string;
  metaKey: boolean;
  ctrlKey: boolean;
  isComposing?: boolean;
  nativeEvent?: { isComposing?: boolean };
};

/**
 * True for a pointer gesture that means "open in new window": a primary
 * click with the platform modifier (Cmd on macOS, Ctrl elsewhere) or a
 * middle click. A plain click — and Ctrl-click on macOS, which is a
 * secondary click there — is never a new-window gesture.
 */
export function isNewWindowPointerEvent(
  event: PointerLike,
  mac: boolean = isMacPlatform(),
): boolean {
  if (event.button === 1) return true;
  if (event.button !== 0) return false;
  return mac
    ? event.metaKey && !event.ctrlKey
    : event.ctrlKey && !event.metaKey;
}

/** True for Cmd+Enter (macOS) / Ctrl+Enter (Windows/Linux). */
export function isNewWindowKeyEvent(
  event: KeyLike,
  mac: boolean = isMacPlatform(),
): boolean {
  if (event.key !== "Enter") return false;
  if (event.isComposing || event.nativeEvent?.isComposing) return false;
  return mac
    ? event.metaKey && !event.ctrlKey
    : event.ctrlKey && !event.metaKey;
}
