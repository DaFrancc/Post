// Postern's look, defined once. Every screen takes its colours, shapes and
// type from here, so changing the look is an edit to this file.
//
// Direction: Discord's layout, made more modern: separate rounded panels on a
// near-black ground, generous spacing, colour used sparingly. Accents come
// from the (placeholder) icon's cyan-blue-violet.

import 'package:flutter/material.dart';

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

  static const title = TextStyle(fontFamily: display, fontWeight: FontWeight.w600, fontSize: 20, letterSpacing: -0.2, color: PColors.text);
  static const heading = TextStyle(fontFamily: display, fontWeight: FontWeight.w600, fontSize: 18, letterSpacing: -0.2, color: PColors.text);
  static const label = TextStyle(fontFamily: body, fontWeight: FontWeight.w600, fontSize: 11, letterSpacing: 0.7, color: PColors.textFaint);
  static const name = TextStyle(fontFamily: body, fontWeight: FontWeight.w600, fontSize: 14, color: PColors.textStrong);
  static const text = TextStyle(fontFamily: body, fontSize: 14, height: 1.5, color: PColors.textBody);
  static const small = TextStyle(fontFamily: body, fontSize: 12.5, color: PColors.textFaint);
}

/// Extra line height split evenly above and below the text, as CSS does.
/// Flutter's default splits it by the font's ascent and descent, and Inter's
/// tall ascent then pushes text low in its line: visibly off-centre in
/// fields and buttons. Every text style inherits this from the theme's
/// default text style unless it sets its own.
TextTheme _evenLeading(TextTheme t) {
  TextStyle? even(TextStyle? s) => s?.copyWith(leadingDistribution: TextLeadingDistribution.even);
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

ThemeData posternTheme() {
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
    textTheme: _evenLeading(base.textTheme.apply(bodyColor: PColors.text, displayColor: PColors.text, fontFamily: PType.body)),
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
      side: const BorderSide(color: PColors.borderStrong, width: 1.5),
      shape: RoundedRectangleBorder(borderRadius: BorderRadius.circular(5)),
    ),
    textButtonTheme: TextButtonThemeData(
      style: TextButton.styleFrom(
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
    filledButtonTheme: FilledButtonThemeData(
      style: FilledButton.styleFrom(
        backgroundColor: PColors.accent,
        foregroundColor: PColors.onAccent,
        minimumSize: const Size(44, 44),
        padding: const EdgeInsets.symmetric(horizontal: 20),
        shape: RoundedRectangleBorder(borderRadius: BorderRadius.circular(14)),
        textStyle: const TextStyle(fontFamily: PType.body, fontWeight: FontWeight.w600, fontSize: 14),
      ),
    ),
    outlinedButtonTheme: OutlinedButtonThemeData(
      style: OutlinedButton.styleFrom(
        foregroundColor: PColors.textBody,
        minimumSize: const Size(44, 44),
        padding: const EdgeInsets.symmetric(horizontal: 18),
        side: const BorderSide(color: PColors.borderStrong),
        shape: RoundedRectangleBorder(borderRadius: BorderRadius.circular(14)),
        textStyle: const TextStyle(fontFamily: PType.body, fontWeight: FontWeight.w500, fontSize: 14),
      ),
    ),
  );
}
