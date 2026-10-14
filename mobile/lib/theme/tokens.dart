// Design tokens of docs/design/ChatApp-Premium.dc.html (TOK and STRUCT) for the two visual
// directions (A "Clasic cald", B "Panouri flotante"), light and dark, with the mobile overrides
// (no shell padding / pane gap / pane radius / divider).

import 'package:flutter/material.dart';

enum Direction { a, b }

Color _c(int argb) => Color(argb);

/// Colors of one direction + theme (TOK[dir][theme]).
class _Palette {
  final Color canvas, pane, sunk, hover, active, line, ink, muted, faint;
  final Color accent, accentHover, onAccent, accentSoft, accentText;
  final Color chatBg, bubbleIn, bubbleInFg, bubbleOut, bubbleOutFg, outMeta, inMeta, tick;
  final Color replyIn, replyOut, sysBg, sysFg, brandPanel, brandInk, brandMuted, danger;
  final Gradient? bubbleOutGradient;

  const _Palette({
    required this.canvas,
    required this.pane,
    required this.sunk,
    required this.hover,
    required this.active,
    required this.line,
    required this.ink,
    required this.muted,
    required this.faint,
    required this.accent,
    required this.accentHover,
    required this.onAccent,
    required this.accentSoft,
    required this.accentText,
    required this.chatBg,
    required this.bubbleIn,
    required this.bubbleInFg,
    required this.bubbleOut,
    required this.bubbleOutFg,
    required this.outMeta,
    required this.inMeta,
    required this.tick,
    required this.replyIn,
    required this.replyOut,
    required this.sysBg,
    required this.sysFg,
    required this.brandPanel,
    required this.brandInk,
    required this.brandMuted,
    required this.danger,
    this.bubbleOutGradient,
  });
}

// linear-gradient(135deg, ...) in CSS goes from the top-left to the bottom-right corner.
LinearGradient _grad135(Color a, Color b) =>
    LinearGradient(begin: Alignment.topLeft, end: Alignment.bottomRight, colors: [a, b]);

final _aLight = _Palette(
  canvas: _c(0xFFEEEAE2), pane: _c(0xFFFBFAF7), sunk: _c(0xFFF2EFE8), hover: _c(0xFFEFEBE3), active: _c(0xFFE3EEE7),
  line: _c(0xFFE2DDD2), ink: _c(0xFF1C1B18), muted: _c(0xFF6B665C), faint: _c(0xFF9A9488),
  accent: _c(0xFF0F7A5E), accentHover: _c(0xFF0B684F), onAccent: _c(0xFFFFFFFF), accentSoft: _c(0xFFDCEDE3), accentText: _c(0xFF0B6B52),
  chatBg: _c(0xFFF3EFE7), bubbleIn: _c(0xFFFFFFFF), bubbleInFg: _c(0xFF1C1B18), bubbleOut: _c(0xFFD8EBDF), bubbleOutFg: _c(0xFF14261D),
  outMeta: _c(0xFF557062), inMeta: _c(0xFF8A8479), tick: _c(0xFF0F7A5E),
  replyIn: _c(0xFFF2EFE8), replyOut: const Color.fromRGBO(15, 122, 94, .10), sysBg: _c(0xFFF6EBCB), sysFg: _c(0xFF6A5419),
  brandPanel: _c(0xFF12382D), brandInk: _c(0xFFF3EEE3), brandMuted: _c(0xFFA9C1B5), danger: _c(0xFFB3261E),
);

final _aDark = _Palette(
  canvas: _c(0xFF0E0E0C), pane: _c(0xFF181715), sunk: _c(0xFF211F1C), hover: _c(0xFF23211E), active: _c(0xFF1C2C25),
  line: _c(0xFF2C2A26), ink: _c(0xFFEDE9E1), muted: _c(0xFFA29C90), faint: _c(0xFF77726A),
  accent: _c(0xFF3DBE92), accentHover: _c(0xFF52CCA2), onAccent: _c(0xFF08170F), accentSoft: _c(0xFF1B3028), accentText: _c(0xFF62D4AA),
  chatBg: _c(0xFF12110F), bubbleIn: _c(0xFF22211E), bubbleInFg: _c(0xFFEDE9E1), bubbleOut: _c(0xFF1B392D), bubbleOutFg: _c(0xFFE4F2EA),
  outMeta: _c(0xFF8DB5A2), inMeta: _c(0xFF8D877C), tick: _c(0xFF62D4AA),
  replyIn: _c(0xFF2B2926), replyOut: const Color.fromRGBO(61, 190, 146, .14), sysBg: _c(0xFF2A2415), sysFg: _c(0xFFD9C58E),
  brandPanel: _c(0xFF0B1F18), brandInk: _c(0xFFEDE9E1), brandMuted: _c(0xFF8FAA9D), danger: _c(0xFFF07167),
);

final _bLight = _Palette(
  canvas: _c(0xFFE5E9ED), pane: _c(0xFFFFFFFF), sunk: _c(0xFFF1F4F6), hover: _c(0xFFEEF2F5), active: _c(0xFFE3F4EE),
  line: _c(0xFFE3E8EC), ink: _c(0xFF0E181D), muted: _c(0xFF5C6C75), faint: _c(0xFF93A1A9),
  accent: _c(0xFF0A9676), accentHover: _c(0xFF088266), onAccent: _c(0xFFFFFFFF), accentSoft: _c(0xFFDBF2EA), accentText: _c(0xFF07785F),
  chatBg: _c(0xFFFFFFFF), bubbleIn: _c(0xFFEEF2F5), bubbleInFg: _c(0xFF0E181D), bubbleOut: _c(0xFF0FA283), bubbleOutFg: _c(0xFFFFFFFF),
  outMeta: const Color.fromRGBO(255, 255, 255, .8), inMeta: _c(0xFF7B8A92), tick: _c(0xFFFFFFFF),
  replyIn: _c(0xFFE1E8EC), replyOut: const Color.fromRGBO(255, 255, 255, .17), sysBg: _c(0xFFF1F4F6), sysFg: _c(0xFF5C6C75),
  brandPanel: _c(0xFF0B1417), brandInk: _c(0xFFEAF3F2), brandMuted: _c(0xFF8FA7A6), danger: _c(0xFFD93A30),
  bubbleOutGradient: _grad135(_c(0xFF0FA283), _c(0xFF0B8597)),
);

final _bDark = _Palette(
  canvas: _c(0xFF05080A), pane: _c(0xFF0F161A), sunk: _c(0xFF162026), hover: _c(0xFF18232A), active: _c(0xFF12302A),
  line: _c(0xFF1E2A30), ink: _c(0xFFE5EDEF), muted: _c(0xFF8799A1), faint: _c(0xFF5E6F77),
  accent: _c(0xFF27C99E), accentHover: _c(0xFF3ED8AE), onAccent: _c(0xFF03140F), accentSoft: _c(0xFF123028), accentText: _c(0xFF4ADBB3),
  chatBg: _c(0xFF0F161A), bubbleIn: _c(0xFF19242B), bubbleInFg: _c(0xFFE5EDEF), bubbleOut: _c(0xFF0E8C72), bubbleOutFg: _c(0xFFF2FBF9),
  outMeta: const Color.fromRGBO(242, 251, 249, .74), inMeta: _c(0xFF7D9099), tick: _c(0xFFFFFFFF),
  replyIn: _c(0xFF223038), replyOut: const Color.fromRGBO(255, 255, 255, .13), sysBg: _c(0xFF162026), sysFg: _c(0xFF8799A1),
  brandPanel: _c(0xFF0A1216), brandInk: _c(0xFFE5EDEF), brandMuted: _c(0xFF7F979C), danger: _c(0xFFFF6B61),
  bubbleOutGradient: _grad135(_c(0xFF0E8C72), _c(0xFF0A6C80)),
);

/// All tokens of the active direction and theme, available as a [ThemeExtension].
@immutable
class ChatTokens extends ThemeExtension<ChatTokens> {
  const ChatTokens({
    required this.direction,
    required this.dark,
    required this.canvas,
    required this.pane,
    required this.sunk,
    required this.hover,
    required this.active,
    required this.line,
    required this.ink,
    required this.muted,
    required this.faint,
    required this.accent,
    required this.accentHover,
    required this.onAccent,
    required this.accentSoft,
    required this.accentText,
    required this.chatBg,
    required this.bubbleIn,
    required this.bubbleInFg,
    required this.bubbleOut,
    required this.bubbleOutGradient,
    required this.bubbleOutFg,
    required this.outMeta,
    required this.inMeta,
    required this.tick,
    required this.replyIn,
    required this.replyOut,
    required this.sysBg,
    required this.sysFg,
    required this.brandPanel,
    required this.brandInk,
    required this.brandMuted,
    required this.danger,
    required this.font,
    required this.rBubble,
    required this.rCard,
    required this.rInput,
    required this.rChip,
    required this.rowRadius,
    required this.rowMx,
    required this.rowPx,
    required this.sectMx,
    required this.sectR,
    required this.sectBg,
    required this.sectSep,
    required this.rModal,
    required this.bubbleShadow,
    required this.wallDot,
    required this.replyBarOut,
    required this.linkOut,
    required this.playBgOut,
    required this.playFgOut,
    required this.waveOnOut,
    required this.waveOffOut,
    required this.mapBg,
  });

  final Direction direction;
  final bool dark;

  // TOK
  final Color canvas, pane, sunk, hover, active, line, ink, muted, faint;
  final Color accent, accentHover, onAccent, accentSoft, accentText;
  final Color chatBg, bubbleIn, bubbleInFg, bubbleOut, bubbleOutFg, outMeta, inMeta, tick;

  /// Direction B: gradient of outgoing bubbles (null in A).
  final Gradient? bubbleOutGradient;
  final Color replyIn, replyOut, sysBg, sysFg, brandPanel, brandInk, brandMuted, danger;

  // STRUCT (with mobile overrides)
  final String font;
  final double rBubble, rCard, rInput, rChip, rowRadius, rowMx, rowPx, sectMx, sectR, rModal;
  final Color sectBg;

  /// Direction A: 8px band of `sunk` between info sections (0 in B).
  final double sectSep;
  final List<BoxShadow> bubbleShadow;

  /// Direction A: color of the dotted chat wallpaper (null in B = no wallpaper).
  final Color? wallDot;

  // Outgoing bubble details (differ in B because of the gradient).
  final Color replyBarOut, linkOut, playBgOut, playFgOut, waveOnOut, waveOffOut;

  /// Base color of the location placeholder grid.
  final Color mapBg;

  bool get isB => direction == Direction.b;

  static ChatTokens of(BuildContext context) => Theme.of(context).extension<ChatTokens>()!;

  factory ChatTokens.build(Direction direction, {required bool dark}) {
    final b = direction == Direction.b;
    final p = b ? (dark ? _bDark : _bLight) : (dark ? _aDark : _aLight);
    return ChatTokens(
      direction: direction,
      dark: dark,
      canvas: p.canvas,
      pane: p.pane,
      sunk: p.sunk,
      hover: p.hover,
      active: p.active,
      line: p.line,
      ink: p.ink,
      muted: p.muted,
      faint: p.faint,
      accent: p.accent,
      accentHover: p.accentHover,
      onAccent: p.onAccent,
      accentSoft: p.accentSoft,
      accentText: p.accentText,
      chatBg: p.chatBg,
      bubbleIn: p.bubbleIn,
      bubbleInFg: p.bubbleInFg,
      bubbleOut: p.bubbleOut,
      bubbleOutGradient: p.bubbleOutGradient,
      bubbleOutFg: p.bubbleOutFg,
      outMeta: p.outMeta,
      inMeta: p.inMeta,
      tick: p.tick,
      replyIn: p.replyIn,
      replyOut: p.replyOut,
      sysBg: p.sysBg,
      sysFg: p.sysFg,
      brandPanel: p.brandPanel,
      brandInk: p.brandInk,
      brandMuted: p.brandMuted,
      danger: p.danger,
      font: b ? 'Figtree' : 'PlusJakartaSans',
      rBubble: b ? 20 : 14,
      rCard: b ? 18 : 14,
      rInput: b ? 999 : 12,
      rChip: b ? 999 : 8,
      rowRadius: b ? 14 : 0,
      rowMx: b ? 8 : 0,
      rowPx: b ? 12 : 20,
      sectMx: b ? 16 : 0,
      sectR: b ? 16 : 0,
      sectBg: b ? p.sunk : p.pane,
      sectSep: b ? 0 : 8,
      rModal: b ? 28 : 18,
      bubbleShadow: b ? const [] : const [BoxShadow(color: Color.fromRGBO(40, 30, 10, .08), offset: Offset(0, 1), blurRadius: 1.5)],
      wallDot: b ? null : (dark ? const Color.fromRGBO(255, 255, 255, .035) : const Color.fromRGBO(70, 55, 20, .075)),
      replyBarOut: b ? const Color.fromRGBO(255, 255, 255, .92) : p.accentText,
      linkOut: b ? Colors.white : p.accentText,
      playBgOut: b ? const Color.fromRGBO(255, 255, 255, .95) : p.accent,
      playFgOut: b ? const Color(0xFF0B8597) : p.onAccent,
      waveOnOut: b ? Colors.white : p.accent,
      waveOffOut: b ? const Color.fromRGBO(255, 255, 255, .42) : p.faint,
      mapBg: dark ? const Color(0xFF26302C) : const Color(0xFFE6EDE4),
    );
  }

  /// Radius for inputs/buttons of a given height (999 = pill).
  BorderRadius inputRadius([double height = 50]) => BorderRadius.circular(rInput >= 999 ? height / 2 : rInput);
  BorderRadius chipRadius([double height = 32]) => BorderRadius.circular(rChip >= 999 ? height / 2 : rChip);

  @override
  ChatTokens copyWith() => this;

  @override
  ChatTokens lerp(ChatTokens? other, double t) => t < 0.5 ? this : (other ?? this);
}
