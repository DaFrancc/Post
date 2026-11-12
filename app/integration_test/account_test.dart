// Logging out, for quick runs: `tool/screens.sh account`. From a device
// with another beside it, which leaves the account; and from an account's
// only device, which loses the account. Either way the app is back at the
// welcome screens.

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
final h = Harness('account');
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

/// Open Settings › Account and press Log out.
Future<void> _pressLogOut(WidgetTester t, {String? shot}) async {
  await tap(t, find.byTooltip('Settings'));
  await tap(t, find.text('Account'));
  if (shot != null) await _shot(t, shot);
  await tap(t, find.widgetWithText(OutlinedButton, 'Log out'));
}

void main() {
  final binding = IntegrationTestWidgetsFlutterBinding.ensureInitialized();
  binding.framePolicy = LiveTestWidgetsFlutterBindingFramePolicy.fullyLive;

  setUpAll(() async {
    await RustLib.init();
    await h.start();
  });
  tearDownAll(() => h.stop());

  testWidgets('logging out', (t) async {
    await h.register('sam', name: 'Sam');
    await h.register('bob', name: 'Bob B');
    await h.link('sam', 'sam-phone');
    // Bob's DM reaches the phone through the laptop, which adds it; the
    // phone has not fetched that invitation when the laptop logs out.
    await h.cli('bob', ['send', '--to', 'sam', 'hi sam']);
    await h.cli('sam', ['sync']);

    // With another device: this one leaves the account.
    await t.pumpWidget(
      RepaintBoundary(
        key: _screen,
        child: PostApp(profileDir: '${h.work.path}/sam', keyFile: '${h.work.path}/sam.key'),
      ),
    );
    await waitFor(t, find.text('Bob B'));
    await _pressLogOut(t, shot: 'account-00-page');
    await waitFor(t, find.text('Log out of @sam?'));
    await _shot(t, 'account-01-log-out');
    await tap(t, find.widgetWithText(FilledButton, 'Log out'));
    await waitFor(t, find.text('Link to my account'));
    await _shot(t, 'account-02-welcome');
    await h.cli('sam-phone', ['sync']);
    final devices = await h.cliJson('sam-phone', ['devices']);
    expect(devices, hasLength(1), reason: 'the phone is the account\'s only device now: $devices');
    // The phone carries on with Bob, in the same conversation.
    final dm = (await h.cliJson('bob', ['dm', 'sam']))['id'] as String;
    await h.cli('bob', ['send', '--to', 'sam', 'still there?']);
    await h.cli('sam-phone', ['sync']);
    expect(await h.cli('sam-phone', ['messages', dm]), contains('still there?'));

    // On the account's only device, the warning says the account is lost.
    await h.register('solo', name: 'Solo');
    await t.pumpWidget(const SizedBox());
    await settle(t);
    await t.pumpWidget(
      RepaintBoundary(
        key: _screen,
        child: PostApp(profileDir: '${h.work.path}/solo', keyFile: '${h.work.path}/solo.key'),
      ),
    );
    await waitFor(t, find.byTooltip('Settings'));
    await _pressLogOut(t);
    await waitFor(t, find.text('Log out and lose @solo?'));
    await _shot(t, 'account-03-last-device');
    await tap(t, find.text('Log out and lose account'));
    await waitFor(t, find.text('Link to my account'));
    expect(File('${h.work.path}/solo.key').existsSync(), isTrue, reason: 'a key file is the user\'s, not deleted with the profile');
  });
}
