import { mentionOccurrences } from "./mentionOccurrences";

/**
 * `@all` channel mention contract (shared with buzz-cli and other clients):
 *
 * - The body carries the literal, lowercase token `@all`.
 * - The event carries one `["p", <hex>]` per human recipient plus exactly one
 *   marker tag `["buzz:mention-group", "all"]`.
 *
 * The marker deliberately does not reuse the `mention` tag name: other clients
 * parse `mention` tag[1] as a pubkey. Renderers show `@all` as a group pill only
 * when the marker is present, so a body that merely contains the text (from a
 * client that never notified anyone) keeps rendering as plain text.
 */
export const MENTION_GROUP_TAG = "buzz:mention-group";
export const MENTION_GROUP_ALL = "all";
/** The literal token, including its sigil, that names the group in a body. */
export const MENTION_ALL_TOKEN = `@${MENTION_GROUP_ALL}`;
/**
 * Hard recipient cap. Matches `MENTION_CAP` in `crates/buzz-sdk/src/mentions.rs`
 * and `MAX_MENTIONS` in the desktop Tauri event builders, which reject (rather
 * than truncate) a larger mention set.
 */
export const MENTION_ALL_RECIPIENT_CAP = 50;

export function mentionAllMarkerTag(): string[] {
  return [MENTION_GROUP_TAG, MENTION_GROUP_ALL];
}

export function isMentionAllMarkerTag(tag: readonly string[]): boolean {
  return (
    tag.length === 2 &&
    tag[0] === MENTION_GROUP_TAG &&
    tag[1] === MENTION_GROUP_ALL
  );
}

export function hasMentionAllMarker(
  tags: readonly (readonly string[])[] | undefined,
): boolean {
  return (tags ?? []).some(isMentionAllMarkerTag);
}

/**
 * Whether `text` contains an `@all` token that owns its literal range.
 *
 * Uses the same occurrence grammar as member mentions: boundaries, Markdown
 * code masking, and longest-literal ownership, so `@all hands` belongs to a
 * member labelled "all hands" when that label competes. The token must be the
 * exact lowercase `@all`; `@All` stays plain text.
 */
export function containsMentionAllToken(
  text: string,
  competingLabels: readonly string[] = [],
): boolean {
  if (!text.includes(MENTION_ALL_TOKEN)) return false;
  const candidates = [
    { displayName: MENTION_GROUP_ALL, isGroup: true },
    ...competingLabels
      .filter(
        (label) =>
          label.trim().toLowerCase() !== MENTION_GROUP_ALL.toLowerCase(),
      )
      .map((displayName) => ({ displayName, isGroup: false })),
  ];
  return mentionOccurrences(text, candidates).some(
    (occurrence) =>
      occurrence.candidates.some((candidate) => candidate.isGroup) &&
      text.slice(occurrence.start, occurrence.end) === MENTION_ALL_TOKEN,
  );
}
