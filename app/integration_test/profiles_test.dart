// Profile pictures and banners, for quick runs: `tool/screens.sh profiles`.
// Bob sets a picture, a banner and a banner colour from the command line;
// Sam sees them in the conversation list, the chat and Bob's card, and
// her own page in Settings, where nothing is set yet, shows the defaults.

import 'dart:io';
import 'dart:ui' as ui;

import 'package:flutter/material.dart';
import 'package:flutter/rendering.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:integration_test/integration_test.dart';
import 'package:post/main.dart';
import 'package:post/src/rust/frb_generated.dart';
import 'package:post/ui/controls.dart';

import 'harness.dart';

const _shots = String.fromEnvironment('SHOTS', defaultValue: 'build/screens');
final h = Harness('profiles');
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

/// A PNG of [width]×[height]: soft colour bands, like a photo of a sky.
Future<String> _image(String name, int width, int height, List<Color> colors) async {
  final recorder = ui.PictureRecorder();
  final canvas = Canvas(recorder);
  final rect = Rect.fromLTWH(0, 0, width.toDouble(), height.toDouble());
  final stops = [for (var i = 0; i < colors.length; i++) i / (colors.length - 1)];
  canvas.drawRect(rect, Paint()..shader = ui.Gradient.linear(rect.topLeft, rect.bottomRight, colors, stops));
  canvas.drawCircle(Offset(width * 0.5, height * 0.42), height * 0.2, Paint()..color = const Color(0xFFF6D2B8));
  final image = await recorder.endRecording().toImage(width, height);
  final png = await image.toByteData(format: ui.ImageByteFormat.png);
  final file = File('${h.work.path}/$name.png');
  await file.writeAsBytes(png!.buffer.asUint8List());
  return file.path;
}

void main() {
  final binding = IntegrationTestWidgetsFlutterBinding.ensureInitialized();
  binding.framePolicy = LiveTestWidgetsFlutterBindingFramePolicy.fullyLive;

  setUpAll(() async {
    await RustLib.init();
    await h.start();
  });
  tearDownAll(() => h.stop());

  testWidgets('profiles', (t) async {
    await h.register('sam', name: 'Sam');
    await h.register('bob', name: 'Bob B', note: 'in the mountains till Sunday');
    await h.cli('bob', ['send', '--to', 'sam', 'booked the hut for saturday!']);
    await h.cli('bob', ['picture', 'set', await _image('face', 900, 900, const [Color(0xFF3A2C4F), Color(0xFF6A3E9E)])]);
    await h.cli('bob', ['banner', 'set', await _image('sky', 2400, 900, const [Color(0xFF2B3B8F), Color(0xFF6A3E9E), Color(0xFFC2588E)])]);
    // Taller than the card shows at first.
    await h.cli('bob', [
      'profile',
      '--bio',
      'Climber, bad photographer, **worse cook**.\nAsk @sam about the hut.\n\n'
          'Routes this year:\n- the north ridge\n- the long traverse\n- the slab, again\n\n_Back on Sunday._',
    ]);

    await _open(t, 'sam');
    await waitFor(t, find.text('Bob B'));
    // Bob's picture arrives: no default face left for him in the list.
    await waitFor(t, find.byWidgetPredicate((w) => w is PAvatar && w.colorKey == 'bob' && w.image != null));
    await _shot(t, 'profiles-00-list');
    await tap(t, find.text('Bob B').first);
    await waitFor(t, find.textContaining('booked the hut'));
    await _shot(t, 'profiles-01-chat');

    // His card: the banner across the top, the picture on its edge.
    await tap(t, find.byWidgetPredicate((w) => w is PAvatar && w.colorKey == 'bob' && w.size == 40).first);
    await waitFor(t, find.text('@bob'));
    // His bio, formatted, cut off with a way to see the rest.
    await waitFor(t, find.text('ABOUT ME'));
    expect(find.text('Show more'), findsOneWidget);
    await _shot(t, 'profiles-02-card');
    await tap(t, find.text('Show more'));
    await waitFor(t, find.text('Show less'));
    await _shot(t, 'profiles-02-card-bio');
    // Closed with a click outside it.
    await t.tapAt(const Offset(220, 600));
    await settle(t);
    expect(find.text('Messages arrive: Bubble'), findsNothing);

    // Sam's own page: nothing set, so the default face and her colour.
    await tap(t, find.byTooltip('Settings'));
    await waitFor(t, find.text('Add a picture'));
    expect(find.text('Banner colour'), findsOneWidget);
    await _shot(t, 'profiles-03-settings');

    // She writes a bio over two lines, and it is saved as typed.
    final bio = find.byWidgetPredicate((w) => w is TextField && w.minLines == 3);
    await t.ensureVisible(bio);
    await t.enterText(bio, 'Mostly **reading**.\nSometimes climbing.');
    await settle(t);
    await _shot(t, 'profiles-04-bio');
    await tap(t, find.text('Save profile'));
    await waitFor(t, find.text('Profile saved and shared with your conversations.'));
    // The page reloads her profile from the core after saving.
    expect(t.widget<TextField>(bio).controller!.text, 'Mostly **reading**.\nSometimes climbing.');
  });
}
