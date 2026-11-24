// Notices from the operator of our server (maintenance, required updates, a removed username).
// The core checks the operator's signature and keeps them (list_notices, mark_notices_read,
// delete_notice); this file mirrors the list and the unread count for the bell on the chat list,
// the settings row and the notices page. An older core does not know the commands: then
// [NoticesState.supported] stays false and the app shows no notices at all, without errors.

import 'dart:async';

import 'package:flutter/foundation.dart';

import '../app/globals.dart';
import '../brand.g.dart';
import '../core/models.dart';

@immutable
class NoticesState {
  /// The core answered list_notices (false: older core, nothing is shown).
  final bool supported;

  /// Newest first.
  final List<Notice> notices;
  final int unread;

  const NoticesState({this.supported = false, this.notices = const [], this.unread = 0});

  static const none = NoticesState();

  factory NoticesState.loaded(NoticeList l) => NoticesState(supported: true, notices: _sorted(l.notices), unread: l.unread < 0 ? 0 : l.unread);

  static List<Notice> _sorted(Iterable<Notice> list) => list.toList()
    ..sort((a, b) {
      final c = b.createdAt.compareTo(a.createdAt);
      return c != 0 ? c : b.id.compareTo(a.id);
    });

  /// A notice just arrived (or arrived again: replaced, not counted twice). The core follows
  /// with notices_changed carrying the authoritative count.
  NoticesState withNotice(Notice n) {
    final old = notices.where((x) => x.id == n.id).firstOrNull;
    var u = unread;
    if (old == null && !n.read) u++;
    if (old != null && !old.read && n.read) u--;
    return NoticesState(supported: true, notices: _sorted([...notices.where((x) => x.id != n.id), n]), unread: u < 0 ? 0 : u);
  }

  NoticesState withUnread(int count) => NoticesState(supported: true, notices: notices, unread: count < 0 ? 0 : count);

  NoticesState markedRead() => NoticesState(supported: supported, notices: [for (final n in notices) n.read ? n : n.markedRead()], unread: 0);

  NoticesState without(int id) {
    final old = notices.where((x) => x.id == id).firstOrNull;
    if (old == null) return this;
    final u = old.read ? unread : unread - 1;
    return NoticesState(supported: supported, notices: notices.where((x) => x.id != id).toList(), unread: u < 0 ? 0 : u);
  }
}

/// Name of the server the account is on, for the notice notification and the notices page.
String noticeServerName() {
  final s = appData;
  final name = s.serverInfo?.name.trim() ?? '';
  if (name.isNotEmpty) return name;
  final url = s.details?.account.serverUrl ?? s.appState?.account?.serverUrl ?? '';
  final host = Uri.tryParse(url)?.host ?? '';
  return host.isNotEmpty ? host : kBrandName;
}

final noticesState = ValueNotifier<NoticesState>(NoticesState.none);

/// True while the notices page is on screen: a notice arriving then is read at once.
bool noticesPageOpen = false;

Future<void> loadNotices() async {
  try {
    noticesState.value = NoticesState.loaded(await backend.listNotices());
  } catch (_) {
    // Older core (unknown command) or no session: no notices, no bell, no error.
    noticesState.value = NoticesState.none;
  }
}

void resetNotices() => noticesState.value = NoticesState.none;

void applyNotice(Notice n) => noticesState.value = noticesState.value.withNotice(n);

void applyNoticesUnread(int unread) => noticesState.value = noticesState.value.withUnread(unread);

/// Opening the page reads everything on it.
Future<void> markNoticesRead() async {
  final s = noticesState.value;
  if (!s.supported || (s.unread == 0 && s.notices.every((n) => n.read))) return;
  noticesState.value = s.markedRead();
  try {
    final left = await backend.markNoticesRead();
    if (left != noticesState.value.unread) applyNoticesUnread(left);
  } catch (_) {
    // Stays read here; the next list_notices brings the core's view back.
  }
}

/// Removes one notice (optimistically; the list is reloaded if the core refuses).
Future<void> deleteNotice(int id) async {
  noticesState.value = noticesState.value.without(id);
  try {
    await backend.deleteNotice(id);
  } catch (_) {
    unawaited(loadNotices());
    rethrow;
  }
}
