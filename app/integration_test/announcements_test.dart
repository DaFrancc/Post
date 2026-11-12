// The server's announcements, for quick runs: `tool/screens.sh
// announcements`. The operator posts one from the server's command line, a
// host posts from the app and opens the maintenance planner, the operator
// plans maintenance, and a member sees it all.

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
final h = Harness('announcements');
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

Future<void> _open(WidgetTester t, String user) async {
  await t.pumpWidget(
    RepaintBoundary(
      key: _screen,
      child: PostApp(key: UniqueKey(), profileDir: '${h.work.path}/$user', keyFile: '${h.work.path}/$user.key'),
    ),
  );
}

void main() {
  final binding = IntegrationTestWidgetsFlutterBinding.ensureInitialized();
  binding.framePolicy = LiveTestWidgetsFlutterBindingFramePolicy.fullyLive;

  setUpAll(() async {
    await RustLib.init();
    await h.start();
  });
  tearDownAll(() => h.stop());

  testWidgets('announcements', (t) async {
    await h.register('sam', name: 'Sam');
    await h.register('bob', name: 'Bob B');
    await h.cli('bob', ['send', '--to', 'sam', 'see you saturday']);
    await h.postd(['announce', 'Welcome to Post! Calls now work in groups too. Tell me if anything breaks.\n– Dave']);
    await h.postd(['host', 'add', 'sam']);

    // The host: the channel, pinned, with a composer.
    await _open(t, 'sam');
    await tap(t, find.text('Announcements'));
    await waitFor(t, find.textContaining('Welcome to Post!'));
    await t.enterText(find.byType(TextField).last, '**Maintenance tonight at 02:00**\nThe server is being backed up and moved to a new machine.');
    await _shot(t, 'announcements-00-host-writing');
    await tap(t, find.widgetWithText(FilledButton, 'Announce'));
    await waitFor(t, find.textContaining('moved to a new machine'));
    await tap(t, find.byTooltip('Plan maintenance'));
    await waitFor(t, find.text('Plan maintenance'));
    await _shot(t, 'announcements-01-plan');
    await tap(t, find.widgetWithText(OutlinedButton, 'Cancel'));

    // The operator plans maintenance two days out: the server announces it.
    final at = DateTime.now().add(const Duration(days: 2)).millisecondsSinceEpoch ~/ 1000;
    await h.postd(['maintenance', 'plan', '--at', '@$at', '--minutes', '15', '--reason', 'moving to a new machine']);
    await waitFor(t, find.textContaining('Maintenance planned for'), timeout: const Duration(seconds: 20));
    await _shot(t, 'announcements-02-host-planned');

    // A member: unread in the list, then the channel without a composer.
    await _open(t, 'bob');
    await waitFor(t, find.text('Announcements'));
    await _shot(t, 'announcements-03-member-list');
    await tap(t, find.text('Announcements'));
    await waitFor(t, find.textContaining('Sent automatically'));
    expect(find.widgetWithText(FilledButton, 'Announce'), findsNothing);
    await _shot(t, 'announcements-04-member');
  });
}
