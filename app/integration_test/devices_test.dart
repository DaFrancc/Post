// Settings › Devices on the local API (docs/phase-1-api.md, milestone 3),
// for quick runs: `tool/screens.sh devices`. The list loads, a device linked
// from the command line appears by itself (a devices_changed event), and
// renaming and removing work.

import 'dart:io';
import 'dart:ui' as ui;

import 'package:flutter/material.dart';
import 'package:flutter/rendering.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:integration_test/integration_test.dart';
import 'package:post/main.dart';
import 'package:post/src/rust/frb_generated.dart';

import 'harness.dart';

const _shots = String.fromEnvironment('SHOTS', defaultValue: 'build/screens');
final h = Harness('devices');
final _screen = GlobalKey();

Future<void> _shot(WidgetTester t, String name) async {
  await settle(t, 800);
  final boundary = _screen.currentContext!.findRenderObject()! as RenderRepaintBoundary;
  final png = await (await boundary.toImage(pixelRatio: 1)).toByteData(format: ui.ImageByteFormat.png);
  final file = File('$_shots/$name.png');
  await file.parent.create(recursive: true);
  await file.writeAsBytes(png!.buffer.asUint8List());
  // ignore: avoid_print
  print('screenshot: ${file.absolute.path}');
}

void main() {
  final binding = IntegrationTestWidgetsFlutterBinding.ensureInitialized();
  binding.framePolicy = LiveTestWidgetsFlutterBindingFramePolicy.fullyLive;

  setUpAll(() async {
    await RustLib.init();
    await h.start();
  });
  tearDownAll(() => h.stop());

  testWidgets('devices', (t) async {
    await h.register('sam', name: 'Sam');
    await t.pumpWidget(
      RepaintBoundary(
        key: _screen,
        child: PostApp(profileDir: '${h.work.path}/sam', keyFile: '${h.work.path}/sam.key'),
      ),
    );
    await tap(t, find.byTooltip('Settings'));
    await tap(t, find.text('Devices'));

    // The list loads: this device only, which cannot remove itself.
    await waitFor(t, find.text('this device'));
    expect(find.byTooltip('Rename'), findsOneWidget);
    expect(find.byTooltip('Remove from your account'), findsNothing);
    await _shot(t, 'devices-00-one');

    // A device linked from the command line appears without a reload.
    await h.link('sam', 'sam-phone');
    await waitFor(t, find.text('sam-phone'));
    expect(find.byTooltip('Remove from your account'), findsOneWidget);
    await _shot(t, 'devices-01-linked');

    // Rename it: devices are oldest first, so the phone is the second row.
    await tap(t, find.byTooltip('Rename').last);
    await waitFor(t, find.text('Name this device'));
    await t.enterText(find.byType(TextField).last, 'pocket phone');
    await tap(t, find.widgetWithText(FilledButton, 'Save'));
    await waitFor(t, find.text('pocket phone'));
    expect(find.text('sam-phone'), findsNothing);
    final named = await h.cliJson('sam', ['devices']) as List<dynamic>;
    expect(named.map((d) => d['name']), contains('pocket phone'), reason: 'the core has the new name: $named');

    // Remove it.
    await tap(t, find.byTooltip('Remove from your account'));
    await waitFor(t, find.text('Remove pocket phone?'));
    await _shot(t, 'devices-02-remove');
    await tap(t, find.widgetWithText(FilledButton, 'Remove device'));
    await settle(t, 1500);
    expect(find.text('pocket phone'), findsNothing);
    expect(find.text('this device'), findsOneWidget);
    await _shot(t, 'devices-03-removed');
  });
}
