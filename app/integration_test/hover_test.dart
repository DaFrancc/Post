// Hover highlights follow the pointer: `tool/screens.sh hover`. Click
// through the Settings menu, then move over each item in turn; the one lit
// must always be the one under the pointer, never one it left.

import 'package:flutter/gestures.dart';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:integration_test/integration_test.dart';
import 'package:post/main.dart';
import 'package:post/src/rust/frb_generated.dart';
import 'package:post/theme.dart';

import 'harness.dart';

final h = Harness('hover');

const _items = ['Profile', 'Appearance', 'Sounds', 'Spelling', 'Devices', 'Privacy', 'Updates', 'Account'];

/// The menu items drawn lit as hovered.
List<String> _lit(WidgetTester t) => [
  for (final label in _items)
    if (find
        .ancestor(
          of: find.descendant(of: find.byType(Dialog), matching: find.text(label)),
          matching: find.byWidgetPredicate((w) => w is Container && (w.decoration as BoxDecoration?)?.color == PColors.hover),
        )
        .evaluate()
        .isNotEmpty)
      label,
];

void main() {
  final binding = IntegrationTestWidgetsFlutterBinding.ensureInitialized();
  binding.framePolicy = LiveTestWidgetsFlutterBindingFramePolicy.fullyLive;

  setUpAll(() async {
    await RustLib.init();
    await h.start();
  });
  tearDownAll(() => h.stop());

  testWidgets('hover follows the pointer', (t) async {
    await h.register('sam', name: 'Sam');
    await t.pumpWidget(PostApp(key: UniqueKey(), profileDir: '${h.work.path}/sam', keyFile: '${h.work.path}/sam.key'));
    await waitFor(t, find.byTooltip('Settings'));
    await tap(t, find.byTooltip('Settings'));
    await waitFor(t, find.text('Add a picture'));

    final mouse = await t.createGesture(kind: PointerDeviceKind.mouse);
    await mouse.addPointer(location: Offset.zero);
    // The menu's item comes first; the page title can say the same.
    Offset centre(String label) => t.getCenter(find.descendant(of: find.byType(Dialog), matching: find.text(label)).first);

    // Click through them, quickly, as someone exploring would.
    for (final label in ['Sounds', 'Appearance', 'Devices', 'Updates', 'Profile', 'Privacy', 'Spelling', 'Account', 'Sounds']) {
      await mouse.moveTo(centre(label));
      await t.pump(const Duration(milliseconds: 16));
      await mouse.down(centre(label));
      await t.pump(const Duration(milliseconds: 16));
      await mouse.up();
      await t.pump(const Duration(milliseconds: 60));
    }
    await settle(t);

    // Then move over each: only the one under the pointer is lit (the
    // chosen one is drawn as chosen, not hovered).
    for (var round = 0; round < 3; round++) {
      for (final label in _items) {
        await mouse.moveTo(centre(label));
        await t.pump(const Duration(milliseconds: 30));
        await settle(t, 120);
        final lit = _lit(t);
        expect(lit.length <= 1, isTrue, reason: 'over $label, lit: $lit');
        if (lit.isNotEmpty) expect(lit.single, label, reason: 'over $label, lit: $lit');
      }
    }
    await mouse.removePointer();
  });
}
