// Keeps the system in step with the call in progress (1:1 or group), whatever started it:
//
// - Android: the foreground service of the call (services/ongoing_call.dart) runs from the
//   moment the call is placed or answered until it ends (started while the app is in the
//   foreground: Android refuses the microphone type to a service started in the background), so the microphone / camera keep working and the
//   process is not frozen with the screen locked; the proximity wake lock is held for voice
//   calls on the earpiece.
// - iOS: CallKit (services/callkit.dart) is driven from calls.dart / group_calls.dart directly,
//   where the events happen.

import 'dart:async';

import '../l10n/i18n.dart';
import '../services/ongoing_call.dart';
import 'calls.dart';
import 'group_calls.dart';

({bool active, bool video, bool proximity})? _applied;

/// Starts following [callState] and [groupCallState] (main.dart).
void initCallSystem() {
  if (!OngoingCall.supported) return;
  OngoingCall.onHangUp = () {
    if (callState.value != null) {
      unawaited(hangUp());
    } else if (groupCallState.value != null) {
      unawaited(leaveGroupCall());
    }
  };
  callState.addListener(_sync);
  groupCallState.addListener(_sync);
}

void _sync() {
  final call = callState.value;
  final group = groupCallState.value;
  var active = false;
  var video = false;
  var speaker = true;
  if (call != null && call.phase != CallPhase.incoming) {
    active = true;
    video = call.video;
    speaker = call.speaker;
  } else if (group != null && (group.phase == GroupCallPhase.joining || group.phase == GroupCallPhase.active)) {
    active = true;
    video = group.video;
    speaker = group.speaker;
  }
  final next = (active: active, video: video, proximity: active && !video && !speaker);
  final prev = _applied;
  if (prev == next) return;
  _applied = next;
  if (active && (prev == null || !prev.active || prev.video != video)) {
    unawaited(OngoingCall.start(
      title: tr(video ? 'mobile.callOngoingVideo' : 'mobile.callOngoing'),
      text: tr('mobile.callOngoingText'),
      hangUp: tr('mobile.callHangUp'),
      channelName: tr('mobile.callChannel'),
      video: video,
    ));
  }
  if ((prev?.proximity ?? false) != next.proximity) unawaited(OngoingCall.setProximity(next.proximity));
  if (!active && prev?.active == true) unawaited(OngoingCall.stop());
}
