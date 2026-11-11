// How a message arrives on screen. Each person picks a style for their own
// messages (their profile's `arrival`), and it plays on everyone's screen.
//
// Bubble is the flight in chat.dart: the text travels from the composer or
// typing line. The others play in place, on the text where it will sit:
// the text is laid out once, exactly as it ends up, and the effects only
// change how its characters are drawn, so nothing reflows on the way.

import 'dart:math' as math;
import 'dart:ui' as ui;

import 'package:flutter/material.dart';
import 'package:flutter/rendering.dart';

import '../theme.dart';
import 'markup_text.dart';

enum Arrival {
  bubble('Bubble', 'Carried in on a bubble, along a curve.'),
  rise('Rise', 'Fades in, rising into place.'),
  decode('Decode', 'Scrambled characters settle into your words.'),
  typewriter('Typewriter', 'Typed out a letter at a time.'),
  focus('Focus', 'Sharpens out of a blur.'),
  wave('Wave', 'Letters pop up in a ripple.'),
  instant('Instant', 'Just appears.');

  const Arrival(this.label, this.description);

  final String label;
  final String description;

  /// The default, for an empty name.
  static const standard = Arrival.bubble;

  /// A profile's arrival name. Empty, or a name this app does not know (a
  /// newer app's preset), means the default.
  static Arrival parse(String? name) {
    for (final a in values) {
      if (a.name == name) return a;
    }
    return standard;
  }

  /// Plays on the text where it sits (rather than flying in, or nothing).
  bool get inPlace => switch (this) {
    Arrival.bubble || Arrival.instant => false,
    _ => true,
  };
}

/// Effects that draw characters one by one give way to [Arrival.rise] past
/// this many, so a long message neither takes long nor costs much.
const _maxCharacters = 600;

/// A message's text, arriving in the given style when [play] is set (once,
/// from when it is first built with it), and plain otherwise.
class ArrivalText extends StatefulWidget {
  const ArrivalText(this.text, {super.key, required this.style, required this.arrival, required this.play, this.delay = Duration.zero});

  final String text;
  final TextStyle style;
  final Arrival arrival;
  final bool play;

  /// Wait this long first: the row opens before its text arrives.
  final Duration delay;

  @override
  State<ArrivalText> createState() => _ArrivalTextState();
}

class _ArrivalTextState extends State<ArrivalText> with SingleTickerProviderStateMixin {
  late final Arrival _arrival = !widget.arrival.inPlace || widget.text.length <= _maxCharacters ? widget.arrival : Arrival.rise;
  late final AnimationController _c = AnimationController(vsync: this, duration: widget.delay + _length);
  final _runs = <RevealRun>[];
  bool _playing = false;

  /// How long the effect takes, by how much text there is, never over about
  /// a second.
  Duration get _length {
    final n = widget.text.length;
    final ms = switch (_arrival) {
      Arrival.typewriter => (n * 30).clamp(350, 1100),
      Arrival.decode => (n * 22).clamp(550, 1100),
      Arrival.wave => (450 + n * 14).clamp(550, 1100),
      Arrival.focus => 700,
      Arrival.rise => 520,
      Arrival.bubble || Arrival.instant => 0,
    };
    return Duration(milliseconds: ms);
  }

  @override
  void initState() {
    super.initState();
    if (widget.play && _arrival.inPlace) {
      _playing = true;
      _c.forward().whenComplete(() {
        if (mounted) setState(() => _playing = false);
      });
    }
  }

  @override
  void didChangeDependencies() {
    super.didChangeDependencies();
    // Reduced motion: no effects at all.
    if (_playing && MediaQuery.disableAnimationsOf(context)) {
      _playing = false;
      _c.value = 1;
    }
  }

  @override
  void dispose() {
    _c.dispose();
    super.dispose();
  }

  /// The effect's own progress, 0 to 1, after the delay.
  double get _t {
    final total = _c.duration!.inMicroseconds;
    final delay = widget.delay.inMicroseconds;
    if (total <= delay) return 1;
    return ((_c.value * total - delay) / (total - delay)).clamp(0.0, 1.0);
  }

  @override
  Widget build(BuildContext context) {
    // The same shape throughout, playing or not, so the text's own state
    // (spoilers shown) survives the end of the effect.
    return AnimatedBuilder(
      animation: _c,
      builder: (context, _) {
        final playing = _playing;
        final t = playing ? _t : 1.0;
        final e = Curves.easeInOutCubic.transform(t);
        final effect = playing ? _effectFor(t) : null;
        Widget text = MarkupText(widget.text, style: widget.style, shownUpTo: effect?.shownUpTo(_total), runs: effect == null ? null : _runs);
        text = _Glyphs(effect: effect, runs: _runs, child: text);
        final blur = playing && _arrival == Arrival.focus ? 7 * (1 - e) : 0.0;
        final opacity = playing && (_arrival == Arrival.focus || _arrival == Arrival.rise) ? e : 1.0;
        final rise = playing && _arrival == Arrival.rise ? 10 * (1 - e) : 0.0;
        return Transform.translate(
          offset: Offset(0, rise),
          child: Opacity(
            opacity: opacity,
            child: ImageFiltered(
              enabled: blur > 0.01,
              imageFilter: ui.ImageFilter.blur(sigmaX: blur, sigmaY: blur),
              child: text,
            ),
          ),
        );
      },
    );
  }

  /// How many characters the text draws: what the runs add up to, known
  /// from the last build (the first frame of an effect shows nothing).
  int get _total => _runs.isEmpty ? widget.text.length : _runs.last.start + _runs.last.text.length;

  _GlyphEffect? _effectFor(double t) {
    final ms = (_c.value * _c.duration!.inMilliseconds).round();
    return switch (_arrival) {
      Arrival.typewriter => _Typewriter(t),
      Arrival.decode => _Decode(t, ms),
      Arrival.wave => _Wave(t),
      _ => null,
    };
  }
}

// ---------------------------------------------------------------- effects

/// An effect that draws characters itself: the text draws those before
/// [shownUpTo] as usual and leaves the rest transparent in place, and the
/// effect paints over them, given where each character is.
abstract class _GlyphEffect {
  const _GlyphEffect(this.t);

  final double t;

  int shownUpTo(int total);

  void paint(Canvas canvas, _GlyphPainter g);
}

/// Typed out: characters appear in order, each fading in over its moment,
/// behind a caret that fades away once done.
class _Typewriter extends _GlyphEffect {
  const _Typewriter(super.t);

  double _at(int total) => Curves.easeInOutSine.transform(t) * total;

  @override
  int shownUpTo(int total) => _at(total).floor();

  @override
  void paint(Canvas canvas, _GlyphPainter g) {
    final at = _at(g.total);
    final i = at.floor();
    if (i < g.total) g.glyph(canvas, i, opacity: at - i);
    // The caret, after the last character shown.
    final box = g.caretAt(i);
    if (box == null) return;
    final fade = t < 0.85 ? 1.0 : (1 - t) / 0.15;
    final h = box.height * 0.72;
    canvas.drawRRect(
      RRect.fromRectAndRadius(Rect.fromLTWH(box.left, box.center.dy - h / 2, 2, h), const Radius.circular(1)),
      Paint()..color = PColors.accent.withValues(alpha: fade),
    );
  }
}

/// Scrambled: every character not yet settled shows a different random
/// one each moment, and they settle into the real text from the start.
class _Decode extends _GlyphEffect {
  const _Decode(super.t, this.ms);

  /// Time into the effect, for how often the scramble changes.
  final int ms;

  static const _chars = 'ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz0123456789#%&*+=?@';
  static const _green = Color(0xFF3DD68C);

  double _at(int total) => Curves.easeInOutCubic.transform(t) * total;

  @override
  int shownUpTo(int total) => _at(total).floor();

  @override
  void paint(Canvas canvas, _GlyphPainter g) {
    final from = _at(g.total).floor();
    final tick = ms ~/ 55;
    final appear = (t / 0.08).clamp(0.0, 1.0);
    for (var i = from; i < g.total; i++) {
      final c = _chars[(i * 7919 + tick * 104729 + i * tick * 31) % _chars.length];
      // Brightest just ahead of the settled text.
      final near = (1 - (i - from) / 6).clamp(0.0, 1.0);
      g.glyph(canvas, i, replace: c, color: Color.lerp(_green.withValues(alpha: 0.75), Colors.white, near * 0.6)!, opacity: appear);
    }
  }
}

/// A ripple: each character pops up from a little below in turn, overshoots
/// a touch and settles.
class _Wave extends _GlyphEffect {
  const _Wave(super.t);

  /// When each character starts, as a share of the whole, and how long its
  /// own pop takes.
  static const _spread = 0.55;
  static const _pop = 0.45;

  double _u(int i, int total) {
    final start = total <= 1 ? 0.0 : i / (total - 1) * _spread;
    return ((t - start) / _pop).clamp(0.0, 1.0);
  }

  @override
  int shownUpTo(int total) {
    var i = 0;
    while (i < total && _u(i, total) >= 1) {
      i++;
    }
    return i;
  }

  @override
  void paint(Canvas canvas, _GlyphPainter g) {
    for (var i = shownUpTo(g.total); i < g.total; i++) {
      final u = _u(i, g.total);
      if (u <= 0) break;
      final dy = 9 * (1 - Curves.easeOutBack.transform(u));
      g.glyph(canvas, i, offset: Offset(0, dy), opacity: (u * 2.5).clamp(0.0, 1.0));
    }
  }
}

// ---------------------------------------------------------------- drawing glyphs

/// Where each character of the text below is, and how to draw one there.
class _GlyphPainter {
  _GlyphPainter(this.boxes, this.runs, this.origin, this._cache);

  /// Each character's box, in this widget's coordinates; empty for one that
  /// takes no space (a line break).
  final List<Rect> boxes;
  final List<RevealRun> runs;
  final Offset origin;
  final Map<(String, TextStyle), TextPainter> _cache;

  int get total => boxes.length;

  RevealRun? _runAt(int i) {
    for (final r in runs) {
      if (i < r.start + r.text.length) return i >= r.start ? r : null;
    }
    return null;
  }

  /// Draw character [i] where it sits: itself, or [replace] in its place;
  /// in its own style, or [color]; nudged by [offset].
  void glyph(Canvas canvas, int i, {String? replace, Color? color, Offset offset = Offset.zero, double opacity = 1}) {
    if (i < 0 || i >= boxes.length || opacity <= 0) return;
    final box = boxes[i];
    final run = _runAt(i);
    if (box.isEmpty || run == null || run.hidden) return;
    final ch = run.text[i - run.start];
    if (ch.trim().isEmpty) return;
    var style = run.style;
    final base = color ?? style.color ?? PColors.text;
    style = style.copyWith(color: base.withValues(alpha: base.a * opacity));
    if (style.backgroundColor != null) style = style.copyWith(backgroundColor: style.backgroundColor!.withValues(alpha: style.backgroundColor!.a * opacity));
    final text = replace ?? ch;
    final key = (text, style);
    final tp = _cache[key] ??= (TextPainter(
      text: TextSpan(text: text, style: style),
      textDirection: TextDirection.ltr,
    )..layout());
    final at = origin + box.topLeft + offset + Offset((box.width - tp.width) / 2, (box.height - tp.height) / 2);
    tp.paint(canvas, at);
  }

  /// Where a caret goes before character [i]: its box's left edge, or after
  /// the last character shown.
  Rect? caretAt(int i) {
    if (i < boxes.length && !boxes[i].isEmpty) return (origin + boxes[i].topLeft) & Size(0, boxes[i].height);
    for (var j = math.min(i, boxes.length) - 1; j >= 0; j--) {
      if (!boxes[j].isEmpty) return (origin + boxes[j].topRight) & Size(0, boxes[j].height);
    }
    return null;
  }
}

/// Paints an effect's characters over its child (the text), finding each
/// character among the child's paragraphs.
class _Glyphs extends SingleChildRenderObjectWidget {
  const _Glyphs({required this.effect, required this.runs, required super.child});

  final _GlyphEffect? effect;
  final List<RevealRun> runs;

  @override
  RenderObject createRenderObject(BuildContext context) => _RenderGlyphs(effect, runs);

  @override
  void updateRenderObject(BuildContext context, _RenderGlyphs renderObject) {
    renderObject
      ..effect = effect
      ..runs = runs;
  }
}

class _RenderGlyphs extends RenderProxyBox {
  _RenderGlyphs(this._effect, this.runs);

  _GlyphEffect? _effect;
  set effect(_GlyphEffect? e) {
    if (identical(e, _effect)) return;
    _effect = e;
    markNeedsPaint();
  }

  List<RevealRun> runs;

  /// Character boxes, measured once per size: the layout does not change
  /// during an effect, only how characters are drawn.
  List<Rect>? _boxes;
  Size? _boxesFor;
  final _cache = <(String, TextStyle), TextPainter>{};

  @override
  void paint(PaintingContext context, Offset offset) {
    super.paint(context, offset);
    final effect = _effect;
    if (effect == null || child == null) {
      _boxes = null;
      _clearCache();
      return;
    }
    if (_boxes == null || _boxesFor != size) {
      _boxes = _measure();
      _boxesFor = size;
    }
    effect.paint(context.canvas, _GlyphPainter(_boxes!, runs, offset, _cache));
  }

  List<Rect> _measure() {
    final paragraphs = <RenderParagraph>[];
    void visit(RenderObject o) {
      if (o is RenderParagraph) paragraphs.add(o);
      o.visitChildren(visit);
    }

    visit(child!);
    final boxes = <Rect>[];
    for (final p in paragraphs) {
      final n = p.text.toPlainText(includeSemanticsLabels: false).length;
      final transform = p.getTransformTo(this);
      for (var i = 0; i < n; i++) {
        final found = p.getBoxesForSelection(TextSelection(baseOffset: i, extentOffset: i + 1));
        boxes.add(found.isEmpty ? Rect.zero : MatrixUtils.transformRect(transform, found.first.toRect()));
      }
    }
    return boxes;
  }

  void _clearCache() {
    for (final tp in _cache.values) {
      tp.dispose();
    }
    _cache.clear();
  }

  @override
  void dispose() {
    _clearCache();
    super.dispose();
  }
}

// ---------------------------------------------------------------- previews

/// A style playing over and over on a sample line, for choosing one.
class ArrivalPreview extends StatefulWidget {
  const ArrivalPreview({super.key, required this.arrival, this.text = 'see you at the crag at 8'});

  final Arrival arrival;
  final String text;

  @override
  State<ArrivalPreview> createState() => _ArrivalPreviewState();
}

class _ArrivalPreviewState extends State<ArrivalPreview> with SingleTickerProviderStateMixin {
  /// One loop: the arrival, then a pause showing the text.
  late final AnimationController _loop = AnimationController(vsync: this, duration: const Duration(milliseconds: 2400))..repeat();
  int _round = 0;

  @override
  void initState() {
    super.initState();
    _loop.addListener(_tick);
  }

  double _last = 0;
  void _tick() {
    // A new round each time the loop wraps: the text is built afresh and
    // arrives again.
    if (_loop.value < _last) setState(() => _round++);
    _last = _loop.value;
  }

  @override
  void dispose() {
    _loop.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    const style = TextStyle(fontFamily: PType.body, fontSize: 13.5, height: 1.5, color: PColors.textBody);
    final a = widget.arrival;
    return SizedBox(
      height: 40,
      child: ClipRect(
        child: Align(
          alignment: Alignment.centerLeft,
          child: switch (a) {
            Arrival.bubble => _BubblePreview(key: ValueKey(_round), text: widget.text, style: style),
            Arrival.instant => _InstantPreview(key: ValueKey(_round), text: widget.text, style: style),
            _ => ArrivalText(widget.text, key: ValueKey(_round), style: style, arrival: a, play: true, delay: const Duration(milliseconds: 250)),
          },
        ),
      ),
    );
  }
}

/// Gone for a moment, then there.
class _InstantPreview extends StatefulWidget {
  const _InstantPreview({super.key, required this.text, required this.style});

  final String text;
  final TextStyle style;

  @override
  State<_InstantPreview> createState() => _InstantPreviewState();
}

class _InstantPreviewState extends State<_InstantPreview> {
  bool _shown = false;

  @override
  void initState() {
    super.initState();
    Future<void>.delayed(const Duration(milliseconds: 400), () {
      if (mounted) setState(() => _shown = true);
    });
  }

  @override
  Widget build(BuildContext context) => Opacity(
    opacity: _shown ? 1 : 0,
    child: Text(widget.text, style: widget.style),
  );
}

/// The bubble in miniature: the text rides up into place in a pill along a
/// curve, and the pill dissolves.
class _BubblePreview extends StatefulWidget {
  const _BubblePreview({super.key, required this.text, required this.style});

  final String text;
  final TextStyle style;

  @override
  State<_BubblePreview> createState() => _BubblePreviewState();
}

class _BubblePreviewState extends State<_BubblePreview> with SingleTickerProviderStateMixin {
  late final AnimationController _c = AnimationController(vsync: this, duration: const Duration(milliseconds: 1050))..forward();
  static const _travel = Cubic(0.62, 0, 0.32, 1.15);

  @override
  void dispose() {
    _c.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    return AnimatedBuilder(
      animation: _c,
      builder: (context, _) {
        // The first part waits, so each loop starts from nothing.
        final v = ((_c.value - 0.2) / 0.8).clamp(0.0, 1.0);
        final u = _travel.transform(v);
        // A quadratic arc from below and to the right, into place.
        final w = 1 - u;
        final p = const Offset(30, 30) * (w * w) + const Offset(70, 4) * (2 * w * u);
        final settle = Curves.easeInOutCubic.transform(((v - 0.72) / 0.28).clamp(0.0, 1.0));
        final gather = Curves.easeInOutCubic.transform((v / 0.3).clamp(0.0, 1.0));
        return Opacity(
          opacity: v == 0 ? 0 : 1,
          child: Transform.translate(
            offset: p,
            child: Container(
              padding: const EdgeInsets.symmetric(horizontal: 9, vertical: 3),
              transform: Matrix4.translationValues(-9, 0, 0),
              decoration: BoxDecoration(
                color: Color.alphaBlend(PColors.accent.withValues(alpha: 0.16), PColors.panelRaised).withValues(alpha: gather * (1 - settle)),
                border: Border.all(color: PColors.accent.withValues(alpha: 0.45 * gather * (1 - settle))),
                borderRadius: BorderRadius.circular(14),
              ),
              child: Text(widget.text, style: widget.style),
            ),
          ),
        );
      },
    );
  }
}
