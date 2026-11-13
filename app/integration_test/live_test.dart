// Typing and presence reaching the app (`tool/screens.sh live`): Bob, on
// the command line with a profile picture, types and changes his status;
// Sam sees both in the app.

import 'dart:io';
import 'dart:ui' as ui;

import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:integration_test/integration_test.dart';
import 'package:post/main.dart';
import 'package:post/src/rust/frb_generated.dart';

import 'harness.dart';

final h = Harness('live');

void main() {
  final binding = IntegrationTestWidgetsFlutterBinding.ensureInitialized();
  binding.framePolicy = LiveTestWidgetsFlutterBindingFramePolicy.fullyLive;

  setUpAll(() async {
    await RustLib.init();
    await h.start();
  });
  tearDownAll(() => h.stop());

  testWidgets('typing and presence reach the app', (t) async {
    await h.register('sam', name: 'Sam');
    await h.register('bob', name: 'Bob B');
    await h.cli('bob', ['send', '--to', 'sam', 'hello']);
    // A picture, so connecting also fetches pictures.
    final recorder = ui.PictureRecorder();
    Canvas(recorder).drawRect(const Rect.fromLTWH(0, 0, 600, 600), Paint()..color = const Color(0xFF6A3E9E));
    final png = await (await recorder.endRecording().toImage(600, 600)).toByteData(format: ui.ImageByteFormat.png);
    final file = File('${h.work.path}/face.png');
    await file.writeAsBytes(png!.buffer.asUint8List());
    await h.cli('bob', ['picture', 'set', file.path]);

    await t.pumpWidget(PostApp(key: UniqueKey(), profileDir: '${h.work.path}/sam', keyFile: '${h.work.path}/sam.key'));
    await tap(t, find.text('Bob B'));
    await waitFor(t, find.text('hello'));
    // Give the app time to connect and listen.
    await settle(t, 3000);

    // Bob, kept online and away.
    await h.online('bob', 'away');
    await waitFor(t, find.textContaining('Away'), timeout: const Duration(seconds: 20));

    final convs = await h.cliJson('bob', ['conversations']) as List<dynamic>;
    final dm = (convs.first as Map<String, dynamic>)['id'] as String;
    await h.cli('bob', ['typing', dm]);
    await waitFor(t, find.textContaining('typing'), timeout: const Duration(seconds: 20));
  });
}
