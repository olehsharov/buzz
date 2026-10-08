import { isMacPlatform } from "@/shared/lib/platform";

import { microphoneErrorName } from "./microphone";

export type HuddleAction = "join" | "start";

const HUDDLE_AUDIO_UNAVAILABLE_MESSAGE =
  "Huddle audio isn’t available on this server. Ask an administrator to turn it on.";

const MIC_PERMISSION_DENIED_MAC_MESSAGE =
  "Buzz can’t use your microphone. Turn it on in System Settings → Privacy & Security → Microphone, then try again.";
const MIC_PERMISSION_DENIED_OTHER_MESSAGE =
  "Buzz can’t use your microphone. Allow microphone access for Buzz in your system settings, then try again.";
const MIC_NOT_FOUND_MESSAGE =
  "No microphone found. Connect a microphone, then try again.";
const MIC_UNREADABLE_MESSAGE =
  "Your microphone is in use by another app or unavailable. Close other apps using it, then try again.";

/** Actionable copy for a `getUserMedia` failure, or null if it is not one. */
function microphoneErrorMessage(error: unknown, isMac: boolean): string | null {
  switch (microphoneErrorName(error)) {
    case "NotAllowedError":
    case "SecurityError":
      return isMac
        ? MIC_PERMISSION_DENIED_MAC_MESSAGE
        : MIC_PERMISSION_DENIED_OTHER_MESSAGE;
    case "NotFoundError":
    case "OverconstrainedError":
      return MIC_NOT_FOUND_MESSAGE;
    case "NotReadableError":
      return MIC_UNREADABLE_MESSAGE;
    default:
      return null;
  }
}

function rawErrorMessage(error: unknown): string | null {
  if (error instanceof Error) {
    return error.message;
  }
  if (typeof error === "string") {
    return error;
  }
  return null;
}

/**
 * User-facing copy for a failed huddle start/join. `isMac` selects the
 * platform-specific microphone-permission instructions.
 */
export function formatHuddleActionError(
  error: unknown,
  action: HuddleAction,
  isMac: boolean = isMacPlatform(),
): string {
  const microphoneMessage = microphoneErrorMessage(error, isMac);
  if (microphoneMessage) {
    return microphoneMessage;
  }

  const message = rawErrorMessage(error)?.trim();
  const normalized = message?.toLowerCase();

  if (
    normalized?.includes("huddle_audio_unavailable") ||
    normalized?.includes("huddle audio unavailable in this deployment")
  ) {
    return HUDDLE_AUDIO_UNAVAILABLE_MESSAGE;
  }

  if (message) {
    return message;
  }

  return action === "join"
    ? "Couldn’t join the huddle."
    : "Couldn’t start the huddle.";
}
