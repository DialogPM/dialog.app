// High level actions: call the core and update the store (port of app/src/store/actions.ts).

import 'dart:async';

import 'package:flutter/foundation.dart';

import '../app/globals.dart';
import '../core/backend.dart';
import '../core/models.dart';
import '../l10n/i18n.dart';
import '../widgets/toast.dart';
import 'call_settings.dart';
import 'default_timer.dart';
import 'media_retention.dart';
import 'dnd.dart';
import 'store.dart';
import '../services/remote_wipe.dart';

const _page = 40;

String errorText(Object err) {
  final e = parseError(err);
  final key = 'errors.${e.code}';
  if (hasTranslation(key)) return tr(key);
  return e.message.isNotEmpty ? e.message : tr('errors.internal');
}

void notifyError(Object err) {
  debugPrint('error: $err');
  showToast(errorText(err), error: true, title: tr('errors.title'));
}

void notifyOk(String message) => showToast(message);

/// Runs an async action and shows an error toast on failure.
Future<T?> attempt<T>(Future<T> Function() fn) async {
  try {
    return await fn();
  } catch (e) {
    notifyError(e);
    return null;
  }
}

/// Hooks for platform services (notifications, push) set by main.dart.
void Function()? onSessionStarted;

Future<void> bootstrap() async {
  try {
    final s = await backend.getState();
    store.set((d) => d.copyWith(appState: s));
    if (s.ready) {
      // A saved session opened at app start: honour the app lock.
      initAppLock(s.account?.accountId, atStartup: true);
      await loadSession();
    }
  } catch (e) {
    notifyError(e);
    store.set((d) => d.copyWith(appState: const AppState(status: 'no_account', defaultServerUrl: kDefaultServer)));
  }
}

Future<void> loadSession() async {
  final results = await Future.wait([
    backend.getAccountDetails(),
    backend.getProfile(),
    backend.listContacts(),
    backend.listConversations(),
    backend.getConnectionState(),
  ]);
  final details = results[0] as AccountDetails;
  final contacts = results[2] as List<Contact>;
  final conversations = results[3] as List<Conversation>;
  store.set((d) => d.copyWith(
        details: details,
        profile: results[1] as Profile,
        connection: results[4] as String,
        contacts: {for (final c in contacts) c.accountId: c},
        conversations: {for (final c in conversations) c.id: c},
      ));
  unawaited(backend.checkServer(details.account.serverUrl).then((info) {
    store.set((d) => d.copyWith(serverInfo: info));
  }).catchError((_) {}));
  unawaited(loadDnd());
  unawaited(loadCallSettings());
  unawaited(loadDefaultTimer());
  startMediaChecks();
  onSessionStarted?.call();
}

/// Called after register / login / recover / device linking.
Future<void> enterApp() async {
  final s = await backend.getState();
  store.set((d) => d.copyWith(appState: s, loggedOutReason: null));
  initAppLock(s.account?.accountId, atStartup: false);
  await loadSession();
}

Future<void> logout() async {
  await backend.logout();
  signedOut(null);
}

/// Returns to onboarding, optionally explaining why.
void signedOut(String? reason) {
  final defaultServer = appData.appState?.defaultServerUrl ?? kDefaultServer;
  store.resetSession();
  applyDnd(0);
  store.set((d) => d.copyWith(
        loggedOutReason: reason,
        locked: false,
        appState: AppState(status: 'no_account', defaultServerUrl: defaultServer),
      ));
}

// ---------------------------------------------------------------------------
// app lock
// ---------------------------------------------------------------------------

bool isLockEnabled(String? accountId) => accountId != null && prefs.lockedAccounts.contains(accountId);

void initAppLock(String? accountId, {required bool atStartup}) {
  store.set((d) => d.copyWith(locked: isLockEnabled(accountId) && atStartup));
}

void unlockApp() => store.set((d) => d.copyWith(locked: false));

DateTime? _hiddenAt;

/// Called by the lifecycle observer.
void onAppHidden() => _hiddenAt ??= DateTime.now();

void onAppVisible() {
  final at = _hiddenAt;
  _hiddenAt = null;
  final id = appData.appState?.account?.accountId;
  // Default (0): the app lock only asks at app start (cold start), never while the app runs.
  final relockMinutes = prefs.relockAfterMinutes;
  if (at != null && relockMinutes > 0 && isLockEnabled(id) && DateTime.now().difference(at) > Duration(minutes: relockMinutes)) {
    store.set((d) => d.copyWith(locked: true));
  }
  markActiveRead();
  unawaited(checkRemoteWipe());
  // Plaintext copies left while the app was in the background.
  if (appData.appState?.account != null) unawaited(sweepAppMedia());
}

// ---------------------------------------------------------------------------
// conversations
// ---------------------------------------------------------------------------

/// Prepares a conversation that is being opened (messages, read state, presence, group).
void prepareConversation(String id) {
  final s = appData;
  final c = s.conversations[id];
  if (s.convMessages[id]?.loaded != true) unawaited(loadMessages(id));
  if ((c?.unreadCount ?? 0) > 0) unawaited(backend.markRead(id).catchError((_) {}));
  if (c != null && !c.isGroup && c.peerAccountId != null) {
    unawaited(backend.watchPresence([c.peerAccountId!]).catchError((_) {}));
  }
  if (c?.isGroup == true) unawaited(loadGroup(id));
}

/// Marks the conversation on screen as read (app back in the foreground).
void markActiveRead() {
  final s = appData;
  final id = s.activeId;
  if (id != null && (s.conversations[id]?.unreadCount ?? 0) > 0) {
    unawaited(backend.markRead(id).catchError((_) {}));
  }
}

Future<Conversation?> openDirectConversation(String accountId) async {
  final c = await attempt(() => backend.openDirectConversation(accountId));
  if (c == null) return null;
  store.upsertConversation(c);
  final f = appData.chatFilter;
  store.set((d) => d.copyWith(chatFilter: c.archived ? 'archived' : f == 'archived' ? 'all' : f));
  return c;
}

Future<void> loadMessages(String conversationId) async {
  store.setConvLoading(conversationId, true);
  try {
    final page = await backend.getMessages(conversationId, limit: _page);
    store.addMessagePage(conversationId, page, page.length >= _page, reset: true);
  } catch (e) {
    store.setConvLoading(conversationId, false);
    notifyError(e);
  }
}

/// Loads the previous page; returns the number of messages loaded.
Future<int> loadOlder(String conversationId) async {
  final s = appData;
  final cm = s.convMessages[conversationId];
  if (cm == null || cm.loading || !cm.hasMore) return 0;
  final oldest = cm.ids.isEmpty ? null : s.messages[cm.ids.first]?.receivedAt;
  store.setConvLoading(conversationId, true);
  try {
    final page = await backend.getMessages(conversationId, before: oldest, limit: _page);
    store.addMessagePage(conversationId, page, page.length >= _page);
    return page.length;
  } catch (e) {
    store.setConvLoading(conversationId, false);
    notifyError(e);
    return 0;
  }
}

Future<void> sendText(String conversationId, String text, {List<Mention> mentions = const []}) async {
  final comp = appData.composing[conversationId];
  final editing = comp?.editing;
  if (editing != null) {
    final edited = await attempt(() => backend.editMessage(editing.id, text, mentions: mentions));
    if (edited != null) store.upsertMessage(edited);
    store.setComposing(conversationId, editing: null);
    return;
  }
  final replyTo = comp?.replyTo?.id;
  store.setComposing(conversationId, replyTo: null);
  final m = await attempt(() => backend.sendMessage(conversationId: conversationId, text: text, replyTo: replyTo, mentions: mentions));
  if (m != null) store.upsertMessage(m);
}

Future<Message?> sendMessage({
  required String conversationId,
  String? text,
  String? replyTo,
  List<OutgoingAttachment>? attachments,
  Location? location,
  List<String>? contactIds,
  bool viewOnce = false,
}) async {
  final comp = appData.composing[conversationId];
  final reply = replyTo ?? comp?.replyTo?.id;
  store.setComposing(conversationId, replyTo: null);
  final m = await attempt(() => backend.sendMessage(
        conversationId: conversationId,
        text: text,
        replyTo: reply,
        attachments: attachments,
        location: location,
        contactIds: contactIds,
        viewOnce: viewOnce,
      ));
  if (m != null) store.upsertMessage(m);
  return m;
}

Future<void> react(Message m, String? emoji) async {
  final res = await attempt(() => backend.react(m.id, emoji));
  if (res != null) store.upsertMessage(res);
}

Future<void> deleteMessages(List<String> ids, {required bool forEveryone}) async {
  for (final id in ids) {
    await attempt(() => backend.deleteMessage(id, forEveryone: forEveryone));
  }
}

/// Pins a message for [seconds] (null unpins); this device only.
Future<void> pinMessage(Message m, int? seconds) async {
  final res = await attempt(() => backend.pinMessage(m.id, seconds));
  if (res != null) store.upsertMessage(res);
}

/// Stars (for a week) or unstars a message; this device only.
Future<void> starMessage(Message m, bool starred) async {
  final res = await attempt(() => backend.starMessage(m.id, starred));
  if (res != null) store.upsertMessage(res);
}

Future<void> retryMessage(Message m) async {
  final res = await attempt(() => backend.retryMessage(m.id));
  if (res != null) store.upsertMessage(res);
}

Future<Conversation?> patchConversation(String conversationId, ConversationSettingsPatch patch) async {
  final c = await attempt(() => backend.setConversationSettings(conversationId, patch));
  if (c != null) store.upsertConversation(c);
  return c;
}

Future<GroupInfo?> loadGroup(String conversationId) async {
  try {
    final g = await backend.getGroupInfo(conversationId);
    store.upsertGroup(g);
    return g;
  } catch (e) {
    notifyError(e);
    return null;
  }
}

Future<void> setBlocked(String accountId, bool blocked) async {
  final c = await attempt(() => backend.setBlocked(accountId, blocked));
  if (c != null) store.upsertContact(c);
}

Future<void> reloadConversations() async {
  final list = await attempt(() => backend.listConversations());
  if (list != null) store.set((d) => d.copyWith(conversations: {for (final c in list) c.id: c}));
}

Future<void> refreshDetails() async {
  final details = await attempt(() => backend.getAccountDetails());
  if (details != null) store.set((d) => d.copyWith(details: details));
}

Future<void> reloadContacts() async {
  final list = await attempt(() => backend.listContacts());
  if (list != null) store.set((d) => d.copyWith(contacts: {for (final c in list) c.accountId: c}));
}

void toggleSelect(String conversationId, String messageId) {
  final sel = appData.selection;
  if (sel == null || sel.conversationId != conversationId) {
    store.set((d) => d.copyWith(selection: Selection(conversationId, [messageId])));
    return;
  }
  final ids = sel.ids.contains(messageId) ? sel.ids.where((x) => x != messageId).toList() : [...sel.ids, messageId];
  store.set((d) => d.copyWith(selection: ids.isEmpty ? null : Selection(conversationId, ids)));
}
