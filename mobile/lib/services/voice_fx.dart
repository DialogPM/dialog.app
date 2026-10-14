// Live voice effects and AI noise reduction of the call microphone, like the desktop app
// (app/src/lib/voiceFx.ts): processed natively by crates/voice-fx inside WebRTC's capture
// pipeline (Android VoiceFx.kt, iOS AppDelegate.swift), so the other side only ever receives
// the processed voice. "none" without noise reduction leaves the microphone untouched.

import 'dart:math';

import 'package:flutter/services.dart';

/// Effect names, in the order of the effect list; the index is voice-fx's effect number.
const kVoiceEffects = ['none', 'anonymous', 'deep', 'chipmunk', 'robot', 'hall'];

const kVoiceEffectEmoji = {
  'none': '🎙️',
  'anonymous': '🕶️',
  'deep': '🐻',
  'chipmunk': '🐿️',
  'robot': '🤖',
  'hall': '🎤',
};

class VoiceFxNative {
  static const _channel = MethodChannel('dialog/voicefx');

  /// Applies an effect and/or the noise reduction from the next audio block on. False when the
  /// native hook is not available (then the call goes on with the plain microphone).
  static Future<bool> configure(String effect, bool denoise) async {
    try {
      final ok = await _channel.invokeMethod<bool>('configure', {'effect': max(0, kVoiceEffects.indexOf(effect)), 'denoise': denoise});
      return ok ?? false;
    } catch (_) {
      return false;
    }
  }

  /// A new call: a fresh processor and a new random anonymous voice.
  static Future<void> newCall() async {
    try {
      await _channel.invokeMethod<void>('newCall', {'seed': Random.secure().nextInt(0x7fffffff) + 1});
    } catch (_) {}
  }
}
