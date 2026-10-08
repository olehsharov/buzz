import 'dart:async';

import 'package:buzz/shared/read_state/read_state_format.dart';
import 'package:buzz/shared/read_state/read_state_provider.dart';
import 'package:buzz/shared/community/community_provider.dart';
import 'package:buzz/shared/relay/relay.dart';
import 'package:buzz/shared/theme/theme_provider.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:hooks_riverpod/hooks_riverpod.dart';
import 'package:nostr/nostr.dart' as nostr;
import 'package:shared_preferences/shared_preferences.dart';

/// Drives the production [ReadStateNotifier] (real bookkeeping, real
/// [ReadStateManager]) against an inert fake relay session, so a defect in
/// the forced-unread map removal or the manager-emission path fails here
/// instead of being masked by a fake notifier.
void main() {
  const channelId = 'chan-1';
  const otherChannelId = 'chan-2';
  final msgKey = msgContextKey('msg-1');
  final otherMsgKey = msgContextKey('msg-2');

  late ProviderContainer container;

  Future<ReadStateNotifier> pumpNotifier({
    _FakeRelaySession Function() session = _FakeRelaySession.new,
    bool expectReady = true,
  }) async {
    SharedPreferences.setMockInitialValues({});
    final prefs = await SharedPreferences.getInstance();
    final nsec = nostr.Keys.generate().nsec;

    container = ProviderContainer(
      overrides: [
        savedPrefsProvider.overrideWithValue(prefs),
        relayConfigProvider.overrideWith(() => _FakeRelayConfig(nsec)),
        relaySessionProvider.overrideWith(session),
        activeCommunityProvider.overrideWith((ref) async => null),
        appLifecycleProvider.overrideWith(_FakeAppLifecycle.new),
      ],
    );
    addTearDown(container.dispose);

    final notifier = container.read(readStateProvider.notifier);
    // Let the async activeCommunity resolution, the resulting rebuild, and
    // the manager initialize() microtasks run.
    await container.read(activeCommunityProvider.future);
    for (var i = 0; i < 10 && !container.read(readStateProvider).isReady; i++) {
      await Future<void>.delayed(Duration.zero);
    }
    expect(container.read(readStateProvider).isReady, expectReady);
    return notifier;
  }

  ReadStateState state() => container.read(readStateProvider);

  test('message unread → read → unread round-trips through the real '
      'notifier', () async {
    final notifier = await pumpNotifier();

    // Force the message unread.
    notifier.markContextUnread(msgKey, channelId: channelId);
    expect(state().isForcedUnread(msgKey), isTrue);
    expect(state().locallyForcedChannelIds, {channelId});

    // Mark read: the force flag must drop out of the production map AND the
    // marker must land in the manager's effective contexts.
    notifier.markContextRead(msgKey, 1000);
    expect(state().isForcedUnread(msgKey), isFalse);
    expect(state().forcedUnreadContexts, isEmpty);
    expect(state().effectiveTimestamp(msgKey), 1000);

    // Force unread again: read markers are monotonic, so the flag is the
    // only mechanism — it must win even though the marker persists.
    notifier.markContextUnread(msgKey, channelId: channelId);
    expect(state().isForcedUnread(msgKey), isTrue);
    expect(state().effectiveTimestamp(msgKey), 1000);
  });

  test('explicit channel-level Mark read clears forced message entries in '
      'that channel only', () async {
    final notifier = await pumpNotifier();

    notifier.markContextUnread(msgKey, channelId: channelId);
    notifier.markContextUnread(otherMsgKey, channelId: otherChannelId);

    // Channel tile "Mark as read" passes clearForcedMessages: true.
    notifier.markContextRead(channelId, 2000, clearForcedMessages: true);

    expect(state().isForcedUnread(msgKey), isFalse);
    expect(state().isForcedUnread(otherMsgKey), isTrue);
    expect(state().locallyForcedChannelIds, {otherChannelId});
    expect(state().effectiveTimestamp(channelId), 2000);
  });

  test(
    'automatic channel-open read preserves forced message entries',
    () async {
      final notifier = await pumpNotifier();

      notifier.markContextUnread(msgKey, channelId: channelId);

      // Channel open marks the channel read without clearForcedMessages.
      notifier.markContextRead(channelId, 3000);

      expect(state().isForcedUnread(msgKey), isTrue);
      expect(state().locallyForcedChannelIds, {channelId});
      expect(state().effectiveTimestamp(channelId), 3000);
    },
  );

  test('automatic channel-open read still clears a channel-level force for '
      'the same channel', () async {
    final notifier = await pumpNotifier();

    // Channel forced unread from the tile, message forced from the sheet.
    notifier.markContextUnread(channelId, channelId: channelId);
    notifier.markContextUnread(msgKey, channelId: channelId);

    notifier.markContextRead(channelId, 4000);

    // Opening the channel satisfies the channel-scoped force, but the
    // deliberate message-level force survives until acted on.
    expect(state().isForcedUnread(channelId), isFalse);
    expect(state().isForcedUnread(msgKey), isTrue);
    expect(state().locallyForcedChannelIds, {channelId});
  });

  test('manager emissions that change nothing do not notify', () async {
    final notifier = await pumpNotifier();
    var notifications = 0;
    container.listen(readStateProvider, (_, _) => notifications++);

    notifier.seedContextRead(channelId, 100);
    expect(notifications, 1);

    // An older explicit read only promotes the context to publishable: the
    // manager emits onChanged, but every exposed field is unchanged.
    notifier.markContextRead(channelId, 50);
    expect(state().effectiveTimestamp(channelId), 100);
    expect(notifications, 1);

    notifier.markContextUnread(msgKey, channelId: channelId);
    notifier.markContextUnread(msgKey, channelId: channelId);
    expect(notifications, 2);
  });

  test('one manager survives disconnected → connecting → connected', () async {
    final initializations = <String>[];
    final reinitializations = <String>[];
    final originalDebugPrint = debugPrint;
    debugPrint = (String? message, {int? wrapWidth}) {
      if (message == null) return;
      if (message.startsWith('[ReadStateManager] initialize pubkey')) {
        initializations.add(message);
      } else if (message == '[ReadStateManager] reinitializeRemote') {
        reinitializations.add(message);
      }
    };
    addTearDown(() => debugPrint = originalDebugPrint);

    final notifier = await pumpNotifier();
    notifier.markContextUnread(msgKey, channelId: channelId);
    final session =
        container.read(relaySessionProvider.notifier) as _FakeRelaySession;

    session.setStatus(SessionStatus.connecting);
    await Future<void>.delayed(Duration.zero);
    session.setStatus(SessionStatus.connected);
    for (var i = 0; i < 10 && reinitializations.isEmpty; i++) {
      await Future<void>.delayed(Duration.zero);
    }
    await Future<void>.delayed(Duration.zero);

    expect(initializations, hasLength(1));
    // The reconnect refreshes the existing manager instead of replacing it.
    expect(reinitializations, hasLength(1));
    expect(state().isReady, isTrue);
    // Session-local forces live on the notifier; a rebuild would drop them.
    expect(state().isForcedUnread(msgKey), isTrue);

    // Dropping back out of connected keeps the same manager too.
    session.setStatus(SessionStatus.reconnecting);
    await Future<void>.delayed(Duration.zero);
    container.read(readStateProvider);
    await Future<void>.delayed(Duration.zero);
    expect(initializations, hasLength(1));
    expect(state().isReady, isTrue);
  });

  test('a reconnect refresh makes read state ready while the first fetch, '
      'issued before the relay was reachable, is still pending', () async {
    await pumpNotifier(
      session: _OfflineUntilConnectedSession.new,
      expectReady: false,
    );
    final session =
        container.read(relaySessionProvider.notifier) as _FakeRelaySession;

    session.setStatus(SessionStatus.connected);
    for (var i = 0; i < 10 && !state().isReady; i++) {
      await Future<void>.delayed(Duration.zero);
    }
    expect(state().isReady, isTrue);
  });

  test('ReadStateState equality covers every exposed field', () {
    const base = ReadStateState(
      isReady: true,
      pubkey: 'abc',
      contexts: {'a': 1, 'b': 2},
      forcedUnreadContexts: {'msg:x': 'a'},
    );
    final cases = <(String, ReadStateState, bool)>[
      (
        'same values, different map instances and order',
        ReadStateState(
          isReady: true,
          pubkey: 'abc',
          contexts: {'b': 2, 'a': 1},
          forcedUnreadContexts: {'msg:x': 'a'},
        ),
        true,
      ),
      (
        'isReady differs',
        const ReadStateState(
          isReady: false,
          pubkey: 'abc',
          contexts: {'a': 1, 'b': 2},
          forcedUnreadContexts: {'msg:x': 'a'},
        ),
        false,
      ),
      (
        'pubkey differs',
        const ReadStateState(
          isReady: true,
          pubkey: 'def',
          contexts: {'a': 1, 'b': 2},
          forcedUnreadContexts: {'msg:x': 'a'},
        ),
        false,
      ),
      (
        'context timestamp differs',
        const ReadStateState(
          isReady: true,
          pubkey: 'abc',
          contexts: {'a': 1, 'b': 3},
          forcedUnreadContexts: {'msg:x': 'a'},
        ),
        false,
      ),
      (
        'context missing',
        const ReadStateState(
          isReady: true,
          pubkey: 'abc',
          contexts: {'a': 1},
          forcedUnreadContexts: {'msg:x': 'a'},
        ),
        false,
      ),
      (
        'forced context differs',
        const ReadStateState(
          isReady: true,
          pubkey: 'abc',
          contexts: {'a': 1, 'b': 2},
          forcedUnreadContexts: {'msg:y': 'a'},
        ),
        false,
      ),
      (
        'forced channel differs',
        const ReadStateState(
          isReady: true,
          pubkey: 'abc',
          contexts: {'a': 1, 'b': 2},
          forcedUnreadContexts: {'msg:x': 'b'},
        ),
        false,
      ),
      (
        'no forced contexts',
        const ReadStateState(
          isReady: true,
          pubkey: 'abc',
          contexts: {'a': 1, 'b': 2},
        ),
        false,
      ),
    ];
    for (final (label, other, equal) in cases) {
      expect(base == other, equal, reason: label);
      expect(other == base, equal, reason: '$label (symmetric)');
      if (equal) expect(other.hashCode, base.hashCode, reason: label);
    }
    expect(const ReadStateState.inert(), const ReadStateState.inert());
  });
}

class _FakeRelayConfig extends RelayConfigNotifier {
  final String nsec;

  _FakeRelayConfig(this.nsec);

  @override
  RelayConfig build() => RelayConfig(baseUrl: 'http://localhost:1', nsec: nsec);
}

/// Relay session that never connects and returns no history, keeping the
/// manager fully local while exercising its real code paths.
class _FakeRelaySession extends RelaySessionNotifier {
  @override
  SessionState build() =>
      const SessionState(status: SessionStatus.disconnected);

  void setStatus(SessionStatus status) => state = SessionState(status: status);

  @override
  Future<List<NostrEvent>> fetchHistory(
    NostrFilter filter, {
    Duration timeout = const Duration(seconds: 8),
  }) async => [];

  @override
  Future<void Function()> subscribe(
    NostrFilter filter,
    void Function(NostrEvent) onEvent, {
    void Function(String message)? onClosed,
  }) async => () {};
}

/// History requests sent before the relay connects are never answered, the
/// way a REQ written to an absent socket is dropped until its timeout.
class _OfflineUntilConnectedSession extends _FakeRelaySession {
  @override
  Future<List<NostrEvent>> fetchHistory(
    NostrFilter filter, {
    Duration timeout = const Duration(seconds: 8),
  }) {
    if (state.status != SessionStatus.connected) {
      return Completer<List<NostrEvent>>().future;
    }
    return Future.value(const []);
  }
}

class _FakeAppLifecycle extends AppLifecycleNotifier {
  @override
  AppLifecycleState build() => AppLifecycleState.resumed;
}
