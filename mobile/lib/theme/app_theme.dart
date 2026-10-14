// ThemeData built from the design tokens, plus text style helpers. The bundled fonts are
// variable fonts, so every style sets the "wght" axis explicitly (the design uses weights such
// as 550, 650 and 750).

import 'package:flutter/cupertino.dart';
import 'package:flutter/material.dart';

import 'tokens.dart';

const String kMonoFont = 'JetBrainsMono';

/// Prefer the color emoji fonts over other fallback fonts that also contain some symbols
/// (on Linux, U+2764 "❤" would otherwise come from a monochrome font).
const List<String> kEmojiFallback = ['Noto Color Emoji', 'Apple Color Emoji', 'Segoe UI Emoji'];

FontWeight _nearestWeight(double w) {
  final i = ((w / 100).round() - 1).clamp(0, 8);
  return FontWeight.values[i];
}

/// Text style with an exact variable-font weight. The font family is inherited from the theme
/// unless [mono] is set.
TextStyle ts(
  double size, {
  double weight = 400,
  Color? color,
  double? height,
  double? letterSpacing,
  bool mono = false,
  FontStyle? fontStyle,
  List<Shadow>? shadows,
  TextDecoration? decoration,
}) =>
    TextStyle(
      fontSize: size,
      fontWeight: _nearestWeight(weight),
      fontVariations: [FontVariation.weight(weight)],
      color: color,
      height: height,
      letterSpacing: letterSpacing,
      fontFamily: mono ? kMonoFont : null,
      fontFamilyFallback: kEmojiFallback,
      fontStyle: fontStyle,
      shadows: shadows,
      decoration: decoration,
      decorationColor: color,
    );

ThemeData buildTheme(ChatTokens t) {
  final brightness = t.dark ? Brightness.dark : Brightness.light;
  TextStyle base(double size, double weight) => ts(size, weight: weight, color: t.ink).copyWith(fontFamilyFallback: kEmojiFallback);
  final textTheme = TextTheme(
    displayLarge: base(44, 700),
    displayMedium: base(36, 700),
    displaySmall: base(30, 700),
    headlineLarge: base(27, 700),
    headlineMedium: base(25, 750),
    headlineSmall: base(22, 700),
    titleLarge: base(18, 700),
    titleMedium: base(16, 650),
    titleSmall: base(14.5, 650),
    bodyLarge: base(15, 400),
    bodyMedium: base(14.5, 400),
    bodySmall: base(12.5, 400),
    labelLarge: base(14.5, 600),
    labelMedium: base(13, 600),
    labelSmall: base(11.5, 600),
  );
  final scheme = ColorScheme(
    brightness: brightness,
    primary: t.accent,
    onPrimary: t.onAccent,
    primaryContainer: t.accentSoft,
    onPrimaryContainer: t.accentText,
    secondary: t.accentText,
    onSecondary: t.onAccent,
    error: t.danger,
    onError: Colors.white,
    surface: t.pane,
    onSurface: t.ink,
    onSurfaceVariant: t.muted,
    surfaceContainerHighest: t.sunk,
    surfaceContainerHigh: t.sunk,
    surfaceContainer: t.pane,
    surfaceContainerLow: t.pane,
    surfaceContainerLowest: t.pane,
    outline: t.line,
    outlineVariant: t.line,
    shadow: Colors.black,
    inverseSurface: t.ink,
    onInverseSurface: t.pane,
  );
  return ThemeData(
    useMaterial3: true,
    brightness: brightness,
    colorScheme: scheme,
    fontFamily: t.font,
    textTheme: textTheme,
    primaryTextTheme: textTheme,
    scaffoldBackgroundColor: t.pane,
    canvasColor: t.pane,
    dividerColor: t.line,
    hoverColor: t.hover,
    highlightColor: t.hover,
    splashColor: t.hover.withValues(alpha: .6),
    splashFactory: InkRipple.splashFactory,
    extensions: [t],
    textSelectionTheme: TextSelectionThemeData(
      cursorColor: t.accent,
      selectionColor: t.accent.withValues(alpha: .25),
      selectionHandleColor: t.accent,
    ),
    progressIndicatorTheme: ProgressIndicatorThemeData(color: t.accent, linearTrackColor: t.sunk),
    dialogTheme: DialogThemeData(
      backgroundColor: t.pane,
      surfaceTintColor: Colors.transparent,
      shape: RoundedRectangleBorder(borderRadius: BorderRadius.circular(t.rModal)),
      titleTextStyle: ts(18, weight: 700, color: t.ink),
      contentTextStyle: ts(14, color: t.muted, height: 1.45),
      barrierColor: const Color.fromRGBO(8, 10, 10, .5),
    ),
    bottomSheetTheme: BottomSheetThemeData(
      backgroundColor: t.pane,
      surfaceTintColor: Colors.transparent,
      modalBarrierColor: const Color.fromRGBO(8, 10, 10, .5),
      shape: RoundedRectangleBorder(borderRadius: BorderRadius.vertical(top: Radius.circular(t.rModal))),
    ),
    snackBarTheme: SnackBarThemeData(
      behavior: SnackBarBehavior.floating,
      backgroundColor: t.ink,
      contentTextStyle: ts(14, weight: 550, color: t.pane),
      shape: RoundedRectangleBorder(borderRadius: BorderRadius.circular(t.rCard)),
    ),
    popupMenuTheme: PopupMenuThemeData(
      color: t.pane,
      surfaceTintColor: Colors.transparent,
      shape: RoundedRectangleBorder(borderRadius: BorderRadius.circular(t.rCard)),
      textStyle: ts(14.5, weight: 550, color: t.ink),
    ),
    tooltipTheme: TooltipThemeData(
      decoration: BoxDecoration(color: t.ink, borderRadius: BorderRadius.circular(8)),
      textStyle: ts(12.5, weight: 550, color: t.pane),
    ),
    pageTransitionsTheme: const PageTransitionsTheme(builders: {
      TargetPlatform.android: CupertinoPageTransitionsBuilder(),
      TargetPlatform.iOS: CupertinoPageTransitionsBuilder(),
      TargetPlatform.linux: CupertinoPageTransitionsBuilder(),
      TargetPlatform.macOS: CupertinoPageTransitionsBuilder(),
      TargetPlatform.windows: CupertinoPageTransitionsBuilder(),
    }),
  );
}
