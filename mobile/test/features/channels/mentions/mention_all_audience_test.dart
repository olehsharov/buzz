import 'package:flutter_test/flutter_test.dart';
import 'package:buzz/features/channels/channel_management_provider.dart';
import 'package:buzz/features/channels/mentions/mention_all_audience.dart';
import 'package:buzz/features/channels/mentions/mention_ranking.dart';
import 'package:buzz/shared/mentions/mention_group.dart';

void main() {
  String key(int n) => n.toRadixString(16).padLeft(64, '0');
  final sender = 'f' * 64;
  ChannelMember member(String pubkey, {String role = 'member'}) =>
      ChannelMember(pubkey: pubkey, role: role, joinedAt: DateTime(2025));

  group('resolveMentionAllAudience', () {
    test('every member but the sender, agents included, lowercased once', () {
      final audience = resolveMentionAllAudience(
        members: [
          member(sender.toUpperCase(), role: 'owner'),
          member('AB${key(1).substring(2)}'),
          member(key(2), role: 'bot'),
          member('ab${key(1).substring(2)}'),
        ],
        currentPubkey: sender,
      );
      expect(audience, isA<MentionAllAvailable>());
      expect((audience as MentionAllAvailable).recipients, [
        'ab${key(1).substring(2)}',
        key(2),
      ]);
    });

    test('loading without a roster or a signed-in key', () {
      expect(
        resolveMentionAllAudience(members: null, currentPubkey: sender),
        isA<MentionAllLoading>(),
      );
      expect(
        resolveMentionAllAudience(members: const [], currentPubkey: null),
        isA<MentionAllLoading>(),
      );
    });

    test('empty when the sender is alone', () {
      expect(
        resolveMentionAllAudience(
          members: [member(sender)],
          currentPubkey: sender,
        ),
        isA<MentionAllEmpty>(),
      );
    });

    test('the cap is inclusive and never truncates', () {
      List<ChannelMember> roster(int n) => [
        member(sender),
        for (var i = 1; i <= n; i++) member(key(i)),
      ];
      final atCap = resolveMentionAllAudience(
        members: roster(mentionAllRecipientCap),
        currentPubkey: sender,
      );
      expect(
        (atCap as MentionAllAvailable).recipients,
        hasLength(mentionAllRecipientCap),
      );
      final over = resolveMentionAllAudience(
        members: roster(mentionAllRecipientCap + 1),
        currentPubkey: sender,
      );
      expect((over as MentionAllOverCap).count, mentionAllRecipientCap + 1);
      expect(mentionAllDisabledReason(over), mentionAllOverCapReason);
      expect(
        mentionAllSendResolution(over).error,
        '@all can notify at most 50 members. This channel has 51 besides you.',
      );
      expect(mentionAllSendResolution(over).recipients, isNull);
    });

    test('summaries and send decisions for every state', () {
      expect(
        mentionAllSummary(MentionAllAvailable([key(1)])),
        'Notify 1 member in this channel',
      );
      expect(
        mentionAllSummary(MentionAllAvailable([key(1), key(2)])),
        'Notify 2 members in this channel',
      );
      expect(mentionAllSummary(const MentionAllEmpty()), mentionAllEmptyReason);
      expect(
        mentionAllSendResolution(const MentionAllLoading()).error,
        mentionAllRosterError,
      );
      expect(
        mentionAllSendResolution(const MentionAllEmpty()).error,
        isNotNull,
      );
      expect(mentionAllCombinedCapError(mentionAllRecipientCap), isNull);
      expect(mentionAllCombinedCapError(mentionAllRecipientCap + 1), isNotNull);
    });
  });

  group('mentionSuggestionEntries', () {
    final members = [
      MentionCandidate(pubkey: key(1), displayName: 'Alice', isMember: true),
      MentionCandidate(pubkey: key(2), displayName: 'Bob', isMember: true),
    ];
    final outsider = MentionCandidate(pubkey: key(3), displayName: 'Alba');
    const audience = MentionAllAvailable(['x']);

    int? groupIndex(List<MentionSuggestionEntry> entries) {
      final i = entries.indexWhere((e) => e is GroupMentionSuggestion);
      return i < 0 ? null : i;
    }

    test('a non-empty prefix of "all" ranks it first, any case', () {
      for (final query in ['a', 'al', 'all', 'AL']) {
        final entries = mentionSuggestionEntries(
          candidates: [...members, outsider],
          query: query,
          audience: audience,
          limit: 50,
        );
        expect(groupIndex(entries), 0, reason: query);
      }
    });

    test('a bare @ lists it after members, before non-members', () {
      final entries = mentionSuggestionEntries(
        candidates: [...members, outsider],
        query: '',
        audience: audience,
        limit: 50,
      );
      expect(groupIndex(entries), 2);
      expect(
        groupIndex(
          mentionSuggestionEntries(
            candidates: members,
            query: '',
            audience: audience,
            limit: 50,
          ),
        ),
        2,
      );
    });

    test('other queries and DMs (no audience) hide it', () {
      for (final query in ['b', 'alls', 'l']) {
        expect(
          groupIndex(
            mentionSuggestionEntries(
              candidates: members,
              query: query,
              audience: audience,
              limit: 50,
            ),
          ),
          isNull,
          reason: query,
        );
      }
      expect(
        groupIndex(
          mentionSuggestionEntries(
            candidates: members,
            query: 'a',
            audience: null,
            limit: 50,
          ),
        ),
        isNull,
      );
    });

    test('ranking first keeps it inside the suggestion cap', () {
      final entries = mentionSuggestionEntries(
        candidates: [
          for (var i = 0; i < 60; i++)
            MentionCandidate(pubkey: key(i), isMember: true),
        ],
        query: 'a',
        audience: const MentionAllOverCap(60),
        limit: 50,
      );
      expect(entries, hasLength(50));
      expect(entries.first, isA<GroupMentionSuggestion>());
      expect((entries.first as GroupMentionSuggestion).isDisabled, isTrue);
    });
  });

  group('mention group token', () {
    test('marker helpers match the wire contract exactly', () {
      expect(mentionAllMarkerTag(), ['buzz:mention-group', 'all']);
      expect(hasMentionAllMarker([mentionAllMarkerTag()]), isTrue);
      expect(
        hasMentionAllMarker([
          ['mention', 'all'],
          ['buzz:mention-group', 'all', 'extra'],
          ['buzz:mention-group', 'ALL'],
        ]),
        isFalse,
      );
    });

    test('only an owned, exact lowercase token outside code counts', () {
      expect(containsMentionAllToken('@all ship it'), isTrue);
      expect(containsMentionAllToken('ship it, @all.'), isTrue);
      expect(containsMentionAllToken('**@all**'), isTrue);
      expect(containsMentionAllToken('@All ship it'), isFalse);
      expect(containsMentionAllToken('@allison'), isFalse);
      expect(containsMentionAllToken('mail@all'), isFalse);
      expect(containsMentionAllToken('`@all`'), isFalse);
      expect(containsMentionAllToken('```\n@all\n```'), isFalse);
      expect(containsMentionAllToken('`x` @all'), isTrue);
      expect(containsMentionAllToken('@all hands', ['all hands']), isFalse);
      expect(containsMentionAllToken('@all hands', ['All']), isTrue);
    });
  });
}
