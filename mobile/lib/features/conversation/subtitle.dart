import 'package:flutter/widgets.dart';
import 'package:flutter_riverpod/flutter_riverpod.dart';

import '../../core/models.dart';
import '../../l10n/i18n.dart';
import '../../state/store.dart';
import '../../util/format.dart';

/// Status line under a conversation title: typing, online / last seen, or the group members.
({String text, bool accent}) conversationSubtitle(BuildContext context, WidgetRef ref, Conversation conv) {
  final typing = ref.watch(storeProvider.select((s) => s.typing[conv.id]));
  final presence = ref.watch(storeProvider.select((s) => conv.peerAccountId == null ? null : s.presence[conv.peerAccountId]));
  final group = ref.watch(storeProvider.select((s) => s.groups[conv.id]));
  final names = typing?.values.map((x) => x.name).toList() ?? const <String>[];
  final lang = context.lang;
  if (names.isNotEmpty) {
    if (!conv.isGroup) return (text: context.t('presence.typing'), accent: true);
    if (names.length == 1) return (text: context.t('presence.typingName', args: {'name': names.first}), accent: true);
    return (text: context.t('presence.typingMany', count: names.length), accent: true);
  }
  if (conv.isGroup) {
    if (conv.left) return (text: context.t('group.notMember'), accent: false);
    if (group != null && group.membersHidden) return (text: context.t('group.members', count: group.memberCount), accent: false);
    if (group != null) {
      // Others first, "You" last (like the design: "Luca, Elena, Maria, Tu").
      final list = [...group.members]..sort((a, b) => (a.isMe ? 1 : 0) - (b.isMe ? 1 : 0));
      return (text: list.map((m) => m.isMe ? context.t('common.you') : m.displayName == 'Member' ? context.t('groupRules.unnamed') : m.displayName.split(' ').first).join(', '), accent: false);
    }
    return (text: context.t('group.members', count: conv.memberCount ?? 0), accent: false);
  }
  if (conv.blocked) return (text: context.t('chat.block.blocked'), accent: false);
  if (presence?.online == true) return (text: context.t('presence.online'), accent: true);
  final seen = presence?.lastSeen;
  if (seen != null) return (text: formatLastSeen(seen, lang), accent: false);
  return (text: context.t('chat.tapForInfo'), accent: false);
}
