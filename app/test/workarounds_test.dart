// Workarounds for bugs in Flutter itself, each to be removed once a Flutter
// release fixes it. This test fails as soon as Flutter is upgraded, as the
// reminder: check each workaround below against the new version, remove
// the ones it no longer needs, and move the version on.

import 'dart:io';

import 'package:flutter_test/flutter_test.dart';

/// The Dart that came with the Flutter every workaround was last checked
/// against (Flutter 3.47).
const _checkedWith = '3.13.7';

void main() {
  test('every Flutter workaround is rechecked after a Flutter upgrade', () {
    final dart = Platform.version.split(' ').first;
    expect(
      dart,
      _checkedWith,
      reason:
          'Flutter was upgraded (Dart $dart, workarounds last checked with $_checkedWith). Check each, then set _checkedWith to $dart:\n'
          '\n'
          '1. Wayland showing the frame before the newest (flutter/flutter#195821): '
          'repaint_once_more in linux/runner/my_application.cc paints once more after each paint. '
          'Later Flutter draws through a Wayland subsurface instead (flutter/flutter#195287). '
          'To check: remove it, build, and on Wayland move the pointer fast up and down the '
          'Settings menu, then stop. If the item under the pointer is always the lit one, '
          'the workaround can go.\n',
    );
  });
}
