// How smoothly the profile colours' angle slider drags: frame times while
// it moves, for `tool/screens.sh slider_perf`.

import 'dart:ui' as ui;

import 'package:flutter/material.dart';
import 'package:flutter/scheduler.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:integration_test/integration_test.dart';
import 'package:post/main.dart';
import 'package:post/src/rust/frb_generated.dart';

import 'harness.dart';

final h = Harness('slider_perf');

void main() {
  final binding = IntegrationTestWidgetsFlutterBinding.ensureInitialized();
  binding.framePolicy = LiveTestWidgetsFlutterBindingFramePolicy.fullyLive;

  setUpAll(() async {
    await RustLib.init();
    await h.start();
  });
  tearDownAll(() => h.stop());

  testWidgets('the angle slider drags smoothly', (t) async {
    await h.register('sam', name: 'Sam');
    await t.pumpWidget(PostApp(key: UniqueKey(), profileDir: '${h.work.path}/sam', keyFile: '${h.work.path}/sam.key'));
    await tap(t, find.byTooltip('Settings'));
    await waitFor(t, find.text('Add a picture'));
    final themeFill = find.byKey(const ValueKey('theme-fill'));
    final page = find.descendant(of: find.byType(Dialog), matching: find.byType(Scrollable)).first;
    await t.scrollUntilVisible(themeFill, 200, scrollable: page);
    final gradient = find.descendant(of: themeFill, matching: find.text('Gradient'));
    await t.ensureVisible(gradient);
    await settle(t);
    await tap(t, gradient);
    final slider = find.descendant(of: themeFill, matching: find.byType(Slider));
    await t.ensureVisible(slider);
    await settle(t);

    double ms(Duration d) => d.inMicroseconds / 1000;
    List<double> sorted(Iterable<double> xs) => xs.toList()..sort();
    String stats(List<double> xs) => xs.isEmpty
        ? 'none'
        : 'median ${xs[xs.length ~/ 2].toStringAsFixed(2)} ms, 90th ${xs[(xs.length * 0.9).floor()].toStringAsFixed(2)} ms, max ${xs.last.toStringAsFixed(2)} ms';

    // The page on its own, untouched, for comparison.
    final idle = <ui.FrameTiming>[];
    void collectIdle(List<ui.FrameTiming> ts) => idle.addAll(ts);
    SchedulerBinding.instance.addTimingsCallback(collectIdle);
    await settle(t, 1500);
    SchedulerBinding.instance.removeTimingsCallback(collectIdle);
    // ignore: avoid_print
    print('PERF idle frames ${idle.length}, build: ${stats(sorted(idle.map((f) => ms(f.buildDuration))))}');

    final timings = <ui.FrameTiming>[];
    void collect(List<ui.FrameTiming> ts) => timings.addAll(ts);
    SchedulerBinding.instance.addTimingsCallback(collect);
    final box = t.getRect(slider);
    final drag = await t.startGesture(box.centerLeft + const Offset(20, 0));
    for (var i = 0; i < 180; i++) {
      await drag.moveBy(Offset((box.width - 40) / 90 * (i < 90 ? 1 : -1), 0));
      await t.pump(const Duration(milliseconds: 8));
    }
    await drag.up();
    await settle(t);
    SchedulerBinding.instance.removeTimingsCallback(collect);

    final build = sorted(timings.map((f) => ms(f.buildDuration)));
    final raster = sorted(timings.map((f) => ms(f.rasterDuration)));
    // ignore: avoid_print
    print('PERF frames ${timings.length}\nPERF build: ${stats(build)}\nPERF raster: ${stats(raster)}');
  });
}
