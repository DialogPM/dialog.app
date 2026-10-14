// Do not disturb (port of app/src/lib/dnd.ts): incoming calls do not ring (the core turns them
// into missed calls) and new messages make no sound and show no notification. The setting lives
// in the core and is synced to our other devices; prefs.dndUntil mirrors it (also for the
// background push handler) and this file ends it on time.

import 'dart:async';

import 'package:flutter/material.dart';
import 'package:intl/intl.dart';

import '../app/globals.dart';
import '../core/backend.dart';
import '../l10n/i18n.dart';
import '../util/format.dart';
import '../widgets/dialogs.dart';
import '../widgets/toast.dart';
import 'actions.dart';

const dndChoices = ['1h', '8h', 'morning', 'forever'];

const _hour = 3600000;

/// Ends at or beyond this (ms) mean "until I turn it off".
const _foreverFrom = 1000000000000000;

Timer? _expiry;

/// Mirrors the core's state (`until` 0 = off) and schedules the automatic end.
void applyDnd(int until) {
  _expiry?.cancel();
  final now = DateTime.now().millisecondsSinceEpoch;
  final v = until > now ? until : 0;
  if (prefs.dndUntil != v) prefsNotifier.update((p) => p.copyWith(dndUntil: v));
  if (v != 0 && !isDndForever(v)) {
    _expiry = Timer(Duration(milliseconds: v - now + 50), () => applyDnd(prefs.dndUntil));
  }
}

bool dndActive() => prefs.dndUntil > DateTime.now().millisecondsSinceEpoch;

bool isDndForever(int until) => until >= _foreverFrom;

/// End time of a choice; "morning" is the next 08:00.
int dndUntilFor(String choice, [DateTime? at]) {
  final now = at ?? DateTime.now();
  switch (choice) {
    case '1h':
      return now.millisecondsSinceEpoch + _hour;
    case '8h':
      return now.millisecondsSinceEpoch + 8 * _hour;
    case 'morning':
      var d = DateTime(now.year, now.month, now.day, 8);
      if (!d.isAfter(now)) d = DateTime(now.year, now.month, now.day + 1, 8);
      return d.millisecondsSinceEpoch;
    default:
      return kDndForever;
  }
}

Future<void> loadDnd() async {
  try {
    applyDnd(await backend.getDnd());
  } catch (_) {
    // Older core: stays off.
  }
}

Future<void> setDnd(int until) async => applyDnd(await backend.setDnd(until));

/// "Until 21:30", "Until Tue 08:00" or "Until you turn it off".
String dndUntilText(int until, String lang) {
  if (isDndForever(until)) return translate(lang, 'dnd.untilOff');
  final d = DateTime.fromMillisecondsSinceEpoch(until);
  final now = DateTime.now();
  final sameDay = d.year == now.year && d.month == now.month && d.day == now.day;
  final time = formatTime(until, lang);
  final when = sameDay ? time : '${DateFormat.E(lang == 'en' ? 'en_US' : lang).format(d)} $time';
  return translate(lang, 'dnd.untilTime', args: {'time': when});
}

/// Picks how long do not disturb stays on, then turns it on.
Future<void> openDndDialog(BuildContext context) async {
  final v = await showOptions(
    context,
    title: context.t('dnd.title'),
    description: context.t('dnd.description'),
    initial: '1h',
    options: [for (final c in dndChoices) (c, context.t('dnd.choice.$c'))],
  );
  if (v == null) return;
  await attempt(() async {
    await setDnd(dndUntilFor(v));
    showToast(tr('dnd.enabled'));
  });
}

/// Quick toggle: off when on, otherwise asks for how long.
Future<void> toggleDnd(BuildContext context) async {
  if (!dndActive()) return openDndDialog(context);
  await attempt(() async {
    await setDnd(0);
    showToast(tr('dnd.disabled'));
  });
}
