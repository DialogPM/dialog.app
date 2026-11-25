// Settings → Bots: the bots this account created (design "ChatApp Boti", screen 1, as pages).
//
// A bot's token is shown only on the device that created or reset it (the server keeps a
// hash); subscribers are chat ids, the same handle the bot itself gets, never a name.

import 'dart:async';
import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import '../../app/globals.dart';
import '../../core/backend.dart';
import '../../core/models.dart';
import '../../l10n/i18n.dart';
import '../../services/user_links.dart';
import '../../state/actions.dart';
import '../../theme/app_theme.dart';
import '../../widgets/app_icon.dart';
import '../../widgets/dialogs.dart';
import '../../widgets/qr.dart';
import '../../widgets/ui.dart';
import 'bot_widgets.dart';

/// Address part typed by the user -> lowercase ASCII with underscores ("Alarmă casă" -> "alarma_casa").
String botSlug(String input) {
  const from = 'ăâîșşțţáàäãåçéèêëíìïñóòôöõúùûüýÿ';
  const to = 'aaissttaaaaaceeeeiiinooooouuuuyy';
  final b = StringBuffer();
  for (final ch in input.toLowerCase().split('')) {
    final i = from.indexOf(ch);
    b.write(i >= 0 ? to[i] : ch);
  }
  var s = b.toString().replaceAll(RegExp(r'[^a-z0-9]+'), '_').replaceAll(RegExp(r'^[_0-9]+|_+$'), '').replaceAll(RegExp(r'_?bot$'), '');
  if (s.length > 28) s = s.substring(0, 28);
  return s;
}

String _server() => (appData.appState?.account?.serverUrl ?? '').replaceAll(RegExp(r'/+$'), '');

String _snippet(String lang, String token, String chat, String text) {
  final url = '${_server()}/bot/$token/sendMessage';
  final q = '"${text.replaceAll(r'\', r'\\').replaceAll('"', r'\"')}"';
  switch (lang) {
    case 'js':
      return 'await fetch("$url", {\n  method: "POST",\n  headers: { "Content-Type": "application/json" },\n  body: JSON.stringify({ chat_id: "$chat", text: $q })\n});';
    case 'py':
      return 'import requests\n\nrequests.post(\n    "$url",\n    json={"chat_id": "$chat", "text": $q},\n)';
    default:
      return 'curl $url \\\n  -d chat_id=$chat \\\n  --data-urlencode text=$q';
  }
}

Future<void> _copy(String value) async {
  await Clipboard.setData(ClipboardData(text: value));
  notifyOk(tr('common.copied'));
}

class BotsPage extends StatefulWidget {
  const BotsPage({super.key});

  @override
  State<BotsPage> createState() => _BotsPageState();
}

class _BotsPageState extends State<BotsPage> {
  List<OwnedBot>? _bots;
  String? _error;

  @override
  void initState() {
    super.initState();
    unawaited(_load());
  }

  Future<void> _load() async {
    try {
      final list = await backend.botsList();
      if (mounted) {
        setState(() {
          _bots = list;
          _error = null;
        });
      }
    } catch (e) {
      if (mounted) setState(() => _error = errorText(e));
    }
  }

  Future<void> _create() async {
    final created = await showAppDialog<OwnedBot>(context, title: tr('bots.newTitle'), builder: (_) => const _CreateBot());
    if (created == null || !mounted) return;
    setState(() => _bots = [...?_bots, created]);
    _open(created);
  }

  void _open(OwnedBot bot) {
    unawaited(Navigator.of(context)
        .push(MaterialPageRoute<void>(builder: (_) => _BotDetailPage(bot: bot), settings: const RouteSettings(name: 'settings/bots/detail')))
        .then((_) => _load()));
  }

  @override
  Widget build(BuildContext context) {
    final t = context.tk;
    final bots = _bots;
    Widget body;
    if (bots == null) {
      body = _error != null ? Center(child: Padding(padding: const EdgeInsets.all(32), child: Text(_error!, textAlign: TextAlign.center, style: ts(14, color: t.muted)))) : const Spinner();
    } else if (bots.isEmpty) {
      body = Center(
        child: Padding(
          padding: const EdgeInsets.all(32),
          child: Column(mainAxisSize: MainAxisSize.min, children: [
            const Avatar(name: 'bot', seed: 'bots-empty', size: 60, bot: true),
            const SizedBox(height: 12),
            Text(context.t('bots.emptyTitle'), style: ts(18, weight: 700, color: t.ink)),
            const SizedBox(height: 8),
            Text(context.t('bots.emptyBody'), textAlign: TextAlign.center, style: ts(13.5, color: t.muted, height: 1.5)),
            const SizedBox(height: 16),
            Btn(key: const ValueKey('bot-new'), label: context.t('bots.create'), icon: 'plus', height: 44, expand: false, onPressed: _create),
          ]),
        ),
      );
    } else {
      body = ListView(padding: const EdgeInsets.fromLTRB(8, 4, 8, 24), children: [
        Padding(
          padding: const EdgeInsets.fromLTRB(12, 8, 12, 12),
          child: Text(context.t('bots.subtitle'), style: ts(13, color: t.muted)),
        ),
        for (final b in bots)
          Tap(
            radius: BorderRadius.circular(12),
            padding: const EdgeInsets.all(10),
            onTap: () => _open(b),
            child: Row(children: [
              Avatar(name: b.name, seed: b.id, size: 42, bot: true),
              const SizedBox(width: 12),
              Expanded(
                child: Column(crossAxisAlignment: CrossAxisAlignment.start, children: [
                  Text(b.name, maxLines: 1, overflow: TextOverflow.ellipsis, style: ts(14.5, weight: 650, color: t.ink)),
                  Text('@${b.username}', maxLines: 1, overflow: TextOverflow.ellipsis, style: ts(12, color: t.muted, mono: true)),
                ]),
              ),
              Text('${b.subscribers}', style: ts(12, color: t.faint)),
              const SizedBox(width: 6),
              AppIcon('chevron', size: 16, color: t.faint),
            ]),
          ),
        const SizedBox(height: 6),
        Tap(
          key: const ValueKey('bot-new'),
          radius: BorderRadius.circular(12),
          padding: const EdgeInsets.all(10),
          onTap: _create,
          child: Row(children: [
            SizedBox(width: 42, height: 42, child: Center(child: AppIcon('plus', size: 19, color: t.accentText))),
            const SizedBox(width: 12),
            Text(context.t('bots.new'), style: ts(14, weight: 650, color: t.accentText)),
          ]),
        ),
      ]);
    }
    return PaneScaffold(header: PaneHeader(title: context.t('bots.title')), body: body);
  }
}

class _BotDetailPage extends StatefulWidget {
  const _BotDetailPage({required this.bot});
  final OwnedBot bot;

  @override
  State<_BotDetailPage> createState() => _BotDetailPageState();
}

class _BotDetailPageState extends State<_BotDetailPage> {
  late OwnedBot _bot = widget.bot;
  List<BotSubscriber>? _subs;
  String? _to;
  bool _showToken = false;
  bool _regenerated = false;
  bool _sending = false;
  String _lang = 'curl';
  late final _text = TextEditingController(text: tr('bots.testDefault'));

  @override
  void initState() {
    super.initState();
    _text.addListener(() => setState(() {}));
    unawaited(_loadSubs());
  }

  @override
  void dispose() {
    _text.dispose();
    super.dispose();
  }

  Future<void> _loadSubs() async {
    try {
      final list = await backend.botSubscribers(_bot.id);
      if (mounted) {
        setState(() {
          _subs = list;
          _bot = _bot.withSubscribers(list.length);
        });
      }
    } catch (_) {
      if (mounted) setState(() => _subs = const []);
    }
  }

  BotSubscriber? get _target {
    final subs = _subs;
    if (subs == null || subs.isEmpty) return null;
    return subs.firstWhere((s) => s.chatId == _to, orElse: () => subs.first);
  }

  String _subName(BotSubscriber s) {
    final subs = _subs ?? const [];
    return s.mine ? tr('bots.subscriberYou') : '${tr('bots.subscriber')} ${subs.length - subs.indexOf(s)}';
  }

  Future<void> _regenerate() async {
    final ok = await showConfirm(context, title: tr('bots.tokenRegenTitle'), body: tr('bots.tokenRegenBody'), confirmLabel: tr('bots.tokenRegenerate'));
    if (!ok) return;
    final fresh = await attempt(() => backend.botResetToken(_bot.id));
    if (fresh == null || !mounted) return;
    setState(() {
      _bot = _bot.withToken(fresh);
      _regenerated = true;
      _showToken = true;
    });
  }

  Future<void> _send() async {
    final target = _target;
    final text = _text.text.trim();
    if (target == null || text.isEmpty || _sending) return;
    setState(() => _sending = true);
    try {
      await backend.botSendTest(_bot.id, target.chatId, text);
      notifyOk(tr('bots.testSent'));
    } catch (e) {
      notifyError(e);
      unawaited(_loadSubs());
    }
    if (mounted) setState(() => _sending = false);
  }

  Future<void> _delete() async {
    final ok = await showConfirm(context, title: tr('bots.deleteTitle', args: {'name': _bot.name}), body: tr('bots.deleteBody'), confirmLabel: tr('bots.delete'));
    if (!ok) return;
    final done = await attempt(() async {
      await backend.botDelete(_bot.id);
      return true;
    });
    if (done == true && mounted) Navigator.of(context).pop();
  }

  @override
  Widget build(BuildContext context) {
    final t = context.tk;
    final bot = _bot;
    final web = userLinksAvailable;
    final link = web ? userLink(bot.username) : '@${bot.username}';
    final shown = web ? '$kUserLinkHost/@${bot.username}' : '@${bot.username}';
    final token = bot.token;
    final subs = _subs;
    final target = _target;
    final text = _text.text.trim().isEmpty ? tr('bots.testDefault') : _text.text.trim();
    String code(String tok) => _snippet(_lang, tok, target?.chatId ?? '<CHAT_ID>', text);

    Widget label(String key, {Widget? trailing}) => Padding(
          padding: const EdgeInsets.only(bottom: 8),
          child: Row(children: [
            Expanded(child: Text(context.t(key), style: ts(14, weight: 700, color: t.ink))),
            ?trailing,
          ]),
        );
    Widget note(String s, {Color? color}) => Padding(
          padding: const EdgeInsets.only(top: 8),
          child: Text(s, style: ts(12.5, color: color ?? t.muted, height: 1.45)),
        );
    Widget chip(String label, {String? icon, required VoidCallback onTap, Color? color, Key? key}) => Tap(
          key: key,
          onTap: onTap,
          color: t.pane,
          radius: BorderRadius.circular(9),
          padding: const EdgeInsets.symmetric(horizontal: 12),
          child: SizedBox(
            height: 34,
            child: Row(mainAxisSize: MainAxisSize.min, children: [
              if (icon != null) ...[AppIcon(icon, size: 14, color: color ?? t.accentText), const SizedBox(width: 6)],
              Text(label, style: ts(13, weight: 650, color: color ?? t.accentText)),
            ]),
          ),
        );

    return PaneScaffold(
      header: PaneHeader(title: bot.name),
      body: ListView(padding: const EdgeInsets.fromLTRB(16, 20, 16, 40), children: [
        Row(children: [
          Avatar(name: bot.name, seed: bot.id, size: 60, bot: true),
          const SizedBox(width: 16),
          Expanded(
            child: Column(crossAxisAlignment: CrossAxisAlignment.start, children: [
              Row(children: [
                Flexible(child: Text(bot.name, maxLines: 1, overflow: TextOverflow.ellipsis, style: ts(21, weight: 750, letterSpacing: -0.3, color: t.ink))),
                const SizedBox(width: 8),
                const BotBadge(),
              ]),
              if (bot.about.isNotEmpty) Padding(padding: const EdgeInsets.only(top: 2), child: Text(bot.about, style: ts(13.5, color: t.muted))),
            ]),
          ),
        ]),
        const SizedBox(height: 12),
        Btn(label: context.t('bots.openChat'), icon: 'chat', kind: BtnKind.secondary, height: 42, onPressed: () => unawaited(openBot(bot.id))),

        const SizedBox(height: 26),
        label('bots.publicLink'),
        Container(
          padding: const EdgeInsets.fromLTRB(14, 6, 6, 6),
          decoration: BoxDecoration(color: t.sunk, borderRadius: BorderRadius.circular(12)),
          child: Row(children: [
            AppIcon('globe', size: 16, color: t.muted),
            const SizedBox(width: 8),
            Expanded(child: Text(shown, maxLines: 1, overflow: TextOverflow.ellipsis, style: ts(13.5, color: t.ink, mono: true))),
            chip(context.t('common.copy'), icon: 'copy', key: const ValueKey('bot-copy-link'), onTap: () => unawaited(_copy(link))),
            if (web) ...[
              const SizedBox(width: 6),
              chip(
                context.t('bots.qr'),
                icon: 'qr',
                color: t.ink,
                onTap: () => unawaited(showAppDialog<void>(
                  context,
                  title: '@${bot.username}',
                  builder: (_) => Padding(padding: const EdgeInsets.only(bottom: 8), child: Center(child: QrCodeView(data: link))),
                )),
              ),
            ],
          ]),
        ),
        note(context.t(web ? 'bots.publicLinkHint' : 'bots.usernameOnlyHint')),

        const SizedBox(height: 26),
        label(
          'bots.token',
          trailing: Tap(
            key: const ValueKey('bot-regen'),
            onTap: () => unawaited(_regenerate()),
            child: Row(mainAxisSize: MainAxisSize.min, children: [
              AppIcon('refresh', size: 13, color: t.muted),
              const SizedBox(width: 5),
              Text(context.t('bots.tokenRegenerate'), style: ts(12.5, weight: 650, color: t.muted)),
            ]),
          ),
        ),
        if (token != null)
          Container(
            padding: const EdgeInsets.fromLTRB(14, 6, 6, 6),
            decoration: BoxDecoration(color: t.pane, borderRadius: BorderRadius.circular(12), border: Border.all(color: t.line, width: 1.5)),
            child: Row(children: [
              AppIcon('key', size: 16, color: t.muted),
              const SizedBox(width: 8),
              Expanded(
                child: Text(
                  _showToken ? token : '${token.substring(0, 4)}${'•' * 14}${token.substring(token.length - 4)}',
                  key: const ValueKey('bot-token'),
                  maxLines: _showToken ? 3 : 1,
                  overflow: TextOverflow.ellipsis,
                  style: ts(13, color: t.ink, mono: true),
                ),
              ),
              IconBtn(_showToken ? 'eyeoff' : 'eye', color: t.muted, iconSize: 16, tooltip: context.t(_showToken ? 'bots.tokenHide' : 'bots.tokenShow'), onPressed: () => setState(() => _showToken = !_showToken)),
              Tap(
                onTap: () => unawaited(_copy(token)),
                color: t.sunk,
                radius: BorderRadius.circular(9),
                padding: const EdgeInsets.symmetric(horizontal: 12),
                child: SizedBox(height: 34, child: Center(child: Text(context.t('common.copy'), style: ts(13, weight: 650, color: t.accentText)))),
              ),
            ]),
          )
        else
          Container(
            padding: const EdgeInsets.all(16),
            decoration: BoxDecoration(color: t.sunk, borderRadius: BorderRadius.circular(12)),
            child: Text(context.t('bots.tokenHidden'), style: ts(13.5, color: t.muted)),
          ),
        if (_regenerated) note(context.t('bots.tokenRegenerated'), color: t.danger),

        const SizedBox(height: 26),
        label('bots.sendTitle'),
        Row(children: [
          for (final (key, name) in const [('curl', 'cURL'), ('js', 'JavaScript'), ('py', 'Python')])
            Padding(
              padding: const EdgeInsets.only(right: 6, bottom: 8),
              child: Tap(
                onTap: () => setState(() => _lang = key),
                color: _lang == key ? t.sunk : null,
                radius: BorderRadius.circular(7),
                padding: const EdgeInsets.symmetric(horizontal: 10, vertical: 6),
                child: Text(name, style: ts(12.5, weight: 650, color: _lang == key ? t.ink : t.muted)),
              ),
            ),
          const Spacer(),
          Tap(
            onTap: () => unawaited(_copy(code(token ?? '<TOKEN>'))),
            padding: const EdgeInsets.symmetric(horizontal: 8, vertical: 6),
            child: Text(context.t('common.copy'), style: ts(12.5, weight: 650, color: t.accentText)),
          ),
        ]),
        Container(
          width: double.infinity,
          padding: const EdgeInsets.symmetric(horizontal: 16, vertical: 14),
          decoration: BoxDecoration(color: t.dark ? const Color(0xFF0A0A09) : const Color(0xFF1E1D1A), borderRadius: BorderRadius.circular(12)),
          child: SingleChildScrollView(
            scrollDirection: Axis.horizontal,
            child: Directionality(
              textDirection: TextDirection.ltr,
              child: Text(code(_showToken && token != null ? token : '<TOKEN>'), style: ts(12, color: const Color(0xFFE9E4D8), height: 1.7, mono: true)),
            ),
          ),
        ),
        note(context.t('bots.sendHint')),

        const SizedBox(height: 26),
        Padding(
          padding: const EdgeInsets.only(bottom: 8),
          child: Text(tr('bots.subscribers', args: {'count': subs?.length ?? bot.subscribers}), style: ts(14, weight: 700, color: t.ink)),
        ),
        if (subs != null && subs.isEmpty)
          Container(
            padding: const EdgeInsets.all(16),
            decoration: BoxDecoration(color: t.sunk, borderRadius: BorderRadius.circular(12)),
            child: Text(context.t('bots.noSubscribers'), style: ts(13.5, color: t.muted)),
          ),
        if (subs != null && subs.isNotEmpty)
          Container(
            decoration: BoxDecoration(borderRadius: BorderRadius.circular(12), border: Border.all(color: t.line)),
            clipBehavior: Clip.antiAlias,
            child: Column(children: [
              for (final s in subs)
                Tap(
                  onTap: () => setState(() => _to = s.chatId),
                  onLongPress: () => unawaited(_copy(s.chatId)),
                  color: s.chatId == target?.chatId ? t.accentSoft : t.pane,
                  padding: const EdgeInsets.symmetric(horizontal: 14, vertical: 11),
                  child: Row(children: [
                    Avatar(name: s.mine ? tr('bots.subscriberYou') : '#', seed: s.chatId, size: 36),
                    const SizedBox(width: 12),
                    Expanded(
                      child: Column(crossAxisAlignment: CrossAxisAlignment.start, children: [
                        Text(_subName(s), style: ts(14, weight: 600, color: t.ink)),
                        Text(
                          tr('bots.startedOn', args: {'date': MaterialLocalizations.of(context).formatMediumDate(DateTime.fromMillisecondsSinceEpoch(s.since))}),
                          style: ts(12, color: t.muted),
                        ),
                      ]),
                    ),
                    Text('chat_id ${s.chatId.length > 8 ? s.chatId.substring(0, 8) : s.chatId}…', style: ts(12, color: t.muted, mono: true)),
                    const SizedBox(width: 6),
                    IconBtn('copy', color: t.muted, iconSize: 15, tooltip: context.t('common.copy'), onPressed: () => unawaited(_copy(s.chatId))),
                  ]),
                ),
            ]),
          ),

        const SizedBox(height: 26),
        Container(
          padding: const EdgeInsets.all(16),
          decoration: BoxDecoration(color: t.sunk, borderRadius: BorderRadius.circular(14)),
          child: Column(crossAxisAlignment: CrossAxisAlignment.start, children: [
            Text(context.t('bots.testTitle'), style: ts(14, weight: 700, color: t.ink)),
            const SizedBox(height: 4),
            Text(target == null ? context.t('bots.testNoSubs') : _subName(target), style: ts(12.5, color: t.muted)),
            const SizedBox(height: 10),
            AppField(controller: _text, maxLength: 4096, onSubmitted: (_) => unawaited(_send())),
            const SizedBox(height: 10),
            Btn(
              key: const ValueKey('bot-test'),
              label: context.t('bots.send'),
              icon: 'send',
              height: 44,
              busy: _sending,
              onPressed: target == null || _text.text.trim().isEmpty ? null : () => unawaited(_send()),
            ),
          ]),
        ),

        const SizedBox(height: 26),
        Align(
          alignment: AlignmentDirectional.centerStart,
          child: Tap(
            key: const ValueKey('bot-delete'),
            onTap: () => unawaited(_delete()),
            padding: const EdgeInsets.symmetric(vertical: 6),
            child: Row(mainAxisSize: MainAxisSize.min, children: [
              AppIcon('trash', size: 16, color: t.danger),
              const SizedBox(width: 8),
              Text(context.t('bots.delete'), style: ts(14, weight: 650, color: t.danger)),
            ]),
          ),
        ),
      ]),
    );
  }
}

class _CreateBot extends StatefulWidget {
  const _CreateBot();

  @override
  State<_CreateBot> createState() => _CreateBotState();
}

class _CreateBotState extends State<_CreateBot> {
  final _name = TextEditingController();
  final _handle = TextEditingController();
  final _about = TextEditingController();
  bool _touched = false;
  bool _busy = false;
  String? _error;
  String? _inUse;
  OwnedBot? _done;
  Timer? _check;

  @override
  void dispose() {
    _check?.cancel();
    _name.dispose();
    _handle.dispose();
    _about.dispose();
    super.dispose();
  }

  String get _raw => botSlug(_handle.text);
  String get _username => '${_raw}_bot';

  void _changed() {
    setState(() {});
    _check?.cancel();
    final u = _username;
    if (_raw.length < 3) return;
    // The server has the last word; this only catches a bot that already has the name.
    _check = Timer(const Duration(milliseconds: 400), () async {
      try {
        await backend.botFind(u);
        if (mounted) setState(() => _inUse = u);
      } catch (_) {}
    });
  }

  Future<void> _submit() async {
    setState(() {
      _busy = true;
      _error = null;
    });
    try {
      final bot = await backend.botCreate(username: _username, name: _name.text.trim(), about: _about.text.trim());
      if (mounted) setState(() => _done = bot);
    } on CoreError catch (e) {
      if (!mounted) return;
      if (e.code == 'conflict') {
        setState(() => _inUse = _username);
      } else {
        setState(() => _error = errorText(e));
      }
    } catch (e) {
      if (mounted) setState(() => _error = errorText(e));
    }
    if (mounted) setState(() => _busy = false);
  }

  @override
  Widget build(BuildContext context) {
    final t = context.tk;
    final prefix = userLinksAvailable ? '$kUserLinkHost/@' : '@';
    final done = _done;
    if (done != null) {
      return Column(mainAxisSize: MainAxisSize.min, crossAxisAlignment: CrossAxisAlignment.stretch, children: [
        Container(
          padding: const EdgeInsets.all(14),
          decoration: BoxDecoration(color: t.accentSoft, borderRadius: BorderRadius.circular(14)),
          child: Row(children: [
            AppIcon('okcircle', size: 22, color: t.accentText),
            const SizedBox(width: 12),
            Expanded(
              child: Column(crossAxisAlignment: CrossAxisAlignment.start, children: [
                Text(tr('bots.ready', args: {'name': done.username}), style: ts(14.5, weight: 700, color: t.ink)),
                Text('$prefix${done.username}', style: ts(12.5, color: t.muted, mono: true)),
              ]),
            ),
          ]),
        ),
        const SizedBox(height: 18),
        Text(context.t('bots.token'), style: ts(13, weight: 700, color: t.ink)),
        const SizedBox(height: 7),
        Container(
          padding: const EdgeInsets.fromLTRB(14, 8, 6, 8),
          decoration: BoxDecoration(color: t.sunk, borderRadius: BorderRadius.circular(12)),
          child: Row(children: [
            Expanded(child: Text(done.token ?? '', key: const ValueKey('bot-new-token'), style: ts(13, color: t.ink, mono: true))),
            Tap(
              onTap: () => unawaited(_copy(done.token ?? '')),
              color: t.pane,
              radius: BorderRadius.circular(9),
              padding: const EdgeInsets.symmetric(horizontal: 12),
              child: SizedBox(height: 34, child: Center(child: Text(context.t('common.copy'), style: ts(13, weight: 650, color: t.accentText)))),
            ),
          ]),
        ),
        const SizedBox(height: 7),
        Text(context.t('bots.tokenWarning'), style: ts(12.5, color: t.muted, height: 1.45)),
        const SizedBox(height: 18),
        Btn(label: context.t('bots.done'), height: 46, onPressed: () => Navigator.of(context).pop(done)),
      ]);
    }

    final raw = _raw;
    final taken = raw.length >= 3 && _inUse == _username;
    final ok = raw.length >= 3 && !taken;
    final can = _name.text.trim().isNotEmpty && ok && !_busy;
    final stateColor = raw.isEmpty ? t.muted : ok ? t.accentText : t.danger;
    return Column(mainAxisSize: MainAxisSize.min, crossAxisAlignment: CrossAxisAlignment.stretch, children: [
      AppField(
        key: const ValueKey('bot-name'),
        controller: _name,
        label: context.t('bots.name'),
        hint: context.t('bots.namePlaceholder'),
        autofocus: true,
        maxLength: 64,
        onChanged: (v) {
          if (!_touched) _handle.text = botSlug(v);
          _changed();
        },
      ),
      const SizedBox(height: 18),
      Text(context.t('bots.address'), style: ts(13, weight: 700, color: t.ink)),
      const SizedBox(height: 7),
      Directionality(
        textDirection: TextDirection.ltr,
        child: Container(
          height: 46,
          padding: const EdgeInsets.symmetric(horizontal: 14),
          decoration: BoxDecoration(
            color: t.pane,
            borderRadius: BorderRadius.circular(12),
            border: Border.all(color: raw.isEmpty ? t.line : ok ? t.accent : t.danger, width: 1.5),
          ),
          child: Row(children: [
            Text(prefix, style: ts(13.5, color: t.muted, mono: true)),
            Expanded(
              child: TextField(
                key: const ValueKey('bot-address'),
                controller: _handle,
                autocorrect: false,
                enableSuggestions: false,
                textCapitalization: TextCapitalization.none,
                inputFormatters: [FilteringTextInputFormatter.allow(RegExp(r'[a-z0-9_]')), LengthLimitingTextInputFormatter(28)],
                style: ts(13.5, color: t.ink, mono: true),
                cursorColor: t.accent,
                decoration: InputDecoration(isDense: true, border: InputBorder.none, hintText: context.t('bots.addressPlaceholder'), hintStyle: ts(13.5, color: t.faint, mono: true)),
                onChanged: (_) {
                  _touched = true;
                  _changed();
                },
              ),
            ),
            Text('_bot', style: ts(13.5, color: t.muted, mono: true)),
          ]),
        ),
      ),
      const SizedBox(height: 7),
      Row(children: [
        AppIcon(ok ? 'okcircle' : 'info', size: 14, color: stateColor),
        const SizedBox(width: 6),
        Expanded(
          child: Text(
            raw.isEmpty
                ? context.t('bots.addressRules')
                : raw.length < 3
                    ? context.t('bots.addressShort')
                    : taken
                        ? context.t('bots.addressTaken')
                        : tr('bots.addressFree', args: {'address': '$prefix$_username'}),
            style: ts(12.5, color: stateColor),
          ),
        ),
      ]),
      const SizedBox(height: 18),
      AppField(controller: _about, label: '${context.t('bots.description')} ${context.t('bots.optional')}', hint: context.t('bots.descriptionPlaceholder'), maxLength: 512),
      if (_error != null) ...[
        const SizedBox(height: 10),
        Text(_error!, style: ts(12.5, color: t.danger)),
      ],
      const SizedBox(height: 18),
      Btn(key: const ValueKey('bot-create'), label: context.t('bots.createButton'), height: 46, busy: _busy, onPressed: can ? () => unawaited(_submit()) : null),
    ]);
  }
}
