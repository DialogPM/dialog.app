// Small building blocks in the design's style (buttons, fields, switches, cards, headers...).

import 'dart:io';
import 'dart:ui' as ui;

import 'package:flutter/material.dart';
import 'package:flutter/services.dart';

import '../app/globals.dart';
import '../services/media.dart';
import '../l10n/i18n.dart';
import '../theme/app_theme.dart';
import '../theme/oklch.dart';
import '../theme/tokens.dart';
import '../util/format.dart';
import 'app_icon.dart';

extension TokensContext on BuildContext {
  ChatTokens get tk => ChatTokens.of(this);
}

// ---------------------------------------------------------------------------
// avatar
// ---------------------------------------------------------------------------

const Map<int, double> _avatarFont = {22: 9, 28: 11, 36: 12.5, 38: 13, 40: 13.5, 42: 14.5, 44: 15, 50: 16, 58: 19, 96: 31, 112: 36, 124: 40};

/// Round avatar: photo when available, otherwise initials on an oklch tint derived from the id.
class Avatar extends StatelessWidget {
  const Avatar({super.key, required this.name, this.path, this.seed, this.size = 44, this.group = false});

  final String name;
  final String? path;
  final String? seed;
  final double size;
  final bool group;

  @override
  Widget build(BuildContext context) {
    final t = context.tk;
    final colors = avatarColors(hueFor(seed ?? name), dark: t.dark);
    final text = initials(name);
    final p = path;
    Widget child;
    if (p != null && p.isNotEmpty && File(p).existsSync()) {
      child = MediaImage(p, width: size, height: size, fit: BoxFit.cover, cacheWidth: (size * 3).round());
    } else if (group && (text == '?' || text == '#' || text.trim().isEmpty)) {
      child = Center(child: AppIcon('users', size: (size * 0.45).roundToDouble(), color: colors.fg));
    } else {
      final fs = _avatarFont[size.round()] ?? (size * 0.33 * 2).round() / 2;
      child = Center(child: Text(text, style: ts(fs, weight: 700, color: colors.fg, height: 1)));
    }
    return Semantics(
      label: name,
      image: true,
      child: ClipOval(
        child: Container(width: size, height: size, color: colors.bg, child: child),
      ),
    );
  }
}

// ---------------------------------------------------------------------------
// buttons
// ---------------------------------------------------------------------------

/// Pressable surface with the design's hover/pressed feedback.
class Tap extends StatelessWidget {
  const Tap({super.key, required this.child, this.onTap, this.onLongPress, this.radius, this.color, this.padding, this.semanticLabel});

  final Widget child;
  final VoidCallback? onTap;
  final VoidCallback? onLongPress;
  final BorderRadius? radius;
  final Color? color;
  final EdgeInsetsGeometry? padding;
  final String? semanticLabel;

  @override
  Widget build(BuildContext context) {
    final w = Material(
      color: color ?? Colors.transparent,
      borderRadius: radius,
      clipBehavior: radius != null ? Clip.antiAlias : Clip.none,
      child: InkWell(
        onTap: onTap,
        onLongPress: onLongPress,
        borderRadius: radius,
        child: padding != null ? Padding(padding: padding!, child: child) : child,
      ),
    );
    return semanticLabel == null ? w : Semantics(button: true, label: semanticLabel, child: w);
  }
}

enum BtnKind { primary, secondary, danger, soft, ghost }

/// Filled button (.btn): 50px high by default, radius r-input.
class Btn extends StatelessWidget {
  const Btn({
    super.key,
    required this.label,
    this.onPressed,
    this.kind = BtnKind.primary,
    this.icon,
    this.height = 50,
    this.fontSize,
    this.expand = true,
    this.busy = false,
  });

  final String label;
  final VoidCallback? onPressed;
  final BtnKind kind;
  final String? icon;
  final double height;
  final double? fontSize;
  final bool expand;
  final bool busy;

  @override
  Widget build(BuildContext context) {
    final t = context.tk;
    final disabled = onPressed == null || busy;
    Color bg, fg;
    switch (kind) {
      case BtnKind.primary:
        bg = disabled ? t.sunk : t.accent;
        fg = disabled ? t.faint : t.onAccent;
      case BtnKind.secondary:
        bg = t.sunk;
        fg = disabled ? t.faint : t.ink;
      case BtnKind.danger:
        bg = disabled ? t.sunk : t.danger;
        fg = disabled ? t.faint : Colors.white;
      case BtnKind.soft:
        bg = t.accentSoft;
        fg = t.accentText;
      case BtnKind.ghost:
        bg = Colors.transparent;
        fg = disabled ? t.faint : t.danger;
    }
    final fs = fontSize ?? (height >= 50 ? 15.5 : height >= 48 ? 15 : 14);
    final content = Row(
      mainAxisSize: expand ? MainAxisSize.max : MainAxisSize.min,
      mainAxisAlignment: MainAxisAlignment.center,
      children: [
        if (busy)
          Padding(padding: const EdgeInsets.only(right: 8), child: SizedBox.square(dimension: 16, child: CircularProgressIndicator(strokeWidth: 2, color: fg)))
        else if (icon != null)
          Padding(padding: const EdgeInsets.only(right: 8), child: AppIcon(icon!, size: 18, color: fg)),
        Flexible(
          child: Text(label,
              maxLines: 1,
              overflow: TextOverflow.ellipsis,
              style: ts(fs, weight: kind == BtnKind.secondary ? 600 : 650, color: fg)),
        ),
      ],
    );
    return SizedBox(
      height: height,
      width: expand ? double.infinity : null,
      child: Tap(
        onTap: disabled ? null : onPressed,
        color: bg,
        radius: t.inputRadius(height),
        padding: const EdgeInsets.symmetric(horizontal: 18),
        child: content,
      ),
    );
  }
}

/// Small pill/chip button (.chip-btn), e.g. "Copy".
class ChipBtn extends StatelessWidget {
  const ChipBtn({super.key, required this.label, this.icon, this.onPressed, this.tone = 'accent'});

  final String label;
  final String? icon;
  final VoidCallback? onPressed;

  /// "accent" | "pane" | "sunk" | "danger"
  final String tone;

  @override
  Widget build(BuildContext context) {
    final t = context.tk;
    final bg = switch (tone) { 'pane' => t.pane, 'sunk' || 'danger' => t.sunk, _ => t.accentSoft };
    final fg = tone == 'danger' ? t.danger : t.accentText;
    final h = tone == 'danger' ? 32.0 : 34.0;
    return SizedBox(
      height: h,
      child: Tap(
        onTap: onPressed,
        color: bg,
        radius: t.chipRadius(h),
        padding: const EdgeInsets.symmetric(horizontal: 12),
        child: Row(mainAxisSize: MainAxisSize.min, children: [
          if (icon != null) ...[AppIcon(icon!, size: 15, color: fg), const SizedBox(width: 6)],
          Text(label, style: ts(13, weight: 650, color: fg)),
        ]),
      ),
    );
  }
}

/// Text-only button (.link-btn).
class LinkBtn extends StatelessWidget {
  const LinkBtn({super.key, required this.label, this.onPressed, this.color, this.fontSize = 14, this.weight = 600, this.underline = false});

  final String label;
  final VoidCallback? onPressed;
  final Color? color;
  final double fontSize;
  final double weight;
  final bool underline;

  @override
  Widget build(BuildContext context) {
    final c = color ?? context.tk.accentText;
    return GestureDetector(
      behavior: HitTestBehavior.opaque,
      onTap: onPressed,
      child: Padding(
        padding: const EdgeInsets.symmetric(vertical: 4),
        child: Text(label,
            textAlign: TextAlign.center,
            style: ts(fontSize, weight: weight, color: c, decoration: underline ? TextDecoration.underline : null)),
      ),
    );
  }
}

/// "← Back" link of the onboarding screens.
class BackLink extends StatelessWidget {
  const BackLink({super.key, required this.onPressed});
  final VoidCallback onPressed;

  @override
  Widget build(BuildContext context) {
    final t = context.tk;
    return Align(
      alignment: Alignment.centerLeft,
      child: GestureDetector(
        behavior: HitTestBehavior.opaque,
        onTap: onPressed,
        child: Row(mainAxisSize: MainAxisSize.min, children: [
          AppIcon('back', size: 16, color: t.muted),
          const SizedBox(width: 6),
          Text(context.t('common.back'), style: ts(14, weight: 600, color: t.muted)),
        ]),
      ),
    );
  }
}

/// Square icon button (.icon-btn), 40x40 radius 12 by default.
class IconBtn extends StatelessWidget {
  const IconBtn(this.icon, {super.key, this.onPressed, this.size = 40, this.iconSize = 19, this.color, this.bg, this.tooltip, this.round = false});

  final String icon;
  final VoidCallback? onPressed;
  final double size;
  final double iconSize;
  final Color? color;
  final Color? bg;
  final String? tooltip;
  final bool round;

  @override
  Widget build(BuildContext context) {
    final t = context.tk;
    final w = SizedBox.square(
      dimension: size,
      child: Tap(
        onTap: onPressed,
        color: bg,
        radius: BorderRadius.circular(round ? size / 2 : 12),
        semanticLabel: tooltip,
        child: Center(child: AppIcon(icon, size: iconSize, color: color ?? t.ink)),
      ),
    );
    return tooltip == null ? w : Tooltip(message: tooltip!, child: w);
  }
}

// ---------------------------------------------------------------------------
// form fields
// ---------------------------------------------------------------------------

class FieldLabel extends StatelessWidget {
  const FieldLabel(this.text, {super.key, this.muted = false});
  final String text;
  final bool muted;

  @override
  Widget build(BuildContext context) =>
      Text(text, style: ts(13, weight: 600, color: muted ? context.tk.muted : context.tk.ink));
}

/// Text input in the design's style (.field-input): 1.5px border, r-input radius.
class AppField extends StatefulWidget {
  const AppField({
    super.key,
    this.controller,
    this.label,
    this.hint,
    this.description,
    this.error,
    this.mono = false,
    this.large = false,
    this.password = false,
    this.autofocus = false,
    this.keyboardType,
    this.inputFormatters,
    this.onChanged,
    this.onSubmitted,
    this.maxLength,
    this.maxLines = 1,
    this.enabled = true,
    this.mutedLabel = false,
    this.fontSize,
    this.letterSpacing,
    this.textInputAction,
    this.focusNode,
    this.sunk = false,
    this.suffix,
    this.onEditingComplete,
  });

  final TextEditingController? controller;
  final String? label;
  final String? hint;
  final String? description;
  final String? error;
  final bool mono;
  final bool large;
  final bool password;
  final bool autofocus;
  final TextInputType? keyboardType;
  final List<TextInputFormatter>? inputFormatters;
  final ValueChanged<String>? onChanged;
  final ValueChanged<String>? onSubmitted;
  final int? maxLength;
  final int? maxLines;
  final bool enabled;
  final bool mutedLabel;
  final double? fontSize;
  final double? letterSpacing;
  final TextInputAction? textInputAction;
  final FocusNode? focusNode;
  final bool sunk;
  final Widget? suffix;
  final VoidCallback? onEditingComplete;

  @override
  State<AppField> createState() => _AppFieldState();
}

class _AppFieldState extends State<AppField> {
  bool _visible = false;

  @override
  Widget build(BuildContext context) {
    final t = context.tk;
    final h = widget.large ? 50.0 : 48.0;
    final multiline = widget.maxLines != 1;
    final radius = multiline ? BorderRadius.circular(t.rCard) : t.inputRadius(h);
    final border = OutlineInputBorder(borderRadius: radius, borderSide: BorderSide(color: widget.error != null ? t.danger : t.line, width: 1.5));
    final fs = widget.fontSize ?? (widget.mono ? (widget.large ? 18 : 16) : 15);
    final field = TextField(
      enableIMEPersonalizedLearning: imePersonalizedLearning,
      controller: widget.controller,
      focusNode: widget.focusNode,
      autofocus: widget.autofocus,
      obscureText: widget.password && !_visible,
      enableSuggestions: !widget.password && !widget.mono,
      autocorrect: !widget.password && !widget.mono,
      keyboardType: widget.keyboardType ?? (multiline ? TextInputType.multiline : null),
      inputFormatters: [
        ...?widget.inputFormatters,
        if (widget.maxLength != null) LengthLimitingTextInputFormatter(widget.maxLength),
      ],
      onChanged: widget.onChanged,
      onSubmitted: widget.onSubmitted,
      onEditingComplete: widget.onEditingComplete,
      textInputAction: widget.textInputAction,
      enabled: widget.enabled,
      maxLines: widget.password ? 1 : widget.maxLines,
      minLines: multiline ? 3 : null,
      style: ts(fs, weight: widget.mono ? 500 : 400, color: t.ink, mono: widget.mono, letterSpacing: widget.letterSpacing ?? (widget.mono ? fs * 0.06 : null)),
      cursorColor: t.accent,
      decoration: InputDecoration(
        isDense: true,
        hintText: widget.hint ?? (widget.password ? '••••••••' : null),
        hintStyle: ts(fs, color: t.ink.withValues(alpha: .5), mono: widget.mono),
        filled: true,
        fillColor: widget.sunk ? t.sunk : t.pane,
        contentPadding: EdgeInsets.symmetric(horizontal: 16, vertical: multiline ? 12 : (h - fs * 1.35) / 2),
        border: border,
        enabledBorder: border,
        disabledBorder: border,
        focusedBorder: border.copyWith(borderSide: BorderSide(color: widget.error != null ? t.danger : t.accent, width: 1.5)),
        suffixIcon: widget.password
            ? IconButton(
                onPressed: () => setState(() => _visible = !_visible),
                tooltip: context.t('password.toggle'),
                icon: AppIcon(_visible ? 'eyeoff' : 'eye', size: 18, color: t.muted),
              )
            : widget.suffix,
      ),
    );
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      mainAxisSize: MainAxisSize.min,
      children: [
        if (widget.label != null) ...[FieldLabel(widget.label!, muted: widget.mutedLabel), const SizedBox(height: 8)],
        field,
        if (widget.error != null) ...[
          const SizedBox(height: 6),
          Text(widget.error!, style: ts(12.5, color: t.danger, height: 1.4)),
        ] else if (widget.description != null) ...[
          const SizedBox(height: 6),
          Text(widget.description!, style: ts(12.5, color: t.muted, height: 1.45)),
        ],
      ],
    );
  }
}

/// Formats a 16-digit account number in groups of four while typing.
class AccountNumberFormatter extends TextInputFormatter {
  @override
  TextEditingValue formatEditUpdate(TextEditingValue oldValue, TextEditingValue newValue) {
    final digits = digitsOnly(newValue.text);
    final capped = digits.length > 16 ? digits.substring(0, 16) : digits;
    final text = formatAccountNumber(capped);
    return TextEditingValue(text: text, selection: TextSelection.collapsed(offset: text.length));
  }
}

class AccountNumberField extends StatelessWidget {
  const AccountNumberField({super.key, required this.controller, this.label, this.autofocus = false, this.large = true, this.onChanged});

  final TextEditingController controller;
  final String? label;
  final bool autofocus;
  final bool large;
  final ValueChanged<String>? onChanged;

  @override
  Widget build(BuildContext context) => AppField(
        controller: controller,
        label: label,
        hint: '0000 0000 0000 0000',
        mono: true,
        large: large,
        autofocus: autofocus,
        keyboardType: TextInputType.number,
        inputFormatters: [AccountNumberFormatter()],
        onChanged: onChanged,
      );
}

// ---------------------------------------------------------------------------
// switches, checks, radios
// ---------------------------------------------------------------------------

/// 42x25 track with a 19px knob.
class ToggleSwitch extends StatelessWidget {
  const ToggleSwitch({super.key, required this.on});
  final bool on;

  @override
  Widget build(BuildContext context) {
    final t = context.tk;
    return AnimatedContainer(
      duration: const Duration(milliseconds: 150),
      width: 42,
      height: 25,
      padding: const EdgeInsets.all(3),
      decoration: BoxDecoration(color: on ? t.accent : t.line, borderRadius: BorderRadius.circular(999)),
      child: AnimatedAlign(
        duration: const Duration(milliseconds: 150),
        alignment: on ? Alignment.centerRight : Alignment.centerLeft,
        child: Container(
          width: 19,
          height: 19,
          decoration: const BoxDecoration(
            color: Colors.white,
            shape: BoxShape.circle,
            boxShadow: [BoxShadow(color: Color.fromRGBO(0, 0, 0, .2), blurRadius: 3, offset: Offset(0, 1))],
          ),
        ),
      ),
    );
  }
}

class CheckBox extends StatelessWidget {
  const CheckBox({super.key, required this.on, this.round = false});
  final bool on;
  final bool round;

  @override
  Widget build(BuildContext context) {
    final t = context.tk;
    return AnimatedContainer(
      duration: const Duration(milliseconds: 120),
      width: 22,
      height: 22,
      decoration: BoxDecoration(
        color: on ? t.accent : Colors.transparent,
        border: Border.all(color: on ? t.accent : t.faint, width: 1.5),
        borderRadius: BorderRadius.circular(round ? 11 : 7),
      ),
      child: on ? Center(child: AppIcon('check', size: 14, strokeWidth: 2.6, color: t.onAccent)) : null,
    );
  }
}

class RadioDot extends StatelessWidget {
  const RadioDot({super.key, required this.on});
  final bool on;

  @override
  Widget build(BuildContext context) {
    final t = context.tk;
    return Container(
      width: 20,
      height: 20,
      decoration: BoxDecoration(shape: BoxShape.circle, border: Border.all(color: on ? t.accent : t.line, width: 1.5)),
      alignment: Alignment.center,
      child: Container(width: 10, height: 10, decoration: BoxDecoration(shape: BoxShape.circle, color: on ? t.accent : Colors.transparent)),
    );
  }
}

/// A row with a label, optional description and a switch; the whole row toggles.
class ToggleRow extends StatelessWidget {
  const ToggleRow({super.key, this.icon, required this.label, this.sub, required this.on, this.onToggle, this.padding});

  final String? icon;
  final String label;
  final String? sub;
  final bool on;
  final VoidCallback? onToggle;
  final EdgeInsetsGeometry? padding;

  @override
  Widget build(BuildContext context) {
    final t = context.tk;
    return Semantics(
      toggled: on,
      child: Tap(
        onTap: onToggle,
        padding: padding ?? const EdgeInsets.symmetric(horizontal: 16, vertical: 12),
        child: Row(children: [
          if (icon != null) ...[AppIcon(icon!, size: 19, color: t.muted), const SizedBox(width: 14)],
          Expanded(
            child: Column(crossAxisAlignment: CrossAxisAlignment.start, children: [
              Text(label, style: ts(14.5, weight: icon != null ? 550 : 600, color: onToggle == null ? t.faint : t.ink)),
              if (sub != null) ...[
                const SizedBox(height: 2),
                Text(sub!, style: ts(12.5, color: t.muted, height: 1.4)),
              ],
            ]),
          ),
          const SizedBox(width: 12),
          ToggleSwitch(on: on),
        ]),
      ),
    );
  }
}

// ---------------------------------------------------------------------------
// layout pieces
// ---------------------------------------------------------------------------

/// Header of a full-screen pane (66px, bottom border), with a back button.
class PaneHeader extends StatelessWidget implements PreferredSizeWidget {
  const PaneHeader({super.key, this.title, this.titleWidget, this.onBack, this.actions = const [], this.backIcon = 'back'});

  final String? title;
  final Widget? titleWidget;
  final VoidCallback? onBack;
  final List<Widget> actions;
  final String backIcon;

  @override
  Size get preferredSize => const Size.fromHeight(66);

  @override
  Widget build(BuildContext context) {
    final t = context.tk;
    final back = onBack ?? () => Navigator.of(context).maybePop();
    return Container(
      height: 66,
      padding: const EdgeInsets.only(left: 6, right: 12),
      decoration: BoxDecoration(color: t.pane, border: Border(bottom: BorderSide(color: t.line))),
      child: Row(children: [
        IconBtn(backIcon, iconSize: 21, onPressed: back, tooltip: context.t('common.back')),
        const SizedBox(width: 10),
        Expanded(child: titleWidget ?? Text(title ?? '', maxLines: 1, overflow: TextOverflow.ellipsis, style: ts(17, weight: 700, color: t.ink))),
        ...actions,
      ]),
    );
  }
}

/// Screen scaffold: status-bar safe, pane background.
class PaneScaffold extends StatelessWidget {
  const PaneScaffold({super.key, this.header, required this.body, this.bottom, this.background});

  final Widget? header;
  final Widget body;
  final Widget? bottom;
  final Color? background;

  @override
  Widget build(BuildContext context) {
    final t = context.tk;
    return Scaffold(
      backgroundColor: background ?? t.pane,
      resizeToAvoidBottomInset: true,
      body: Container(
        color: t.pane,
        child: SafeArea(
          bottom: bottom == null,
          child: Column(children: [
            ?header,
            Expanded(child: Container(color: background ?? t.pane, child: body)),
            ?bottom,
          ]),
        ),
      ),
    );
  }
}

/// Bordered card with rows separated by lines (.list-card).
class ListCard extends StatelessWidget {
  const ListCard({super.key, required this.children});
  final List<Widget> children;

  @override
  Widget build(BuildContext context) {
    final t = context.tk;
    final rows = <Widget>[];
    for (var i = 0; i < children.length; i++) {
      if (i > 0) rows.add(Divider(height: 1, thickness: 1, color: t.line));
      rows.add(children[i]);
    }
    return Container(
      decoration: BoxDecoration(
        color: t.pane,
        borderRadius: BorderRadius.circular(t.rCard),
        border: Border.all(color: t.line),
      ),
      clipBehavior: Clip.antiAlias,
      child: Material(color: Colors.transparent, child: Column(crossAxisAlignment: CrossAxisAlignment.stretch, children: rows)),
    );
  }
}

/// Row of a [ListCard]: icon, title, subtitle, chevron.
class ListRow extends StatelessWidget {
  const ListRow({super.key, this.icon, this.iconWidget, required this.title, this.sub, this.onTap, this.trailing, this.chevron = true, this.titleColor});

  final String? icon;
  final Widget? iconWidget;
  final String title;
  final String? sub;
  final VoidCallback? onTap;
  final Widget? trailing;
  final bool chevron;
  final Color? titleColor;

  @override
  Widget build(BuildContext context) {
    final t = context.tk;
    return Tap(
      onTap: onTap,
      padding: const EdgeInsets.symmetric(horizontal: 16, vertical: 15),
      child: Row(children: [
        if (iconWidget != null) ...[iconWidget!, const SizedBox(width: 14)] else if (icon != null) ...[
          AppIcon(icon!, size: 19, color: t.muted),
          const SizedBox(width: 14),
        ],
        Expanded(
          child: Column(crossAxisAlignment: CrossAxisAlignment.start, children: [
            Text(title, style: ts(14.5, weight: 600, color: titleColor ?? t.ink)),
            if (sub != null) Text(sub!, style: ts(12.5, color: t.muted, height: 1.4)),
          ]),
        ),
        ?trailing,
        if (trailing == null && chevron && onTap != null) AppIcon('chevron', size: 17, color: t.faint),
      ]),
    );
  }
}

/// Section card of the info pane (flush band in A, rounded sunk card in B).
class Sect extends StatelessWidget {
  const Sect({super.key, required this.child, this.padding = const EdgeInsets.fromLTRB(16, 14, 16, 14)});
  final Widget child;
  final EdgeInsetsGeometry padding;

  @override
  Widget build(BuildContext context) {
    final t = context.tk;
    return Container(
      margin: EdgeInsets.fromLTRB(t.sectMx, 0, t.sectMx, 12),
      decoration: BoxDecoration(
        color: t.sectBg,
        borderRadius: BorderRadius.circular(t.sectR),
        border: t.sectSep > 0 ? Border(top: BorderSide(color: t.sunk, width: t.sectSep)) : null,
      ),
      clipBehavior: Clip.antiAlias,
      child: Material(color: Colors.transparent, child: Padding(padding: padding, child: child)),
    );
  }
}

/// Notice box (.notice): accent-soft, warn or danger.
class NoticeBox extends StatelessWidget {
  const NoticeBox({super.key, required this.text, this.icon = 'shield', this.tone = 'info', this.title, this.onClose});

  final String text;
  final String icon;

  /// "info" | "warn" | "danger"
  final String tone;
  final String? title;
  final VoidCallback? onClose;

  @override
  Widget build(BuildContext context) {
    final t = context.tk;
    final (bg, fg, ic) = switch (tone) {
      'warn' => (t.sysBg, t.sysFg, t.sysFg),
      'danger' => (Color.alphaBlend(t.danger.withValues(alpha: .1), t.pane), t.danger, t.danger),
      _ => (t.sunk, t.muted, t.accentText),
    };
    return Container(
      padding: const EdgeInsets.fromLTRB(14, 12, 14, 12),
      decoration: BoxDecoration(color: bg, borderRadius: BorderRadius.circular(t.rCard)),
      child: Row(crossAxisAlignment: CrossAxisAlignment.start, children: [
        Padding(padding: const EdgeInsets.only(top: 1), child: AppIcon(icon, size: 16, color: ic)),
        const SizedBox(width: 10),
        Expanded(
          child: Column(crossAxisAlignment: CrossAxisAlignment.start, children: [
            if (title != null) Padding(padding: const EdgeInsets.only(bottom: 2), child: Text(title!, style: ts(13.5, weight: 700, color: tone == 'info' ? t.ink : fg))),
            Text(text, style: ts(12.5, color: fg, height: 1.45)),
          ]),
        ),
        if (onClose != null) GestureDetector(onTap: onClose, child: AppIcon('x', size: 16, color: fg)),
      ]),
    );
  }
}

/// Small uppercase label badge (.badge-soft).
class SoftBadge extends StatelessWidget {
  const SoftBadge(this.text, {super.key, this.tone = 'accent'});
  final String text;
  final String tone;

  @override
  Widget build(BuildContext context) {
    final t = context.tk;
    final (bg, fg) = switch (tone) {
      'danger' => (t.danger.withValues(alpha: .12), t.danger),
      'muted' => (t.sunk, t.muted),
      _ => (t.accentSoft, t.accentText),
    };
    return Container(
      padding: const EdgeInsets.symmetric(horizontal: 8, vertical: 4),
      decoration: BoxDecoration(color: bg, borderRadius: BorderRadius.circular(6)),
      child: Text(text.toUpperCase(), style: ts(10.5, weight: 700, color: fg, letterSpacing: .5)),
    );
  }
}

/// Uppercase eyebrow label.
class Eyebrow extends StatelessWidget {
  const Eyebrow(this.text, {super.key});
  final String text;

  @override
  Widget build(BuildContext context) =>
      Text(text.toUpperCase(), style: ts(12, weight: 600, color: context.tk.muted, letterSpacing: 0.72));
}

/// Centered spinner.
class Spinner extends StatelessWidget {
  const Spinner({super.key, this.size = 28, this.padding = 24});
  final double size;
  final double padding;

  @override
  Widget build(BuildContext context) => Padding(
        padding: EdgeInsets.all(padding),
        child: Center(child: SizedBox.square(dimension: size, child: CircularProgressIndicator(strokeWidth: 2.5, color: context.tk.accent))),
      );
}

/// The 24 recovery words in a 3-column grid, blurred until "Show phrase" is pressed.
class SeedGrid extends StatefulWidget {
  const SeedGrid({super.key, required this.words, this.onReveal});
  final List<String> words;
  final VoidCallback? onReveal;

  @override
  State<SeedGrid> createState() => _SeedGridState();
}

class _SeedGridState extends State<SeedGrid> {
  bool _shown = false;

  @override
  Widget build(BuildContext context) {
    final t = context.tk;
    final grid = LayoutBuilder(builder: (context, c) {
      final w = (c.maxWidth - 12) / 3;
      return Wrap(
        spacing: 6,
        runSpacing: 6,
        children: [
          for (var i = 0; i < widget.words.length; i++)
            Container(
              width: w,
              height: 38,
              padding: const EdgeInsets.symmetric(horizontal: 8),
              decoration: BoxDecoration(color: t.pane, borderRadius: BorderRadius.circular(10), border: Border.all(color: t.line)),
              child: Row(children: [
                SizedBox(width: 15, child: Text('${i + 1}', textAlign: TextAlign.right, style: ts(11, color: t.faint, mono: true))),
                const SizedBox(width: 6),
                Expanded(child: Text(widget.words[i], maxLines: 1, overflow: TextOverflow.fade, softWrap: false, style: ts(13.5, color: t.ink, mono: true))),
              ]),
            ),
        ],
      );
    });
    return Stack(children: [
      ImageFiltered(
        enabled: !_shown,
        imageFilter: ui.ImageFilter.blur(sigmaX: 7, sigmaY: 7),
        child: ExcludeSemantics(excluding: !_shown, child: grid),
      ),
      if (!_shown)
        Positioned.fill(
          child: Center(
            child: Material(
              color: t.ink,
              borderRadius: BorderRadius.circular(999),
              elevation: 6,
              shadowColor: Colors.black38,
              child: InkWell(
                borderRadius: BorderRadius.circular(999),
                onTap: () {
                  setState(() => _shown = true);
                  widget.onReveal?.call();
                },
                child: Container(
                  height: 44,
                  padding: const EdgeInsets.symmetric(horizontal: 18),
                  child: Row(mainAxisSize: MainAxisSize.min, children: [
                    AppIcon('eye', size: 17, color: t.pane),
                    const SizedBox(width: 8),
                    Text(context.t('onboarding.seed.reveal'), style: ts(14, weight: 650, color: t.pane)),
                  ]),
                ),
              ),
            ),
          ),
        ),
    ]);
  }
}

// ---------------------------------------------------------------------------
// password
// ---------------------------------------------------------------------------


class PasswordStrengthBar extends StatelessWidget {
  const PasswordStrengthBar({super.key, required this.value});
  final String value;

  @override
  Widget build(BuildContext context) {
    final t = context.tk;
    if (value.isEmpty) return const SizedBox.shrink();
    final score = passwordStrength(value);
    final colors = [t.danger, t.danger, const Color(0xFFD98B1B), const Color(0xFFC9A800), t.accent];
    final on = score < 1 ? 1 : score;
    return Column(crossAxisAlignment: CrossAxisAlignment.stretch, children: [
      const SizedBox(height: 8),
      Row(children: [
        for (var i = 0; i < 4; i++) ...[
          if (i > 0) const SizedBox(width: 4),
          Expanded(child: Container(height: 4, decoration: BoxDecoration(color: i < on ? colors[score] : t.line, borderRadius: BorderRadius.circular(2)))),
        ],
      ]),
      const SizedBox(height: 6),
      Row(mainAxisAlignment: MainAxisAlignment.spaceBetween, children: [
        Text(context.t('password.strength'), style: ts(12, color: t.muted)),
        Text(context.t('password.level$score'), style: ts(12, weight: 650, color: colors[score])),
      ]),
    ]);
  }
}

/// State of a "new password + confirmation" pair.
class PasswordPair {
  final password = TextEditingController();
  final confirm = TextEditingController();

  List<String> get problems => passwordProblems(password.text);
  bool get mismatch => confirm.text.isNotEmpty && confirm.text != password.text;
  bool get valid => password.text.isNotEmpty && problems.isEmpty && confirm.text == password.text;

  void reset() {
    password.clear();
    confirm.clear();
  }

  void dispose() {
    password.dispose();
    confirm.dispose();
  }
}

class PasswordPairFields extends StatelessWidget {
  const PasswordPairFields({super.key, required this.pair, this.label, this.autofocus = false, this.large = false, required this.onChanged});

  final PasswordPair pair;
  final String? label;
  final bool autofocus;
  final bool large;
  final VoidCallback onChanged;

  @override
  Widget build(BuildContext context) {
    return Column(crossAxisAlignment: CrossAxisAlignment.stretch, children: [
      AppField(
        controller: pair.password,
        label: label ?? context.t('password.new'),
        password: true,
        large: large,
        autofocus: autofocus,
        onChanged: (_) => onChanged(),
      ),
      PasswordStrengthBar(value: pair.password.text),
      const SizedBox(height: 8),
      for (final rule in const ['length', 'mix', 'common'])
        Builder(builder: (context) {
          final t = context.tk;
          final ok = pair.password.text.isNotEmpty && !pair.problems.contains(rule);
          return Padding(
            padding: const EdgeInsets.only(top: 3),
            child: Row(children: [
              AppIcon(ok ? 'check' : 'x', size: 14, color: ok ? t.accentText : t.muted),
              const SizedBox(width: 6),
              Expanded(
                child: Text(context.t('password.rule.$rule', count: kMinPassword), style: ts(12.5, color: ok ? t.accentText : t.muted)),
              ),
            ]),
          );
        }),
      const SizedBox(height: 18),
      AppField(
        controller: pair.confirm,
        label: context.t('password.confirm'),
        password: true,
        large: large,
        onChanged: (_) => onChanged(),
        error: pair.mismatch ? context.t('password.mismatch') : null,
      ),
    ]);
  }
}
