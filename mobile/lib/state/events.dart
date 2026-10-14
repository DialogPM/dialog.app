// Subscribes to core events and keeps the store in sync; also drives the local notifications for
// new incoming messages (port of app/src/store/events.ts).

import 'dart:async';

import 'package:flutter/services.dart';

import '../app/globals.dart';
import '../l10n/i18n.dart';
import '../services/call_sounds.dart' show playMessageTone;
import '../services/notifications.dart';
import '../util/format.dart';
import 'actions.dart';
import 'call_settings.dart';
import 'default_timer.dart';
import 'media_retention.dart';
import 'calls.dart';
import 'dnd.dart';
import 'group_calls.dart';

final _typingTimers = <String, Timer>{};
final _subs = <StreamSubscription<dynamic>>[];

/// True while the app is in the foreground.
bool appInForeground = true;

void startEventBridge() {
  _subs.addAll([
    backend.onConversationUpdated.listen(store.upsertConversation),
    backend.onConversationRemoved.listen(store.removeConversation),
    backend.onMessage.listen((ev) {
      store.upsertMessage(ev.message);
      if (!ev.isNewIncoming) return;
      final s = appData;
      final p = prefs;
      final conv = s.conversations[ev.conversationId];
      // Muted chats still notify when we are @mentioned (like WhatsApp). Do not disturb silences
      // everything (the message still arrives and counts as unread).
      final muted = dndActive() || (conv != null && conv.isMuted() && !ev.message.mentionsMe);
      final active = s.activeId == ev.conversationId;
      if (active && appInForeground) {
        unawaited(backend.markRead(ev.conversationId).catchError((_) {}));
        if (!muted && p.soundInChat) {
          unawaited(HapticFeedback.lightImpact());
          if (callState.value == null) unawaited(playMessageTone(inChat: true));
        }
        return;
      }
      if (muted) return;
      // In the foreground the app plays the tone itself (iOS does not sound notifications of
      // the app in use; Android would, so the notification is posted silently there).
      if (appInForeground && p.sound && callState.value == null) unawaited(playMessageTone(inChat: false));
      if (!p.notifications) return;
      final m = ev.message;
      final lang = currentLanguage;
      // "Notification preview" off: never put message content in the notification.
      final body = !p.notifPreview
          ? tr('notifications.hidden')
          : m.deleted
              ? tr('message.deleted')
              : m.kind == 'system' && m.system != null
                  ? systemText(m.system!, lang)
                  : previewText(m.kind, m.text, lang);
      final title = !p.notifPreview
          ? kBrandName
          : conv?.isGroup == true
              ? '${m.senderName} @ ${conv!.title}'
              : (conv?.title ?? m.senderName);
      unawaited(Notifications.instance.showMessage(
        conversationId: ev.conversationId,
        title: title,
        body: body,
        sound: p.sound && !appInForeground,
      ));
    }),
    backend.onMessageDeleted.listen((p) => store.removeMessage(p.conversationId, p.messageId)),
    backend.onTyping.listen((p) {
      final key = '${p.conversationId}/${p.accountId}';
      _typingTimers.remove(key)?.cancel();
      store.setTyping(p.conversationId, p.accountId, p.senderName, p.typing);
      if (p.typing) {
        // Safety net in case the "stopped typing" event never arrives.
        _typingTimers[key] = Timer(const Duration(seconds: 8), () => store.setTyping(p.conversationId, p.accountId, '', false));
      }
    }),
    backend.onPresence.listen(store.setPresence),
    backend.onConnection.listen((c) => store.set((d) => d.copyWith(connection: c))),
    backend.onContactUpdated.listen(store.upsertContact),
    backend.onProfileUpdated.listen((p) => store.set((d) => d.copyWith(profile: p))),
    backend.onDndChanged.listen(applyDnd),
    backend.onCallSettingsChanged.listen(applyCallSettings),
    backend.onMediaRetentionChanged.listen(applyMediaRetention),
    backend.onMediaCheck.listen(applyMediaCheck),
    backend.onDefaultDisappearingChanged.listen(applyDefaultTimer),
    backend.onDevicesChanged.listen((_) => store.set((d) => d.copyWith(devicesVersion: d.devicesVersion + 1))),
    backend.onPinsChanged.listen((_) => store.set((d) => d.copyWith(pinsVersion: d.pinsVersion + 1))),
    backend.onGroupUpdated.listen(store.upsertGroup),
    backend.onHistorySync.listen((p) {
      store.set((d) => d.copyWith(historySync: p));
      if (p.done) {
        unawaited(reloadConversations());
        Timer(const Duration(milliseconds: 2500), () => store.set((d) => d.copyWith(historySync: null)));
      }
    }),
    backend.onConversationsReset.listen((_) {
      store.set((d) => d.copyWith(messages: const {}, convMessages: const {}, activeId: null));
      unawaited(reloadConversations());
    }),
    backend.onCallSignal.listen((ev) => unawaited(handleCallSignal(ev))),
    backend.onGroupCall.listen((ev) => unawaited(handleGroupCall(ev))),
    backend.onGroupCallSignal.listen((ev) => unawaited(handleGroupCallSignal(ev))),
    backend.onLoggedOut.listen((p) {
      unawaited(hangUp());
      unawaited(Notifications.instance.cancelAll());
      signedOut(p.reason);
    }),
  ]);
}

void stopEventBridge() {
  for (final s in _subs) {
    unawaited(s.cancel());
  }
  _subs.clear();
}
