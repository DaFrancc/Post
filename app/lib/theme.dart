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
  static const selected = Color(0xFF1E2134);
  static const border = Color(0xFF1A1C2B);
  static const borderStrong = Color(0xFF2A2D44);

  static const text = Color(0xFFE8EAF2);
  static const textStrong = Color(0xFFF2F3F8);
  static const textBody = Color(0xFFD9DCE8);
  static const textMuted = Color(0xFFA3A8BD);
  static const textFaint = Color(0xFF8C92AA);

  static const accent = Color(0xFF6E8BFF);
  static const onAccent = Color(0xFF070812);
  static const accentSoft = Color(0xFF121A2E);
  static const accentText = Color(0xFF8EB4FF);

  static const online = Color(0xFF3DD68C);
  static const away = Color(0xFFF2B33D);
  static const dnd = Color(0xFFF25F7A);
  static const offline = Color(0xFF6B7088);
  static const danger = Color(0xFFF25F7A);

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
}

/// Type.
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
    textTheme: base.textTheme.apply(bodyColor: PColors.text, displayColor: PColors.text, fontFamily: PType.body),
    dividerColor: PColors.border,
    splashFactory: InkSparkle.splashFactory,
    tooltipTheme: const TooltipThemeData(
      decoration: BoxDecoration(color: PColors.panelRaised, borderRadius: BorderRadius.all(Radius.circular(8))),
      textStyle: TextStyle(fontFamily: PType.body, fontSize: 12.5, color: PColors.text),
    ),
    inputDecorationTheme: const InputDecorationTheme(
      filled: true,
      fillColor: PColors.sunken,
      hintStyle: TextStyle(color: PColors.textFaint),
      contentPadding: EdgeInsets.symmetric(horizontal: 14, vertical: 14),
      border: OutlineInputBorder(borderRadius: BorderRadius.all(Radius.circular(PShape.controlRadius)), borderSide: BorderSide(color: PColors.borderStrong)),
      enabledBorder: OutlineInputBorder(borderRadius: BorderRadius.all(Radius.circular(PShape.controlRadius)), borderSide: BorderSide(color: PColors.borderStrong)),
      focusedBorder: OutlineInputBorder(borderRadius: BorderRadius.all(Radius.circular(PShape.controlRadius)), borderSide: BorderSide(color: PColors.accent)),
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
