import {
  currentWindowLabel,
  HUDDLE_WINDOW_LABEL_PREFIX,
  windowKindFromLabel,
} from "@/shared/lib/windowKind";

/** Returns the ephemeral channel id only for a dedicated huddle room window. */
export function huddleWindowChannelId(): string | null {
  const label = currentWindowLabel();
  if (!label || windowKindFromLabel(label) !== "huddle") return null;
  return label.slice(HUDDLE_WINDOW_LABEL_PREFIX.length);
}
