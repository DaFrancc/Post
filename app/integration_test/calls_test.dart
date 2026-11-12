// Calls only, for quick runs: `tool/screens.sh calls`. The others in each
// call are CLI accounts, which carry a call's signalling but no audio; the
// app joins with real encrypted audio through LiveKit. Needs the LiveKit
// that tool/screens.sh starts.

import 'dart:io';
import 'dart:ui' as ui;

import 'package:flutter/gestures.dart';
import 'package:flutter/material.dart';
import 'package:flutter/rendering.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:integration_test/integration_test.dart';
import 'package:post/main.dart';
import 'package:post/src/rust/frb_generated.dart';

import 'harness.dart';

const _shots = String.fromEnvironment('SHOTS', defaultValue: 'build/screens');
final h = Harness('calls');
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

  testWidgets('calls', (t) async {
    if (Platform.environment['POST_TEST_LIVEKIT_URL'] == null) {
      markTestSkipped('no LiveKit: run through tool/screens.sh calls, with Podman');
      return;
    }
    await h.register('sam', name: 'Sam');
    await h.register('alice', name: 'Alice A');
    await h.register('bob', name: 'Bob B');
    await h.register('carol', name: 'Carol C');
    await h.cli('bob', ['send', '--to', 'sam', 'call you in a bit']);
    await h.cli('carol', ['send', '--to', 'sam', 'free later?']);
    final dm = (await h.cliJson('bob', ['dm', 'sam']))['id'] as String;
    final carolDm = (await h.cliJson('carol', ['dm', 'sam']))['id'] as String;
    final group = (await h.cliJson('alice', ['group', 'create', 'climbing', 'bob', 'carol', 'sam']))['id'] as String;
    await h.cli('alice', ['send', '--conversation', group, 'call at 8?']);
    for (final u in ['bob', 'carol', 'sam']) {
      await h.cli(u, ['sync']);
    }

    await t.pumpWidget(
      RepaintBoundary(
        key: _screen,
        child: PostApp(profileDir: '${h.work.path}/sam', keyFile: '${h.work.path}/sam.key'),
      ),
    );
    await waitFor(t, find.text('Bob B'));

    // A DM call while Sam is in another chat: it rings anyway, and
    // answering takes him to Bob's.
    await tap(t, find.text('climbing'));
    await waitFor(t, find.textContaining('call at 8?'));
    final call = (await h.cliJson('bob', ['call', 'start', dm]))['call']['id'] as String;
    await waitFor(t, find.text('Bob B is calling'));
    await _shot(t, 'calls-01-ringing');
    await t.tap(find.text('Answer'));
    await waitFor(t, find.text('In a call'), timeout: const Duration(seconds: 30));
    await waitFor(t, find.text('2 in the call'));
    expect(find.textContaining('call you in a bit'), findsWidgets, reason: 'answering opened Bob\'s chat');
    await _shot(t, 'calls-02-in-call');

    // Moving to another chat keeps the call; the card above Sam's name
    // shows it and brings him back.
    await tap(t, find.text('climbing'));
    await waitFor(t, find.textContaining('call at 8?'));
    await settle(t, 800); // the previous chat fades out
    expect(find.text('In a call'), findsOneWidget, reason: 'still in the call from another chat');
    expect(find.text('2 in the call'), findsNothing, reason: 'the strip belongs to the call\'s own chat');
    await _shot(t, 'calls-02b-other-chat');
    await t.tap(find.text('In a call'));
    await waitFor(t, find.text('2 in the call'));
    await t.tap(find.byTooltip('Mute').first);
    await settle(t, 300);
    expect(find.byTooltip('Unmute'), findsWidgets, reason: 'muted');
    await h.cli('bob', ['call', 'leave', dm, call]);
    await waitFor(t, find.textContaining('Call ended'), timeout: const Duration(seconds: 20));
    expect(find.text('In a call'), findsNothing, reason: 'the call closed when Bob hung up');
    await _shot(t, 'calls-03-ended');

    // A group call ringing only Sam: it rings "in climbing", Sam answers,
    // Bob joins too; Sam rings Carol in; Carol calls Sam while he is in it;
    // Sam leaves and the call goes on without him.
    final groupCall = (await h.cliJson('alice', ['call', 'start', group, '--ring', 'sam']))['call']['id'] as String;
    await waitFor(t, find.text('Alice A is calling'));
    expect(find.text('in climbing'), findsOneWidget);
    await _shot(t, 'calls-04-group-ringing');
    await t.tap(find.text('Answer'));
    await waitFor(t, find.text('2 in the call'), timeout: const Duration(seconds: 30));
    // One process sees the call start and joins it: a CLI call lives only as
    // long as its process.
    await h.cli('bob', ['call', 'join', group, groupCall]);
    await waitFor(t, find.text('3 in the call'));
    await _shot(t, 'calls-05-group-in-call');

    // Ring Carol, who was not rung by the start.
    await t.tap(find.byTooltip('Ring someone'));
    await waitFor(t, find.text('Ring Carol C'));
    await _shot(t, 'calls-05b-ring-someone');
    await t.tap(find.text('Ring Carol C'));
    await settle(t, 1500);
    final carolCalls = await h.cliJson('carol', ['call', 'list']) as List<dynamic>;
    expect(carolCalls.single['ringing'], isTrue, reason: 'Carol is rung into the call');

    // The member list's right-click menu rings people too: Carol, not in the
    // call, can be rung; Bob, in it, cannot.
    Future<void> rightClick(String name) async {
      final row = t.getRect(find.text(name).last);
      await t.tapAt(row.center, buttons: kSecondaryButton, kind: PointerDeviceKind.mouse);
      await settle(t, 500);
    }

    await rightClick('Carol C');
    expect(find.text('Ring Carol C into the call'), findsOneWidget);
    await _shot(t, 'calls-05a-member-menu');
    await t.sendKeyEvent(LogicalKeyboardKey.escape);
    await settle(t, 500);
    await rightClick('Bob B');
    expect(find.text('Ring Bob B into the call'), findsNothing, reason: 'Bob is already in it');
    await t.sendKeyEvent(LogicalKeyboardKey.escape);
    await settle(t, 500);

    // Carol calls Sam while he is in the group call: he sees it, declines,
    // and stays where he is.
    await h.cli('carol', ['call', 'start', carolDm]);
    await waitFor(t, find.text('Carol C is calling'));
    await _shot(t, 'calls-05c-call-waiting');
    await t.tap(find.text('Decline'));
    await settle(t, 1500);
    expect(find.text('Carol C is calling'), findsNothing, reason: 'declined');
    expect(find.text('3 in the call'), findsOneWidget, reason: 'still in the group call');
    await t.tap(find.byTooltip('Leave the call').first);
    await waitFor(t, find.text('Join'));
    expect(find.text('In a call'), findsNothing);
    expect(find.textContaining('Call in progress · 2 people'), findsOneWidget, reason: 'Alice and Bob carry on');
    await _shot(t, 'calls-06-group-left');

    // Settings › Sounds: Post's own pack, every sound listed and playable.
    await tap(t, find.byTooltip('Settings'));
    await tap(t, find.text('Sounds'));
    expect(find.text('Post\'s own'), findsWidgets, reason: 'the pack, and each sound from it');
    expect(find.text('Incoming call'), findsOneWidget);
    await t.tap(find.byTooltip('Play incoming call'));
    await settle(t, 500);
    await _shot(t, 'calls-07-sound-settings');
  });
}
