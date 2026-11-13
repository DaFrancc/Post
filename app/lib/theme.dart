// Post's look, defined once. Every screen takes its colours, shapes and
// type from here, so changing the look is an edit to this file.
//
// Direction: Discord's layout, made more modern: separate rounded panels on a
// near-black ground, generous spacing, colour used sparingly. Accents come
// from the (placeholder) icon's cyan-blue-violet.

import 'dart:math' as math;

import 'package:flutter/material.dart';
import 'package:flutter/rendering.dart';

/// Colours.
abstract final class PColors {
  static const ground = Color(0xFF06070C);
  static const panel = Color(0xFF10111A);
  static const panelRaised = Color(0xFF161827);
  static const sunken = Color(0xFF0A0B12);
  static const hover = Color(0xFF181A29);
  static const hoverStrong = Color(0xFF1D2034);
  static const selected = Color(0xFF1E2134);

  /// The chosen row in a list: a touch of the accent, so it reads as chosen
  /// rather than hovered.
  static const active = Color(0xFF1B2140);
  static const activeEdge = Color(0xFF2B3463);
  static const dialog = Color(0xFF12131F);
  static const messageHover = Color(0xFF141625);
  static const focusEdge = Color(0xFF3A4A8C);

  // Formatted text.
  /// Code blocks sit darker than the panel; inline code lighter, so it shows
  /// within a line.
  static const codeBg = Color(0xFF0C0D16);
  static const codeInline = Color(0xFF1F2236);
  static const codeText = Color(0xFFE2E5F0);
  static const spoiler = Color(0xFF2A2D44);
  static const spoilerShown = Color(0x1FFFFFFF);
  static const quoteBar = Color(0xFF3A3E5C);
  static const misspelled = Color(0xFFF25F7A);

  /// Mentions: of someone, in the accent; of you, in amber, with the
  /// message's row warmed too and a bar down its edge.
  static const mention = Color(0x336E8BFF);
  static const mentionText = Color(0xFFB4C8FF);
  static const mentionYou = Color(0x40F2B33D);
  static const mentionYouText = Color(0xFFFFD58C);
  static const mentionRow = Color(0xFF1C1A1C);
  static const mentionRowHover = Color(0xFF221F1F);
  static const mentionBar = Color(0xFFF2B33D);
  static const border = Color(0xFF1A1C2B);
  static const borderStrong = Color(0xFF2A2D44);

  static const text = Color(0xFFE8EAF2);
  static const textStrong = Color(0xFFF2F3F8);
  static const textBody = Color(0xFFD9DCE8);
  static const textMuted = Color(0xFFA3A8BD);
  static const textFaint = Color(0xFF8C92AA);

  /// The icon's cyan, blue and violet, for the few places that carry the
  /// brand: the send button and the selected rail item.
  static const brand = LinearGradient(begin: Alignment.topLeft, end: Alignment.bottomRight, colors: [Color(0xFF52C8F5), Color(0xFF6E8BFF), Color(0xFF9B7BFF)]);

  static const groundGlow = RadialGradient(
    center: Alignment(-1.1, -1.2),
    radius: 1.6,
    colors: [Color(0xFF111833), Color(0xFF0A0C18), ground],
    stops: [0, 0.45, 1],
  );

  static const accent = Color(0xFF6E8BFF);
  static const onAccent = Color(0xFF070812);
  static const accentSoft = Color(0xFF121A2E);
  static const accentText = Color(0xFF8EB4FF);

  static const online = Color(0xFF3DD68C);
  static const away = Color(0xFFF2B33D);
  static const dnd = Color(0xFFF25F7A);
  static const offline = Color(0xFF6B7088);

  /// The Update button: green, so it reads as "go" at a glance.
  static const update = Color(0xFF3DD68C);
  static const onUpdate = Color(0xFF06140D);

  /// Unread counts.
  static const unread = Color(0xFFEB1A40);
  static const onUnread = Color(0xFFFFFFFF);

  static const danger = Color(0xFFF25F7A);
  static const dangerSoft = Color(0xFF2A1520);
  static const dangerText = Color(0xFFFFB3C1);
  static const verified = Color(0xFF3DD68C);
  static const unverified = Color(0xFFF2B33D);
  static const adminSoft = Color(0xFF1D2240);
  static const adminText = Color(0xFF9DB0FF);

  /// Text on an avatar colour.
  static const onAvatar = Color(0xFF0B0C14);

  /// Avatar colours, picked by a stable hash of the username.
  static const avatars = [
    Color(0xFF7C8CFF),
    Color(0xFFF28CB5),
    Color(0xFF4FD1C5),
    Color(0xFFF2B866),
    Color(0xFF9B7BFF),
    Color(0xFF5EC8F2),
    Color(0xFF8BD17C),
    Color(0xFFF2937C),
  ];

  static Color avatarFor(String key) {
    var h = 0;
    for (final c in key.codeUnits) {
      h = (h * 31 + c) & 0x7fffffff;
    }
    return avatars[h % avatars.length];
  }

  static Color presence(String word) => switch (word) {
    'online' => online,
    'away' => away,
    'dnd' => dnd,
    _ => offline,
  };
}

/// Corner radii and spacing.
abstract final class PShape {
  static const panelRadius = 20.0;
  static const rowRadius = 14.0;
  static const controlRadius = 12.0;
  static const gap = 10.0;

  /// Every panel's header is at least this tall, so their titles line up.
  /// A minimum, not a fixed height: larger system text makes it taller
  /// rather than clipping.
  static const header = 64.0;
}

/// Type.
/// Motion: short and eased, so the app feels quick rather than busy.
abstract final class PMotion {
  static const fast = Duration(milliseconds: 120);
  static const normal = Duration(milliseconds: 200);
  static const slow = Duration(milliseconds: 320);
  static const curve = Curves.easeOutCubic;
}

abstract final class PType {
  static const body = 'Inter';
  static const display = 'SpaceGrotesk';

  /// Code, link codes, IDs and safety numbers. Bundled, never the system's.
  static const mono = 'JetBrainsMono';

  static const title = TextStyle(fontFamily: display, fontWeight: FontWeight.w600, fontSize: 20, letterSpacing: -0.2, color: PColors.text);
  static const heading = TextStyle(fontFamily: display, fontWeight: FontWeight.w600, fontSize: 18, letterSpacing: -0.2, color: PColors.text);
  static const label = TextStyle(fontFamily: body, fontWeight: FontWeight.w600, fontSize: 11, letterSpacing: 0.7, color: PColors.textFaint);
  static const name = TextStyle(fontFamily: body, fontWeight: FontWeight.w600, fontSize: 14, color: PColors.textStrong);
  static const text = TextStyle(fontFamily: body, fontSize: 14, height: 1.5, color: PColors.textBody);
  static const small = TextStyle(fontFamily: body, fontSize: 12.5, color: PColors.textFaint);
}

/// How every text style starts, inherited through the theme's default text
/// style unless a style sets its own:
///
/// - Extra line height split evenly above and below the text, as CSS does.
///   Flutter's default splits it by the font's ascent and descent, and
///   Inter's tall ascent then pushes text low in its line: visibly
///   off-centre in fields and buttons.
/// - No fallback font list. Flutter's Linux theme names six system fonts
///   (Ubuntu, Cantarell, DejaVu Sans, ...), and text layout asks the system
///   font matcher about each the first time: a 23 ms stall on the first
///   frame of the main screen, 46 ms on the link-code screen. Every font
///   the app uses is bundled; a character missing from them (an emoji)
///   still finds a system font on its own.
TextTheme _textStyles(TextTheme t) {
  TextStyle? even(TextStyle? s) => s?.copyWith(leadingDistribution: TextLeadingDistribution.even, fontFamilyFallback: const <String>[]);
  return t.copyWith(
    displayLarge: even(t.displayLarge),
    displayMedium: even(t.displayMedium),
    displaySmall: even(t.displaySmall),
    headlineLarge: even(t.headlineLarge),
    headlineMedium: even(t.headlineMedium),
    headlineSmall: even(t.headlineSmall),
    titleLarge: even(t.titleLarge),
    titleMedium: even(t.titleMedium),
    titleSmall: even(t.titleSmall),
    bodyLarge: even(t.bodyLarge),
    bodyMedium: even(t.bodyMedium),
    bodySmall: even(t.bodySmall),
    labelLarge: even(t.labelLarge),
    labelMedium: even(t.labelMedium),
    labelSmall: even(t.labelSmall),
  );
}

/// The pointing hand over anything clickable, the arrow when it is disabled.
/// Flutter shows the hand only on the web and keeps the arrow in desktop
/// apps, following native toolkits; Post follows the web and chat apps
/// people know, Discord among them, where a hand says "this is a button".
final WidgetStateProperty<MouseCursor?> pClickable = WidgetStateProperty.resolveWith(
  (states) => states.contains(WidgetState.disabled) ? SystemMouseCursors.basic : SystemMouseCursors.click,
);

/// Post's buttons ("tactile"): a face standing on a solid edge, like a key.
/// It rises a pixel under the pointer and goes all the way down when
/// pressed. The theme gives every filled and outlined button one of these;
/// a button in a colour of its own takes [PButtons.tactile] with that
/// colour, and its edge is worked out from it. One rule sizes them all
/// (see [_TactilePainter]): full size, or compact at 32 pixels or less.
///
/// Which to use: anything that does something is tactile, Cancel included
/// (filled for the main action, outlined for the rest, [danger] to remove
/// or end). Text buttons are for links only: set a nickname, show older
/// ones, next page, reset a setting. Icon-only toolbar buttons are
/// [PIconButton]s.
abstract final class PButtons {
  /// The main action.
  static final primary = tactile(face: PColors.accent, onFace: PColors.onAccent);

  /// Anything else: Cancel, Back.
  static final secondary = tactile(face: const Color(0xFF1C1F33), onFace: PColors.textBody, border: const Color(0xFF2E3250));

  /// An accent action that is not the main one.
  static final tonal = tactile(face: const Color(0xFF2B3560), onFace: const Color(0xFFD9E2FF));

  /// Something that removes or ends: deleting, logging out.
  static final danger = tactile(face: const Color(0xFF3A1A24), onFace: const Color(0xFFFF8EA2));

  /// A tactile button in [face], with [onFace] for its label. A disabled
  /// one fades and lies flat, unless [fadeDisabled] is false (a button
  /// showing progress while it works).
  static ButtonStyle tactile({required Color face, required Color onFace, Color? border, bool fadeDisabled = true}) {
    return ButtonStyle(
      backgroundColor: const WidgetStatePropertyAll(Colors.transparent),
      foregroundColor: WidgetStateProperty.resolveWith(
        (s) => s.contains(WidgetState.disabled) && fadeDisabled ? onFace.withValues(alpha: 0.55) : onFace,
      ),
      iconColor: WidgetStateProperty.resolveWith(
        (s) => s.contains(WidgetState.disabled) && fadeDisabled ? onFace.withValues(alpha: 0.55) : onFace,
      ),
      // The press is the feedback: no ink.
      overlayColor: const WidgetStatePropertyAll(Colors.transparent),
      splashFactory: NoSplash.splashFactory,
      shadowColor: const WidgetStatePropertyAll(Colors.transparent),
      elevation: const WidgetStatePropertyAll(0),
      side: const WidgetStatePropertyAll(BorderSide.none),
      mouseCursor: pClickable,
      backgroundBuilder: (context, states, child) =>
          _TactileFace(states: states, face: face, border: border, fadeDisabled: fadeDisabled, child: child),
    );
  }
}

/// Where a tactile button is in its travel: 1 resting, 0 pressed all the
/// way down, a little over 1 risen under the pointer; and how lit its face
/// is (hovered).
@immutable
class _Travel {
  const _Travel(this.up, this.lit);

  final double up;
  final double lit;

  _Travel lerp(_Travel to, double t) => _Travel(up + (to.up - up) * t, lit + (to.lit - lit) * t);
}

class _TravelTween extends Tween<_Travel> {
  _TravelTween({required super.begin, required super.end});

  @override
  _Travel lerp(double t) => begin!.lerp(end!, t);
}

class _TactileFace extends StatelessWidget {
  const _TactileFace({required this.states, required this.face, required this.border, required this.fadeDisabled, required this.child});

  final Set<WidgetState> states;
  final Color face;
  final Color? border;
  final bool fadeDisabled;
  final Widget? child;

  @override
  Widget build(BuildContext context) {
    final disabled = states.contains(WidgetState.disabled);
    final pressed = !disabled && states.contains(WidgetState.pressed);
    final hovered = !disabled && !pressed && states.contains(WidgetState.hovered);
    final focused = !disabled && states.contains(WidgetState.focused);
    final goal = _Travel(disabled || pressed ? 0 : (hovered ? 1.34 : 1), hovered ? 1 : 0);
    return TweenAnimationBuilder<_Travel>(
      tween: _TravelTween(begin: goal, end: goal),
      duration: pressed ? const Duration(milliseconds: 40) : const Duration(milliseconds: 110),
      curve: Curves.easeOut,
      builder: (context, travel, child) {
        // The face and its label move together; how far is known only at
        // paint time, from the button's size, so the label follows by the
        // same rule there.
        return CustomPaint(
          painter: _TactilePainter(
            face: face,
            border: border,
            travel: travel,
            disabled: disabled,
            fadeDisabled: fadeDisabled,
            focused: focused,
          ),
          child: _Lifted(travel: travel, child: child),
        );
      },
      child: child,
    );
  }
}

/// The resting edge under a button [height] pixels tall: 3 pixels for a
/// full-size button, 2 for a compact one, so small buttons are not heavy.
double _edgeDepth(double height) => height >= 36 ? 3 : 2;

/// Moves the label with the face: down by the edge's depth when pressed,
/// up a fraction of it when risen.
class _Lifted extends SingleChildRenderObjectWidget {
  const _Lifted({required this.travel, super.child});

  final _Travel travel;

  @override
  RenderObject createRenderObject(BuildContext context) => _RenderLifted(travel);

  @override
  void updateRenderObject(BuildContext context, _RenderLifted r) => r.travel = travel;
}

class _RenderLifted extends RenderProxyBox {
  _RenderLifted(this._travel);

  _Travel _travel;
  set travel(_Travel t) {
    if (t.up == _travel.up) return;
    _travel = t;
    markNeedsPaint();
  }

  double get _shift => (1 - _travel.up) * _edgeDepth(size.height);

  @override
  void paint(PaintingContext context, Offset offset) => super.paint(context, offset + Offset(0, _shift));

  @override
  bool hitTestChildren(BoxHitTestResult result, {required Offset position}) => super.hitTestChildren(result, position: position - Offset(0, _shift));

  @override
  void applyPaintTransform(RenderBox child, Matrix4 transform) => transform.translateByDouble(0, _shift, 0, 1);
}

class _TactilePainter extends CustomPainter {
  const _TactilePainter({
    required this.face,
    required this.border,
    required this.travel,
    required this.disabled,
    required this.fadeDisabled,
    required this.focused,
  });

  final Color face;
  final Color? border;
  final _Travel travel;
  final bool disabled;
  final bool fadeDisabled;
  final bool focused;

  /// The edge under a face: the same hue, about a third darker and a
  /// little greyer, as shade is (a vivid face would get a garish edge).
  static Color _edge(Color c) {
    final hsl = HSLColor.fromColor(c);
    return hsl.withLightness(hsl.lightness * 0.68).withSaturation(hsl.saturation * 0.6).toColor();
  }

  static Color _lit(Color c) {
    final hsl = HSLColor.fromColor(c);
    return hsl.withLightness((hsl.lightness * 1.07 + 0.02).clamp(0.0, 1.0)).toColor();
  }

  @override
  void paint(Canvas canvas, Size size) {
    // Every size of button follows one rule: corners rounded to 30% of
    // its height (12 pixels at full size), an edge of 3 pixels (2 when
    // compact), the face travelling the edge's depth.
    final depth = _edgeDepth(size.height);
    final radius = Radius.circular(math.min(12, size.height * 0.3));
    final rest = RRect.fromRectAndRadius(Offset.zero & size, radius);
    final shift = (1 - travel.up) * depth;
    final top = rest.shift(Offset(0, shift));
    if (!disabled) {
      // A soft shadow and the edge stay where the button rests; only the
      // face moves, down on to the edge or up off it.
      final shadow = Paint()
        ..color = const Color(0x59000000)
        ..maskFilter = MaskFilter.blur(BlurStyle.normal, travel.up < 0.5 ? 1.5 : 5);
      canvas.drawRRect(rest.shift(Offset(0, depth + 1)), shadow);
      canvas.drawRRect(rest.shift(Offset(0, depth)), Paint()..color = _edge(face));
    }
    final colour = disabled && fadeDisabled ? face.withValues(alpha: face.a * 0.45) : Color.lerp(face, _lit(face), travel.lit)!;
    canvas.drawRRect(top, Paint()..color = colour);
    final b = border;
    if (focused) {
      // Keyboard focus: a ring in the accent, so it shows on any face.
      canvas.drawRRect(
        top.deflate(1),
        Paint()
          ..style = PaintingStyle.stroke
          ..strokeWidth = 2
          ..color = PColors.accentText,
      );
    } else if (b != null) {
      canvas.drawRRect(
        top.deflate(0.5),
        Paint()
          ..style = PaintingStyle.stroke
          ..color = b,
      );
    }
  }

  @override
  bool shouldRepaint(_TactilePainter old) =>
      old.face != face ||
      old.border != border ||
      old.travel.up != travel.up ||
      old.travel.lit != travel.lit ||
      old.disabled != disabled ||
      old.fadeDisabled != fadeDisabled ||
      old.focused != focused;
}

ThemeData postTheme() {
  final scheme = ColorScheme.fromSeed(
    seedColor: PColors.accent,
    brightness: Brightness.dark,
    primary: PColors.accent,
    onPrimary: PColors.onAccent,
    surface: PColors.panel,
    onSurface: PColors.text,
    error: PColors.danger,
  );
  final base = ThemeData(brightness: Brightness.dark, colorScheme: scheme, fontFamily: PType.body, useMaterial3: true);
  return base.copyWith(
    scaffoldBackgroundColor: PColors.ground,
    textTheme: _textStyles(base.textTheme.apply(bodyColor: PColors.text, displayColor: PColors.text, fontFamily: PType.body)),
    primaryTextTheme: _textStyles(base.primaryTextTheme.apply(fontFamily: PType.body)),
    dividerColor: PColors.border,
    splashFactory: InkSparkle.splashFactory,
    popupMenuTheme: PopupMenuThemeData(
      color: PColors.panelRaised,
      surfaceTintColor: Colors.transparent,
      elevation: 12,
      shape: RoundedRectangleBorder(
        borderRadius: BorderRadius.circular(16),
        side: const BorderSide(color: PColors.borderStrong),
      ),
      textStyle: const TextStyle(fontFamily: PType.body, fontSize: 14, color: PColors.text),
    ),
    dialogTheme: const DialogThemeData(backgroundColor: PColors.dialog, surfaceTintColor: Colors.transparent),
    scrollbarTheme: ScrollbarThemeData(
      thumbColor: WidgetStateProperty.all(PColors.borderStrong),
      thickness: WidgetStateProperty.all(6),
      radius: const Radius.circular(3),
    ),
    checkboxTheme: CheckboxThemeData(
      mouseCursor: pClickable,
      side: const BorderSide(color: PColors.borderStrong, width: 1.5),
      shape: RoundedRectangleBorder(borderRadius: BorderRadius.circular(5)),
    ),
    iconButtonTheme: IconButtonThemeData(style: ButtonStyle(mouseCursor: pClickable)),
    segmentedButtonTheme: SegmentedButtonThemeData(style: ButtonStyle(mouseCursor: pClickable)),
    elevatedButtonTheme: ElevatedButtonThemeData(style: ButtonStyle(mouseCursor: pClickable)),
    textButtonTheme: TextButtonThemeData(
      style: TextButton.styleFrom(
        enabledMouseCursor: SystemMouseCursors.click,
        disabledMouseCursor: SystemMouseCursors.basic,
        foregroundColor: PColors.accentText,
        shape: RoundedRectangleBorder(borderRadius: BorderRadius.circular(10)),
        textStyle: const TextStyle(fontFamily: PType.body, fontWeight: FontWeight.w500, fontSize: 13.5),
      ),
    ),
    tooltipTheme: const TooltipThemeData(
      decoration: BoxDecoration(color: PColors.panelRaised, borderRadius: BorderRadius.all(Radius.circular(8))),
      textStyle: TextStyle(fontFamily: PType.body, fontSize: 12.5, color: PColors.text),
    ),
    inputDecorationTheme: const InputDecorationTheme(
      filled: true,
      fillColor: PColors.sunken,
      hintStyle: TextStyle(color: PColors.textFaint),
      contentPadding: EdgeInsets.symmetric(horizontal: 14, vertical: 14),
      border: OutlineInputBorder(
        borderRadius: BorderRadius.all(Radius.circular(PShape.controlRadius)),
        borderSide: BorderSide(color: PColors.borderStrong),
      ),
      enabledBorder: OutlineInputBorder(
        borderRadius: BorderRadius.all(Radius.circular(PShape.controlRadius)),
        borderSide: BorderSide(color: PColors.borderStrong),
      ),
      focusedBorder: OutlineInputBorder(
        borderRadius: BorderRadius.all(Radius.circular(PShape.controlRadius)),
        borderSide: BorderSide(color: PColors.accent),
      ),
    ),
    // Filled buttons are the main action, outlined ones the rest; both
    // tactile (see PButtons). Text buttons stay flat.
    // Standard density: a button is the size it says. Flutter otherwise
    // takes 8 pixels off every button on desktop, so a "32" compact button
    // came out at 24, and sizes stopped matching across the app.
    filledButtonTheme: FilledButtonThemeData(
      style: PButtons.primary.merge(
        FilledButton.styleFrom(
          visualDensity: VisualDensity.standard,
          minimumSize: const Size(40, 40),
          padding: const EdgeInsets.symmetric(horizontal: 20),
          shape: RoundedRectangleBorder(borderRadius: BorderRadius.circular(12)),
          textStyle: const TextStyle(fontFamily: PType.body, fontWeight: FontWeight.w600, fontSize: 14),
        ),
      ),
    ),
    outlinedButtonTheme: OutlinedButtonThemeData(
      style: PButtons.secondary.merge(
        OutlinedButton.styleFrom(
          visualDensity: VisualDensity.standard,
          minimumSize: const Size(40, 40),
          padding: const EdgeInsets.symmetric(horizontal: 18),
          shape: RoundedRectangleBorder(borderRadius: BorderRadius.circular(12)),
          textStyle: const TextStyle(fontFamily: PType.body, fontWeight: FontWeight.w500, fontSize: 14),
        ),
      ),
    ),
  );
}
