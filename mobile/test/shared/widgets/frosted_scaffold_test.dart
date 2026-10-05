import 'package:buzz/features/channels/jump_to_latest_button.dart';
import 'package:buzz/features/channels/sticky_date_header.dart';
import 'package:buzz/shared/theme/theme.dart';
import 'package:buzz/shared/widgets/frosted_app_bar.dart';
import 'package:buzz/shared/widgets/frosted_scaffold.dart';
import 'package:flutter/foundation.dart';
import 'package:flutter/material.dart';
import 'package:flutter/rendering.dart';
import 'package:flutter_test/flutter_test.dart';

Border? _appBarBorder(WidgetTester tester) {
  final container = tester.widget<Container>(
    find.byKey(const ValueKey('frosted-app-bar-background')),
  );
  return (container.decoration as BoxDecoration?)?.border as Border?;
}

void main() {
  testWidgets('utility pages expose the inverted surface hierarchy', (
    tester,
  ) async {
    const pageSurface = Color(0xFFEEEEEE);
    const containerSurface = Color(0xFFFFFFFF);

    await tester.pumpWidget(
      MaterialApp(
        theme: AppTheme.light(
          colorScheme: lightColorScheme.copyWith(
            surface: containerSurface,
            surfaceContainerHighest: pageSurface,
          ),
        ),
        home: FrostedScaffold(
          useUtilitySurfaceTheme: true,
          appBar: const FrostedAppBar(title: Text('Settings')),
          body: Builder(
            builder: (context) => Column(
              children: [
                ColoredBox(
                  key: const ValueKey('utility-page-surface'),
                  color: Theme.of(context).colorScheme.surface,
                  child: const SizedBox.square(dimension: 20),
                ),
                ColoredBox(
                  key: const ValueKey('utility-container-surface'),
                  color: Theme.of(context).colorScheme.surfaceContainerHighest,
                  child: const SizedBox.square(dimension: 20),
                ),
              ],
            ),
          ),
        ),
      ),
    );

    expect(
      tester
          .widget<ColoredBox>(
            find.byKey(const ValueKey('utility-page-surface')),
          )
          .color,
      pageSurface,
    );
    expect(
      tester
          .widget<ColoredBox>(
            find.byKey(const ValueKey('utility-container-surface')),
          )
          .color,
      containerSurface,
    );
  });

  testWidgets('page divider appears only after content scrolls under header', (
    tester,
  ) async {
    await tester.pumpWidget(
      MaterialApp(
        theme: AppTheme.light(),
        home: FrostedScaffold(
          appBar: const FrostedAppBar(title: Text('Theme')),
          body: ListView.builder(
            key: const ValueKey('page-scroll-view'),
            padding: const EdgeInsets.only(top: 57),
            itemCount: 40,
            itemBuilder: (_, index) =>
                SizedBox(height: 48, child: Text('Theme option $index')),
          ),
        ),
      ),
    );

    expect(_appBarBorder(tester)?.bottom.color.a, 0);

    await tester.drag(
      find.byKey(const ValueKey('page-scroll-view')),
      const Offset(0, -120),
    );
    await tester.pumpAndSettle();

    final border = _appBarBorder(tester);
    expect(border, isNotNull);
    expect(border!.bottom.color.a, greaterThan(0));

    await tester.drag(
      find.byKey(const ValueKey('page-scroll-view')),
      const Offset(0, 500),
    );
    await tester.pumpAndSettle();

    expect(_appBarBorder(tester)?.bottom.color.a, 0);
  });

  testWidgets('frosted surfaces share one backdrop snapshot per scaffold', (
    tester,
  ) async {
    debugDefaultTargetPlatformOverride = TargetPlatform.android;
    final stickyState = ValueNotifier(
      const StickyDateHeaderState(label: 'Today'),
    );
    try {
      await tester.pumpWidget(
        MaterialApp(
          theme: AppTheme.light(),
          home: FrostedScaffold(
            appBar: const FrostedAppBar(title: Text('general')),
            body: Stack(
              children: [
                Positioned(
                  top: 120,
                  left: 0,
                  right: 0,
                  child: StickyDateHeader(state: stickyState),
                ),
                Positioned(
                  bottom: 40,
                  left: 0,
                  right: 0,
                  child: Center(child: JumpToLatestButton(onPressed: () {})),
                ),
              ],
            ),
          ),
        ),
      );

      List<RenderBackdropFilter> filters() => tester
          .renderObjectList<RenderBackdropFilter>(find.byType(BackdropFilter))
          .toList();
      // App bar, sticky date pill, and jump-to-latest button.
      expect(filters(), hasLength(3));
      final keys = filters().map((filter) => filter.backdropKey).toSet();
      expect(keys, hasLength(1));
      expect(keys.single, isNotNull);

      // The group key survives rebuilds; a fresh key per build would make the
      // engine re-snapshot and rebuild every grouped filter.
      final firstKey = keys.single;
      stickyState.value = const StickyDateHeaderState(label: 'Yesterday');
      tester.element(find.byType(FrostedScaffold)).markNeedsBuild();
      await tester.pump();
      expect(filters().map((filter) => filter.backdropKey).toSet(), {firstKey});
    } finally {
      stickyState.dispose();
      debugDefaultTargetPlatformOverride = null;
    }
  });
}
