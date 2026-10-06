import 'package:hooks_riverpod/hooks_riverpod.dart';

import '../../shared/read_state/read_state_format.dart';
import '../../shared/read_state/read_state_provider.dart';
import 'channels_provider.dart';
import 'unread_badge/observed_unread_event.dart';

/// How one channel's unread state appears in the channel list.
///
/// [hasUnread] is whether observed activity is newer than the channel's read
/// markers (or the channel is forced unread). [hasReadMarker] is whether the
/// channel has a read marker at all; until the list has seeded markers for a
/// community, only channels with one may show as unread.
typedef ChannelListUnread = ({bool hasUnread, bool hasReadMarker});

/// Unread state for a single channel tile.
///
/// Each tile watches its own channel, so a read marker moving in one channel
/// rebuilds that tile instead of the whole channel list.
final channelListUnreadProvider = Provider.autoDispose
    .family<ChannelListUnread, String>((ref, channelId) {
      final readState = ref.watch(readStateProvider);
      // Observed unread events live on the notifier; its state emissions
      // carry their changes.
      ref.watch(channelsProvider);
      return channelListUnread(
        channelId,
        readState: readState,
        channelsNotifier: ref.read(channelsProvider.notifier),
      );
    });

/// Computes [channelListUnreadProvider] for [channelId].
ChannelListUnread channelListUnread(
  String channelId, {
  required ReadStateState readState,
  required ChannelsNotifier channelsNotifier,
}) {
  final channelReadAt = readState.effectiveTimestamp(channelId);
  final hasReadMarker = channelReadAt != null;
  if (!readState.isReady) {
    return (hasUnread: false, hasReadMarker: hasReadMarker);
  }
  if (readState.locallyForcedChannelIds.contains(channelId)) {
    return (hasUnread: true, hasReadMarker: hasReadMarker);
  }

  final latestObserved = channelsNotifier.latestObservedByChannel[channelId];
  if (latestObserved == null ||
      (channelReadAt != null && latestObserved <= channelReadAt)) {
    return (hasUnread: false, hasReadMarker: hasReadMarker);
  }

  int? readAtForObservedEvent(ObservedUnreadEvent event) =>
      observedUnreadEventReadAt(
        event,
        channelReadAt,
        (rootId) => readState.effectiveTimestamp(threadContextKey(rootId)),
        (messageId) => readState.effectiveTimestamp(msgContextKey(messageId)),
      );

  final unreadCount = countUnreadObservedEvents(
    channelsNotifier.observedUnreadEventsByChannel[channelId],
    readAtForObservedEvent,
  );
  return (hasUnread: unreadCount > 0, hasReadMarker: hasReadMarker);
}
