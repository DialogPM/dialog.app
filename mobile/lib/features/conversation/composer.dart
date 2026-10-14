// Message composer: attachments (photos/videos, camera, document, location), emoji, text with
// drafts and typing indicator, reply / edit panel, voice notes.

import 'dart:async';

import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:flutter_riverpod/flutter_riverpod.dart';

import '../../app/globals.dart';
import '../../core/models.dart';
import '../../l10n/i18n.dart';
import '../../services/files.dart';
import '../../state/actions.dart';
import '../../state/store.dart';
import '../../theme/app_theme.dart';
import '../../util/format.dart';
import '../../util/mentions.dart';
import '../../widgets/app_icon.dart';
import '../../widgets/dialogs.dart';
import '../../widgets/emoji_picker.dart';
import '../../widgets/ui.dart';
import 'location.dart';
import 'message_bubble.dart';
import 'send_media.dart';
import 'share_contacts.dart';
import 'voice.dart';

const _typingRefresh = Duration(seconds: 3);
const _typingIdle = Duration(seconds: 4);
const _draftDelay = Duration(milliseconds: 800);

class Composer extends ConsumerStatefulWidget {
  const Composer({super.key, required this.conv});
  final Conversation conv;

  @override
  ConsumerState<Composer> createState() => _ComposerState();
}

class _ComposerState extends ConsumerState<Composer> {
  late final _text = TextEditingController(text: widget.conv.draft ?? '');
  late final _focus = FocusNode(onKeyEvent: _onKey);
  DateTime? _lastTyping;
  Timer? _typingTimer;
  Timer? _draftTimer;
  bool _recording = false;
  String? _editingId;
  String _beforeEdit = '';
  String? _replyId;

  /// Members picked from the @ suggestions (turned into mention ranges when sending).
  List<MentionPick> _picks = [];
  List<MentionPick> _beforeEditPicks = [];

  /// Start of an "@query" the user dismissed (or just completed).
  int? _dismissed;
  int _active = 0;

  String get id => widget.conv.id;

  @override
  void initState() {
    super.initState();
    _text.addListener(() => setState(() {}));
  }

  @override
  void dispose() {
    _draftTimer?.cancel();
    _typingTimer?.cancel();
    final editing = appData.composing[id]?.editing != null;
    if (!editing) {
      final v = _text.text;
      unawaited(backend.setDraft(id, v.trim().isEmpty ? null : v).catchError((_) {}));
    }
    if (_lastTyping != null) unawaited(backend.sendTyping(id, false).catchError((_) {}));
    _text.dispose();
    _focus.dispose();
    super.dispose();
  }

  /// Desktop builds (used for development): Enter sends, Shift+Enter adds a line.
  KeyEventResult _onKey(FocusNode node, KeyEvent e) {
    if (isMobilePlatform || e is! KeyDownEvent) return KeyEventResult.ignored;
    final (query, suggestions) = _suggestions();
    if (query != null && suggestions.isNotEmpty) {
      final k = e.logicalKey;
      if (k == LogicalKeyboardKey.arrowDown || k == LogicalKeyboardKey.arrowUp) {
        setState(() => _active = (_active + (k == LogicalKeyboardKey.arrowDown ? 1 : -1) + suggestions.length) % suggestions.length);
        return KeyEventResult.handled;
      }
      if (k == LogicalKeyboardKey.enter || k == LogicalKeyboardKey.tab) {
        _pickMention(query, suggestions[_active.clamp(0, suggestions.length - 1)].$1);
        return KeyEventResult.handled;
      }
      if (k == LogicalKeyboardKey.escape) {
        setState(() => _dismissed = query.$1);
        return KeyEventResult.handled;
      }
    }
    if ((e.logicalKey == LogicalKeyboardKey.enter || e.logicalKey == LogicalKeyboardKey.numpadEnter) && !HardwareKeyboard.instance.isShiftPressed) {
      _send();
      return KeyEventResult.handled;
    }
    return KeyEventResult.ignored;
  }

  bool get _typingEnabled => appData.details?.privacy.typingIndicators ?? true;

  void _stopTyping() {
    _typingTimer?.cancel();
    if (_lastTyping != null) {
      _lastTyping = null;
      unawaited(backend.sendTyping(id, false).catchError((_) {}));
    }
  }

  void _onChanged(String value) {
    if (value.isEmpty) _dismissed = null;
    if (_editingId == null) {
      _draftTimer?.cancel();
      _draftTimer = Timer(_draftDelay, () => unawaited(backend.setDraft(id, value.trim().isEmpty ? null : value).catchError((_) {})));
    }
    if (!_typingEnabled) return;
    final now = DateTime.now();
    if (value.isNotEmpty && (_lastTyping == null || now.difference(_lastTyping!) > _typingRefresh)) {
      _lastTyping = now;
      unawaited(backend.sendTyping(id, true).catchError((_) {}));
    }
    _typingTimer?.cancel();
    _typingTimer = Timer(_typingIdle, _stopTyping);
    if (value.isEmpty) _stopTyping();
  }

  void _send() {
    final raw = _text.text;
    final value = raw.trim();
    if (value.isEmpty) return;
    _stopTyping();
    _draftTimer?.cancel();
    final editing = _editingId != null;
    // Mentions are computed on the untrimmed text, then shifted by the trimmed prefix.
    final lead = raw.length - raw.trimLeft().length;
    final mentions = [
      for (final m in collectMentions(raw, _picks))
        if (m.start - lead >= 0 && m.start - lead + m.length <= value.length) Mention(accountId: m.accountId, start: m.start - lead, length: m.length),
    ];
    _text.clear();
    _picks = [];
    _dismissed = null;
    if (editing) _beforeEdit = '';
    unawaited(sendText(id, value, mentions: mentions));
    if (!editing) unawaited(backend.setDraft(id, null).catchError((_) {}));
  }

  /// Members that can be mentioned (groups only).
  List<(MentionPick, String?)> _candidates() {
    final group = appData.groups[id];
    if (!widget.conv.isGroup || group == null) return const [];
    final out = <String, (MentionPick, String?)>{};
    for (final m in group.members) {
      if (m.isMe) continue;
      final contact = appData.contacts[m.accountId];
      final name = group.settings.firstNames
          ? (m.displayName == 'Member' ? context.t('groupRules.unnamed') : m.displayName)
          : (contact?.displayName ?? m.displayName);
      out[m.accountId] = (MentionPick(m.accountId, name), contact?.avatarPath ?? m.avatarPath);
    }
    // Hidden member list: suggest the people who wrote in the loaded history.
    if (group.membersHidden) {
      for (final mid in appData.convMessages[id]?.ids ?? const <String>[]) {
        final msg = appData.messages[mid];
        if (msg == null || msg.outgoing || msg.system != null || out.containsKey(msg.senderAccountId)) continue;
        out[msg.senderAccountId] = (MentionPick(msg.senderAccountId, msg.senderName), null);
      }
    }
    final list = out.values.where((c) => c.$1.name.trim().isNotEmpty).toList()..sort((a, b) => a.$1.name.compareTo(b.$1.name));
    return list;
  }

  /// The current "@query" and its suggestions.
  ((int, String)?, List<(MentionPick, String?)>) _suggestions() {
    final sel = _text.selection;
    if (!widget.conv.isGroup || !sel.isValid || !sel.isCollapsed) return (null, const []);
    final q = mentionQuery(_text.text, sel.baseOffset);
    if (q == null || q.$1 == _dismissed) return (q, const []);
    return (q, filterMentionCandidates(_candidates(), (c) => c.$1.name, q.$2));
  }

  void _pickMention((int, String) q, MentionPick p) {
    final caret = _text.selection.baseOffset;
    final insert = '@${p.name} ';
    final next = _text.text.replaceRange(q.$1, caret, insert);
    _picks = [..._picks.where((x) => x.accountId != p.accountId || x.name != p.name), p];
    _dismissed = q.$1;
    _active = 0;
    _text.value = TextEditingValue(text: next, selection: TextSelection.collapsed(offset: q.$1 + insert.length));
    _onChanged(next);
    _focus.requestFocus();
  }

  void _format(String mark) {
    final sel = _text.selection;
    if (!sel.isValid) return;
    final (next, start, end) = wrapSelection(_text.text, sel.start, sel.end, mark);
    _text.value = TextEditingValue(text: next, selection: TextSelection(baseOffset: start, extentOffset: end));
    _onChanged(next);
  }

  Future<void> _insertEmoji() async {
    final e = await pickEmoji(context);
    if (e == null) return;
    final sel = _text.selection;
    final text = _text.text;
    final start = sel.isValid ? sel.start : text.length;
    final end = sel.isValid ? sel.end : text.length;
    _text.value = TextEditingValue(text: text.replaceRange(start, end, e), selection: TextSelection.collapsed(offset: start + e.length));
    _onChanged(_text.text);
    _focus.requestFocus();
  }

  Future<void> _attach() async {
    await showActionSheet(context, actions: [
      SheetAction(
        label: context.t('composer.photos'),
        icon: 'image',
        onTap: () async {
          final files = await pickFiles(kind: 'media', multiple: true);
          if (files.isNotEmpty && mounted) openSendMedia(context, id, files, media: true);
        },
      ),
      SheetAction(
        label: context.t('mobile.takePhoto'),
        icon: 'camera',
        hidden: !isMobilePlatform,
        onTap: () async {
          final f = await takePhoto();
          if (f != null && mounted) openSendMedia(context, id, [f], media: true);
        },
      ),
      SheetAction(
        label: context.t('composer.document'),
        icon: 'file',
        onTap: () async {
          final files = await pickFiles(multiple: true);
          if (files.isNotEmpty && mounted) openSendMedia(context, id, files, media: false);
        },
      ),
      SheetAction(
        label: context.t('composer.location'),
        icon: 'mappin',
        onTap: () => Navigator.of(context).push(MaterialPageRoute<void>(builder: (_) => LocationPickerPage(conversationId: id))),
      ),
      SheetAction(
        label: context.t('composer.contact'),
        icon: 'user',
        onTap: () => Navigator.of(context).push(MaterialPageRoute<void>(builder: (_) => ShareContactsPage(conversationId: id))),
      ),
    ]);
  }

  @override
  Widget build(BuildContext context) {
    final t = context.tk;
    final comp = ref.watch(storeProvider.select((s) => s.composing[id]));
    final editing = comp?.editing;
    final replyTo = comp?.replyTo;
    // Entering / leaving edit mode swaps the input text.
    if ((editing?.id) != _editingId) {
      final wasEditing = _editingId != null;
      _editingId = editing?.id;
      WidgetsBinding.instance.addPostFrameCallback((_) {
        if (editing != null) {
          if (!wasEditing) {
            _beforeEdit = _text.text;
            _beforeEditPicks = _picks;
          }
          _text.text = editing.text ?? '';
          _picks = picksFromMentions(editing.text ?? '', editing.mentions);
          _focus.requestFocus();
        } else {
          _text.text = _beforeEdit;
          _picks = _beforeEditPicks;
        }
      });
    }
    if (replyTo?.id != _replyId) {
      _replyId = replyTo?.id;
      if (replyTo != null) WidgetsBinding.instance.addPostFrameCallback((_) => mounted ? _focus.requestFocus() : null);
    }
    final bottom = MediaQuery.of(context).padding.bottom;
    final hasText = _text.text.trim().isNotEmpty;
    final lang = context.lang;
    final (query, suggestions) = _suggestions();
    final active = suggestions.isEmpty ? 0 : _active.clamp(0, suggestions.length - 1);

    return Container(
      decoration: BoxDecoration(color: t.pane, border: Border(top: BorderSide(color: t.line))),
      padding: EdgeInsets.only(bottom: bottom),
      child: Column(mainAxisSize: MainAxisSize.min, children: [
        if (query != null && suggestions.isNotEmpty)
          ConstrainedBox(
            constraints: const BoxConstraints(maxHeight: 240),
            child: Semantics(
              label: context.t('mention.suggestions'),
              child: ListView(
                key: const ValueKey('mention-suggestions'),
                shrinkWrap: true,
                padding: const EdgeInsets.symmetric(vertical: 4),
                children: [
                  for (var i = 0; i < suggestions.length; i++)
                    Tap(
                      onTap: () => _pickMention(query, suggestions[i].$1),
                      color: i == active ? t.sunk : null,
                      child: Padding(
                        padding: const EdgeInsets.symmetric(horizontal: 16, vertical: 7),
                        child: Row(children: [
                          Avatar(name: suggestions[i].$1.name, path: suggestions[i].$2, seed: suggestions[i].$1.accountId, size: 32),
                          const SizedBox(width: 12),
                          Expanded(child: Text(suggestions[i].$1.name, maxLines: 1, overflow: TextOverflow.ellipsis, style: ts(15, color: t.ink))),
                        ]),
                      ),
                    ),
                ],
              ),
            ),
          ),
        if (editing != null || replyTo != null)
          Padding(
            padding: const EdgeInsets.fromLTRB(16, 10, 10, 0),
            child: Row(children: [
              if (editing != null) ...[
                AppIcon('edit', size: 19, color: t.accentText),
                const SizedBox(width: 10),
                Expanded(
                  child: QuoteBlock(
                    margin: EdgeInsets.zero,
                    bg: t.sunk,
                    bar: t.accentText,
                    q: QuotedMessage(
                      id: editing.id,
                      senderName: context.t('message.editing'),
                      kind: editing.kind,
                      text: (editing.text?.isNotEmpty ?? false) ? editing.text : kindLabel(editing.kind, lang),
                    ),
                  ),
                ),
              ] else if (replyTo != null)
                Expanded(
                  child: QuoteBlock(
                    margin: EdgeInsets.zero,
                    bg: t.sunk,
                    bar: t.accentText,
                    q: QuotedMessage(
                      id: replyTo.id,
                      senderName: replyTo.outgoing ? 'You' : replyTo.senderName,
                      kind: replyTo.kind,
                      text: replyTo.text ?? replyTo.location?.name,
                      thumbnail: replyTo.attachments.isEmpty ? null : replyTo.attachments.first.thumbnail,
                    ),
                  ),
                ),
              IconBtn('x', color: t.muted, tooltip: context.t('common.cancel'), onPressed: () => store.setComposing(id, replyTo: null, editing: null)),
            ]),
          ),
        Padding(
          padding: const EdgeInsets.fromLTRB(10, 10, 10, 12),
          child: _recording
              ? VoiceRecorder(
                  onCancel: () => setState(() => _recording = false),
                  onSend: (att, viewOnce) {
                    setState(() => _recording = false);
                    unawaited(sendMessage(conversationId: id, attachments: [att], viewOnce: viewOnce));
                  },
                )
              : Row(crossAxisAlignment: CrossAxisAlignment.end, children: [
                  IconBtn('clip', size: 44, iconSize: 20, round: true, color: t.muted, tooltip: context.t('composer.attach'), onPressed: _attach),
                  const SizedBox(width: 8),
                  Expanded(
                    child: Container(
                      constraints: const BoxConstraints(minHeight: 44),
                      padding: const EdgeInsets.only(left: 16, right: 6),
                      decoration: BoxDecoration(color: t.sunk, borderRadius: BorderRadius.circular(t.rInput.clamp(0, 22))),
                      child: Row(crossAxisAlignment: CrossAxisAlignment.end, children: [
                        Expanded(
                          child: TextField(
                            enableIMEPersonalizedLearning: imePersonalizedLearning,
                            controller: _text,
                            focusNode: _focus,
                            minLines: 1,
                            maxLines: 6,
                            keyboardType: TextInputType.multiline,
                            textCapitalization: TextCapitalization.sentences,
                            onChanged: _onChanged,
                            contextMenuBuilder: (context, editable) {
                              final items = [...editable.contextMenuButtonItems];
                              if (!_text.selection.isCollapsed) {
                                for (final (mark, key) in const [('*', 'format.bold'), ('_', 'format.italic'), ('~', 'format.strike'), ('```', 'format.mono')]) {
                                  items.add(ContextMenuButtonItem(
                                    label: context.t(key),
                                    onPressed: () {
                                      ContextMenuController.removeAny();
                                      _format(mark);
                                    },
                                  ));
                                }
                              }
                              return AdaptiveTextSelectionToolbar.buttonItems(anchors: editable.contextMenuAnchors, buttonItems: items);
                            },
                            style: ts(15, color: t.ink, height: 22 / 15),
                            decoration: InputDecoration(
                              isDense: true,
                              border: InputBorder.none,
                              // Disappearing messages on: say so where the user types.
                              hintText: widget.conv.disappearingSeconds > 0
                                  ? context.t('composer.placeholderTimed', args: {'duration': disappearingLabel(widget.conv.disappearingSeconds, context.lang)})
                                  : context.t('composer.placeholder'),
                              hintStyle: ts(15, color: t.ink.withValues(alpha: .5), height: 22 / 15),
                              contentPadding: const EdgeInsets.symmetric(vertical: 11),
                            ),
                          ),
                        ),
                        Padding(
                          padding: const EdgeInsets.only(bottom: 5),
                          child: IconBtn('smile', size: 34, iconSize: 20, round: true, color: t.muted, tooltip: context.t('composer.emoji'), onPressed: _insertEmoji),
                        ),
                      ]),
                    ),
                  ),
                  const SizedBox(width: 8),
                  SizedBox(
                    width: 44,
                    height: 44,
                    child: Tap(
                      onTap: hasText || editing != null ? (hasText ? _send : null) : () => setState(() => _recording = true),
                      color: t.accent.withValues(alpha: (editing != null && !hasText) ? .5 : 1),
                      radius: BorderRadius.circular(22),
                      semanticLabel: context.t(editing != null ? 'common.save' : hasText ? 'composer.send' : 'voice.record'),
                      child: Center(child: AppIcon(editing != null ? 'check' : hasText ? 'send' : 'mic', size: 19, color: t.onAccent)),
                    ),
                  ),
                ]),
        ),
      ]),
    );
  }
}
