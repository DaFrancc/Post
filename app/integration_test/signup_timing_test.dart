// Frame timings around the welcome screen's two buttons, in a profile build:
// what a user would see as a hitch. One scenario per run (SCENARIO=create or
// link), so neither benefits from the other having warmed up the app.
//
// Run with tool/timing.sh.

import 'dart:io';
import 'dart:ui';

import 'package:flutter/rendering.dart';
import 'package:flutter/scheduler.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:integration_test/integration_test.dart';
import 'package:post/main.dart';
import 'package:post/src/rust/frb_generated.dart';

import 'harness.dart';

const _scenario = String.fromEnvironment('SCENARIO', defaultValue: 'create');

final h = Harness('timing');

/// Each frame as `vsync gap / build / raster`, in milliseconds.
String _report(List<FrameTiming> frames) {
  final b = StringBuffer();
  int? last;
  var worstGap = 0.0, worstBuild = 0.0, worstRaster = 0.0;
  for (final (i, f) in frames.indexed) {
    final vsync = f.timestampInMicroseconds(FramePhase.vsyncStart);
    final gap = last == null ? 0.0 : (vsync - last) / 1000;
    last = vsync;
    final build = f.buildDuration.inMicroseconds / 1000, raster = f.rasterDuration.inMicroseconds / 1000;
    if (gap > worstGap) worstGap = gap;
    if (build > worstBuild) worstBuild = build;
    if (raster > worstRaster) worstRaster = raster;
    if (gap > 20 || build > 8 || raster > 8) {
      b.writeln('  frame $i: gap ${gap.toStringAsFixed(1)}  build ${build.toStringAsFixed(1)}  raster ${raster.toStringAsFixed(1)}');
    }
  }
  return 'frames ${frames.length}, worst gap ${worstGap.toStringAsFixed(1)} ms, worst build ${worstBuild.toStringAsFixed(1)} ms, '
      'worst raster ${worstRaster.toStringAsFixed(1)} ms\n$b';
}

/// Put text in the `i`th field. Simulated typing (enterText) does not reach
/// fields under `flutter drive`; setting the controller does the same here.
void _fill(WidgetTester t, int i, String text) => t.widget<EditableText>(find.byType(EditableText).at(i)).controller.text = text;

/// The costliest timeline events: what the slow frames spent their time on.
/// Widget builds show by widget name (debugProfileBuildsEnabled).
String _topEvents(Map<String, dynamic> timeline) {
  final events = (timeline['traceEvents'] as List).cast<Map<String, dynamic>>();
  final open = <String, List<Map<String, dynamic>>>{};
  final spans = <(String, double)>[];
  for (final e in events) {
    final key = '${e['tid']}';
    switch (e['ph']) {
      case 'X':
        spans.add((e['name'] as String, ((e['dur'] ?? 0) as num) / 1000));
      case 'B':
        (open[key] ??= []).add(e);
      case 'E':
        final stack = open[key];
        if (stack != null && stack.isNotEmpty) {
          final b = stack.removeLast();
          spans.add((b['name'] as String, ((e['ts'] as num) - (b['ts'] as num)) / 1000));
        }
    }
  }
  spans.sort((a, b) => b.$2.compareTo(a.$2));
  return spans.take(70).map((s) => '  ${s.$2.toStringAsFixed(1).padLeft(6)} ms  ${s.$1}').join('\n');
}

void main() {
  final binding = IntegrationTestWidgetsFlutterBinding.ensureInitialized();
  binding.framePolicy = LiveTestWidgetsFlutterBindingFramePolicy.fullyLive;
  debugProfileBuildsEnabled = true;
  debugProfileLayoutsEnabled = true;

  setUpAll(() async {
    await RustLib.init();
    await h.start();
  });

  tearDownAll(() => h.stop());

  testWidgets('welcome buttons: $_scenario', (t) async {
    final key = await h.keyFor('me');
    await t.pumpWidget(PostApp(profileDir: '${h.work.path}/me', keyFile: key));
    await waitFor(t, find.text('Link to my account'));
    await settle(t, 1500);

    final frames = <FrameTiming>[];
    void collect(List<FrameTiming> f) => frames.addAll(f);

    final String report;
    if (_scenario == 'create') {
      await tap(t, find.text('Create an account'));
      await settle(t);
      _fill(t, 0, h.url);
      _fill(t, 1, await h.invite());
      _fill(t, 2, 'timing');
      await settle(t, 800);
      SchedulerBinding.instance.addTimingsCallback(collect);
      var shown = 0, atHome = 0;
      await binding.traceAction(() async {
        final pressed = DateTime.now();
        await t.tap(find.text('Create account'));
        await waitFor(t, find.text('Messages'));
        shown = DateTime.now().difference(pressed).inMilliseconds;
        atHome = frames.length;
        await settle(t, 600);
      }, reportKey: 'timeline');
      SchedulerBinding.instance.removeTimingsCallback(collect);
      report =
          'create: main screen after $shown ms (frame $atHome)\n${_report(frames)}\ncostliest:\n'
          '${_topEvents(binding.reportData!['timeline'] as Map<String, dynamic>)}';
    } else {
      await tap(t, find.text('Link to my account'));
      await settle(t);
      _fill(t, 0, h.url);
      await settle(t, 800);
      SchedulerBinding.instance.addTimingsCallback(collect);
      var shown = 0, atCode = 0;
      await binding.traceAction(() async {
        final pressed = DateTime.now();
        await t.tap(find.text('Get a link code'));
        await waitFor(t, find.text('Enter this code on your other device'));
        shown = DateTime.now().difference(pressed).inMilliseconds;
        atCode = frames.length;
        await settle(t, 600);
      }, reportKey: 'timeline');
      SchedulerBinding.instance.removeTimingsCallback(collect);
      report =
          'link: code screen after $shown ms (frame $atCode)\n${_report(frames)}\ncostliest:\n'
          '${_topEvents(binding.reportData!['timeline'] as Map<String, dynamic>)}';
    }
    final out = File('build/timing-$_scenario.txt');
    await out.writeAsString(report);
    // ignore: avoid_print
    print(report);
  });
}
