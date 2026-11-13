// How long the local API's JSON costs (docs/phase-1-api.md, milestone 3):
// `api.messages(limit: 150)` on a conversation of 150 messages, the median
// of 20 calls, split into the bridge call and the decoding. Run it in a
// debug and a profile build:
//
//   flutter test integration_test/api_timing_test.dart -d linux
//   flutter drive --profile -d linux --driver=test_driver/integration_test.dart \
//     --target=integration_test/api_timing_test.dart
//
// The report lands in build/api-timing-<mode>.txt.

import 'dart:convert';
import 'dart:io';

import 'package:flutter/foundation.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:integration_test/integration_test.dart';
import 'package:post/api/bridge_transport.dart';
import 'package:post/api/post_api.dart';
import 'package:post/src/rust/api/client.dart';
import 'package:post/src/rust/frb_generated.dart';

import 'harness.dart';

final h = Harness('api-timing');

/// Bob sends `n` messages to `conversation` through one daemon, much faster
/// than a CLI run each.
Future<void> _seed(String conversation, int n) async {
  final p = await Process.start('${h.bin}/post', ['--profile', '${h.work.path}/bob', '--db-key-file', '${h.work.path}/bob.key', 'daemon', '--stdio']);
  final lines = p.stdout.transform(utf8.decoder).transform(const LineSplitter()).asBroadcastStream();
  for (var i = 0; i < n; i++) {
    final text = 'Message $i: the weather at the crag looks fine for Saturday, bring the long rope and some snacks.';
    p.stdin.writeln(
      jsonEncode({
        'jsonrpc': '2.0',
        'id': i,
        'method': 'send_text',
        'params': {'conversation': conversation, 'text': text},
      }),
    );
    final reply = jsonDecode(await lines.first) as Map<String, dynamic>;
    if (reply['error'] != null) throw StateError('seeding failed: ${reply['error']}');
  }
  await p.stdin.close();
  p.kill();
}

Duration _median(List<Duration> xs) => (xs.toList()..sort())[xs.length ~/ 2];

String _ms(Duration d) => '${(d.inMicroseconds / 1000).toStringAsFixed(2)} ms';

void main() {
  IntegrationTestWidgetsFlutterBinding.ensureInitialized();

  setUpAll(() async {
    await RustLib.init();
    await h.start();
  });
  tearDownAll(() => h.stop());

  testWidgets('messages(limit: 150) over JSON', (t) async {
    await h.register('sam', name: 'Sam');
    await h.register('bob', name: 'Bob');
    final dm = (await h.cliJson('bob', ['dm', 'sam']))['id'] as String;
    await _seed(dm, 150);
    await h.cli('sam', ['sync']);

    final client = await Client.open(profileDir: '${h.work.path}/sam', keyFile: '${h.work.path}/sam.key');
    final api = RpcPostApi(BridgeTransport(client));
    final samsDm = (await api.conversations()).single.id;
    final params = jsonEncode({'conversation': samsDm, 'limit': 150});

    // Warm up: the first call opens caches on both sides.
    expect(await api.messages(conversation: samsDm, limit: 150), hasLength(150));

    final total = <Duration>[], bridge = <Duration>[], decode = <Duration>[], typed = <Duration>[];
    var bytes = 0;
    for (var i = 0; i < 20; i++) {
      final sw = Stopwatch()..start();
      await api.messages(conversation: samsDm, limit: 150);
      total.add(sw.elapsed);

      sw.reset();
      final raw = await client.call(method: 'messages', params: params);
      bridge.add(sw.elapsed);
      bytes = raw.length;

      sw.reset();
      final list = (jsonDecode(raw) as List<Object?>).map((e) => Message.fromJson(e as Map<String, Object?>)).toList();
      decode.add(sw.elapsed);
      expect(list, hasLength(150));

      // The same through the old typed bridge, for comparison.
      sw.reset();
      await client.messages(conversation: samsDm, limit: 150);
      typed.add(sw.elapsed);
    }

    final mode = kReleaseMode ? 'release' : (kProfileMode ? 'profile' : 'debug');
    final report = [
      'api.messages(limit: 150), median of 20, $mode build',
      'whole call:              ${_ms(_median(total))}',
      '  bridge call (core + JSON text across): ${_ms(_median(bridge))}',
      '  decoding (jsonDecode + fromJson):      ${_ms(_median(decode))}',
      'old typed bridge, same call:           ${_ms(_median(typed))}',
      'JSON size: ${(bytes / 1024).toStringAsFixed(1)} KiB',
    ].join('\n');
    // ignore: avoid_print
    print(report);
    await File('build/api-timing-$mode.txt').writeAsString('$report\n');
    client.dispose();
  });
}
