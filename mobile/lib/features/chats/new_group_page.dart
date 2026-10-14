// "New group": pick members, then name, description and optional photo.

import 'dart:io';

import 'package:flutter/material.dart';
import 'package:image_picker/image_picker.dart';

import '../../app/globals.dart';
import '../../app/nav.dart';
import '../../core/models.dart';
import '../../l10n/i18n.dart';
import '../../state/actions.dart';
import '../../state/default_timer.dart';
import '../../theme/app_theme.dart';
import '../../widgets/app_icon.dart';
import '../../widgets/ui.dart';
import '../../util/format.dart' show kDisappearingOptions;
import 'contact_picker.dart';

/// Picks an image from the gallery (null when cancelled or unavailable).
Future<String?> pickImagePath() async {
  try {
    final f = await ImagePicker().pickImage(source: ImageSource.gallery, maxWidth: 1600, maxHeight: 1600, imageQuality: 90);
    return f?.path;
  } catch (e) {
    notifyError(e);
    return null;
  }
}

class NewGroupPage extends StatefulWidget {
  const NewGroupPage({super.key});

  @override
  State<NewGroupPage> createState() => _NewGroupPageState();
}

class _NewGroupPageState extends State<NewGroupPage> {
  int _step = 1;
  List<String> _members = [];
  final _name = TextEditingController();
  final _description = TextEditingController();
  String? _avatar;
  bool _busy = false;
  // Preselects the default timer for new chats (Settings → Privacy).
  int _timer = defaultTimer.value;
  bool _showMembers = true;
  bool _firstNames = false;

  @override
  void dispose() {
    _name.dispose();
    _description.dispose();
    super.dispose();
  }

  Future<void> _create() async {
    setState(() => _busy = true);
    final conv = await attempt(() => backend.createGroup(
          name: _name.text.trim(),
          description: _description.text.trim().isEmpty ? null : _description.text.trim(),
          memberAccountIds: _members,
          avatarPath: _avatar,
          settings: GroupSettings(
            onlyAdminsSend: false,
            onlyAdminsEdit: false,
            disappearingSeconds: _timer,
            hideMembers: !_showMembers,
            memberNames: _firstNames ? 'first_name' : 'number',
          ),
        ));
    if (!mounted) return;
    setState(() => _busy = false);
    if (conv == null) return;
    store.upsertConversation(conv);
    store.set((d) => d.copyWith(chatFilter: 'all'));
    openChat(conv.id);
  }

  @override
  Widget build(BuildContext context) {
    final t = context.tk;
    if (_step == 1) {
      return PaneScaffold(
        header: PaneHeader(
          titleWidget: Row(children: [
            Flexible(child: Text(context.t('newGroup.title'), maxLines: 1, overflow: TextOverflow.ellipsis, style: ts(17, weight: 700, color: t.ink))),
            const SizedBox(width: 8),
            Text(context.t('newGroup.selected', count: _members.length), style: ts(13, weight: 500, color: t.muted)),
          ]),
        ),
        body: ContactPicker(selected: _members, onChanged: (v) => setState(() => _members = v)),
        bottom: Container(
          padding: EdgeInsets.fromLTRB(16, 12, 16, 12 + MediaQuery.of(context).padding.bottom),
          decoration: BoxDecoration(color: t.pane, border: Border(top: BorderSide(color: t.line))),
          child: Btn(
            label: context.t('common.next'),
            height: 48,
            onPressed: _members.isEmpty ? null : () => setState(() => _step = 2),
          ),
        ),
      );
    }
    return PopScope(
      canPop: false,
      onPopInvokedWithResult: (didPop, _) {
        if (!didPop) setState(() => _step = 1);
      },
      child: PaneScaffold(
        header: PaneHeader(title: context.t('newGroup.details'), onBack: () => setState(() => _step = 1)),
        body: ListView(padding: const EdgeInsets.fromLTRB(20, 24, 20, 24), children: [
          Center(
            child: GestureDetector(
              onTap: () async {
                final p = await pickImagePath();
                if (p != null) setState(() => _avatar = p);
              },
              child: Stack(clipBehavior: Clip.none, children: [
                _avatar != null
                    ? ClipOval(child: Image.file(File(_avatar!), width: 96, height: 96, fit: BoxFit.cover))
                    : Avatar(name: _name.text.isEmpty ? '#' : _name.text, seed: 'new-group', size: 96, group: true),
                Positioned(
                  right: -4,
                  bottom: -2,
                  child: Container(
                    width: 42,
                    height: 42,
                    decoration: BoxDecoration(color: t.accent, shape: BoxShape.circle, border: Border.all(color: t.pane, width: 4)),
                    child: Center(child: AppIcon('camera', size: 15, color: t.onAccent)),
                  ),
                ),
              ]),
            ),
          ),
          const SizedBox(height: 8),
          Text(context.t('newGroup.photoOptional'), textAlign: TextAlign.center, style: ts(12.5, color: t.muted)),
          const SizedBox(height: 16),
          AppField(controller: _name, label: context.t('group.name'), maxLength: 80, autofocus: true, onChanged: (_) => setState(() {})),
          const SizedBox(height: 16),
          AppField(controller: _description, label: context.t('group.description'), maxLength: 500, maxLines: 3),
          const SizedBox(height: 16),
          Text(context.t('newGroup.membersCount', count: _members.length), style: ts(12.5, color: t.muted)),
          const SizedBox(height: 16),
          Container(
            decoration: BoxDecoration(border: Border.all(color: t.line), borderRadius: BorderRadius.circular(14)),
            padding: const EdgeInsets.symmetric(vertical: 8),
            child: Column(crossAxisAlignment: CrossAxisAlignment.stretch, children: [
              Padding(
                padding: const EdgeInsets.fromLTRB(16, 4, 16, 6),
                child: Text(context.t('groupRules.title'), style: ts(12.5, weight: 600, color: t.muted)),
              ),
              Padding(
                padding: const EdgeInsets.symmetric(horizontal: 16),
                child: Column(crossAxisAlignment: CrossAxisAlignment.start, children: [
                  Text(context.t('disappearing.title'), style: ts(14.5, weight: 600, color: t.ink)),
                  const SizedBox(height: 2),
                  Text(context.t('groupRules.timerHint'), style: ts(12.5, color: t.muted, height: 1.4)),
                  const SizedBox(height: 8),
                  Wrap(spacing: 8, runSpacing: 8, children: [
                    for (final (secs, key) in kDisappearingOptions)
                      ChoiceChip(
                        label: Text(context.t('disappearing.$key')),
                        selected: _timer == secs,
                        onSelected: (_) => setState(() => _timer = secs),
                      ),
                  ]),
                ]),
              ),
              const SizedBox(height: 6),
              ToggleRow(
                label: context.t('groupRules.showMembers'),
                sub: context.t('groupRules.showMembersHint'),
                on: _showMembers,
                onToggle: () => setState(() => _showMembers = !_showMembers),
              ),
              ToggleRow(
                label: context.t('groupRules.firstNames'),
                sub: context.t('groupRules.firstNamesHint'),
                on: _firstNames,
                onToggle: () => setState(() => _firstNames = !_firstNames),
              ),
            ]),
          ),
          const SizedBox(height: 16),
          Btn(
            label: context.t('newGroup.create'),
            icon: 'check',
            height: 48,
            busy: _busy,
            onPressed: _name.text.trim().isEmpty ? null : _create,
          ),
        ]),
      ),
    );
  }
}
