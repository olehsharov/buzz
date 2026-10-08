part of '../compose_bar.dart';

/// The picker's `@all` audience, or null where `@all` is not offered.
///
/// Offered only in a known non-DM channel: DMs already notify every
/// participant. Edits never reach the composer (they use the edit sheet), so
/// they never offer or resolve it either.
MentionAllAudience? _pickerMentionAllAudience({
  required Channel? channel,
  required AsyncValue<List<ChannelMember>> membersAsync,
  required SessionStatus sessionStatus,
  required List<ChannelMember> cachedMembers,
  required String? currentPubkey,
}) {
  if (channel == null || channel.isDm) return null;
  final known = membersAsync.hasValue || cachedMembers.isNotEmpty;
  return resolveMentionAllAudience(
    members: known
        ? channelMembersForAutocomplete(
            membersAsync: membersAsync,
            sessionStatus: sessionStatus,
            cachedMembers: cachedMembers,
          )
        : null,
    currentPubkey: currentPubkey,
  );
}

/// Send-time `@all` re-check against a freshly fetched roster.
///
/// Returns null when the draft does not send `@all` (not offered here, no
/// owned token, or a member explicitly picked under the label "all" owns the
/// literal). Otherwise returns the recipients, or the visible error that
/// blocks the send; the caller keeps the draft. Never truncates.
Future<({List<String>? recipients, String? error})?> _resolveMentionAllForSend(
  WidgetRef ref, {
  required String channelId,
  required bool offered,
  required String text,
  required Map<String, MentionCandidate> selected,
  required List<MentionCandidate> members,
  required String? currentPubkey,
}) async {
  if (!offered) return null;
  final competingLabels = [
    ...selected.keys,
    for (final member in members) member.label,
  ];
  if (!containsMentionAllToken(text, competingLabels)) return null;
  bool isAllLabel(String label) =>
      label.trim().toLowerCase() == mentionGroupAll;
  if (selected.keys.any(isAllLabel)) return null;
  // A typed `@all` that also names a member has two meanings: fail visibly.
  if (members.any((member) => isAllLabel(member.label))) {
    return (recipients: null, error: mentionAllAmbiguousError);
  }
  List<ChannelMember>? freshMembers;
  try {
    freshMembers = await ref.refresh(channelMembersProvider(channelId).future);
  } catch (_) {
    freshMembers = null;
  }
  return mentionAllSendResolution(
    resolveMentionAllAudience(
      members: freshMembers,
      currentPubkey: currentPubkey,
    ),
  );
}

/// The `@all` picker row: one semantics node owns its whole label, and a
/// disabled row explains itself without inserting anything.
class _GroupMentionSuggestionTile extends StatelessWidget {
  final GroupMentionSuggestion entry;
  final VoidCallback onSelect;

  const _GroupMentionSuggestionTile({
    required this.entry,
    required this.onSelect,
  });

  @override
  Widget build(BuildContext context) {
    final disabled = entry.isDisabled;
    final summary = mentionAllSummary(entry.audience);
    final colors = context.colors;
    return Semantics(
      key: const ValueKey('mention-suggestion-group-all'),
      container: true,
      button: true,
      enabled: !disabled,
      excludeSemantics: true,
      label:
          '${disabled ? "Can't mention" : 'Mention'} $mentionAllToken: $summary',
      onTap: disabled ? null : () => _runComposerAction(onSelect),
      child: ListTile(
        dense: true,
        visualDensity: VisualDensity.compact,
        enabled: !disabled,
        leading: CircleAvatar(
          radius: 18,
          backgroundColor: colors.primaryContainer,
          child: Icon(
            LucideIcons.users,
            size: 18,
            color: colors.onPrimaryContainer,
          ),
        ),
        title: Text(mentionAllToken, style: context.textTheme.titleSmall),
        subtitle: Text(
          summary,
          style: context.textTheme.labelSmall?.copyWith(
            color: colors.onSurfaceVariant,
          ),
          overflow: TextOverflow.ellipsis,
        ),
        // `enabled: false` is what refuses a pointer tap on a disabled row.
        onTap: () => _runComposerAction(onSelect),
      ),
    );
  }
}
