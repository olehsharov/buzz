import 'package:buzz/features/channels/channel_messages_provider.dart';
import 'package:buzz/features/channels/channel_timeline_provider.dart';
import 'package:buzz/features/channels/channel_window.dart';
import 'package:buzz/features/profile/profile_provider.dart';
import 'package:buzz/shared/profile/user_profile.dart';
import 'package:buzz/shared/relay/relay.dart';
import 'package:flutter/foundation.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:hooks_riverpod/hooks_riverpod.dart';

const _channelId = 'channel-1';

NostrEvent _message(String id, int createdAt, {String content = 'hello'}) =>
    NostrEvent(
      id: id,
      pubkey: 'alice',
      createdAt: createdAt,
      kind: EventKind.streamMessage,
      tags: const [
        ['h', _channelId],
      ],
      content: content,
      sig: '',
    );

NostrEvent _reaction(String id, String targetId) => NostrEvent(
  id: id,
  pubkey: 'bob',
  createdAt: 5000,
  kind: EventKind.reaction,
  tags: [
    ['h', _channelId],
    ['e', targetId],
  ],
  content: '👍',
  sig: '',
);

void main() {
  late _MessagesNotifier messages;
  late ProviderContainer container;

  setUp(() {
    messages = _MessagesNotifier([_message('a', 1000), _message('b', 1100)]);
    container = ProviderContainer(
      overrides: [
        channelMessagesProvider(_channelId).overrideWith(() => messages),
        profileProvider.overrideWith(_ProfileNotifier.new),
      ],
    );
    addTearDown(container.dispose);
  });

  ChannelTimeline read() => container.read(channelTimelineProvider(_channelId));

  test('unchanged messages keep their instances across list changes', () {
    final notifications = <ChannelTimeline>[];
    container.listen(
      channelTimelineProvider(_channelId),
      (_, next) => notifications.add(next),
    );
    final initial = read();
    expect(initial.messages.map((m) => m.id), ['a', 'b']);

    // Same events in a fresh list: nothing to rebuild, nothing to notify.
    messages.set([_message('a', 1000), _message('b', 1100)]);
    expect(identical(read(), initial), isTrue);
    expect(notifications, isEmpty);

    // A reaction on b replaces b only; a and its entry stay identical.
    messages.set([
      _message('a', 1000),
      _message('b', 1100),
      _reaction('r', 'b'),
    ]);
    final reacted = read();
    expect(notifications, hasLength(1));
    expect(identical(reacted.messages[0], initial.messages[0]), isTrue);
    expect(identical(reacted.entries[0], initial.entries[0]), isTrue);
    expect(identical(reacted.messages[1], initial.messages[1]), isFalse);
    expect(reacted.messages[1].reactions.single.emoji, '👍');

    // An edit-free older page keeps every loaded instance.
    messages.set([
      _message('z', 900),
      _message('a', 1000),
      _message('b', 1100),
      _reaction('r', 'b'),
    ]);
    final paged = read();
    expect(identical(paged.messages[1], reacted.messages[0]), isTrue);
    expect(identical(paged.messages[2], reacted.messages[1]), isTrue);
  });

  test('content changes always produce a new instance', () {
    final initial = read();
    messages.set([
      _message('a', 1000, content: 'edited in place'),
      _message('b', 1100),
    ]);
    final next = read();
    expect(identical(next.messages[0], initial.messages[0]), isFalse);
    expect(next.messages[0].content, 'edited in place');
    expect(identical(next.messages[1], initial.messages[1]), isTrue);
  });
}

class _MessagesNotifier extends ChannelMessagesNotifier {
  _MessagesNotifier(this._events) : super(_channelId);

  List<NostrEvent> _events;

  @override
  AsyncValue<List<NostrEvent>> build() => AsyncData(_events);

  @override
  Map<String, ChannelWindowThreadSummary> get threadSummaries => const {};

  void set(List<NostrEvent> events) {
    _events = events;
    state = AsyncData(events);
  }
}

class _ProfileNotifier extends ProfileNotifier {
  @override
  Future<UserProfile?> build() =>
      SynchronousFuture(const UserProfile(pubkey: 'self'));
}
