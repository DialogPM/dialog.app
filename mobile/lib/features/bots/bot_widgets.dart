// Bots in conversations (the core: crates/chat-core/src/engine/bots.rs; design "ChatApp Boti").
//
// A bot conversation is not end-to-end encrypted and carries text only. The bot may write to
// us only after "Start" and until "Stop"; the server enforces that.

import 'dart:async';
import 'package:flutter/material.dart';
import 'package:url_launcher/url_launcher.dart';
import '../../app/globals.dart';
import '../../app/nav.dart';
import '../../core/models.dart';
import '../../l10n/i18n.dart';
import '../../state/actions.dart';
import '../../theme/app_theme.dart';
import '../../theme/oklch.dart';
import '../../widgets/app_icon.dart';
import '../../widgets/dialogs.dart';
import '../../widgets/ui.dart';

/// A bot's address: a username that ends in "bot".
bool looksLikeBotName(String name) => RegExp(r'^[a-z][a-z0-9_.]{1,28}bot$').hasMatch(name);

/// Opens the conversation with a bot. Nothing is sent to it: it starts only on "Start".
Future<bool> openBot(String botId) async {
  final c = await attempt(() => backend.botOpen(botId));
  if (c == null) return false;
  store.upsertConversation(c);
  openChat(c.id);
  return true;
}

Future<void> startBot(BotInfo bot) async {
  final c = await attempt(() => backend.botStart(bot.id));
  if (c != null) store.upsertConversation(c);
}

Future<void> confirmStopBot(BuildContext context, BotInfo bot) async {
  final ok = await showConfirm(
    context,
    title: tr('bots.stopTitle', args: {'name': bot.name}),
    body: tr('bots.stopBody'),
    confirmLabel: tr('bots.stop'),
  );
  if (!ok) return;
  final c = await attempt(() => backend.botStop(bot.id));
  if (c != null) store.upsertConversation(c);
}

/// The label that marks a bot wherever its name is shown.
class BotBadge extends StatelessWidget {
  const BotBadge({super.key, this.small = false});
  final bool small;

  @override
  Widget build(BuildContext context) {
    final dark = context.tk.dark;
    return Container(
      padding: small ? const EdgeInsets.symmetric(horizontal: 6, vertical: 2) : const EdgeInsets.symmetric(horizontal: 7, vertical: 3),
      decoration: BoxDecoration(
        color: dark ? oklch(0.3, 0.05, 245) : oklch(0.93, 0.035, 245),
        borderRadius: BorderRadius.circular(small ? 5 : 6),
      ),
      child: Text(
        context.t('bots.badge'),
        style: ts(small ? 9.5 : 10.5, weight: 800, letterSpacing: 0.6, height: 1.25, color: dark ? oklch(0.83, 0.09, 245) : oklch(0.45, 0.12, 245)),
      ),
    );
  }
}

/// What a bot can and cannot do, said before it is started.
class BotFacts extends StatelessWidget {
  const BotFacts({super.key});

  @override
  Widget build(BuildContext context) {
    final t = context.tk;
    Widget fact(String icon, String key, {bool ok = false}) => Row(crossAxisAlignment: CrossAxisAlignment.start, children: [
          Padding(padding: const EdgeInsets.only(top: 2), child: AppIcon(icon, size: 15, color: ok ? t.accentText : t.muted)),
          const SizedBox(width: 10),
          Expanded(child: Text(context.t(key), style: ts(13.5, color: t.ink, height: 1.4))),
        ]);
    return Container(
      width: double.infinity,
      padding: const EdgeInsets.symmetric(horizontal: 16, vertical: 14),
      decoration: BoxDecoration(color: t.sunk, borderRadius: BorderRadius.circular(14)),
      child: Column(children: [
        fact('check', 'bots.canWrite', ok: true),
        const SizedBox(height: 10),
        fact('x', 'bots.cannotSee'),
        const SizedBox(height: 10),
        fact('x', 'bots.notEncrypted'),
      ]),
    );
  }
}

class _BotCard extends StatelessWidget {
  const _BotCard({required this.seed, required this.bot, this.avatar = 96});
  final String seed;
  final BotInfo bot;
  final double avatar;

  @override
  Widget build(BuildContext context) {
    final t = context.tk;
    return Column(children: [
      Avatar(name: bot.name, seed: seed, size: avatar, bot: true),
      const SizedBox(height: 14),
      Row(mainAxisAlignment: MainAxisAlignment.center, children: [
        Flexible(child: Text(bot.name, maxLines: 2, textAlign: TextAlign.center, overflow: TextOverflow.ellipsis, style: ts(23, weight: 750, letterSpacing: -0.3, color: t.ink))),
        const SizedBox(width: 8),
        const BotBadge(),
      ]),
      const SizedBox(height: 4),
      Text('@${bot.username}', style: ts(13, color: t.muted, mono: true)),
      if (bot.about.isNotEmpty) ...[
        const SizedBox(height: 14),
        Text(bot.about, textAlign: TextAlign.center, style: ts(15, color: t.ink, height: 1.5)),
      ],
    ]);
  }
}

/// A bot that was never started: its card, instead of the (empty) message list.
class BotLanding extends StatelessWidget {
  const BotLanding({super.key, required this.conv, required this.bot});
  final Conversation conv;
  final BotInfo bot;

  @override
  Widget build(BuildContext context) {
    return Container(
      color: context.tk.pane,
      child: ListView(padding: const EdgeInsets.fromLTRB(24, 28, 24, 24), children: [
        _BotCard(seed: conv.id, bot: bot),
        const SizedBox(height: 22),
        const BotFacts(),
      ]),
    );
  }
}

const int _muteForever = -1;

/// The bottom of a bot conversation: "Start", or the text composer (with the commands after "/").
class BotFooter extends StatefulWidget {
  const BotFooter({super.key, required this.conv, required this.bot});
  final Conversation conv;
  final BotInfo bot;

  @override
  State<BotFooter> createState() => _BotFooterState();
}

class _BotFooterState extends State<BotFooter> {
  final _text = TextEditingController();
  final _focus = FocusNode();
  bool _busy = false;

  @override
  void initState() {
    super.initState();
    _text.addListener(() => setState(() {}));
  }

  @override
  void dispose() {
    _text.dispose();
    _focus.dispose();
    super.dispose();
  }

  /// The commands matching what is typed: "/" at the very start, up to the first space.
  List<BotCommand> get _matches {
    final m = RegExp(r'^/([a-z0-9_]*)$', caseSensitive: false).firstMatch(_text.text);
    if (m == null) return const [];
    final q = m.group(1)!.toLowerCase();
    return widget.bot.commands.where((c) => c.command.startsWith(q)).toList();
  }

  void _send([String? value]) {
    final v = (value ?? _text.text).trim();
    if (v.isEmpty) return;
    _text.clear();
    unawaited(sendText(widget.conv.id, v));
  }

  @override
  Widget build(BuildContext context) {
    final t = context.tk;
    final bot = widget.bot;
    final bottom = MediaQuery.of(context).padding.bottom;
    Widget wrap(Widget child) => Container(
          decoration: BoxDecoration(color: t.pane, border: Border(top: BorderSide(color: t.line))),
          padding: EdgeInsets.fromLTRB(12, 10, 12, 12 + bottom),
          child: child,
        );
    Widget note(String key) => Padding(
          padding: const EdgeInsets.only(bottom: 10),
          child: Text(context.t(key), textAlign: TextAlign.center, style: ts(13, color: t.muted)),
        );

    if (bot.deleted) return wrap(Text(context.t('bots.deletedNote'), textAlign: TextAlign.center, style: ts(13, color: t.muted)));
    if (bot.state != 'active') {
      return wrap(Column(mainAxisSize: MainAxisSize.min, children: [
        if (bot.state == 'stopped') note('bots.stoppedNote'),
        Btn(
          key: const ValueKey('bot-start'),
          label: context.t('bots.start'),
          height: 52,
          fontSize: 16,
          busy: _busy,
          onPressed: () async {
            setState(() => _busy = true);
            await startBot(bot);
            if (mounted) setState(() => _busy = false);
          },
        ),
      ]));
    }
    final matches = _matches;
    return Container(
      decoration: BoxDecoration(color: t.pane, border: Border(top: BorderSide(color: t.line))),
      padding: EdgeInsets.only(bottom: bottom),
      child: Column(mainAxisSize: MainAxisSize.min, children: [
        if (matches.isNotEmpty)
          ConstrainedBox(
            constraints: const BoxConstraints(maxHeight: 240),
            child: ListView(
              key: const ValueKey('bot-commands'),
              shrinkWrap: true,
              padding: const EdgeInsets.symmetric(vertical: 6),
              children: [
                for (final c in matches)
                  Tap(
                    onTap: () => _send('/${c.command}'),
                    padding: const EdgeInsets.symmetric(horizontal: 16, vertical: 10),
                    child: Row(crossAxisAlignment: CrossAxisAlignment.baseline, textBaseline: TextBaseline.alphabetic, children: [
                      Text('/${c.command}', style: ts(13.5, weight: 600, color: t.accentText, mono: true)),
                      const SizedBox(width: 10),
                      Expanded(child: Text(c.description, maxLines: 1, overflow: TextOverflow.ellipsis, style: ts(13, color: t.muted))),
                    ]),
                  ),
              ],
            ),
          ),
        Padding(
          padding: const EdgeInsets.fromLTRB(8, 8, 8, 8),
          child: Row(crossAxisAlignment: CrossAxisAlignment.end, children: [
            if (bot.commands.isNotEmpty) ...[
              Tap(
                semanticLabel: context.t('bots.commands'),
                radius: BorderRadius.circular(22),
                onTap: () {
                  _text.text = _text.text.startsWith('/') ? '' : '/';
                  _text.selection = TextSelection.collapsed(offset: _text.text.length);
                  _focus.requestFocus();
                },
                child: SizedBox(width: 44, height: 44, child: Center(child: Text('/', style: ts(19, weight: 600, color: t.muted, mono: true)))),
              ),
              const SizedBox(width: 4),
            ],
            Expanded(
              child: Container(
                constraints: const BoxConstraints(minHeight: 44),
                padding: const EdgeInsets.symmetric(horizontal: 16),
                decoration: BoxDecoration(color: t.sunk, borderRadius: BorderRadius.circular(22)),
                child: TextField(
                  key: const ValueKey('bot-input'),
                  enableIMEPersonalizedLearning: imePersonalizedLearning,
                  controller: _text,
                  focusNode: _focus,
                  minLines: 1,
                  maxLines: 6,
                  maxLength: 4096,
                  keyboardType: TextInputType.multiline,
                  textCapitalization: TextCapitalization.sentences,
                  style: ts(15.5, color: t.ink),
                  cursorColor: t.accent,
                  decoration: InputDecoration(
                    isDense: true,
                    border: InputBorder.none,
                    counterText: '',
                    contentPadding: const EdgeInsets.symmetric(vertical: 11),
                    hintText: context.t('bots.placeholder'),
                    hintStyle: ts(15.5, color: t.faint),
                  ),
                ),
              ),
            ),
            const SizedBox(width: 8),
            Tap(
              key: const ValueKey('bot-send'),
              semanticLabel: context.t('composer.send'),
              radius: BorderRadius.circular(22),
              color: _text.text.trim().isEmpty ? t.sunk : t.accent,
              onTap: _text.text.trim().isEmpty ? null : _send,
              child: SizedBox(width: 44, height: 44, child: Center(child: AppIcon('send', size: 19, color: _text.text.trim().isEmpty ? t.faint : t.onAccent))),
            ),
          ]),
        ),
      ]),
    );
  }
}

/// A bot found by its username in search.
class BotFoundRow extends StatelessWidget {
  const BotFoundRow({super.key, required this.bot});
  final BotInfo bot;

  @override
  Widget build(BuildContext context) {
    final t = context.tk;
    return Padding(
      padding: EdgeInsets.symmetric(horizontal: t.rowMx),
      child: Tap(
        radius: BorderRadius.circular(t.rowRadius),
        onTap: () => unawaited(openBot(bot.id)),
        padding: EdgeInsets.symmetric(horizontal: t.rowPx, vertical: 10),
        child: Row(children: [
          Avatar(name: bot.name, seed: bot.id, size: 50, bot: true),
          const SizedBox(width: 13),
          Expanded(
            child: Column(crossAxisAlignment: CrossAxisAlignment.start, children: [
              Row(children: [
                Flexible(child: Text(bot.name, maxLines: 1, overflow: TextOverflow.ellipsis, style: ts(15.5, weight: 650, color: t.ink))),
                const SizedBox(width: 6),
                const BotBadge(small: true),
              ]),
              Text('@${bot.username}', maxLines: 1, overflow: TextOverflow.ellipsis, style: ts(12.5, color: t.muted, mono: true)),
            ]),
          ),
          Text(context.t('bots.lookupOpen'), style: ts(13.5, weight: 650, color: t.accentText)),
        ]),
      ),
    );
  }
}

/// The info page of a bot conversation.
class BotInfoPage extends StatelessWidget {
  const BotInfoPage({super.key, required this.conv, required this.bot});
  final Conversation conv;
  final BotInfo bot;

  @override
  Widget build(BuildContext context) {
    final t = context.tk;
    final muted = conv.isMuted();
    return PaneScaffold(
      header: PaneHeader(titleWidget: Text(context.t('bots.info'), style: ts(16.5, weight: 700, color: t.ink))),
      body: ListView(padding: const EdgeInsets.fromLTRB(20, 24, 20, 32), children: [
        _BotCard(seed: conv.id, bot: bot, avatar: 112),
        const SizedBox(height: 22),
        const BotFacts(),
        if (bot.commands.isNotEmpty) ...[
          const SizedBox(height: 22),
          Text(context.t('bots.commands'), style: ts(13, weight: 700, color: t.muted)),
          const SizedBox(height: 6),
          for (final c in bot.commands)
            Padding(
              padding: const EdgeInsets.symmetric(vertical: 6),
              child: Row(crossAxisAlignment: CrossAxisAlignment.start, children: [
                Text('/${c.command}', style: ts(13.5, weight: 600, color: t.accentText, mono: true)),
                const SizedBox(width: 10),
                Expanded(child: Text(c.description, style: ts(13.5, color: t.muted))),
              ]),
            ),
        ],
        const SizedBox(height: 22),
        if (bot.deleted)
          Text(context.t('bots.deletedNote'), textAlign: TextAlign.center, style: ts(13, color: t.muted))
        else ...[
          Btn(
            label: context.t(muted ? 'bots.unmute' : 'bots.silent'),
            icon: muted ? 'mute' : 'bell',
            kind: BtnKind.secondary,
            height: 46,
            onPressed: () => unawaited(patchConversation(conv.id, ConversationSettingsPatch(setMuted: true, mutedUntil: muted ? null : _muteForever))),
          ),
          const SizedBox(height: 8),
          if (bot.state == 'active')
            Btn(label: context.t('bots.stop'), kind: BtnKind.danger, height: 46, onPressed: () => unawaited(confirmStopBot(context, bot)))
          else
            Btn(label: context.t('bots.start'), height: 46, onPressed: () => unawaited(startBot(bot))),
          if (bot.mine) ...[
            const SizedBox(height: 8),
            Btn(label: context.t('bots.manage'), icon: 'gear', kind: BtnKind.secondary, height: 46, onPressed: () => openSettingsSection('bots')),
          ],
        ],
      ]),
    );
  }
}

/// The buttons under a bot's message. One kind tells the bot it was pressed; the other
/// opens a link, after showing where it leads.
class BotButtons extends StatefulWidget {
  const BotButtons({super.key, required this.message, this.enabled = true});
  final Message message;
  final bool enabled;

  @override
  State<BotButtons> createState() => _BotButtonsState();
}

class _BotButtonsState extends State<BotButtons> {
  String? _busy;
  String? _done;

  Future<void> _press(String key, BotButton b) async {
    final url = b.url;
    if (url != null) {
      final ok = await showConfirm(context, title: tr('bots.openLinkTitle'), body: url, confirmLabel: tr('bots.openLink'), danger: false);
      final uri = Uri.tryParse(url);
      if (ok && uri != null && uri.scheme == 'https') await launchUrl(uri, mode: LaunchMode.externalApplication);
      return;
    }
    final data = b.data;
    if (data == null || _busy != null) return;
    setState(() => _busy = key);
    try {
      await backend.botPress(widget.message.id, data);
      if (mounted) setState(() => _done = key);
      Future.delayed(const Duration(milliseconds: 1500), () {
        if (mounted && _done == key) setState(() => _done = null);
      });
    } catch (e) {
      notifyError(e);
    }
    if (mounted) setState(() => _busy = null);
  }

  @override
  Widget build(BuildContext context) {
    final t = context.tk;
    final rows = widget.message.buttons;
    return Padding(
      padding: const EdgeInsets.fromLTRB(2, 6, 2, 2),
      child: Column(crossAxisAlignment: CrossAxisAlignment.stretch, mainAxisSize: MainAxisSize.min, children: [
        for (var i = 0; i < rows.length; i++)
          Padding(
            padding: EdgeInsets.only(top: i == 0 ? 0 : 6),
            child: Row(children: [
              for (var j = 0; j < rows[i].length; j++) ...[
                if (j > 0) const SizedBox(width: 6),
                Expanded(
                  child: Opacity(
                    opacity: _busy == '$i:$j' ? .6 : 1,
                    child: Tap(
                      key: ValueKey('bot-button-$i-$j'),
                      onTap: widget.enabled ? () => unawaited(_press('$i:$j', rows[i][j])) : null,
                      color: t.accentSoft,
                      radius: BorderRadius.circular(10),
                      child: SizedBox(
                        height: 38,
                        child: Row(mainAxisAlignment: MainAxisAlignment.center, children: [
                          Flexible(
                            child: Padding(
                              padding: const EdgeInsets.symmetric(horizontal: 8),
                              child: Text(rows[i][j].text, maxLines: 1, overflow: TextOverflow.ellipsis, style: ts(13.5, weight: 650, color: t.accentText)),
                            ),
                          ),
                          if (rows[i][j].url != null)
                            Padding(padding: const EdgeInsets.only(right: 8), child: AppIcon('external', size: 13, color: t.accentText))
                          else if (_done == '$i:$j')
                            Padding(padding: const EdgeInsets.only(right: 8), child: AppIcon('check', size: 13, color: t.accentText)),
                        ]),
                      ),
                    ),
                  ),
                ),
              ],
            ]),
          ),
      ]),
    );
  }
}
