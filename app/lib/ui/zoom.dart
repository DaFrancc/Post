// Zooming the whole app, as a browser or Discord does: Ctrl+= and Ctrl+-
// step through set levels, Ctrl+0 goes back to 100%. Everything scales
// together (layout, text, icons), not just the text.

import 'dart:async';

import 'package:flutter/material.dart';
import 'package:flutter/services.dart';

import '../theme.dart';

/// The app's zoom, as a factor: 1 is 100%.
final appZoom = ValueNotifier<double>(1);

/// The levels the shortcuts step through.
const zoomLevels = [0.5, 0.67, 0.75, 0.8, 0.9, 1.0, 1.1, 1.25, 1.5, 1.75, 2.0];

double zoomedIn(double z) => zoomLevels.firstWhere((l) => l > z + 0.001, orElse: () => zoomLevels.last);
double zoomedOut(double z) => zoomLevels.lastWhere((l) => l < z - 0.001, orElse: () => zoomLevels.first);

String zoomLabel(double z) => '${(z * 100).round()}%';

/// Draws [child] at [appZoom], handles the zoom shortcuts, and shows the
/// level for a moment when it changes.
class Zoom extends StatefulWidget {
  const Zoom({super.key, required this.child});

  final Widget child;

  @override
  State<Zoom> createState() => _ZoomState();
}

class _ZoomState extends State<Zoom> {
  bool _showLevel = false;
  Timer? _hideLevel;

  @override
  void initState() {
    super.initState();
    appZoom.addListener(_changed);
    HardwareKeyboard.instance.addHandler(_key);
  }

  @override
  void dispose() {
    appZoom.removeListener(_changed);
    HardwareKeyboard.instance.removeHandler(_key);
    _hideLevel?.cancel();
    super.dispose();
  }

  void _changed() {
    setState(() => _showLevel = true);
    _hideLevel?.cancel();
    _hideLevel = Timer(const Duration(milliseconds: 1200), () {
      if (mounted) setState(() => _showLevel = false);
    });
  }

  bool _key(KeyEvent e) {
    if (e is! KeyDownEvent && e is! KeyRepeatEvent) return false;
    final keys = HardwareKeyboard.instance;
    if (!keys.isControlPressed || keys.isAltPressed || keys.isMetaPressed) return false;
    final z = appZoom.value;
    final next = switch (e.logicalKey) {
      LogicalKeyboardKey.equal || LogicalKeyboardKey.add || LogicalKeyboardKey.numpadAdd => zoomedIn(z),
      LogicalKeyboardKey.minus || LogicalKeyboardKey.numpadSubtract => zoomedOut(z),
      LogicalKeyboardKey.digit0 || LogicalKeyboardKey.numpad0 => 1.0,
      _ => null,
    };
    if (next == null) return false;
    if (next != z) appZoom.value = next;
    return true;
  }

  @override
  Widget build(BuildContext context) {
    final mq = MediaQuery.of(context);
    final z = appZoom.value;
    final size = mq.size / z;
    // The same tree at every zoom, so nothing inside loses its state when
    // the level changes.
    return Stack(
      children: [
        // The app lays out in a window 1/z the size and is drawn z times
        // larger, filling the real one. Pointer positions go through the
        // same transform, so clicks land where they look.
        OverflowBox(
          alignment: Alignment.topLeft,
          minWidth: size.width,
          maxWidth: size.width,
          minHeight: size.height,
          maxHeight: size.height,
          child: Transform.scale(
            scale: z,
            alignment: Alignment.topLeft,
            child: MediaQuery(
              data: mq.copyWith(
                size: size,
                devicePixelRatio: mq.devicePixelRatio * z,
                padding: mq.padding / z,
                viewPadding: mq.viewPadding / z,
                viewInsets: mq.viewInsets / z,
              ),
              child: widget.child,
            ),
          ),
        ),
        // The level, briefly, at the top: drawn at 100% so it reads the same
        // at any zoom.
        Positioned(
          top: 44,
          left: 0,
          right: 0,
          child: IgnorePointer(
            child: Center(
              child: AnimatedOpacity(
                opacity: _showLevel ? 1 : 0,
                duration: PMotion.normal,
                curve: PMotion.curve,
                child: Container(
                  padding: const EdgeInsets.symmetric(horizontal: 14, vertical: 7),
                  decoration: BoxDecoration(
                    color: PColors.panelRaised,
                    border: Border.all(color: PColors.borderStrong),
                    borderRadius: BorderRadius.circular(12),
                    boxShadow: const [BoxShadow(color: Color(0x66000000), blurRadius: 14, offset: Offset(0, 4))],
                  ),
                  child: Text(
                    zoomLabel(z),
                    style: const TextStyle(
                      fontFamily: PType.body,
                      fontSize: 13.5,
                      fontWeight: FontWeight.w600,
                      color: PColors.text,
                      decoration: TextDecoration.none,
                    ),
                  ),
                ),
              ),
            ),
          ),
        ),
      ],
    );
  }
}
