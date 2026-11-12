// Receipts, for quick runs: `tool/screens.sh receipts`. Sam's messages to
// Bob at each stage (sent, delivered, read), a tick turning blue while the
// conversation is open, and the setting.

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
final h = Harness('receipts');
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

  testWidgets('receipts', (t) async {
    await h.register('sam', name: 'Sam');
    await h.register('bob', name: 'Bob B');
    // Read, delivered, sent: Bob reads the first, fetches the second, and
    // has not fetched the third.
    await h.cli('sam', ['send', '--to', 'bob', 'are we still on for saturday?']);
    await h.cli('bob', ['sync']);
    final dm = (await h.cliJson('bob', ['dm', 'sam']))['id'] as String;
    await h.cli('bob', ['read', dm]);
    await h.cli('bob', ['send', '--to', 'sam', 'yes! 10am at the wall']);
    await h.cli('sam', ['sync']);
    await h.cli('sam', ['send', '--to', 'bob', 'great, I’ll bring the rope']);
    await h.cli('bob', ['sync']);
    await h.cli('sam', ['send', '--to', 'bob', 'and snacks']);
    await h.cli('sam', ['sync']);

    await t.pumpWidget(
      RepaintBoundary(
        key: _screen,
        child: PostApp(profileDir: '${h.work.path}/sam', keyFile: '${h.work.path}/sam.key'),
      ),
    );
    await tap(t, find.text('Bob B'));
    await waitFor(t, find.textContaining('and snacks'));
    await waitFor(t, find.byTooltip('Read'));
    expect(find.byTooltip('Read'), findsOneWidget);
    expect(find.byTooltip('Delivered'), findsOneWidget);
    expect(find.byTooltip('Sent'), findsOneWidget);
    await _shot(t, 'receipts-00-stages');

    // Bob catches up and reads: every tick turns blue, live.
    await h.cli('bob', ['sync']);
    await h.cli('bob', ['read', dm]);
    await waitFor(t, find.byTooltip('Read').at(2));
    expect(find.byTooltip('Sent'), findsNothing);
    await _shot(t, 'receipts-01-all-read');

    await tap(t, find.byTooltip('Settings'));
    await tap(t, find.text('Privacy'));
    await waitFor(t, find.text('Send read receipts'));
    await _shot(t, 'receipts-02-setting');
  });
}
