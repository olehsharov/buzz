import '../../../shared/mentions/mention_group.dart';
import '../channel_management_provider.dart';
import 'mention_ranking.dart';

/// Who `@all` would notify in the current channel, or why it cannot.
///
/// Mirrors desktop's `MentionAllAudience`
/// (`desktop/src/features/messages/lib/mentionAllAudience.ts`).
sealed class MentionAllAudience {
  const MentionAllAudience();
}

/// The roster (or the signed-in key) is not known yet.
final class MentionAllLoading extends MentionAllAudience {
  const MentionAllLoading();
}

/// The sender is alone in the channel.
final class MentionAllEmpty extends MentionAllAudience {
  const MentionAllEmpty();
}

/// More recipients than [mentionAllRecipientCap]; never truncated.
final class MentionAllOverCap extends MentionAllAudience {
  final int count;
  const MentionAllOverCap(this.count);
}

/// The lowercase, deduplicated recipients, in roster order.
final class MentionAllAvailable extends MentionAllAudience {
  final List<String> recipients;
  const MentionAllAvailable(this.recipients);
}

const mentionAllOverCapReason =
    '@all can notify at most $mentionAllRecipientCap members';
const mentionAllEmptyReason = 'No one else in this channel to notify';
const mentionAllLoadingReason = 'Loading channel members…';
const mentionAllRosterError =
    "Couldn't load this channel's members for @all. Try again.";
const mentionAllAmbiguousError =
    'The mention @all is ambiguous: a member is named “all”. '
    'Choose a recipient from the mention picker.';

/// Resolve `@all` to every channel member except the sender — people and
/// agents alike.
///
/// Mobile has no per-member agent mention eligibility: every channel member,
/// agents included, is a first-class mention candidate here. So this matches
/// buzz-cli's expansion (each agent harness ignores senders it would not
/// answer), not desktop's admitted-agent filter.
MentionAllAudience resolveMentionAllAudience({
  required List<ChannelMember>? members,
  required String? currentPubkey,
}) {
  if (members == null || currentPubkey == null) {
    return const MentionAllLoading();
  }
  final sender = currentPubkey.toLowerCase();
  final recipients = <String>{
    for (final member in members)
      if (member.pubkey.trim().isNotEmpty &&
          member.pubkey.toLowerCase() != sender)
        member.pubkey.toLowerCase(),
  }.toList();
  if (recipients.isEmpty) return const MentionAllEmpty();
  if (recipients.length > mentionAllRecipientCap) {
    return MentionAllOverCap(recipients.length);
  }
  return MentionAllAvailable(recipients);
}

/// Why `@all` cannot be picked, or null when it can.
String? mentionAllDisabledReason(MentionAllAudience audience) =>
    switch (audience) {
      MentionAllAvailable() => null,
      MentionAllOverCap() => mentionAllOverCapReason,
      MentionAllEmpty() => mentionAllEmptyReason,
      MentionAllLoading() => mentionAllLoadingReason,
    };

/// The picker summary: the disabled reason, or how many it would notify.
String mentionAllSummary(MentionAllAudience audience) {
  if (mentionAllDisabledReason(audience) case final reason?) return reason;
  final count = (audience as MentionAllAvailable).recipients.length;
  return 'Notify $count ${count == 1 ? 'member' : 'members'} in this channel';
}

/// The send-time decision for a fresh [audience]: its recipients, or the
/// visible message that blocks the send (the caller keeps the draft).
({List<String>? recipients, String? error}) mentionAllSendResolution(
  MentionAllAudience audience,
) => switch (audience) {
  MentionAllAvailable(:final recipients) => (
    recipients: recipients,
    error: null,
  ),
  MentionAllOverCap(:final count) => (
    recipients: null,
    error: '$mentionAllOverCapReason. This channel has $count besides you.',
  ),
  MentionAllEmpty() => (recipients: null, error: '$mentionAllEmptyReason.'),
  MentionAllLoading() => (recipients: null, error: mentionAllRosterError),
};

/// A send that adds `@all` must still fit the cap once explicit mentions are
/// merged in; null when [recipientCount] fits.
String? mentionAllCombinedCapError(int recipientCount) =>
    recipientCount > mentionAllRecipientCap
    ? '@all plus the other mentions would notify $recipientCount recipients; '
          'the limit is $mentionAllRecipientCap.'
    : null;

/// One row of the mention picker: a member/identity, or the `@all` group.
sealed class MentionSuggestionEntry {
  const MentionSuggestionEntry();
}

final class IdentityMentionSuggestion extends MentionSuggestionEntry {
  final MentionCandidate candidate;
  const IdentityMentionSuggestion(this.candidate);
}

final class GroupMentionSuggestion extends MentionSuggestionEntry {
  final MentionAllAudience audience;
  const GroupMentionSuggestion(this.audience);

  /// Shown for explanation only; selecting it never inserts anything.
  bool get isDisabled => mentionAllDisabledReason(audience) != null;
}

/// Merge the `@all` entry into ranked identity [candidates], then cap at
/// [limit]. Pass a null [audience] where `@all` is not offered (DMs).
///
/// Mirrors desktop's group rank: a non-empty prefix of "all" (`a`, `al`,
/// `all`, any case) puts it first, so a large roster cannot push it past the
/// cap; a bare `@` lists it after channel members (before non-members), never
/// as the default; any other query hides it.
List<MentionSuggestionEntry> mentionSuggestionEntries({
  required List<MentionCandidate> candidates,
  required String query,
  required MentionAllAudience? audience,
  required int limit,
}) {
  final entries = <MentionSuggestionEntry>[
    for (final candidate in candidates) IdentityMentionSuggestion(candidate),
  ];
  if (audience != null) {
    final lower = query.toLowerCase();
    final group = GroupMentionSuggestion(audience);
    if (lower.isNotEmpty && mentionGroupAll.startsWith(lower)) {
      entries.insert(0, group);
    } else if (lower.isEmpty) {
      final membersEnd = candidates.indexWhere((c) => !c.isMember);
      entries.insert(membersEnd < 0 ? entries.length : membersEnd, group);
    }
  }
  return entries.take(limit).toList();
}
