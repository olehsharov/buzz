/**
 * Errors `getUserMedia` raises when a pinned `deviceId: { exact }` no longer
 * matches an attached input (unplugged headset, renamed device id).
 */
const MISSING_DEVICE_ERROR_NAMES = new Set([
  "NotFoundError",
  "OverconstrainedError",
]);

/** The DOMException-style `name` of a media error, if it has one. */
export function microphoneErrorName(error: unknown): string | null {
  if (typeof error !== "object" || error === null || !("name" in error)) {
    return null;
  }
  return typeof error.name === "string" ? error.name : null;
}

export type AcquiredMicrophone = {
  stream: MediaStream;
  /**
   * True when the selected device was unavailable and the system default
   * microphone was used instead. The caller should forget the selection so
   * the next huddle does not fail the same way.
   */
  fellBackToDefault: boolean;
};

/**
 * Open the microphone for a huddle.
 *
 * With a selected device, requests it exactly. If that device is missing,
 * retries once with the system default. Any other failure (permission denied,
 * device busy) — or the default also failing — propagates unchanged so the
 * caller can report it.
 */
export async function acquireMicrophone(
  getUserMedia: (constraints: MediaStreamConstraints) => Promise<MediaStream>,
  baseConstraints: MediaTrackConstraints,
  selectedDeviceId: string,
): Promise<AcquiredMicrophone> {
  if (!selectedDeviceId) {
    return {
      stream: await getUserMedia({ audio: { ...baseConstraints } }),
      fellBackToDefault: false,
    };
  }

  try {
    const stream = await getUserMedia({
      audio: { ...baseConstraints, deviceId: { exact: selectedDeviceId } },
    });
    return { stream, fellBackToDefault: false };
  } catch (error) {
    if (!MISSING_DEVICE_ERROR_NAMES.has(microphoneErrorName(error) ?? "")) {
      throw error;
    }
    const stream = await getUserMedia({ audio: { ...baseConstraints } });
    return { stream, fellBackToDefault: true };
  }
}
