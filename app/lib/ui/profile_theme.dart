// Colour fills (a colour, or a gradient between two at an angle) for
// banners and profile colours; someone's card drawn in their profile
// colours; and holes cut out of what is drawn, so whatever lies behind
// shows through (the gap around a picture on a banner, around a presence
// dot on a picture), on any background, gradients included.

import 'dart:math' as math;
import 'dart:typed_data';
import 'dart:ui' as ui;

import 'package:flutter/foundation.dart' show listEquals;
import 'package:flutter/material.dart';
import 'package:post/src/rust/api/client.dart' show ColorFill;

import '../theme.dart';

/// A colour, or a gradient from the first colour to the second at
/// [angle] degrees, as in CSS: 0 runs from the bottom up, 90 left to
/// right, 180 top to bottom.
@immutable
class Fill {
  const Fill(this.colors, [this.angle = 0]);

  Fill.solid(Color c) : this([c]);

  /// As a profile carries it.
  Fill.of(ColorFill f) : this([for (final c in f.colors) Color(0xFF000000 | c)], f.angle);

  final List<Color> colors;
  final int angle;

  bool get solid => colors.length == 1;

  /// As a profile carries it.
  ColorFill get carried => ColorFill(colors: Uint32List.fromList([for (final c in colors) c.toARGB32() & 0xFFFFFF]), angle: solid ? 0 : angle);

  /// To paint with (a solid colour as a gradient of one colour).
  LinearGradient get gradient => fillGradient(colors, angle);

  /// The same fill in other colours.
  Fill map(Color Function(Color) f) => Fill([for (final c in colors) f(c)], angle);

  @override
  bool operator ==(Object other) => other is Fill && other.angle == angle && listEquals(other.colors, colors);

  @override
  int get hashCode => Object.hash(angle, Object.hashAll(colors));
}

/// [colors] running at [angle] degrees (as CSS: 0 up, 90 right, 180 down)
/// across whatever box it fills.
LinearGradient fillGradient(List<Color> colors, int angle) {
  final a = angle * math.pi / 180;
  final to = Alignment(math.sin(a), -math.cos(a));
  return LinearGradient(begin: -to, end: to, colors: colors.length == 1 ? [colors[0], colors[0]] : colors);
}

/// Someone's profile colours, as their card is drawn: an edge in the
/// colours themselves, and behind the text the same fill deepened towards
/// the app's panel, so the app's light text stays readable on any colour.
@immutable
class ProfileTheme {
  const ProfileTheme(this.chosen);

  final Fill chosen;

  /// The edge around the card.
  LinearGradient get edge => chosen.gradient;

  /// The card behind the text.
  LinearGradient get fill => chosen.map(_deep).gradient;

  /// The card's colour where it is a single colour is wanted (its first).
  Color get base => _deep(chosen.colors.first);

  /// [c] mixed into the panel colour, no more than dark enough for the
  /// app's light text: a pale colour is mixed in less.
  static Color _deep(Color c) {
    var t = 0.34;
    var mixed = Color.lerp(PColors.panelRaised, c, t)!;
    while (mixed.computeLuminance() > 0.045 && t > 0.08) {
      t -= 0.02;
      mixed = Color.lerp(PColors.panelRaised, c, t)!;
    }
    return mixed;
  }

  @override
  bool operator ==(Object other) => other is ProfileTheme && other.chosen == chosen;

  @override
  int get hashCode => chosen.hashCode;
}

/// A profile card's frame: in someone's profile colours, else the app's
/// raised panel. [header] is told the radius of its top corners.
class ProfileFrame extends StatelessWidget {
  const ProfileFrame({super.key, required this.theme, required this.header, required this.body});

  final ProfileTheme? theme;
  final Widget Function(double innerRadius) header;
  final Widget body;

  static const _radius = 16.0;
  static const _edge = 3.0;

  @override
  Widget build(BuildContext context) {
    final t = theme;
    if (t == null) {
      return Material(
        color: PColors.panelRaised,
        elevation: 12,
        clipBehavior: Clip.antiAlias,
        shape: RoundedRectangleBorder(
          borderRadius: BorderRadius.circular(_radius),
          side: const BorderSide(color: PColors.borderStrong),
        ),
        child: Column(mainAxisSize: MainAxisSize.min, crossAxisAlignment: CrossAxisAlignment.stretch, children: [header(_radius - 1), body]),
      );
    }
    // Shapes, not clips: smooth corners on every renderer. One fill behind
    // header and body alike; the gap around the picture is cut out of the
    // banner, so it shows this fill whatever its angle.
    const inner = _radius - _edge;
    return Material(
      color: t.base,
      elevation: 12,
      shape: RoundedRectangleBorder(borderRadius: BorderRadius.circular(_radius)),
      child: DecoratedBox(
        decoration: BoxDecoration(gradient: t.edge, borderRadius: BorderRadius.circular(_radius)),
        child: Padding(
          padding: const EdgeInsets.all(_edge),
          child: DecoratedBox(
            decoration: BoxDecoration(gradient: t.fill, borderRadius: BorderRadius.circular(inner)),
            child: Column(mainAxisSize: MainAxisSize.min, crossAxisAlignment: CrossAxisAlignment.stretch, children: [header(inner), body]),
          ),
        ),
      ),
    );
  }
}

/// [child] with a round hole cut out of it, centred at [centre] (in its
/// own coordinates) with [radius]: whatever lies behind shows through.
/// The hole's edge is smoothed over a pixel, like a drawn shape's.
class Punch extends StatelessWidget {
  const Punch({super.key, required this.centre, required this.radius, required this.child});

  final Offset centre;
  final double radius;
  final Widget child;

  @override
  Widget build(BuildContext context) {
    final soft = 0.5 / MediaQuery.devicePixelRatioOf(context);
    final outer = radius + soft;
    return ShaderMask(
      // Keeps the child only where the mask is clear: out of the circle.
      blendMode: BlendMode.dstOut,
      shaderCallback: (_) => ui.Gradient.radial(centre, outer, const [Colors.black, Colors.black, Colors.transparent], [0, (radius - soft) / outer, 1]),
      child: child,
    );
  }
}
