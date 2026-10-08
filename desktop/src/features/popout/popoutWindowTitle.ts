import type { PopoutDestination } from "@/features/popout/popoutRoute";

/**
 * Title for a pop-out window: the channel name (`#general`), the DM/person
 * name, or a neutral fallback while names are still loading.
 */
export function popoutWindowTitle({
  destination,
  channelLabel,
  channelIsDm,
  profileName,
}: {
  destination: PopoutDestination | null;
  /** Display label of the destination channel, when resolved. */
  channelLabel?: string | null;
  channelIsDm?: boolean;
  /** Display name of the destination person, when resolved. */
  profileName?: string | null;
}): string {
  if (!destination) return "Buzz";
  if (destination.kind === "profile") return profileName?.trim() || "Profile";
  const label = channelLabel?.trim();
  if (!label) return destination.kind === "thread" ? "Thread" : "Buzz";
  const channelTitle = channelIsDm ? label : `#${label}`;
  return destination.kind === "thread"
    ? `Thread in ${channelTitle}`
    : channelTitle;
}
