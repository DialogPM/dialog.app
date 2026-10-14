// Navigation between the full-screen panes of the mobile layout (one pane at a time, like the
// design's device === 'mobile' variant). The Android back button pops the stack.

import 'package:flutter/material.dart';

import '../features/chats/new_chat_page.dart';
import '../features/chats/new_group_page.dart';
import '../features/chats/qr_sheet.dart';
import '../features/conversation/conversation_page.dart';
import '../features/info/info_page.dart';
import '../features/settings/settings_pages.dart';
import '../state/actions.dart';
import 'globals.dart';

NavigatorState? get _nav => navigatorKey.currentState;

Route<T> pageRoute<T>(Widget page, {String? name}) =>
    MaterialPageRoute<T>(builder: (_) => page, settings: RouteSettings(name: name));

/// Opens a conversation on top of the chat list (replacing any open conversation).
void openChat(String conversationId, {String? highlightId}) {
  prepareConversation(conversationId);
  if (highlightId != null) store.set((d) => d.copyWith(highlightId: highlightId));
  final nav = _nav;
  if (nav == null) return;
  nav.popUntil((r) => r.isFirst);
  nav.push(pageRoute<void>(ConversationPage(conversationId: conversationId), name: 'chat'));
}

void openChatFromNotification(String conversationId) {
  if (appData.appState?.ready != true || !appData.conversations.containsKey(conversationId)) return;
  openChat(conversationId);
}

/// Opens (or creates) the direct chat with an account.
Future<void> openDirect(String accountId) async {
  final c = await openDirectConversation(accountId);
  if (c != null) openChat(c.id);
}

void openInfo(String conversationId) => _nav?.push(pageRoute<void>(InfoPage(conversationId: conversationId), name: 'info'));

void openSettingsSection(String section) => _nav?.push(pageRoute<void>(SettingsSectionPage(section: section), name: 'settings/$section'));

void openNewChat() => _nav?.push(pageRoute<void>(const NewChatPage(), name: 'new_chat'));

void openNewGroup() => _nav?.push(pageRoute<void>(const NewGroupPage(), name: 'new_group'));

void openQr({String tab = 'mine'}) {
  final ctx = navigatorKey.currentContext;
  if (ctx != null) showQrSheet(ctx, initial: tab);
}
