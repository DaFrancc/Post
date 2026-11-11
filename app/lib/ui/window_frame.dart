// The window's own frame: Post draws its top bar, rounded corners and
// shadow instead of the system's. Native window operations go through the
// runner's "post/window" channel (linux/runner/window_channel.cc).
//
// When not maximized, the window keeps a transparent margin around the
// rounded body: the shadow is drawn there, and the margin is where the edges
// are grabbed to resize, as native client-side decorations do. Maximized, the
// body fills the window with square corners. Where the display cannot do
// transparency, the frame is square and fills the window.

import 'package:flutter/gestures.dart';
import 'package:flutter/material.dart';
import 'package:flutter/services.dart';

import '../theme.dart';

const _channel = MethodChannel('post/window');

/// Width of the transparent margin around the window body. Must match the
/// runner's minimum-size padding.
const _margin = 12.0;
const _radius = 14.0;

/// GTK's GdkWindowEdge numbering, which the runner expects.
enum _Edge { topLeft, top, topRight, left, right, bottomLeft, bottom, bottomRight }

class WindowFrame extends StatefulWidget {
  const WindowFrame({super.key, required this.child});

  final Widget child;

  @override
  State<WindowFrame> createState() => _WindowFrameState();
}

class _WindowFrameState extends State<WindowFrame> {
  bool _maximized = false;
  bool _composited = false;

  @override
  void initState() {
    super.initState();
    _channel.setMethodCallHandler((call) async {
      if (call.method == 'maximizedChanged' && call.arguments is bool && mounted) {
        setState(() => _maximized = call.arguments as bool);
      }
    });
    _load();
  }

  Future<void> _load() async {
    final maximized = await _channel.invokeMethod<bool>('isMaximized') ?? false;
    final composited = await _channel.invokeMethod<bool>('isComposited') ?? false;
    if (!mounted) return;
    setState(() {
      _maximized = maximized;
      _composited = composited;
    });
  }

  @override
  void dispose() {
    _channel.setMethodCallHandler(null);
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    final framed = _composited && !_maximized;
    final radius = BorderRadius.circular(framed ? _radius : 0);
    final body = DecoratedBox(
      decoration: BoxDecoration(
        color: PColors.ground,
        borderRadius: radius,
        border: framed ? Border.all(color: PColors.borderStrong.withValues(alpha: 0.7)) : null,
        boxShadow: framed ? const [BoxShadow(color: Color(0x99000000), blurRadius: 16, offset: Offset(0, 3))] : null,
      ),
      child: ClipRRect(
        borderRadius: radius,
        child: DecoratedBox(
          // A faint glow of the brand colours from the top left, seen in the
          // gaps between panels.
          decoration: const BoxDecoration(gradient: PColors.groundGlow),
          child: Column(
            children: [
              _TitleBar(maximized: _maximized),
              Expanded(child: widget.child),
            ],
          ),
        ),
      ),
    );
    if (_maximized) return body;
    return Stack(
      children: [
        Padding(padding: EdgeInsets.all(framed ? _margin : 0), child: body),
        ..._resizeEdges(framed ? _margin + 4 : 5),
      ],
    );
  }

  /// Grab zones along the window's outer edge, `size` deep.
  List<Widget> _resizeEdges(double size) {
    Widget zone(_Edge edge, MouseCursor cursor, {double? left, double? top, double? right, double? bottom, double? width, double? height}) {
      return Positioned(
        left: left,
        top: top,
        right: right,
        bottom: bottom,
        width: width,
        height: height,
        child: MouseRegion(
          cursor: cursor,
          child: Listener(
            behavior: HitTestBehavior.opaque,
            // On the press itself: compositors start a resize only for a real press.
            onPointerDown: (e) {
              if (e.buttons == kPrimaryButton) _channel.invokeMethod('startResize', edge.index);
            },
          ),
        ),
      );
    }

    final c = size * 2; // corners are larger, so they are easy to catch
    return [
      zone(_Edge.top, SystemMouseCursors.resizeUp, left: c, right: c, top: 0, height: size),
      zone(_Edge.bottom, SystemMouseCursors.resizeDown, left: c, right: c, bottom: 0, height: size),
      zone(_Edge.left, SystemMouseCursors.resizeLeft, left: 0, top: c, bottom: c, width: size),
      zone(_Edge.right, SystemMouseCursors.resizeRight, right: 0, top: c, bottom: c, width: size),
      zone(_Edge.topLeft, SystemMouseCursors.resizeUpLeft, left: 0, top: 0, width: c, height: size),
      zone(_Edge.topLeft, SystemMouseCursors.resizeUpLeft, left: 0, top: 0, width: size, height: c),
      zone(_Edge.topRight, SystemMouseCursors.resizeUpRight, right: 0, top: 0, width: c, height: size),
      zone(_Edge.topRight, SystemMouseCursors.resizeUpRight, right: 0, top: 0, width: size, height: c),
      zone(_Edge.bottomLeft, SystemMouseCursors.resizeDownLeft, left: 0, bottom: 0, width: c, height: size),
      zone(_Edge.bottomLeft, SystemMouseCursors.resizeDownLeft, left: 0, bottom: 0, width: size, height: c),
      zone(_Edge.bottomRight, SystemMouseCursors.resizeDownRight, right: 0, bottom: 0, width: c, height: size),
      zone(_Edge.bottomRight, SystemMouseCursors.resizeDownRight, right: 0, bottom: 0, width: size, height: c),
    ];
  }
}

class _TitleBar extends StatelessWidget {
  const _TitleBar({required this.maximized});

  final bool maximized;

  @override
  Widget build(BuildContext context) {
    return SizedBox(
      height: 40,
      child: Row(
        children: [
          Expanded(
            child: MouseRegion(
              cursor: SystemMouseCursors.basic,
              child: GestureDetector(
                behavior: HitTestBehavior.opaque,
                onPanStart: (_) => _channel.invokeMethod('startMove'),
                onDoubleTap: () => _channel.invokeMethod('toggleMaximize'),
                child: Padding(
                  padding: const EdgeInsets.only(left: 16),
                  child: Row(
                    children: [
                      ClipRRect(
                        borderRadius: BorderRadius.circular(5),
                        child: Image.asset('assets/brand/logo.png', width: 18, height: 18, filterQuality: FilterQuality.medium),
                      ),
                      const SizedBox(width: 10),
                      const Text(
                        'Post',
                        style: TextStyle(fontFamily: PType.display, fontWeight: FontWeight.w600, fontSize: 13.5, color: PColors.textMuted),
                      ),
                    ],
                  ),
                ),
              ),
            ),
          ),
          _WindowButton(tooltip: 'Minimize', icon: _Glyph.minimize, onPressed: () => _channel.invokeMethod('minimize')),
          _WindowButton(
            tooltip: maximized ? 'Restore' : 'Maximize',
            icon: maximized ? _Glyph.restore : _Glyph.maximize,
            onPressed: () => _channel.invokeMethod('toggleMaximize'),
          ),
          _WindowButton(tooltip: 'Close', icon: _Glyph.close, onPressed: () => _channel.invokeMethod('close'), danger: true),
          const SizedBox(width: 8),
        ],
      ),
    );
  }
}

enum _Glyph { minimize, maximize, restore, close }

class _WindowButton extends StatefulWidget {
  const _WindowButton({required this.tooltip, required this.icon, required this.onPressed, this.danger = false});

  final String tooltip;
  final _Glyph icon;
  final VoidCallback onPressed;
  final bool danger;

  @override
  State<_WindowButton> createState() => _WindowButtonState();
}

class _WindowButtonState extends State<_WindowButton> {
  bool _hover = false;

  @override
  Widget build(BuildContext context) {
    final bg = !_hover ? Colors.transparent : (widget.danger ? PColors.danger : PColors.selected);
    final fg = _hover ? (widget.danger ? PColors.onAccent : PColors.text) : PColors.textMuted;
    return Semantics(
      button: true,
      label: widget.tooltip,
      child: Tooltip(
        message: widget.tooltip,
        waitDuration: const Duration(milliseconds: 600),
        child: MouseRegion(
          cursor: SystemMouseCursors.click,
          onEnter: (_) => setState(() => _hover = true),
          onExit: (_) => setState(() => _hover = false),
          child: GestureDetector(
            onTap: widget.onPressed,
            child: AnimatedContainer(
              duration: const Duration(milliseconds: 120),
              width: 40,
              height: 30,
              margin: const EdgeInsets.symmetric(horizontal: 2),
              decoration: BoxDecoration(color: bg, borderRadius: BorderRadius.circular(9)),
              child: CustomPaint(painter: _GlyphPainter(widget.icon, fg)),
            ),
          ),
        ),
      ),
    );
  }
}

/// Thin line glyphs for the window buttons, drawn rather than taken from an
/// icon font so they stay crisp at this small size.
class _GlyphPainter extends CustomPainter {
  _GlyphPainter(this.glyph, this.color);

  final _Glyph glyph;
  final Color color;

  @override
  void paint(Canvas canvas, Size size) {
    final p = Paint()
      ..color = color
      ..strokeWidth = 1.3
      ..style = PaintingStyle.stroke
      ..strokeCap = StrokeCap.round;
    final c = size.center(Offset.zero);
    const r = 5.0;
    switch (glyph) {
      case _Glyph.minimize:
        canvas.drawLine(c + const Offset(-r, 0), c + const Offset(r, 0), p);
      case _Glyph.maximize:
        canvas.drawRRect(RRect.fromRectAndRadius(Rect.fromCenter(center: c, width: 2 * r, height: 2 * r), const Radius.circular(2)), p);
      case _Glyph.restore:
        canvas.drawRRect(RRect.fromRectAndRadius(Rect.fromCenter(center: c + const Offset(-1.5, 1.5), width: 8, height: 8), const Radius.circular(1.5)), p);
        canvas.drawPath(
          Path()
            ..moveTo(c.dx - 2, c.dy - 3.5)
            ..lineTo(c.dx - 2, c.dy - 4.5)
            ..lineTo(c.dx + 4.5, c.dy - 4.5)
            ..lineTo(c.dx + 4.5, c.dy + 2)
            ..lineTo(c.dx + 3.5, c.dy + 2),
          p,
        );
      case _Glyph.close:
        canvas.drawLine(c + const Offset(-r, -r), c + const Offset(r, r), p);
        canvas.drawLine(c + const Offset(r, -r), c + const Offset(-r, r), p);
    }
  }

  @override
  bool shouldRepaint(_GlyphPainter old) => old.glyph != glyph || old.color != color;
}
