// Normalized application state, kept up to date by core events (see events.dart). Mirrors
// app/src/store/app.ts of the desktop app.

import 'package:flutter_riverpod/flutter_riverpod.dart';

import '../core/models.dart';

/// "all" | "unread" | "groups" | "archived"
typedef ChatFilter = String;

class ConvMessages {
  /// Message ids sorted oldest -> newest.
  final List<String> ids;
  final bool hasMore;
  final bool loading;
  final bool loaded;

  const ConvMessages({this.ids = const [], this.hasMore = true, this.loading = false, this.loaded = false});

  ConvMessages copyWith({List<String>? ids, bool? hasMore, bool? loading, bool? loaded}) => ConvMessages(
        ids: ids ?? this.ids,
        hasMore: hasMore ?? this.hasMore,
        loading: loading ?? this.loading,
        loaded: loaded ?? this.loaded,
      );
}

class TypingEntry {
  final String name;
  final int at;
  const TypingEntry(this.name, this.at);
}

class Composing {
  final Message? replyTo;
  final Message? editing;
  const Composing({this.replyTo, this.editing});
}

class Selection {
  final String conversationId;
  final List<String> ids;
  const Selection(this.conversationId, this.ids);
}

class AppData {
  const AppData({
    this.appState,
    this.profile,
    this.details,
    this.serverInfo,
    this.connection = 'connecting',
    this.historySync,
    this.loggedOutReason,
    this.conversations = const {},
    this.messages = const {},
    this.convMessages = const {},
    this.contacts = const {},
    this.groups = const {},
    this.typing = const {},
    this.presence = const {},
    this.activeId,
    this.chatFilter = 'all',
    this.composing = const {},
    this.selection,
    this.highlightId,
    this.devicesVersion = 0,
    this.pinsVersion = 0,
    this.locked = false,
  });

  // session
  final AppState? appState;
  final Profile? profile;
  final AccountDetails? details;
  final ServerInfo? serverInfo;
  final String connection;
  final HistorySyncEvent? historySync;
  final String? loggedOutReason;

  // data
  final Map<String, Conversation> conversations;
  final Map<String, Message> messages;
  final Map<String, ConvMessages> convMessages;
  final Map<String, Contact> contacts;
  final Map<String, GroupInfo> groups;
  final Map<String, Map<String, TypingEntry>> typing;
  final Map<String, Presence> presence;

  // ui
  /// Conversation currently shown on screen.
  final String? activeId;
  final ChatFilter chatFilter;
  final Map<String, Composing> composing;
  final Selection? selection;
  final String? highlightId;

  /// Bumped by the devices_changed event.
  final int devicesVersion;

  /// Bumped when pinned messages change (pins_changed).
  final int pinsVersion;

  /// App lock screen shown.
  final bool locked;

  static const _unset = Object();

  AppData copyWith({
    Object? appState = _unset,
    Object? profile = _unset,
    Object? details = _unset,
    Object? serverInfo = _unset,
    String? connection,
    Object? historySync = _unset,
    Object? loggedOutReason = _unset,
    Map<String, Conversation>? conversations,
    Map<String, Message>? messages,
    Map<String, ConvMessages>? convMessages,
    Map<String, Contact>? contacts,
    Map<String, GroupInfo>? groups,
    Map<String, Map<String, TypingEntry>>? typing,
    Map<String, Presence>? presence,
    Object? activeId = _unset,
    ChatFilter? chatFilter,
    Map<String, Composing>? composing,
    Object? selection = _unset,
    Object? highlightId = _unset,
    int? devicesVersion,
    int? pinsVersion,
    bool? locked,
  }) =>
      AppData(
        appState: appState == _unset ? this.appState : appState as AppState?,
        profile: profile == _unset ? this.profile : profile as Profile?,
        details: details == _unset ? this.details : details as AccountDetails?,
        serverInfo: serverInfo == _unset ? this.serverInfo : serverInfo as ServerInfo?,
        connection: connection ?? this.connection,
        historySync: historySync == _unset ? this.historySync : historySync as HistorySyncEvent?,
        loggedOutReason: loggedOutReason == _unset ? this.loggedOutReason : loggedOutReason as String?,
        conversations: conversations ?? this.conversations,
        messages: messages ?? this.messages,
        convMessages: convMessages ?? this.convMessages,
        contacts: contacts ?? this.contacts,
        groups: groups ?? this.groups,
        typing: typing ?? this.typing,
        presence: presence ?? this.presence,
        activeId: activeId == _unset ? this.activeId : activeId as String?,
        chatFilter: chatFilter ?? this.chatFilter,
        composing: composing ?? this.composing,
        selection: selection == _unset ? this.selection : selection as Selection?,
        highlightId: highlightId == _unset ? this.highlightId : highlightId as String?,
        devicesVersion: devicesVersion ?? this.devicesVersion,
        pinsVersion: pinsVersion ?? this.pinsVersion,
        locked: locked ?? this.locked,
      );

  /// Number of (not archived) conversations with unread messages.
  int get unreadChats => conversations.values.where((c) => !c.archived && c.unreadCount > 0).length;
}

class AppStore extends Notifier<AppData> {
  @override
  AppData build() => const AppData();

  AppData get data => state;
  void set(AppData Function(AppData s) fn) => state = fn(state);

  void upsertConversation(Conversation c) => state = state.copyWith(conversations: {...state.conversations, c.id: c});

  void removeConversation(String id) {
    final conversations = {...state.conversations}..remove(id);
    final convMessages = {...state.convMessages}..remove(id);
    state = state.copyWith(conversations: conversations, convMessages: convMessages);
  }

  static List<String> _insertSorted(List<String> ids, Message m, Map<String, Message> all) {
    if (ids.contains(m.id)) return ids;
    final out = [...ids];
    var i = out.length;
    // Arrival order (receivedAt), like the core's list: sentAt is the sender's clock.
    while (i > 0 && (all[out[i - 1]]?.receivedAt ?? 0) > m.receivedAt) {
      i--;
    }
    out.insert(i, m.id);
    return out;
  }

  void upsertMessage(Message m) {
    final messages = {...state.messages, m.id: m};
    final cm = state.convMessages[m.conversationId];
    if (cm == null || !cm.loaded) {
      state = state.copyWith(messages: messages);
      return;
    }
    state = state.copyWith(
      messages: messages,
      convMessages: {...state.convMessages, m.conversationId: cm.copyWith(ids: _insertSorted(cm.ids, m, messages))},
    );
  }

  void removeMessage(String conversationId, String messageId) {
    final messages = {...state.messages}..remove(messageId);
    final cm = state.convMessages[conversationId];
    state = state.copyWith(
      messages: messages,
      convMessages: cm == null
          ? state.convMessages
          : {...state.convMessages, conversationId: cm.copyWith(ids: cm.ids.where((x) => x != messageId).toList())},
    );
  }

  /// Adds a page of messages (any order) to a conversation.
  void addMessagePage(String conversationId, List<Message> page, bool hasMore, {bool reset = false}) {
    final messages = {...state.messages};
    for (final m in page) {
      messages[m.id] = m;
    }
    final prev = reset ? const <String>[] : (state.convMessages[conversationId]?.ids ?? const <String>[]);
    final set = <String>{...prev, ...page.map((m) => m.id)};
    final ids = set.toList()..sort((a, b) => (messages[a]?.receivedAt ?? 0).compareTo(messages[b]?.receivedAt ?? 0));
    state = state.copyWith(
      messages: messages,
      convMessages: {
        ...state.convMessages,
        conversationId: ConvMessages(ids: ids, hasMore: hasMore, loading: false, loaded: true),
      },
    );
  }

  void setConvLoading(String conversationId, bool loading) {
    final cm = state.convMessages[conversationId] ?? const ConvMessages();
    state = state.copyWith(convMessages: {...state.convMessages, conversationId: cm.copyWith(loading: loading)});
  }

  void clearConvMessages(String conversationId) => state = state.copyWith(convMessages: {
        ...state.convMessages,
        conversationId: const ConvMessages(ids: [], hasMore: false, loaded: true),
      });

  void upsertContact(Contact c) => state = state.copyWith(contacts: {...state.contacts, c.accountId: c});

  void upsertGroup(GroupInfo g) => state = state.copyWith(groups: {...state.groups, g.conversationId: g});

  void setTyping(String conversationId, String accountId, String name, bool typing) {
    final cur = {...(state.typing[conversationId] ?? const <String, TypingEntry>{})};
    if (typing) {
      cur[accountId] = TypingEntry(name, DateTime.now().millisecondsSinceEpoch);
    } else {
      cur.remove(accountId);
    }
    state = state.copyWith(typing: {...state.typing, conversationId: cur});
  }

  void setPresence(Presence p) => state = state.copyWith(presence: {...state.presence, p.accountId: p});

  void setComposing(String conversationId, {Object? replyTo = AppData._unset, Object? editing = AppData._unset}) {
    final cur = state.composing[conversationId] ?? const Composing();
    final next = Composing(
      replyTo: replyTo == AppData._unset ? cur.replyTo : replyTo as Message?,
      editing: editing == AppData._unset ? cur.editing : editing as Message?,
    );
    state = state.copyWith(composing: {...state.composing, conversationId: next});
  }

  /// Clears everything of the signed-in session (keeps the app state).
  void resetSession() => state = AppData(appState: state.appState, loggedOutReason: state.loggedOutReason);
}

final storeProvider = NotifierProvider<AppStore, AppData>(AppStore.new);
