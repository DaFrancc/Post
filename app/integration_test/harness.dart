// What the integration tests share: a throwaway server, test accounts made
// with the CLI, and helpers for driving the real app.
//
// Nothing here touches the user's own profile, keyring or development
// server: the server runs on a free port from a temporary directory, and
// every profile, the app's included, takes its key from a file.

import 'dart:async';
import 'dart:convert';
import 'dart:io';

import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

/// A throwaway server and the accounts made on it.
class Harness {
  Harness(this.name);

  /// Names the temporary directory and the PID file.
  final String name;

  late final Directory work;
  late final int port;
  final _processes = <Process>[];

  /// Debug builds, whose dependencies are optimised (the root Cargo.toml):
  /// tests make many CLI calls, each doing post-quantum handshakes.
  final bin = '${Directory.current.parent.path}/target/debug';

  String get url => 'ws://127.0.0.1:$port/v1/ws';

  /// Every process started, one PID per line. A test that crashes may not
  /// get to tear down; the tool scripts stop whatever is listed here.
  File get pids => File('build/$name.pids');

  Map<String, String> get _serverEnv => {'POST_DB': '${work.path}/server.db', 'POST_DB_KEY_FILE': '${work.path}/server.key'};

  /// Start the server, on a free port so one left over from a crashed run
  /// can never be in the way.
  Future<void> start() async {
    work = await Directory.systemTemp.createTemp('post-$name-');
    await pids.parent.create(recursive: true);
    pids.writeAsStringSync('');
    await run('$bin/postd', ['keygen', '${work.path}/server.key']);
    final probe = await ServerSocket.bind('127.0.0.1', 0);
    port = probe.port;
    await probe.close();
    // No releases fetched in tests; calls when tool/screens.sh started LiveKit.
    final livekit = Platform.environment['POST_TEST_LIVEKIT_URL'];
    final keys = Platform.environment['POST_TEST_LIVEKIT_KEYS'];
    final args = [
      'serve',
      '--domain',
      '127.0.0.1:$port',
      '--listen',
      '127.0.0.1:$port',
      '--releases-dir',
      '${work.path}/releases',
      '--releases-from',
      '',
      if (livekit != null && keys != null) ...['--livekit-url', livekit, '--livekit-secret-file', keys],
    ];
    _track(await Process.start('$bin/postd', args, environment: _serverEnv));
    for (var i = 0; i < 50; i++) {
      try {
        (await Socket.connect('127.0.0.1', port)).destroy();
        return;
      } on SocketException {
        await Future<void>.delayed(const Duration(milliseconds: 100));
      }
    }
    throw StateError('the test server did not start');
  }

  Future<void> stop() async {
    for (final p in _processes) {
      p.kill();
    }
    await work.delete(recursive: true);
  }

  void _track(Process p) {
    _processes.add(p);
    pids.writeAsStringSync('${p.pid}\n', mode: FileMode.append);
    unawaited(p.stdout.drain<void>());
    unawaited(p.stderr.drain<void>());
  }

  Future<String> run(String exe, List<String> args, {Map<String, String>? env}) async {
    final r = await Process.run(exe, args, environment: env);
    if (r.exitCode != 0) throw StateError('$exe ${args.join(' ')} failed: ${r.stderr}');
    return (r.stdout as String).trim();
  }

  /// A new invite code.
  Future<String> invite() => postd(['invite']);

  /// Run a `postd` admin command against the test server's database, as
  /// its operator would.
  Future<String> postd(List<String> args) => run('$bin/postd', args, env: _serverEnv);

  /// A key file for a new profile `user`, and its path.
  Future<String> keyFor(String user) async {
    final path = '${work.path}/$user.key';
    await run('$bin/post', ['keygen', path]);
    return path;
  }

  /// Run the CLI as `user`.
  Future<String> cli(String user, List<String> args) =>
      run('$bin/post', ['--profile', '${work.path}/$user', '--db-key-file', '${work.path}/$user.key', ...args]);

  Future<dynamic> cliJson(String user, List<String> args) async => jsonDecode(await cli(user, ['--json', ...args]));

  /// Create an account with the CLI.
  Future<void> register(String user, {String? name, String? note}) async {
    await keyFor(user);
    await cli(user, ['register', '--server', url, '--invite', await invite(), '--username', user]);
    await cli(user, [
      'profile',
      if (name != null) ...['--display-name', name],
      if (note != null) ...['--note', note, '--keep-note'],
    ]);
  }

  /// Link a new CLI device, the profile `device`, to `existing`'s account.
  Future<void> link(String existing, String device) async {
    await keyFor(device);
    final p = await Process.start('$bin/post', [
      '--profile',
      '${work.path}/$device',
      '--db-key-file',
      '${work.path}/$device.key',
      '--json',
      'link',
      'new',
      '--server',
      url,
      '--name',
      device,
    ]);
    unawaited(p.stderr.drain<void>());
    final lines = StreamIterator(p.stdout.transform(utf8.decoder).transform(const LineSplitter()));
    if (!await lines.moveNext()) throw StateError('post link new printed no code');
    final code = (jsonDecode(lines.current) as Map<String, dynamic>)['code'] as String;
    await cli(existing, ['link', 'add', code, '--yes']);
    while (await lines.moveNext()) {}
    if (await p.exitCode != 0) throw StateError('post link new failed');
  }

  /// Keep `user` online, with this presence, until the test ends.
  Future<void> online(String user, String presence) async {
    await cli(user, ['presence', presence]);
    _track(await Process.start('$bin/post', ['--profile', '${work.path}/$user', '--db-key-file', '${work.path}/$user.key', 'daemon', '--stdio']));
  }
}

/// Pump until `f` finds something.
Future<void> waitFor(WidgetTester t, Finder f, {Duration timeout = const Duration(seconds: 30)}) async {
  final end = DateTime.now().add(timeout);
  while (DateTime.now().isBefore(end)) {
    await Future<void>.delayed(const Duration(milliseconds: 100));
    await t.pump();
    if (f.evaluate().isNotEmpty) return;
  }
  // Say what is on screen instead, which usually explains it (an error).
  final shown = find.byType(Text).evaluate().map((e) => (e.widget as Text).data).whereType<String>().where((s) => s.trim().isNotEmpty);
  throw TestFailure('timed out waiting for $f; on screen: ${shown.join(' | ')}');
}

/// Let animations and late data land.
Future<void> settle(WidgetTester t, [int ms = 700]) async {
  final end = DateTime.now().add(Duration(milliseconds: ms));
  while (DateTime.now().isBefore(end)) {
    await Future<void>.delayed(const Duration(milliseconds: 50));
    await t.pump();
  }
}

Future<void> tap(WidgetTester t, Finder f) async {
  await waitFor(t, f);
  await t.tap(f.first);
  await t.pump();
}
