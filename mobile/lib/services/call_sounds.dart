// Ringtone (incoming call) and ringback (outgoing call), synthesized like the desktop app
// (app/src/lib/sounds.ts): no audio files to bundle. Each pattern is rendered once to a short WAV
// in the temporary directory and looped by just_audio.

import 'dart:io';
import 'dart:math' as math;
import 'dart:typed_data';

import 'package:just_audio/just_audio.dart';
import 'package:path_provider/path_provider.dart';

const _rate = 22050;

typedef _Note = ({double freq, double start, double duration, double volume});

/// Renders [notes] (bell-like sine + quiet octave, fast attack, exponential decay) into a mono
/// 16-bit WAV of [length] seconds.
Uint8List _render(List<_Note> notes, double length) {
  final n = (length * _rate).round();
  final pcm = Float64List(n);
  for (final note in notes) {
    final from = (note.start * _rate).round();
    final count = (note.duration * _rate).round();
    for (var i = 0; i < count && from + i < n; i++) {
      final t = i / _rate;
      final attack = math.min(1.0, t / 0.012);
      final env = attack * math.exp(-5.5 * t / note.duration);
      final w = 2 * math.pi * note.freq * t;
      pcm[from + i] += note.volume * env * (math.sin(w) + 0.18 * math.sin(2 * w));
    }
  }
  final data = ByteData(44 + n * 2);
  void str(int o, String s) {
    for (var i = 0; i < s.length; i++) {
      data.setUint8(o + i, s.codeUnitAt(i));
    }
  }

  str(0, 'RIFF');
  data.setUint32(4, 36 + n * 2, Endian.little);
  str(8, 'WAVE');
  str(12, 'fmt ');
  data.setUint32(16, 16, Endian.little);
  data.setUint16(20, 1, Endian.little); // PCM
  data.setUint16(22, 1, Endian.little); // mono
  data.setUint32(24, _rate, Endian.little);
  data.setUint32(28, _rate * 2, Endian.little);
  data.setUint16(32, 2, Endian.little);
  data.setUint16(34, 16, Endian.little);
  str(36, 'data');
  data.setUint32(40, n * 2, Endian.little);
  for (var i = 0; i < n; i++) {
    data.setInt16(44 + i * 2, (pcm[i].clamp(-1.0, 1.0) * 32767).round(), Endian.little);
  }
  return data.buffer.asUint8List();
}

/// Incoming call: a two-tone chime every 2 s.
Uint8List _ringtone() => _render([
      for (final o in [0.0, 0.32]) ...[
        (freq: 784.0, start: o, duration: 0.28, volume: 0.45), // G5
        (freq: 1046.5, start: o + 0.14, duration: 0.32, volume: 0.38), // C6
      ],
    ], 2.0);

/// Outgoing call while the other side rings: a soft tone every 3 s.
Uint8List _ringback() => _render([
      (freq: 440.0, start: 0, duration: 1.1, volume: 0.16),
      (freq: 480.0, start: 0, duration: 1.1, volume: 0.13),
    ], 3.0);

AudioPlayer? _player;

Future<String> _file(String name, Uint8List Function() build) async {
  final f = File('${(await getTemporaryDirectory()).path}/$name');
  if (!await f.exists()) await f.writeAsBytes(build(), flush: true);
  return f.path;
}

Future<void Function()> _loop(String name, Uint8List Function() build) async {
  stopCallSound();
  final player = AudioPlayer();
  _player = player;
  try {
    await player.setFilePath(await _file(name, build));
    await player.setLoopMode(LoopMode.one);
    // play() completes only when playback stops: not awaited.
    if (_player == player) player.play().ignore();
  } catch (_) {
    // No sound is not a reason to fail the call.
  }
  return () {
    if (_player == player) stopCallSound();
  };
}

Future<void Function()> startRingtone() => _loop('dialog-ringtone-v1.wav', _ringtone);

Future<void Function()> startRingback() => _loop('dialog-ringback-v1.wav', _ringback);

void stopCallSound() {
  final p = _player;
  _player = null;
  if (p != null) {
    p.stop().whenComplete(p.dispose).ignore();
  }
}

// --- message tones (same notes as playIncoming / playInChat in app/src/lib/sounds.ts) ----------

/// New message in a chat that is not open: A5 then E6.
Uint8List _incoming() => _render([
      (freq: 880.0, start: 0, duration: 0.32, volume: 0.4),
      (freq: 1318.5, start: 0.11, duration: 0.42, volume: 0.32),
    ], 0.6);

/// New message in the chat that is open: one short, quieter B5.
Uint8List _inChat() => _render([(freq: 987.8, start: 0, duration: 0.16, volume: 0.18)], 0.25);

final _tonePlayers = <String, AudioPlayer>{};
DateTime _lastTone = DateTime.fromMillisecondsSinceEpoch(0);

/// Plays a message tone while the app is in the foreground (in the background the system
/// notification makes the sound). Bursts of messages produce a single tone. The player does not
/// take over the audio session, so the tone follows the silent switch and never pauses music or
/// a call.
Future<void> playMessageTone({required bool inChat}) async {
  final now = DateTime.now();
  if (now.difference(_lastTone) < const Duration(milliseconds: 900)) return;
  _lastTone = now;
  // A call has the audio: no tones over it.
  if (_player != null) return;
  final name = inChat ? 'dialog-inchat-v1.wav' : 'dialog-incoming-v1.wav';
  try {
    var player = _tonePlayers[name];
    if (player == null) {
      player = AudioPlayer(handleAudioSessionActivation: false);
      await player.setFilePath(await _file(name, inChat ? _inChat : _incoming));
      _tonePlayers[name] = player;
    }
    await player.seek(Duration.zero);
    player.play().ignore();
  } catch (_) {
    // No sound is not a reason to fail anything.
  }
}
