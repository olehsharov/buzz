import 'mention_bindings.dart';

/// `@all` channel mention contract, shared with desktop
/// (`desktop/src/shared/lib/mentionGroup.ts`) and buzz-cli
/// (`crates/buzz-cli/src/commands/mention_all.rs`):
///
/// - the body carries the literal, lowercase token `@all`;
/// - the event carries one `["p", <hex>]` per recipient plus exactly one
///   marker tag `["buzz:mention-group", "all"]`.
///
/// The marker deliberately does not reuse the `mention` tag name:
/// [mentionedPubkeysFromTags] (and other clients) parse `mention` tag[1] as a
/// pubkey. Renderers show `@all` as a group pill only when the marker is
/// present, so a body that merely contains the text stays plain.
const mentionGroupTag = 'buzz:mention-group';

/// Marker value, and the reserved label of the all-members group mention.
const mentionGroupAll = 'all';

/// The literal token, sigil included, that names the group in a body.
const mentionAllToken = '@$mentionGroupAll';

/// Hard recipient cap. Matches `MENTION_CAP` in
/// `crates/buzz-sdk/src/mentions.rs` and desktop's
/// `MENTION_ALL_RECIPIENT_CAP`: a larger set is rejected, never truncated.
const mentionAllRecipientCap = 50;

/// The `["buzz:mention-group", "all"]` marker tag.
List<String> mentionAllMarkerTag() => [mentionGroupTag, mentionGroupAll];

/// Whether [tag] is exactly the `@all` marker.
bool isMentionAllMarkerTag(List<String> tag) =>
    tag.length == 2 && tag[0] == mentionGroupTag && tag[1] == mentionGroupAll;

/// Whether [tags] carry the `@all` marker.
bool hasMentionAllMarker(Iterable<List<String>> tags) =>
    tags.any(isMentionAllMarkerTag);

/// Whether [text] contains an `@all` token that owns its literal range.
///
/// Uses the member-mention occurrence grammar ([mentionOccurrences]):
/// boundaries and longest-literal ownership, so `@all hands` belongs to a
/// member labelled "all hands" when that label is in [competingLabels]. Inline
/// and fenced code is masked. The token must be exactly lowercase `@all`;
/// `@All` stays plain text.
bool containsMentionAllToken(
  String text, [
  Iterable<String> competingLabels = const [],
]) {
  if (!text.contains(mentionAllToken)) return false;
  final labels = [
    mentionGroupAll,
    for (final label in competingLabels)
      if (label.trim().toLowerCase() != mentionGroupAll) label,
  ];
  final parts = text.split('`');
  for (var i = 0; i < parts.length; i += 2) {
    final segment = parts[i];
    for (final range in mentionOccurrences(segment, labels)) {
      if (range.label == mentionGroupAll &&
          segment.substring(range.start, range.end) == mentionAllToken) {
        return true;
      }
    }
  }
  return false;
}
