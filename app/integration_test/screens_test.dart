// Drives the real app against a throwaway server filled with test accounts,
// and saves screenshots of its main screens. A smoke test of the whole
// stack, and a way to look at the UI without clicking through it.
//
// Run with tool/screens.sh, which builds the server and CLI first.

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
import 'package:post/ui/markup_text.dart';

import 'harness.dart';

const _shots = String.fromEnvironment('SHOTS', defaultValue: 'build/screens');

final h = Harness('screens');

/// Accounts, conversations and messages to look at.
Future<String> _seed() async {
  await h.register('sam', name: 'Sam', note: 'at the crag');
  await h.register('alice', name: 'Alice A', note: 'at the crag, back sunday');
  await h.register('bob', name: 'Bob B', note: 'driving');
  await h.register('carol', name: 'Carol C', note: 'deep work till 3');
  await h.register('devi', name: 'Devi');

  await h.cli('carol', ['send', '--to', 'sam', 'send me the topo?']);
  await h.cli('alice', ['send', '--to', 'sam', 'did you get the guidebook?']);
  await h.cli('sam', ['sync']);
  await h.cli('sam', ['send', '--to', 'alice', 'yes, the new edition']);
  await h.cli('alice', ['send', '--to', 'sam', 'see you saturday']);

  final group = await h.cliJson('alice', ['group', 'create', 'climbing', 'bob', 'carol', 'devi', 'sam']);
  final g = group['id'] as String;
  await h.cli('alice', ['send', '--conversation', g, 'saturday at the crag? forecast says dry all day']);
  for (final u in ['bob', 'carol', 'sam']) {
    await h.cli(u, ['sync']);
  }
  await h.cli('carol', ['send', '--conversation', g, 'in. bringing the new 70 m rope']);
  final drive = await h.cliJson('bob', ['send', '--conversation', g, 'I can drive, leaving at 9']);
  await h.cli('bob', ['edit', g, drive['id'] as String, 'I can drive, leaving at 8 from mine']);
  await h.cli('sam', ['send', '--conversation', g, 'count me in. who has the quickdraws?']);
  await h.cli('alice', ['send', '--conversation', g, 'me, 12 of them']);
  await h.cli('alice', ['send', '--conversation', g, 'and two spare belay devices']);
  await h.cli('carol', [
    'send',
    '--conversation',
    g,
    '**bold**, *italic*, __underline__, ~~strike~~, `code` and a ||secret||\n> a quote with **bold**\n```\nrope: 70 m\ndraws: 12\n```',
  ]);
  await h.cli('bob', ['send', '--to', 'sam', 'checked the safety number on the call']);
  await h.cli('bob', ['send', '--to', 'sam', 'numbers match']);
  return g;
}

/// Around the whole app, so a screenshot includes dialogs and menus.
final _screen = GlobalKey();

Future<void> _shot(WidgetTester t, String name) async {
  await settle(t);
  await _capture(t, name);
}

/// A screenshot as it is now, without waiting for anything to finish: for
/// frames of an animation.
Future<void> _capture(WidgetTester t, String name) async {
  await t.pump();
  final boundary = _screen.currentContext!.findRenderObject()! as RenderRepaintBoundary;
  final image = await boundary.toImage(pixelRatio: 1);
  final png = await image.toByteData(format: ui.ImageByteFormat.png);
  final file = File('$_shots/$name.png');
  await file.parent.create(recursive: true);
  await file.writeAsBytes(png!.buffer.asUint8List());
  // ignore: avoid_print
  print('screenshot: ${file.absolute.path}');
}

/// The laid-out text of a text field, found under its render object.
RenderEditable _editableIn(RenderObject root) {
  RenderEditable? found;
  void visit(RenderObject o) {
    if (found != null) return;
    if (o is RenderEditable) {
      found = o;
      return;
    }
    o.visitChildren(visit);
  }

  visit(root);
  return found!;
}

/// Wait until no menu or dialog is open over the main screen: a closing
/// one's barrier still takes taps.
Future<void> _noRouteOpen(WidgetTester t) async {
  final end = DateTime.now().add(const Duration(seconds: 3));
  while (DateTime.now().isBefore(end)) {
    if (!t.state<NavigatorState>(find.byType(Navigator).first).canPop()) return;
    await Future<void>.delayed(const Duration(milliseconds: 50));
    await t.pump();
  }
  await _capture(t, 'zz-route-open');
  final texts = find.byType(Text).evaluate().map((e) => (e.widget as Text).data).whereType<String>().join(' | ');
  fail('a menu or dialog stayed open; on screen: $texts');
}

void main() {
  final binding = IntegrationTestWidgetsFlutterBinding.ensureInitialized();
  binding.framePolicy = LiveTestWidgetsFlutterBindingFramePolicy.fullyLive;

  setUpAll(() async {
    await RustLib.init();
    await h.start();
  });

  tearDownAll(() => h.stop());

  testWidgets('main screens', (t) async {
    final group = await _seed();
    await t.pumpWidget(
      RepaintBoundary(
        key: _screen,
        child: PostApp(profileDir: '${h.work.path}/sam', keyFile: '${h.work.path}/sam.key'),
      ),
    );

    await waitFor(t, find.text('Alice A'));
    await h.online('alice', 'online');
    await h.online('bob', 'away');
    await h.online('carol', 'dnd');
    await settle(t, 3000);
    await _shot(t, '01-start');

    await tap(t, find.text('Alice A'));
    await _shot(t, '02-dm');

    // A new message leaves the ones on screen alone: the same widgets, not
    // rebuilt from scratch (which reset them and looked like a refresh).
    State tile(String text) => t.state(find.ancestor(of: find.text(text), matching: find.byWidgetPredicate((w) => w.runtimeType.toString() == '_MessageTile')));
    final before = tile('see you saturday');
    await h.cli('alice', ['send', '--to', 'sam', 'one more thing']);
    await waitFor(t, find.text('one more thing'));
    await settle(t);
    expect(identical(tile('see you saturday'), before), isTrue, reason: 'an existing message was rebuilt when a new one arrived');
    // The composer's text centres beside the send button: same height, same
    // middle, to the pixel (desktop density once moved it by four).
    final field = t.getRect(find.byWidgetPredicate((w) => w is TextField).last);
    final send = t.getRect(find.byTooltip('Send (Enter). Shift+Enter for a new line.'));
    final hint = t.getRect(find.text('Message Alice A'));
    expect(field.height, send.height);
    expect(hint.center.dy, closeTo(send.center.dy, 0.5));

    // Sending: the text flies from the composer into the history. Frames
    // along the way, then exactly one copy left, in the history.
    // The composer: the last field that takes typing (selectable text, like
    // the safety number, is read-only).
    final composer = t.widget<EditableText>(find.byWidgetPredicate((w) => w is EditableText && !w.readOnly).last);
    // Two lines: the composer shrinks back as it goes, and the text keeps
    // its line breaks all the way (it once re-wrapped mid-flight).
    const sent = 'flying in from the composer\non two lines';
    composer.controller.text = sent;
    await t.pump();
    await t.tap(find.byTooltip('Send (Enter). Shift+Enter for a new line.'));
    // Meanwhile the history only rises to make room: it once dipped as the
    // composer shrank before the message's row opened.
    final older = find.text('see you saturday');
    var lowest = t.getRect(older).top;
    // Frames of the flight along the way, once the message has appeared.
    final watch = Stopwatch()..start();
    int? arrived;
    var frame = 0;
    while (watch.elapsedMilliseconds < 1200) {
      await t.pump();
      final top = t.getRect(older).top;
      expect(top, lessThanOrEqualTo(lowest + 0.5), reason: 'the history moved down during a send');
      lowest = top;
      if (arrived == null && find.text(sent).evaluate().isNotEmpty) arrived = watch.elapsedMilliseconds;
      if (arrived != null && frame < 6 && watch.elapsedMilliseconds - arrived >= frame * 80) {
        await _capture(t, '02b-send-${frame * 80}');
        frame++;
      }
      await Future<void>.delayed(const Duration(milliseconds: 8));
    }
    await settle(t);
    expect(find.text(sent), findsOneWidget, reason: 'the flying copy is gone once it lands');

    await tap(t, find.text('climbing'));
    await h.cli('carol', ['typing', group]);
    await waitFor(t, find.textContaining('typing'));
    await _shot(t, '03-group');

    // Someone typing who then sends: their text rises from the typing line.
    await h.cli('carol', ['send', '--conversation', group, 'rising from the typing line']);
    await waitFor(t, find.text('rising from the typing line'));
    for (final ms in [0, 80, 160, 240, 320, 400]) {
      await Future<void>.delayed(Duration(milliseconds: ms == 0 ? 0 : 80));
      await _capture(t, '03b-arrive-$ms');
    }
    await settle(t);
    expect(find.text('rising from the typing line'), findsOneWidget, reason: 'the flying copy is gone once it lands');

    // Each person's messages arrive in the style they chose: frames of a
    // few, mid-effect, and each ends as plain text.
    for (final (who, style, text) in [
      ('carol', 'decode', 'decoding into **place**, `code` too'),
      ('bob', 'typewriter', 'typed out one letter at a time'),
      ('alice', 'wave', 'a ripple across the line'),
      ('carol', 'focus', 'sharpening out of the blur'),
    ]) {
      await h.cli(who, ['profile', '--arrival', style]);
      await settle(t, 1500);
      await h.cli(who, ['send', '--conversation', group, text]);
      await waitFor(t, find.textContaining(text.split(' ').first));
      final watch = Stopwatch()..start();
      for (var frame = 0; frame < 3; frame++) {
        await Future<void>.delayed(Duration(milliseconds: frame == 0 ? 120 : 200));
        await _capture(t, '03c-$style-$frame');
      }
      await settle(t);
      expect(watch.elapsedMilliseconds, lessThan(4000));
    }

    // A message's hover toolbar rides over the row above it, drawn on top of
    // it and taking the pointer there (the list paints older rows over newer
    // ones, and once drew the row above over the toolbar).
    final mouse = await t.createGesture(kind: PointerDeviceKind.mouse);
    await mouse.addPointer(location: Offset.zero);
    final ripple = find.text('a ripple across the line');
    await mouse.moveTo(t.getCenter(ripple));
    await settle(t, 400);
    final copy = find.byTooltip('Copy text');
    expect(copy, findsOneWidget, reason: 'only the hovered row shows its toolbar');
    final above = t.getRect(find.textContaining('typed out one letter')).bottom;
    final copyRect = t.getRect(copy);
    expect(copyRect.top, lessThan(t.getRect(ripple).top - 8), reason: 'the toolbar sits over the top edge of its row');
    final hit = HitTestResult();
    t.binding.hitTestInView(hit, copyRect.topCenter + const Offset(0, 2), t.view.viewId);
    final copyBox = t.renderObject(copy);
    expect(hit.path.any((e) => e.target == copyBox), isTrue, reason: 'the toolbar is on top of the row above it');
    await _shot(t, '03d-hover-toolbar');
    // Onto the toolbar, which keeps it showing; then away, and it goes.
    await mouse.moveTo(copyRect.center);
    await settle(t, 400);
    expect(copy, findsOneWidget, reason: 'moving onto the toolbar keeps it');
    await mouse.moveTo(Offset(copyRect.center.dx, above - 30));
    await settle(t, 400);
    await mouse.moveTo(Offset.zero);
    await settle(t, 400);
    expect(find.byTooltip('Copy text'), findsNothing, reason: 'the toolbar goes once the pointer leaves');
    await mouse.removePointer();

    await h.cli('carol', ['typing', group]);
    await waitFor(t, find.textContaining('typing'));

    await tap(t, find.textContaining('versions'));
    await _shot(t, '04-versions');

    // A message's own menu, on a right-click after dragging across its text.
    // (A right-click with a real selection once crashed the selection's own
    // menu; test input does not make a selection, so this cannot catch that.)
    final quickdraws = t.getRect(find.text('count me in. who has the quickdraws?'));
    await t.dragFrom(quickdraws.centerLeft + const Offset(4, 0), const Offset(120, 0), kind: PointerDeviceKind.mouse);
    await settle(t, 300);
    await t.tapAt(quickdraws.centerLeft + const Offset(40, 0), buttons: kSecondaryButton, kind: PointerDeviceKind.mouse);
    await waitFor(t, find.text('Delete message'));
    await _shot(t, '04b-message-menu');
    await t.sendKeyEvent(LogicalKeyboardKey.escape);
    await settle(t);

    // The menu has gone, barrier and all.
    await _noRouteOpen(t);
    // Into the composer: the field itself (its placeholder takes no taps,
    // and a tap on it sometimes missed the field).
    await tap(t, find.byWidgetPredicate((w) => w is EditableText && !w.readOnly).last);
    expect(
      t.widget<EditableText>(find.byWidgetPredicate((w) => w is EditableText && !w.readOnly).last).focusNode.hasFocus,
      isTrue,
      reason: 'the composer has the keyboard',
    );
    await t.sendKeyEvent(LogicalKeyboardKey.arrowUp);
    await _shot(t, '05-editing');
    await t.sendKeyEvent(LogicalKeyboardKey.escape);
    await settle(t);

    // Typing with the composer unfocused still types into it.
    FocusManager.instance.primaryFocus?.unfocus();
    await settle(t, 200);
    // One key: after it the composer has the keyboard, and the test's
    // simulated keys never reach a text field as text (real typing does).
    await t.sendKeyDownEvent(LogicalKeyboardKey.keyH, character: 'h');
    await t.sendKeyUpEvent(LogicalKeyboardKey.keyH);
    await waitFor(t, find.byWidgetPredicate((w) => w is EditableText && w.controller.text == 'h'));
    await _shot(t, '05b-typed');

    // Ctrl+B bolds the selection; the composer shows formatting as typed,
    // without changing its height.
    final typing = t.widget<EditableText>(find.byWidgetPredicate((w) => w is EditableText && !w.readOnly).last).controller;
    final oneLine = t.getRect(find.byWidgetPredicate((w) => w is TextField).last).height;
    typing.value = const TextEditingValue(text: 'make this bold, `this` code', selection: TextSelection(baseOffset: 10, extentOffset: 14));
    await t.pump();
    await t.sendKeyDownEvent(LogicalKeyboardKey.controlLeft);
    await t.sendKeyEvent(LogicalKeyboardKey.keyB);
    await t.sendKeyUpEvent(LogicalKeyboardKey.controlLeft);
    expect(typing.text, 'make this **bold**, `this` code');
    // The toolbar floats over the selection, bold lit; its buttons format
    // without taking the selection away.
    await waitFor(t, find.byTooltip('Italic (Ctrl+I)'));
    await _shot(t, '05c-formatting');
    await t.tap(find.byTooltip('Italic (Ctrl+I)'));
    await t.pump();
    expect(typing.text, 'make this ***bold***, `this` code');
    expect(typing.selection.textInside(typing.text), 'bold', reason: 'the selection stays on the text');

    // Spelling: a misspelled word gets its red underline once you move on
    // from it (the dictionary for the system's language downloads first).
    typing.value = const TextEditingValue(text: 'this sentense has a typo ', selection: TextSelection.collapsed(offset: 25));
    final spelled = typing as MarkupEditingController;
    await waitFor(t, find.byWidgetPredicate((w) => w is EditableText && spelled.misspelled.isNotEmpty), timeout: const Duration(seconds: 60));
    final marked = spelled.misspelled;
    expect([for (final r in marked) r.textInside(typing.text)], ['sentense']);
    await _shot(t, '05d-spelling');

    // Right-clicking it offers corrections; picking one replaces the word.
    final editable = _editableIn(t.renderObject(find.byWidgetPredicate((w) => w is EditableText && !w.readOnly).last));
    final box = editable.getBoxesForSelection(const TextSelection(baseOffset: 5, extentOffset: 13)).first.toRect();
    final pointer = editable.localToGlobal(box.center);
    await t.tapAt(pointer, buttons: kSecondaryButton, kind: PointerDeviceKind.mouse);
    await waitFor(t, find.text('Add to dictionary'));
    // Over the composer, at the bottom of the window, it opens upward, and
    // stays above the pointer on every frame of opening (it once started
    // below and flipped up as it grew).
    final menu = find.ancestor(of: find.text('Add to dictionary'), matching: find.byType(Material)).first;
    final opening = Stopwatch()..start();
    while (opening.elapsedMilliseconds < 500) {
      expect(t.getRect(menu).bottom, lessThanOrEqualTo(pointer.dy + 1), reason: 'the menu went below the pointer while opening');
      await t.pump();
    }
    await waitFor(t, find.text('sentence'));
    await _shot(t, '05e-spelling-menu');
    await t.tap(find.text('sentence'));
    await settle(t, 300);
    expect(typing.text, 'this sentence has a typo ');
    expect(t.getRect(find.byWidgetPredicate((w) => w is TextField).last).height, oneLine, reason: 'formatting keeps the composer one line tall');
    typing.clear();

    // Search: the box's suggestions open under it, lined up with it from the
    // first frame; from: offers people; results, and opening one.
    typing.clear();
    await settle(t, 300);
    final searchField = find.byWidgetPredicate((w) => w is TextField && w.focusNode?.debugLabel == 'search');
    await t.tap(searchField);
    for (var frame = 0; frame < 4; frame++) {
      await t.pump();
      await _capture(t, '11a-search-open-$frame');
      await Future<void>.delayed(const Duration(milliseconds: 60));
    }
    await _shot(t, '11b-search-filters');
    final searchBox = t.getRect(find.ancestor(of: searchField, matching: find.byType(AnimatedContainer)).first);
    final suggestions = t.getRect(
      find.ancestor(of: find.text('Filters'), matching: find.byWidgetPredicate((w) => w is Container && w.constraints?.maxWidth == 340)),
    );
    expect(suggestions.right, closeTo(searchBox.right, 1), reason: 'the suggestions line up with the box');
    // Its placeholder sits in the middle of it, to the pixel.
    final searchHint = t.getRect(find.text('Search (Ctrl+F)'));
    expect(searchHint.center.dy, closeTo(searchBox.center.dy, 0.5), reason: 'the search text is centred in the box');
    expect(searchBox.height, 36, reason: 'one line is the box at its own height');
    // Set directly: simulated keys never reach a text field as text.
    final searchText = t.widget<EditableText>(find.descendant(of: searchField, matching: find.byType(EditableText))).controller;
    searchText.value = const TextEditingValue(text: 'from:', selection: TextSelection.collapsed(offset: 5));
    await _shot(t, '11c-search-from');
    searchText.value = const TextEditingValue(text: 'rope', selection: TextSelection.collapsed(offset: 4));
    await t.pump();
    await t.sendKeyEvent(LogicalKeyboardKey.enter);
    await waitFor(t, find.textContaining('result'));
    await _shot(t, '11d-search-results');
    await t.tap(find.textContaining('bringing the new 70 m rope').last);
    await _shot(t, '11e-search-jump');

    // A chip and a long search wrap: the box grows down, never wider.
    await t.tap(searchField);
    searchText.value = const TextEditingValue(text: 'from:', selection: TextSelection.collapsed(offset: 5));
    await settle(t, 300);
    await t.sendKeyEvent(LogicalKeyboardKey.enter);
    const long = 'the long rope for the north face and two spare belay devices please';
    searchText.value = const TextEditingValue(
      text: long,
      selection: TextSelection.collapsed(offset: long.length),
    );
    await _shot(t, '11f-search-wrap');
    final wrapped = t.getRect(find.ancestor(of: searchField, matching: find.byType(AnimatedContainer)).first);
    expect(wrapped.width, lessThanOrEqualTo(281), reason: 'the box keeps its width');
    expect(wrapped.height, greaterThan(40), reason: 'the box grows to fit its lines');
    await t.sendKeyEvent(LogicalKeyboardKey.escape);
    await settle(t);

    await tap(t, find.text('Bob B'));
    await _shot(t, '06-dm-unread-read');

    // A right-click menu opens with its corner at the pointer.
    final click = t.getCenter(find.text('Carol C').first);
    await t.tapAt(click, buttons: kSecondaryButton, kind: PointerDeviceKind.mouse);
    await waitFor(t, find.text('Set nickname'));
    await _shot(t, '06b-context-menu');
    final item = t.getRect(find.byWidgetPredicate((w) => w is PMenuItem).first);
    expect(item.height, 32, reason: 'compact entries');
    expect(item.left - click.dx, inInclusiveRange(0, 30), reason: 'menu starts at the pointer, not away from it');
    expect(item.top - click.dy, inInclusiveRange(0, 30), reason: 'menu starts at the pointer, not away from it');
    await t.sendKeyEvent(LogicalKeyboardKey.escape);
    await settle(t);

    // The status menu unrolls upward from the card.
    await t.tap(find.text('Sam'));
    for (final ms in [0, 60, 120]) {
      await Future<void>.delayed(Duration(milliseconds: ms == 0 ? 0 : 60));
      await _capture(t, '07a-presence-open-$ms');
    }
    await _shot(t, '07-presence-menu');
    final card = t.getRect(find.text('Sam'));
    final away = t.getRect(find.text('Away'));
    expect(away.bottom, lessThan(card.top), reason: 'the status menu opens above the card');
    await t.sendKeyEvent(LogicalKeyboardKey.escape);
    await settle(t);
    expect(find.text('Away'), findsNothing, reason: 'Escape closes the status menu');

    await tap(t, find.byTooltip('New group'));
    await _shot(t, '08-new-group');
    await t.sendKeyEvent(LogicalKeyboardKey.escape);
    await settle(t);

    await tap(t, find.byTooltip('Settings'));
    await _shot(t, '09-settings-profile');
    await t.scrollUntilVisible(find.text('How your messages arrive'), 300, scrollable: find.byType(Scrollable).last);
    await t.drag(find.byType(Scrollable).last, const Offset(0, -400));
    await _capture(t, '09b-arrival-picker');
    await Future<void>.delayed(const Duration(milliseconds: 500));
    await _capture(t, '09c-arrival-picker');
    await tap(t, find.text('Spelling'));
    await _shot(t, '09d-settings-spelling');
    await tap(t, find.text('Devices'));
    await _shot(t, '10-settings-devices');
    await t.sendKeyEvent(LogicalKeyboardKey.escape);
    await settle(t);
  });
}
