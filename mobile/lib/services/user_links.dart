// Username links: https://dialog.pm/@name (and dialog://u/name, which the web page of such a
// link uses to open the installed app) lead to the conversation with that name.
//
// The link names an account of the official server only, so it is honoured (and offered in the
// settings) only while signed in to that server: elsewhere the same name is someone else.
// A link comes from outside the app (a web page, a message, another app), so nothing happens
// without a question: the app looks the name up, shows who it found and starts the
// conversation only when the user agrees. It waits until the app is unlocked and signed in.

import 'dart:async';

import 'package:flutter/services.dart';

import '../app/globals.dart';
import '../app/nav.dart';
import '../core/models.dart';
import '../l10n/i18n.dart';
import '../state/actions.dart';
import '../widgets/dialogs.dart';
import '../widgets/toast.dart';

/// Host of the username links, and the server whose accounts they name.
const String kUserLinkHost = 'dialog.pm';
const String kUserLinkServer = 'https://chat.dialog.pm';

final _nameRe = RegExp(r'^[a-z][a-z0-9_.]{2,31}$');

/// The username a link leads to, or null when it is not a username link.
String? usernameOfLink(String link) {
  final Uri uri;
  try {
    uri = Uri.parse(link.trim());
  } catch (_) {
    return null;
  }
  String? raw;
  if (uri.scheme == 'https' && uri.host.toLowerCase() == kUserLinkHost) {
    final segments = uri.pathSegments.where((s) => s.isNotEmpty).toList();
    if (segments.length == 1 && segments.first.startsWith('@')) raw = segments.first.substring(1);
  } else if (uri.scheme == 'dialog' && uri.host == 'u') {
    final segments = uri.pathSegments.where((s) => s.isNotEmpty).toList();
    if (segments.length == 1) raw = segments.first;
  }
  if (raw == null) return null;
  final name = raw.toLowerCase();
  return _nameRe.hasMatch(name) && !name.contains('..') ? name : null;
}

/// The web link of [username].
String userLink(String username) => 'https://$kUserLinkHost/@$username';

/// Whether the signed-in account is on the server the links are for.
bool get userLinksAvailable {
  final url = appData.appState?.account?.serverUrl ?? '';
  return url.replaceAll(RegExp(r'/+$'), '').toLowerCase() == kUserLinkServer;
}

class UserLinks {
  static const _channel = MethodChannel('dialog/links');
  String? _pending;
  bool _home = false;
  bool _asking = false;

  /// Starts listening for links opened while the app runs and takes the one that started it.
  Future<void> start() async {
    _channel.setMethodCallHandler((call) async {
      if (call.method == 'link' && call.arguments is String) _take(call.arguments as String);
    });
    try {
      final first = await _channel.invokeMethod<String>('initial');
      if (first != null) _take(first);
    } on MissingPluginException {
      // A platform without links (iOS for now, desktop Linux builds).
    } catch (_) {}
  }

  void _take(String link) {
    final name = usernameOfLink(link);
    if (name == null) return;
    _pending = name;
    unawaited(drain());
  }

  /// The main screen is (no longer) showing: links are handled only there.
  void home(bool showing) {
    _home = showing;
    if (showing) unawaited(drain());
  }

  /// Handles the waiting link, if the app is signed in, unlocked and on its main screen.
  Future<void> drain() async {
    final name = _pending;
    if (name == null || _asking || !_home || appData.locked || appData.appState?.ready != true) return;
    final context = navigatorKey.currentContext;
    if (context == null) return;
    _pending = null;
    _asking = true;
    try {
      await _open(name);
    } finally {
      _asking = false;
    }
  }

  Future<void> _open(String name) async {
    if (!userLinksAvailable) {
      showToast(tr('userLink.otherServer', args: {'server': Uri.parse(kUserLinkServer).host}), error: true);
      return;
    }
    final FoundAccount? found;
    try {
      found = await backend.findAccount('@$name');
    } catch (e) {
      notifyError(e);
      return;
    }
    if (found == null) {
      showToast(tr('userLink.notFound', args: {'name': '@$name'}), error: true);
      return;
    }
    if (found.isMe) {
      showToast(tr('userLink.self'));
      return;
    }
    final context = navigatorKey.currentContext;
    if (context == null || !context.mounted) return;
    final who = found.displayName.isEmpty || found.displayName == '@$name' ? '@$name' : '${found.displayName} (@$name)';
    final ok = await showConfirm(
      context,
      title: tr('userLink.openTitle'),
      body: tr('userLink.openBody', args: {'name': who}),
      confirmLabel: tr('userLink.open'),
      danger: false,
    );
    if (!ok) return;
    try {
      Conversation c;
      try {
        c = await backend.openDirectConversation(found.accountId);
      } catch (_) {
        // Not a contact yet: add it from the directory first.
        final contact = await backend.addContactByQuery('@$name');
        store.upsertContact(contact);
        c = await backend.openDirectConversation(contact.accountId);
      }
      store.upsertConversation(c);
      openChat(c.id);
    } catch (e) {
      notifyError(e);
    }
  }
}

final userLinks = UserLinks();
