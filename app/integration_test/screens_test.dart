// Drives the real app against a throwaway server filled with test accounts,
// and saves screenshots of its main screens. A smoke test of the whole
// stack, and a way to look at the UI without clicking through it.
//
// Run with tool/screens.sh, which builds the server and CLI first. Nothing
// here touches the user's own profile, keyring or development server: the
// server runs on its own port from a temporary directory, and every profile,
// the app's included, takes its key from a file.

import 'dart:async';
import 'dart:convert';
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
import 'package:post/ui/controls.dart';

const _shots = String.fromEnvironment('SHOTS', defaultValue: 'build/screens');

/// A free port, found when the test starts, so a server left over from a run
/// that crashed can never be in the way.
late int _port;
String get _url => 'ws://127.0.0.1:$_port/v1/ws';

/// Every process the test starts, one PID per line. A test that crashes may
/// not get to tear down; tool/screens.sh stops whatever is listed here.
final _pids = File('build/screens.pids');

/// Release builds: seeding makes dozens of CLI calls, each doing
/// post-quantum handshakes, which take seconds apiece unoptimised.
final _bin = '${Directory.current.parent.path}/target/release';

late Directory _work;
final _processes = <Process>[];

Map<String, String> get _serverEnv => {'POST_DB': '${_work.path}/server.db', 'POST_DB_KEY_FILE': '${_work.path}/server.key'};

/// Keep a started process, to stop at the end.
void _track(Process p) {
  _processes.add(p);
  _pids.writeAsStringSync('${p.pid}\n', mode: FileMode.append);
  unawaited(p.stdout.drain<void>());
  unawaited(p.stderr.drain<void>());
}

Future<String> _run(String exe, List<String> args, {Map<String, String>? env}) async {
  final r = await Process.run(exe, args, environment: env);
  if (r.exitCode != 0) throw StateError('$exe ${args.join(' ')} failed: ${r.stderr}');
  return (r.stdout as String).trim();
}

/// Run the CLI as `user`.
Future<String> cli(String user, List<String> args) =>
    _run('$_bin/post', ['--profile', '${_work.path}/$user', '--db-key-file', '${_work.path}/$user.key', ...args]);

Future<dynamic> cliJson(String user, List<String> args) async => jsonDecode(await cli(user, ['--json', ...args]));

Future<void> _startServer() async {
  await _run('$_bin/postd', ['keygen', '${_work.path}/server.key']);
  final probe = await ServerSocket.bind('127.0.0.1', 0);
  _port = probe.port;
  await probe.close();
  _track(await Process.start('$_bin/postd', ['serve', '--domain', '127.0.0.1:$_port', '--listen', '127.0.0.1:$_port'], environment: _serverEnv));
  for (var i = 0; i < 50; i++) {
    try {
      (await Socket.connect('127.0.0.1', _port)).destroy();
      return;
    } on SocketException {
      await Future<void>.delayed(const Duration(milliseconds: 100));
    }
  }
  throw StateError('the test server did not start');
}

Future<void> _register(String user, {String? name, String? note}) async {
  await _run('$_bin/post', ['keygen', '${_work.path}/$user.key']);
  final invite = await _run('$_bin/postd', ['invite'], env: _serverEnv);
  await cli(user, ['register', '--server', _url, '--invite', invite, '--username', user]);
  await cli(user, [
    'profile',
    if (name != null) ...['--display-name', name],
    if (note != null) ...['--note', note, '--keep-note'],
  ]);
}

/// Keep `user` online, with this presence, until the test ends.
Future<void> _online(String user, String presence) async {
  await cli(user, ['presence', presence]);
  _track(await Process.start('$_bin/post', ['--profile', '${_work.path}/$user', '--db-key-file', '${_work.path}/$user.key', 'daemon', '--stdio']));
}

/// Accounts, conversations and messages to look at.
Future<String> _seed() async {
  await _register('sam', name: 'Sam', note: 'at the crag');
  await _register('alice', name: 'Alice A', note: 'at the crag, back sunday');
  await _register('bob', name: 'Bob B', note: 'driving');
  await _register('carol', name: 'Carol C', note: 'deep work till 3');
  await _register('devi', name: 'Devi');

  await cli('carol', ['send', '--to', 'sam', 'send me the topo?']);
  await cli('alice', ['send', '--to', 'sam', 'did you get the guidebook?']);
  await cli('sam', ['sync']);
  await cli('sam', ['send', '--to', 'alice', 'yes, the new edition']);
  await cli('alice', ['send', '--to', 'sam', 'see you saturday']);

  final group = await cliJson('alice', ['group', 'create', 'climbing', 'bob', 'carol', 'devi', 'sam']);
  final g = group['id'] as String;
  await cli('alice', ['send', '--conversation', g, 'saturday at the crag? forecast says dry all day']);
  for (final u in ['bob', 'carol', 'sam']) {
    await cli(u, ['sync']);
  }
  await cli('carol', ['send', '--conversation', g, 'in. bringing the new 70 m rope']);
  final drive = await cliJson('bob', ['send', '--conversation', g, 'I can drive, leaving at 9']);
  await cli('bob', ['edit', g, drive['id'] as String, 'I can drive, leaving at 8 from mine']);
  await cli('sam', ['send', '--conversation', g, 'count me in. who has the quickdraws?']);
  await cli('alice', ['send', '--conversation', g, 'me, 12 of them']);
  await cli('alice', ['send', '--conversation', g, 'and two spare belay devices']);
  await cli('carol', [
    'send',
    '--conversation',
    g,
    '**bold**, *italic*, __underline__, ~~strike~~, `code` and a ||secret||\n> a quote with **bold**\n```\nrope: 70 m\ndraws: 12\n```',
  ]);
  await cli('bob', ['send', '--to', 'sam', 'checked the safety number on the call']);
  await cli('bob', ['send', '--to', 'sam', 'numbers match']);
  return g;
}

Future<void> _wait(WidgetTester t, Finder f, {Duration timeout = const Duration(seconds: 30)}) async {
  final end = DateTime.now().add(timeout);
  while (DateTime.now().isBefore(end)) {
    await Future<void>.delayed(const Duration(milliseconds: 100));
    await t.pump();
    if (f.evaluate().isNotEmpty) return;
  }
  throw TestFailure('timed out waiting for $f');
}

/// Let animations and late data land.
Future<void> _settle(WidgetTester t, [int ms = 700]) async {
  final end = DateTime.now().add(Duration(milliseconds: ms));
  while (DateTime.now().isBefore(end)) {
    await Future<void>.delayed(const Duration(milliseconds: 50));
    await t.pump();
  }
}

/// Around the whole app, so a screenshot includes dialogs and menus.
final _screen = GlobalKey();

Future<void> _shot(WidgetTester t, String name) async {
  await _settle(t);
  final boundary = _screen.currentContext!.findRenderObject()! as RenderRepaintBoundary;
  final image = await boundary.toImage(pixelRatio: 1);
  final png = await image.toByteData(format: ui.ImageByteFormat.png);
  final file = File('$_shots/$name.png');
  await file.parent.create(recursive: true);
  await file.writeAsBytes(png!.buffer.asUint8List());
  // ignore: avoid_print
  print('screenshot: ${file.absolute.path}');
}

Future<void> _tap(WidgetTester t, Finder f) async {
  await _wait(t, f);
  await t.tap(f.first);
  await t.pump();
}

void main() {
  final binding = IntegrationTestWidgetsFlutterBinding.ensureInitialized();
  binding.framePolicy = LiveTestWidgetsFlutterBindingFramePolicy.fullyLive;

  setUpAll(() async {
    _work = await Directory.systemTemp.createTemp('post-screens-');
    await _pids.parent.create(recursive: true);
    _pids.writeAsStringSync('');
    await RustLib.init();
    await _startServer();
  });

  tearDownAll(() async {
    for (final p in _processes) {
      p.kill();
    }
    await _work.delete(recursive: true);
  });

  testWidgets('main screens', (t) async {
    final group = await _seed();
    await t.pumpWidget(
      RepaintBoundary(
        key: _screen,
        child: PostApp(profileDir: '${_work.path}/sam', keyFile: '${_work.path}/sam.key'),
      ),
    );

    await _wait(t, find.text('Alice A'));
    await _online('alice', 'online');
    await _online('bob', 'away');
    await _online('carol', 'dnd');
    await _settle(t, 3000);
    await _shot(t, '01-start');

    await _tap(t, find.text('Alice A'));
    await _shot(t, '02-dm');
    // The composer's text centres beside the send button: same height, same
    // middle, to the pixel (desktop density once moved it by four).
    final field = t.getRect(find.byWidgetPredicate((w) => w is TextField).last);
    final send = t.getRect(find.byTooltip('Send (Enter). Shift+Enter for a new line.'));
    final hint = t.getRect(find.text('Message Alice A'));
    expect(field.height, send.height);
    expect(hint.center.dy, closeTo(send.center.dy, 0.5));

    await _tap(t, find.text('climbing'));
    await cli('carol', ['typing', group]);
    await _wait(t, find.textContaining('typing'));
    await _shot(t, '03-group');

    await _tap(t, find.textContaining('versions'));
    await _shot(t, '04-versions');

    // A message's own menu, on a right-click after dragging across its text.
    // (A right-click with a real selection once crashed the selection's own
    // menu; test input does not make a selection, so this cannot catch that.)
    final quickdraws = t.getRect(find.text('count me in. who has the quickdraws?'));
    await t.dragFrom(quickdraws.centerLeft + const Offset(4, 0), const Offset(120, 0), kind: PointerDeviceKind.mouse);
    await _settle(t, 300);
    await t.tapAt(quickdraws.centerLeft + const Offset(40, 0), buttons: kSecondaryButton, kind: PointerDeviceKind.mouse);
    await _wait(t, find.text('Delete message'));
    await _shot(t, '04b-message-menu');
    await t.sendKeyEvent(LogicalKeyboardKey.escape);
    await _settle(t);

    await _tap(t, find.text('Message climbing'));
    await t.sendKeyEvent(LogicalKeyboardKey.arrowUp);
    await _shot(t, '05-editing');
    await t.sendKeyEvent(LogicalKeyboardKey.escape);
    await _settle(t);

    // Typing with the composer unfocused still types into it.
    FocusManager.instance.primaryFocus?.unfocus();
    await _settle(t, 200);
    // One key: after it the composer has the keyboard, and the test's
    // simulated keys never reach a text field as text (real typing does).
    await t.sendKeyDownEvent(LogicalKeyboardKey.keyH, character: 'h');
    await t.sendKeyUpEvent(LogicalKeyboardKey.keyH);
    await _wait(t, find.byWidgetPredicate((w) => w is EditableText && w.controller.text == 'h'));
    await _shot(t, '05b-typed');

    await _tap(t, find.text('Bob B'));
    await _shot(t, '06-dm-unread-read');

    // A right-click menu opens with its corner at the pointer.
    final click = t.getCenter(find.text('Carol C').first);
    await t.tapAt(click, buttons: kSecondaryButton, kind: PointerDeviceKind.mouse);
    await _wait(t, find.text('Set nickname'));
    await _shot(t, '06b-context-menu');
    final item = t.getRect(find.byWidgetPredicate((w) => w is PMenuItem).first);
    expect(item.height, 32, reason: 'compact entries');
    expect(item.left - click.dx, inInclusiveRange(0, 30), reason: 'menu starts at the pointer, not away from it');
    expect(item.top - click.dy, inInclusiveRange(0, 30), reason: 'menu starts at the pointer, not away from it');
    await t.sendKeyEvent(LogicalKeyboardKey.escape);
    await _settle(t);

    await _tap(t, find.text('Sam'));
    await _shot(t, '07-presence-menu');
    await t.sendKeyEvent(LogicalKeyboardKey.escape);
    await _settle(t);

    await _tap(t, find.byTooltip('New group'));
    await _shot(t, '08-new-group');
    await t.sendKeyEvent(LogicalKeyboardKey.escape);
    await _settle(t);

    await _tap(t, find.byTooltip('Settings'));
    await _shot(t, '09-settings-profile');
    await _tap(t, find.text('Devices'));
    await _shot(t, '10-settings-devices');
    await t.sendKeyEvent(LogicalKeyboardKey.escape);
    await _settle(t);
  });
}
