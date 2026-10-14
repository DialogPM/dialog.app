// OKLCH -> sRGB conversion and the deterministic avatar colors of the design
// (av() / nameColor() in docs/design/ChatApp-Premium.dc.html).

import 'dart:math' as math;

import 'package:flutter/painting.dart';

double _toSrgb(double x) {
  final v = x <= 0.0031308 ? 12.92 * x : 1.055 * math.pow(x, 1 / 2.4) - 0.055;
  return v.clamp(0.0, 1.0);
}

/// Converts an OKLCH color (L 0..1, C, hue in degrees) to an sRGB [Color] (gamut clipped).
Color oklch(double l, double c, double hueDeg, [double alpha = 1]) {
  final h = hueDeg * math.pi / 180;
  final a = c * math.cos(h);
  final b = c * math.sin(h);
  final l1 = l + 0.3963377774 * a + 0.2158037573 * b;
  final m1 = l - 0.1055613458 * a - 0.0638541728 * b;
  final s1 = l - 0.0894841775 * a - 1.2914855480 * b;
  final l3 = l1 * l1 * l1, m3 = m1 * m1 * m1, s3 = s1 * s1 * s1;
  final r = 4.0767416621 * l3 - 3.3077115913 * m3 + 0.2309699292 * s3;
  final g = -1.2684380046 * l3 + 2.6097574011 * m3 - 0.3413193965 * s3;
  final bl = -0.0041960863 * l3 - 0.7034186147 * m3 + 1.7076147010 * s3;
  return Color.from(alpha: alpha, red: _toSrgb(r), green: _toSrgb(g), blue: _toSrgb(bl));
}

/// Deterministic avatar hue (0-359) for an id; same formula as hueFor() on desktop.
int hueFor(String seed) {
  var h = 0;
  for (final c in seed.codeUnits) {
    h = (h * 31 + c).toSigned(32);
  }
  return h.abs() % 360;
}

/// Avatar background / foreground for a hue.
({Color bg, Color fg}) avatarColors(int hue, {required bool dark}) => dark
    ? (bg: oklch(0.33, 0.055, hue.toDouble()), fg: oklch(0.87, 0.08, hue.toDouble()))
    : (bg: oklch(0.91, 0.045, hue.toDouble()), fg: oklch(0.43, 0.1, hue.toDouble()));

/// Sender name color in group chats.
Color nameColor(int hue, {required bool dark}) =>
    dark ? oklch(0.78, 0.11, hue.toDouble()) : oklch(0.52, 0.13, hue.toDouble());
