import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:lucide_icons_flutter/lucide_icons.dart';
import 'package:buzz/features/channels/message_content.dart';
import 'package:buzz/shared/mentions/mention_group.dart';
import '../../../helpers/widget_helpers.dart';

void main() {
  final bob = 'b' * 64;
  final namedAll = 'c' * 64;
  Finder pill() => find.byKey(const ValueKey('mention-group-pill'));

  Future<void> render(
    WidgetTester tester, {
    required String content,
    required bool marker,
    Map<String, String> names = const {},
  }) async {
    await tester.pumpWidget(
      WidgetHelpers.testable(
        child: MessageContent(
          content: content,
          mentionNames: names,
          tags: [
            for (final key in names.keys) ['p', key],
            ['p', bob],
            if (marker) mentionAllMarkerTag(),
          ],
        ),
      ),
    );
    await tester.pumpAndSettle();
  }

  testWidgets('the marker renders @all as one group chip, not names', (
    tester,
  ) async {
    final semantics = tester.ensureSemantics();
    await render(
      tester,
      content: '@all standup in 5',
      marker: true,
      names: {bob: 'Bob'},
    );
    expect(pill(), findsOneWidget);
    expect(
      find.descendant(of: pill(), matching: find.byIcon(LucideIcons.users)),
      findsOneWidget,
    );
    expect(find.text('Bob'), findsNothing);
    expect(
      find.bySemanticsLabel('@all, everyone in this channel'),
      findsOneWidget,
    );
    expect(
      find.bySemanticsLabel(RegExp(r'^(@|all|@all)$')),
      findsNothing,
      reason: 'the chip owns one label; its glyphs are not separate stops',
    );
    semantics.dispose();
  });

  testWidgets('without the marker @all renders as it always has', (
    tester,
  ) async {
    await render(tester, content: '@all standup in 5', marker: false);
    expect(pill(), findsNothing);
    expect(find.byIcon(LucideIcons.users), findsNothing);
  });

  testWidgets('the marker does not promote @All or code', (tester) async {
    await render(tester, content: '@All and `@all`', marker: true);
    expect(pill(), findsNothing);
  });

  testWidgets('the marker outranks a member alias named "all"', (tester) async {
    await render(
      tester,
      content: '@all hi',
      marker: true,
      names: {namedAll: 'all'},
    );
    expect(pill(), findsOneWidget);
  });
}
