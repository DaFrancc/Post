// Profile colours: someone's two colours, and the card drawn in them. The
// edge shows the colours themselves; behind the text they are deepened
// towards the app's panel, so text stays readable on any colour chosen.

import 'package:flutter/material.dart';

import '../theme.dart';

/// Someone's profile colours, as their card is drawn.
@immutable
class ProfileTheme {
  const ProfileTheme(this.primary, this.accent);

  /// From `0xRRGGBB` values.
  ProfileTheme.rgb(int primary, int accent) : this(Color(0xFF000000 | primary), Color(0xFF000000 | accent));

  /// The colours chosen: the card's edge, top (main) to bottom (accent).
  final Color primary;
  final Color accent;

  /// The card behind the text at the top, and at the bottom.
  Color get top => _deep(primary);
  Color get bottom => _deep(accent);

  /// The edge around the card.
  LinearGradient get edge => LinearGradient(begin: Alignment.topLeft, end: Alignment.bottomRight, colors: [primary, accent]);

  /// The card behind the text, top to bottom.
  LinearGradient get fill => LinearGradient(begin: Alignment.topCenter, end: Alignment.bottomCenter, colors: [top, bottom]);

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
  bool operator ==(Object other) => other is ProfileTheme && other.primary == primary && other.accent == accent;

  @override
  int get hashCode => Object.hash(primary, accent);
}

/// A profile card's frame: in someone's profile colours (an edge in the
/// colours themselves around a fill deepened from them), else the app's
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
    // Shapes, not clips: smooth corners on every renderer. The header sits
    // on the top colour, so the ring around the picture matches it; the
    // body's fill runs from there to the bottom colour.
    const inner = _radius - _edge;
    return Material(
      color: t.top,
      elevation: 12,
      shape: RoundedRectangleBorder(borderRadius: BorderRadius.circular(_radius)),
      child: DecoratedBox(
        decoration: BoxDecoration(gradient: t.edge, borderRadius: BorderRadius.circular(_radius)),
        child: Padding(
          padding: const EdgeInsets.all(_edge),
          child: Column(
            mainAxisSize: MainAxisSize.min,
            crossAxisAlignment: CrossAxisAlignment.stretch,
            children: [
              DecoratedBox(
                decoration: BoxDecoration(
                  color: t.top,
                  borderRadius: const BorderRadius.vertical(top: Radius.circular(inner)),
                ),
                child: header(inner),
              ),
              DecoratedBox(
                decoration: BoxDecoration(
                  gradient: t.fill,
                  borderRadius: const BorderRadius.vertical(bottom: Radius.circular(inner)),
                ),
                child: body,
              ),
            ],
          ),
        ),
      ),
    );
  }
}

/// A named pair of profile colours to choose from.
class ThemePreset {
  const ThemePreset(this.name, this.theme);

  final String name;
  final ProfileTheme theme;

  static const all = [
    ThemePreset('Dusk', ProfileTheme(Color(0xFF6A55C8), Color(0xFF2A7A96))),
    ThemePreset('Rose', ProfileTheme(Color(0xFFC2588E), Color(0xFF5C2E8A))),
    ThemePreset('Forest', ProfileTheme(Color(0xFF2E8A66), Color(0xFF55702E))),
    ThemePreset('Ember', ProfileTheme(Color(0xFFD0632E), Color(0xFF8A2E46))),
    ThemePreset('Ocean', ProfileTheme(Color(0xFF2B6CB0), Color(0xFF3DB8AE))),
    ThemePreset('Sunset', ProfileTheme(Color(0xFFF2937C), Color(0xFF9B5BD6))),
    ThemePreset('Gold', ProfileTheme(Color(0xFFE0B04A), Color(0xFF8A5A1F))),
    ThemePreset('Slate', ProfileTheme(Color(0xFF6B7088), Color(0xFF2A2D44))),
  ];
}
