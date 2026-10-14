// Media retention (port of the desktop's lib/mediaRetention.ts): photos, videos, voice messages
// and files are deleted from every device after 1, 7, 14 or 30 days (30 is the default and the
// longest allowed). The period lives in the core (engine/retention.rs) and is synced to our
// other devices; mediaRetention mirrors it with the last media check. The core sweeps its own
// folders; plaintext copies this app writes (decrypted copies, picked files, voice drafts) are
// swept here: at session start, when the app comes back, hourly and on "Check now".

import 'dart:async';

import 'package:flutter/foundation.dart';

import '../app/globals.dart';
import '../core/backend.dart';
import '../services/media.dart';

final mediaRetention = ValueNotifier<MediaRetention>(MediaRetention.defaults);

Timer? _timer;

void applyMediaRetention(MediaRetention r) => mediaRetention.value = r;

void applyMediaCheck(MediaCheck c) => mediaRetention.value = mediaRetention.value.withCheck(c);

Duration get _period => Duration(days: mediaRetention.value.days.clamp(1, mediaRetention.value.maxDays));

Future<void> loadMediaRetention() async {
  try {
    applyMediaRetention(await backend.getMediaRetention());
  } catch (_) {
    // Older core: the longest period.
    applyMediaRetention(MediaRetention.defaults);
  }
}

/// One of 1, 7, 14, 30; older media is erased at once.
Future<void> setMediaRetention(int days) async {
  applyMediaRetention(await backend.setMediaRetention(days));
  unawaited(sweepAppMedia());
}

/// The app's own sweep (see services/media.dart); returns the number of files removed.
Future<int> sweepAppMedia() async {
  try {
    return await sweepTempMedia(_period);
  } catch (e) {
    debugPrint('media sweep: $e');
    return 0;
  }
}

/// "Check now": the core's check and the app's sweep; shows their total.
Future<MediaCheck> runMediaCheckNow() async {
  final ours = await sweepAppMedia();
  final c = await backend.runMediaCheck();
  final total = MediaCheck(removed: c.removed + ours, checkedAt: c.checkedAt);
  applyMediaCheck(total);
  return total;
}

/// Session start: loads the period, sweeps now and then hourly.
void startMediaChecks() {
  unawaited(loadMediaRetention().then((_) => sweepAppMedia()));
  _timer?.cancel();
  _timer = Timer.periodic(const Duration(hours: 1), (_) => unawaited(sweepAppMedia()));
}
