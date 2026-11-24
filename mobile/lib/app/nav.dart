// Navigation between the full-screen panes of the mobile layout (one pane at a time, like the
// design's device === 'mobile' variant). The Android back button pops the stack.

import 'package:flutter/material.dart';

import '../features/chats/new_chat_page.dart';
import '../features/chats/new_group_page.dart';
import '../features/chats/qr_sheet.dart';
import '../features/conversation/conversation_page.dart';
import '../features/info/info_page.dart';
import '../features/notices/notices_page.dart';
import '../features/settings/settings_pages.dart';
import '../services/notifications.dart' show kNoticesPayload;
import '../state/actions.dart';
import 'globals.dart';

NavigatorState? get _nav => navigatorKey.currentState;

Route<T> pageRoute<T>(Widget page, {String? name}) =>
    MaterialPageRoute<T>(builder: (_) => page, settings: RouteSettings(name: name));

/// Name of the app-lock route (app.dart); nothing may be pushed over it or pop it.
const lockRouteName = 'lock';

/// A conversation asked for while the app was locked (a tapped notification): opened by
/// [openPendingChat] once the user unlocked.
String? _pendingChat;

/// Opens a conversation on top of the chat list (replacing any open conversation). While the
/// app is locked the request is kept for after the unlock: `popUntil` would otherwise remove
/// the lock route (it pops without asking `PopScope`), showing the chat without the password.
void openChat(String conversationId, {String? highlightId}) {
  if (appData.locked) {
    _pendingChat = conversationId;
    return;
  }
  prepareConversation(conversationId);
  if (highlightId != null) store.set((d) => d.copyWith(highlightId: highlightId));
  final nav = _nav;
  if (nav == null) return;
  var lockOnTop = false;
  nav.popUntil((r) {
    if (r.settings.name == lockRouteName) lockOnTop = true;
    return lockOnTop || r.isFirst;
  });
  if (lockOnTop) {
    _pendingChat = conversationId;
    return;
  }
  nav.push(pageRoute<void>(ConversationPage(conversationId: conversationId), name: 'chat'));
}

/// Opens the conversation requested while locked, if any (called after the lock is lifted).
void openPendingChat() {
  final id = _pendingChat;
  _pendingChat = null;
  if (id != null) openChatFromNotification(id);
}

void openChatFromNotification(String conversationId) {
  // A notice from the server operator: its payload is not a conversation id.
  if (conversationId == kNoticesPayload) {
    if (appData.appState?.ready != true) return;
    if (appData.locked) {
      _pendingChat = conversationId;
      return;
    }
    openNotices();
    return;
  }
  if (appData.appState?.ready != true || !appData.conversations.containsKey(conversationId)) return;
  openChat(conversationId);
}

/// Opens the notices from the server operator (bell on the chat list, settings, a notification).
void openNotices() {
  final nav = _nav;
  if (nav == null) return;
  // Already on screen (a second notification tapped): nothing to push.
  var onTop = false;
  nav.popUntil((r) {
    if (r.settings.name == 'notices') onTop = true;
    return true;
  });
  if (!onTop) nav.push(pageRoute<void>(const NoticesPage(), name: 'notices'));
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
