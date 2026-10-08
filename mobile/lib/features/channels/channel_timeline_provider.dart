import 'package:flutter/foundation.dart';
import 'package:hooks_riverpod/hooks_riverpod.dart';

import '../profile/profile_provider.dart';
import 'channel_messages_provider.dart';
import 'timeline_message.dart';

/// A channel's formatted main timeline.
///
/// Messages and entries keep their previous instances whenever their content
/// is unchanged, so widgets keyed on them can skip rebuilding rows that a
/// new message, reaction, or summary update did not touch.
@immutable
class ChannelTimeline {
  const ChannelTimeline({required this.messages, required this.entries});

  /// Every visible message, including thread replies, in event order.
  final List<TimelineMessage> messages;

  /// Root messages (and broadcast replies) with their thread summaries.
  final List<MainTimelineEntry> entries;

  static const empty = ChannelTimeline(messages: [], entries: []);
}

/// The formatted timeline for one channel, derived from its loaded events.
///
/// Formatting runs once per message-list change instead of on every rebuild
/// of the channel page.
final channelTimelineProvider = NotifierProvider.autoDispose
    .family<ChannelTimelineNotifier, ChannelTimeline, String>(
      ChannelTimelineNotifier.new,
    );

class ChannelTimelineNotifier extends Notifier<ChannelTimeline> {
  ChannelTimelineNotifier(this.channelId);

  final String channelId;

  // The last exposed timeline. Riverpod keeps one notifier per element across
  // rebuilds but does not expose the prior state inside build(), so it is
  // held here to reuse unchanged message instances.
  ChannelTimeline? _previous;

  @override
  ChannelTimeline build() {
    final events = ref.watch(
      channelMessagesProvider(channelId).select((state) => state.value),
    );
    final currentPubkey = ref.watch(
      profileProvider.select((profile) => profile.value?.pubkey),
    );
    if (events == null) return _previous = ChannelTimeline.empty;
    // Summary changes republish the message list, so reading them here stays
    // in step with the events watched above.
    final summaries = ref
        .read(channelMessagesProvider(channelId).notifier)
        .threadSummaries;
    final previous = _previous;
    final messages = stabilizeTimelineMessages(
      previous?.messages ?? const [],
      formatTimeline(events, currentPubkey: currentPubkey),
    );
    final entries = stabilizeTimelineEntries(
      previous?.entries ?? const [],
      buildMainTimelineEntries(messages, relaySummaries: summaries),
    );
    if (previous != null &&
        identical(previous.messages, messages) &&
        identical(previous.entries, entries)) {
      return previous;
    }
    return _previous = ChannelTimeline(messages: messages, entries: entries);
  }
}

/// Returns [next] with each message replaced by its [previous] instance when
/// the content is equal, or [previous] itself when nothing changed.
@visibleForTesting
List<TimelineMessage> stabilizeTimelineMessages(
  List<TimelineMessage> previous,
  List<TimelineMessage> next,
) {
  final previousById = {for (final message in previous) message.id: message};
  var unchanged = previous.length == next.length;
  final result = <TimelineMessage>[];
  for (var index = 0; index < next.length; index++) {
    final candidate = next[index];
    final prior = previousById[candidate.id];
    final stable = prior != null && timelineMessagesEqual(prior, candidate)
        ? prior
        : candidate;
    if (unchanged && !identical(previous[index], stable)) unchanged = false;
    result.add(stable);
  }
  return unchanged ? previous : List.unmodifiable(result);
}

/// Returns [next] with each entry replaced by its [previous] instance when it
/// wraps the same message instance and an equal summary, or [previous]
/// itself when nothing changed.
@visibleForTesting
List<MainTimelineEntry> stabilizeTimelineEntries(
  List<MainTimelineEntry> previous,
  List<MainTimelineEntry> next,
) {
  final previousById = {for (final entry in previous) entry.message.id: entry};
  var unchanged = previous.length == next.length;
  final result = <MainTimelineEntry>[];
  for (var index = 0; index < next.length; index++) {
    final candidate = next[index];
    final prior = previousById[candidate.message.id];
    final stable =
        prior != null &&
            identical(prior.message, candidate.message) &&
            _summariesEqual(prior.summary, candidate.summary)
        ? prior
        : candidate;
    if (unchanged && !identical(previous[index], stable)) unchanged = false;
    result.add(stable);
  }
  return unchanged ? previous : List.unmodifiable(result);
}

/// Deep content equality for everything a timeline row renders.
@visibleForTesting
bool timelineMessagesEqual(TimelineMessage a, TimelineMessage b) {
  if (identical(a, b)) return true;
  return a.id == b.id &&
      a.pubkey == b.pubkey &&
      a.createdAt == b.createdAt &&
      a.content == b.content &&
      a.isSystem == b.isSystem &&
      a.edited == b.edited &&
      a.parentId == b.parentId &&
      a.rootId == b.rootId &&
      _systemEventsEqual(a.systemEvent, b.systemEvent) &&
      listEquals(a.mentionPubkeys, b.mentionPubkeys) &&
      _tagsEqual(a.tags, b.tags) &&
      _reactionsEqual(a.reactions, b.reactions);
}

bool _tagsEqual(List<List<String>> a, List<List<String>> b) {
  if (identical(a, b)) return true;
  if (a.length != b.length) return false;
  for (var index = 0; index < a.length; index++) {
    if (!listEquals(a[index], b[index])) return false;
  }
  return true;
}

bool _systemEventsEqual(SystemEvent? a, SystemEvent? b) {
  if (identical(a, b)) return true;
  if (a == null || b == null) return false;
  return a.type == b.type &&
      a.actorPubkey == b.actorPubkey &&
      a.targetPubkey == b.targetPubkey &&
      a.topic == b.topic &&
      a.purpose == b.purpose &&
      a.ephemeralChannelId == b.ephemeralChannelId;
}

bool _reactionsEqual(List<TimelineReaction> a, List<TimelineReaction> b) {
  if (identical(a, b)) return true;
  if (a.length != b.length) return false;
  for (var index = 0; index < a.length; index++) {
    final x = a[index];
    final y = b[index];
    if (x.emoji != y.emoji ||
        x.count != y.count ||
        x.reactedByCurrentUser != y.reactedByCurrentUser ||
        x.emojiUrl != y.emojiUrl ||
        x.currentUserReactionId != y.currentUserReactionId ||
        !listEquals(x.userPubkeys, y.userPubkeys)) {
      return false;
    }
  }
  return true;
}

bool _summariesEqual(ThreadSummary? a, ThreadSummary? b) {
  if (identical(a, b)) return true;
  if (a == null || b == null) return false;
  return a.threadHeadId == b.threadHeadId &&
      a.replyCount == b.replyCount &&
      a.isLowerBound == b.isLowerBound &&
      a.isCountPending == b.isCountPending &&
      a.lastReplyAt == b.lastReplyAt &&
      listEquals(a.participantPubkeys, b.participantPubkeys);
}
