part of '../compose_bar_test.dart';

void mentionAllTests() {
  final sender = 'f' * 64;
  ChannelMember member(String pubkey, {String role = 'member', String? name}) =>
      ChannelMember(
        pubkey: pubkey,
        role: role,
        joinedAt: DateTime(2025),
        displayName: name,
      );
  String key(int n) => n.toRadixString(16).padLeft(64, '0');
  List<ChannelMember> roster(int others) => [
    member(sender, role: 'owner'),
    for (var i = 1; i <= others; i++) member(key(i), name: 'Person $i'),
  ];

  Finder groupRow() =>
      find.byKey(const ValueKey('mention-suggestion-group-all'));

  Future<
    List<({String content, List<String> pubkeys, List<List<String>> tags})>
  >
  mount(
    WidgetTester tester, {
    required List<ChannelMember> members,
    String channelType = 'stream',
    List<ChannelMember> Function()? loadMembers,
  }) async {
    final sends =
        <({String content, List<String> pubkeys, List<List<String>> tags})>[];
    await tester.pumpWidget(
      _buildComposeBar(
        uploadService: _testUploadService(nostr.Keys.generate().nsec),
        members: members,
        loadMembers: loadMembers,
        channels: [_makeCurrentChannel(channelType: channelType)],
        currentPubkey: sender,
        onSend: (content, pubkeys, {mediaTags = const <List<String>>[]}) async {
          sends.add((content: content, pubkeys: pubkeys, tags: mediaTags));
        },
      ),
    );
    await _expandComposer(tester);
    return sends;
  }

  Future<void> type(WidgetTester tester, String text) async {
    await tester.enterText(find.byType(TextField), text);
    await tester.pumpAndSettle();
  }

  Future<void> tapSend(WidgetTester tester) async {
    await tester.tap(find.byIcon(LucideIcons.arrowUp));
    await tester.pumpAndSettle();
  }

  String draft(WidgetTester tester) =>
      tester.widget<TextField>(find.byType(TextField)).controller!.text;

  int rowIndex() {
    final tiles = find
        .descendant(
          of: find.byKey(const ValueKey('mention-suggestions-popover')),
          matching: find.byType(ListTile),
        )
        .evaluate()
        .toList();
    final group = find
        .descendant(of: groupRow(), matching: find.byType(ListTile))
        .evaluate()
        .single;
    return tiles.indexOf(group);
  }

  group('@all mention', () {
    testWidgets('is offered first for a prefix of "all" with its audience', (
      tester,
    ) async {
      await mount(
        tester,
        members: [
          ...roster(2),
          member(key(0xa1), name: 'Alma'),
          member(key(0xa2), name: 'all hands bot'),
        ],
      );
      for (final query in ['@a', '@Al', '@all']) {
        await type(tester, query);
        expect(groupRow(), findsOneWidget, reason: query);
        expect(rowIndex(), 0, reason: query);
      }
      expect(find.text('Notify 4 members in this channel'), findsOneWidget);
      await tester.pump(const Duration(milliseconds: 300)); // search debounce
    });

    testWidgets('follows the members on a bare @ and hides otherwise', (
      tester,
    ) async {
      await mount(tester, members: roster(2));
      await type(tester, '@');
      expect(groupRow(), findsOneWidget);
      expect(rowIndex(), 3);
      await type(tester, '@pers');
      expect(groupRow(), findsNothing);
      await tester.pump(const Duration(milliseconds: 300)); // search debounce
    });

    testWidgets('selecting it inserts the literal token, not a member', (
      tester,
    ) async {
      final sends = await mount(tester, members: roster(2));
      await type(tester, 'hey @al');
      await tester.tap(groupRow());
      await tester.pumpAndSettle();
      expect(draft(tester), 'hey @all ');

      await tapSend(tester);
      expect(sends, hasLength(1));
      expect(sends.single.content, 'hey @all');
      expect(sends.single.pubkeys, [key(1), key(2)]);
      expect(sends.single.pubkeys, isNot(contains(sender)));
      expect(sends.single.tags, contains(equals(mentionAllMarkerTag())));
      expect(
        sends.single.tags.where((t) => t.isNotEmpty && t[0] == 'mention'),
        isEmpty,
        reason: 'the marker must never ride a `mention` tag',
      );
    });

    testWidgets('the signed event carries p tags, the marker and the token', (
      tester,
    ) async {
      final signer = nostr.Keys.generate();
      final members = [
        member(signer.public, role: 'owner'),
        member(key(0xab).toUpperCase()),
        member(key(2), role: 'bot'),
        member(key(0xab)),
      ];
      final events = <Map<String, dynamic>>[];
      late SendMessage sendMessage;
      await tester.pumpWidget(
        _buildComposeBar(
          uploadService: _testUploadService(signer.nsec),
          members: members,
          channels: [_makeCurrentChannel()],
          currentPubkey: signer.public,
          relayConfig: () => _SwitchableRelayConfigNotifier(
            RelayConfig(baseUrl: 'https://relay.example', nsec: signer.nsec),
          ),
          onSend: (text, keys, {mediaTags = const []}) => sendMessage(
            channelId: 'channel-1',
            content: text,
            mentionPubkeys: keys,
            mediaTags: mediaTags,
          ),
        ),
      );
      final container = ProviderScope.containerOf(
        tester.element(find.byType(ComposeBar)),
      );
      final session = container.read(relaySessionProvider.notifier);
      session.debugAttachSocketForTest(
        _RecordingRelaySocket(events, session.debugHandleSocketMessageForTest),
      );
      sendMessage = SendMessage(
        signedEventRelay: SignedEventRelay(session: session, nsec: signer.nsec),
        fetchMembers: (_) async => members,
        readUserCache: () => const {},
        addLocalMessage: (_, _) {},
        completeLocalMessage: (_, _) {},
        removeLocalMessage: (_, _) {},
      );
      await _expandComposer(tester);
      await type(tester, '@all standup');
      await tapSend(tester);

      final event = events.singleWhere(
        (e) => e['kind'] == EventKind.streamMessage,
      );
      final tags = [
        for (final tag in event['tags'] as List) List<String>.from(tag as List),
      ];
      expect(event['content'], '@all standup');
      expect(tags.where((t) => t[0] == 'p').toList(), [
        ['p', key(0xab)],
        ['p', key(2)],
      ]);
      expect(tags.where((t) => t[0] == mentionGroupTag).toList(), [
        mentionAllMarkerTag(),
      ]);
      expect(tags.where((t) => t[0] == 'mention'), isEmpty);
    });

    testWidgets('a typed @all notifies agents too and dedupes explicit picks', (
      tester,
    ) async {
      final agent = key(9);
      final sends = await mount(
        tester,
        members: [
          ...roster(1),
          member(agent, role: 'bot', name: 'Helper'),
        ],
      );
      await type(tester, '@Hel');
      await tester.tap(find.text('Helper').last);
      await tester.pumpAndSettle();
      await tester.enterText(find.byType(TextField), '${draft(tester)}@all go');
      await tester.pumpAndSettle();
      await tapSend(tester);
      expect(sends.single.pubkeys, [agent, key(1)]);
      expect(sends.single.tags, contains(equals(mentionAllMarkerTag())));
    });

    testWidgets('a plain message carries no marker', (tester) async {
      final sends = await mount(tester, members: roster(2));
      await type(tester, 'hello `@all` and @All');
      await tapSend(tester);
      expect(sends.single.pubkeys, isEmpty);
      expect(sends.single.tags, isNot(contains(equals(mentionAllMarkerTag()))));
    });

    testWidgets('over the cap it is listed disabled with its reason', (
      tester,
    ) async {
      await mount(tester, members: roster(mentionAllRecipientCap + 1));
      await type(tester, '@all');
      expect(groupRow(), findsOneWidget);
      expect(find.text(mentionAllOverCapReason), findsOneWidget);
      await tester.tap(groupRow());
      await tester.pumpAndSettle();
      expect(draft(tester), '@all', reason: 'a disabled row never inserts');
    });

    testWidgets('exactly the cap is still available', (tester) async {
      await mount(tester, members: roster(mentionAllRecipientCap));
      await type(tester, '@all');
      expect(
        find.text('Notify $mentionAllRecipientCap members in this channel'),
        findsOneWidget,
      );
    });

    testWidgets('one semantics node owns the row label and its state', (
      tester,
    ) async {
      final handle = tester.ensureSemantics();
      await mount(tester, members: roster(mentionAllRecipientCap + 1));
      await type(tester, '@all');
      final label = "Can't mention @all: $mentionAllOverCapReason";
      expect(find.bySemanticsLabel(label), findsOneWidget);
      expect(
        tester.getSemantics(groupRow()),
        matchesSemantics(
          label: label,
          isButton: true,
          hasEnabledState: true,
          isEnabled: false,
        ),
      );
      expect(
        find.bySemanticsLabel(RegExp(r'^@all$|Notify|at most')),
        findsOneWidget,
        reason: 'no duplicate screen-reader stop for the title or subtitle',
      );

      handle.dispose();
    });

    testWidgets('an available row exposes a tap action', (tester) async {
      final handle = tester.ensureSemantics();
      await mount(tester, members: roster(1));
      await type(tester, '@a');
      expect(
        tester.getSemantics(groupRow()),
        matchesSemantics(
          label: 'Mention @all: Notify 1 member in this channel',
          isButton: true,
          hasEnabledState: true,
          isEnabled: true,
          hasTapAction: true,
        ),
      );
      handle.dispose();
    });

    testWidgets('is not offered in DMs and a typed @all stays plain there', (
      tester,
    ) async {
      final sends = await mount(tester, members: roster(1), channelType: 'dm');
      await type(tester, '@a');
      expect(groupRow(), findsNothing);
      await type(tester, '@all hi');
      await tapSend(tester);
      expect(sends.single.tags, isNot(contains(equals(mentionAllMarkerTag()))));
    });

    testWidgets('send re-checks the roster and blocks over cap, keeping it', (
      tester,
    ) async {
      // The first load feeds the picker; any later load is the send-time
      // refresh, which sees a roster that grew past the cap.
      var loads = 0;
      final sends = await mount(
        tester,
        members: const [],
        loadMembers: () =>
            roster(loads++ == 0 ? 2 : mentionAllRecipientCap + 2),
      );
      await type(tester, '@al');
      expect(find.text('Notify 2 members in this channel'), findsOneWidget);
      await tester.tap(groupRow());
      await tester.pumpAndSettle();
      await type(tester, '@all ship it');
      await tapSend(tester);
      expect(sends, isEmpty);
      expect(draft(tester), '@all ship it');
      expect(
        find.text(
          '$mentionAllOverCapReason. This channel has '
          '${mentionAllRecipientCap + 2} besides you.',
        ),
        findsOneWidget,
      );
    });

    testWidgets('a typed @all that names a member is ambiguous', (
      tester,
    ) async {
      final sends = await mount(
        tester,
        members: [
          ...roster(1),
          member(key(7), name: 'all'),
        ],
      );
      await type(tester, '@all hi');
      await tapSend(tester);
      expect(sends, isEmpty);
      expect(draft(tester), '@all hi');
      expect(find.text(mentionAllAmbiguousError), findsOneWidget);
    });
  });
}
