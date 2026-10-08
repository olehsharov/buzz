import 'dart:async';

import 'package:flutter/foundation.dart';
import 'package:flutter/widgets.dart';
import 'package:hooks_riverpod/hooks_riverpod.dart';

import '../relay/relay.dart';
import '../theme/theme_provider.dart';
import '../community/community_provider.dart';
import 'read_state_manager.dart';

/// Read markers exposed to the UI.
///
/// Value-equal so a manager emission that changes nothing (a reconnect
/// refresh, a publishable-flag promotion, a repeated force) does not notify
/// every channel tile, badge, and open conversation.
@immutable
class ReadStateState {
  final bool isReady;
  final String? pubkey;
  final Map<String, int> contexts;

  /// Session-local forced-unread flags, keyed by the forced context id
  /// (a channel id from the channel tile, or a `msg:` key from the message
  /// actions sheet), each mapped to the channel it belongs to so channel
  /// tiles and badges can surface message-level forces.
  final Map<String, String> forcedUnreadContexts;

  const ReadStateState({
    required this.isReady,
    required this.pubkey,
    required this.contexts,
    this.forcedUnreadContexts = const {},
  });

  const ReadStateState.inert()
    : isReady = false,
      pubkey = null,
      contexts = const {},
      forcedUnreadContexts = const {};

  @override
  bool operator ==(Object other) =>
      identical(this, other) ||
      other is ReadStateState &&
          other.isReady == isReady &&
          other.pubkey == pubkey &&
          mapEquals(other.contexts, contexts) &&
          mapEquals(other.forcedUnreadContexts, forcedUnreadContexts);

  // Order-independent so equal maps hash equally whatever their insertion
  // order.
  @override
  int get hashCode => Object.hash(
    isReady,
    pubkey,
    Object.hashAllUnordered(
      contexts.entries.map((entry) => Object.hash(entry.key, entry.value)),
    ),
    Object.hashAllUnordered(
      forcedUnreadContexts.entries.map(
        (entry) => Object.hash(entry.key, entry.value),
      ),
    ),
  );

  /// Channels that should surface as unread because of a forced-unread flag —
  /// either forced directly, or containing a forced message.
  Set<String> get locallyForcedChannelIds =>
      forcedUnreadContexts.values.toSet();

  /// Whether this exact context (channel id or `msg:` key) is forced unread.
  bool isForcedUnread(String contextId) =>
      forcedUnreadContexts.containsKey(contextId);

  int? effectiveTimestamp(String contextId) => contexts[contextId];

  ReadStateState copyWithContext(String contextId, int timestamp) {
    final current = contexts[contextId] ?? 0;
    if (timestamp <= current) {
      return this;
    }

    return ReadStateState(
      isReady: isReady,
      pubkey: pubkey,
      contexts: Map.unmodifiable({...contexts, contextId: timestamp}),
      forcedUnreadContexts: forcedUnreadContexts,
    );
  }
}

class ReadStateNotifier extends Notifier<ReadStateState> {
  ReadStateManager? _manager;
  bool _isInitialized = false;
  final Map<String, String> _forcedUnreadContexts = {};

  @override
  ReadStateState build() {
    _manager?.dispose(flushPending: false);
    _manager = null;
    _isInitialized = false;
    _forcedUnreadContexts.clear();

    // Depend only on the manager's identity inputs. Session *status* changes
    // are handled by the reconnect listener below; rebuilding on them would
    // recreate the manager, drop session-local forced-unread flags, and flip
    // isReady back to false on every connecting/connected/reconnecting
    // transition. The session notifier instance itself lives as long as its
    // provider element, so reading it once is enough.
    final nsec = ref.watch(
      relayConfigProvider.select((config) => config.nsec?.trim()),
    );
    final session = ref.read(relaySessionProvider.notifier);
    final communityPubkey = ref.watch(
      activeCommunityProvider.select(
        (community) => _normalizePubkey(community.value?.pubkey),
      ),
    );

    if (nsec == null || nsec.isEmpty) {
      return const ReadStateState.inert();
    }

    final signedRelay = SignedEventRelay(session: session, nsec: nsec);
    final pubkey = communityPubkey ?? _safeDerivedPubkey(signedRelay);
    if (pubkey == null) {
      return const ReadStateState.inert();
    }

    final crypto = ReadStateCrypto.tryCreate(nsec: nsec, pubkey: pubkey);
    if (crypto == null) {
      return const ReadStateState.inert();
    }

    final prefs = ref.read(savedPrefsProvider);
    late final ReadStateManager manager;
    manager = ReadStateManager(
      pubkey: pubkey,
      prefs: prefs,
      crypto: crypto,
      relaySession: session,
      signedEventRelay: signedRelay,
      remoteEnabled: true,
      onChanged: () => _emitManagerState(manager),
    );
    _manager = manager;

    ref.onDispose(() {
      manager.dispose();
      if (_manager == manager) {
        _manager = null;
      }
    });

    ref.listen(appLifecycleProvider, (_, next) {
      if (next == AppLifecycleState.paused ||
          next == AppLifecycleState.detached ||
          next == AppLifecycleState.hidden) {
        unawaited(manager.flush());
      }
    });

    ref.listen(relaySessionProvider.select((session) => session.status), (
      prev,
      next,
    ) {
      if (prev != SessionStatus.connected && next == SessionStatus.connected) {
        unawaited(_refreshAfterReconnect(manager));
      }
    });

    Future.microtask(() async {
      await manager.initialize();
      if (_manager != manager) return;
      _isInitialized = true;
      _emitManagerState(manager);
    });

    return _stateFromManager(manager, isReady: false);
  }

  /// A refresh that completes while connected is as authoritative as the
  /// first initialize() pass, which may have been issued before the relay
  /// was reachable and still be waiting out its timeouts.
  Future<void> _refreshAfterReconnect(ReadStateManager manager) async {
    final refreshed = await manager.reinitializeRemote();
    if (!refreshed || _manager != manager) return;
    _isInitialized = true;
    _emitManagerState(manager);
  }

  /// Advance a context's read marker. Clears the forced-unread flag for
  /// exactly this context, if any. An explicit channel-level "Mark read"
  /// (channel tile/menu) should pass [clearForcedMessages] so message-level
  /// forces inside the channel are released too; the automatic read on
  /// channel open must not, so a message deliberately marked unread stays
  /// unread until acted on.
  void markContextRead(
    String contextId,
    int unixTimestamp, {
    bool clearForcedMessages = false,
  }) {
    var removed = _forcedUnreadContexts.remove(contextId) != null;
    if (clearForcedMessages) {
      final before = _forcedUnreadContexts.length;
      _forcedUnreadContexts.removeWhere(
        (_, channelId) => channelId == contextId,
      );
      removed = removed || _forcedUnreadContexts.length != before;
    }
    _manager?.markContextRead(contextId, unixTimestamp);
    if (removed) {
      _refreshForcedState();
    }
  }

  /// Force a context unread for the rest of the session. [contextId] is a
  /// channel id (channel-tile action) or a `msg:` key (message actions
  /// sheet); [channelId] is the channel the context belongs to, so tiles and
  /// badges can surface message-level forces. Read markers are monotonic,
  /// so this is the only way to move a context back to unread.
  void markContextUnread(String contextId, {required String channelId}) {
    if (_manager == null) return;
    _forcedUnreadContexts[contextId] = channelId;
    _refreshForcedState();
  }

  void _refreshForcedState() {
    final manager = _manager;
    if (manager == null) return;
    state = _stateFromManager(manager, isReady: _isInitialized);
  }

  void seedContextRead(String contextId, int unixTimestamp) {
    _manager?.seedContextRead(contextId, unixTimestamp);
  }

  void _emitManagerState(ReadStateManager manager) {
    if (_manager != manager) return;
    final advances = manager.drainSyncedAdvances();
    for (final contextId in advances) {
      _forcedUnreadContexts.remove(contextId);
    }
    state = _stateFromManager(manager, isReady: _isInitialized);
  }

  ReadStateState _stateFromManager(
    ReadStateManager manager, {
    required bool isReady,
  }) {
    return ReadStateState(
      isReady: isReady,
      pubkey: manager.pubkey,
      contexts: manager.effectiveContexts,
      forcedUnreadContexts: Map.unmodifiable(
        Map<String, String>.from(_forcedUnreadContexts),
      ),
    );
  }
}

final readStateProvider = NotifierProvider<ReadStateNotifier, ReadStateState>(
  ReadStateNotifier.new,
);

String? _normalizePubkey(String? value) {
  final normalized = value?.trim().toLowerCase();
  if (normalized == null || normalized.isEmpty) {
    return null;
  }
  return normalized;
}

String? _safeDerivedPubkey(SignedEventRelay relay) {
  try {
    return _normalizePubkey(relay.pubkey);
  } catch (e) {
    debugPrint('[ReadStateManager] pubkey derivation failed: $e');
    return null;
  }
}
