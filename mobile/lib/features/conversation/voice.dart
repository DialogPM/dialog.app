// Voice notes: recorder (record package, opus / aac) with a live waveform and a "1" toggle
// for view-once voice messages (with "anonymized voice" on, the recording is WAV and masked by
// the core before it is sent; if masking fails nothing is sent, never the original voice), and the player (just_audio) with the waveform, seeking and
// playback speed. View-once voice messages are played by view_once_viewer.dart instead.

import 'dart:async';
import 'dart:convert';
import 'dart:io';
import 'dart:math' as math;

import 'package:flutter/material.dart';
import 'package:just_audio/just_audio.dart';
import 'package:record/record.dart';

import '../../app/globals.dart';
import '../../services/media.dart';
import '../../core/models.dart';
import '../../l10n/i18n.dart';
import '../../state/actions.dart';
import '../../theme/app_theme.dart';
import '../../util/format.dart';
import '../../widgets/app_icon.dart';
import '../../widgets/toast.dart';
import '../../widgets/ui.dart';
import 'attachments.dart';
import 'view_once_bubble.dart' show ViewOnceToggle;

const _wavePoints = 48;
const _bars = 34;
const _maxDuration = Duration(minutes: 15);

/// Reduces recorded levels to [n] normalized peaks (0.05..1).
List<double> downsampleLevels(List<double> levels, int n) {
  if (levels.isEmpty) return List.filled(n, 0.1);
  final out = <double>[];
  for (var i = 0; i < n; i++) {
    final a = (i / n * levels.length).floor();
    final b = math.max(a + 1, ((i + 1) / n * levels.length).floor());
    var peak = 0.0;
    for (var k = a; k < b && k < levels.length; k++) {
      peak = math.max(peak, levels[k]);
    }
    out.add(peak);
  }
  final max = out.fold<double>(0.01, math.max);
  return out.map((v) => (v / max).clamp(0.05, 1.0)).toList();
}

/// dBFS (-160..0) to a 0..1 level.
double levelFromDb(double db) {
  if (db.isNaN || db.isInfinite) return 0;
  final v = (db + 50) / 50;
  return v.clamp(0.0, 1.0);
}

class VoiceRecorder extends StatefulWidget {
  const VoiceRecorder({super.key, required this.onCancel, required this.onSend});
  final VoidCallback onCancel;

  /// The recording, and whether it goes view-once.
  final void Function(OutgoingAttachment att, bool viewOnce) onSend;

  @override
  State<VoiceRecorder> createState() => _VoiceRecorderState();
}

class _VoiceRecorderState extends State<VoiceRecorder> {
  final _rec = AudioRecorder();
  final _levels = <double>[];
  final _all = <double>[];
  StreamSubscription<Amplitude>? _ampSub;
  Timer? _timer;
  DateTime? _startedAt;
  Duration _elapsed = Duration.zero;
  String _mime = 'audio/mp4';
  String _ext = 'm4a';
  bool _finishing = false;
  bool _viewOnce = false;
  // Read once: the setting cannot change in the middle of a recording.
  final bool _mask = prefs.voiceMask;
  bool _masking = false;

  @override
  void initState() {
    super.initState();
    _start();
  }

  @override
  void dispose() {
    _timer?.cancel();
    _ampSub?.cancel();
    if (_finishing) {
      _rec.dispose();
    } else {
      // Left without sending or cancelling: the draft recording is deleted.
      final rec = _rec;
      unawaited(rec.cancel().catchError((_) {}).whenComplete(rec.dispose));
    }
    super.dispose();
  }

  Future<void> _start() async {
    try {
      if (!await _rec.hasPermission()) {
        showToast(tr('mobile.micDenied'), error: true);
        widget.onCancel();
        return;
      }
      // AAC in MP4 plays on every receiver (iOS/macOS, Android, desktop webviews); Opus/Ogg
      // does not play on Apple platforms. The desktop app sends WAV, which also plays here.
      // Masking works on PCM: the recording is then WAV (16 kHz, what the masking outputs).
      final encoder = _mask ? AudioEncoder.wav : AudioEncoder.aacLc;
      _mime = _mask ? 'audio/wav' : 'audio/mp4';
      _ext = _mask ? 'wav' : 'm4a';
      final path = await appTempPath('voice.$_ext');
      await _rec.start(RecordConfig(encoder: encoder, numChannels: 1, sampleRate: _mask ? 16000 : 48000, bitRate: 32000), path: path);
      _startedAt = DateTime.now();
      _ampSub = _rec.onAmplitudeChanged(const Duration(milliseconds: 100)).listen((a) {
        final v = levelFromDb(a.current);
        _all.add(v);
        if (mounted) {
          setState(() {
            _levels.add(v);
            if (_levels.length > 60) _levels.removeAt(0);
          });
        }
      });
      _timer = Timer.periodic(const Duration(milliseconds: 200), (_) {
        final s = _startedAt;
        if (s == null || !mounted) return;
        setState(() => _elapsed = DateTime.now().difference(s));
        if (_elapsed > _maxDuration) _finish(true);
      });
    } catch (e) {
      showToast(tr('voice.micError'), error: true);
      widget.onCancel();
    }
  }

  Future<void> _finish(bool send) async {
    if (_finishing) return;
    _finishing = true;
    _timer?.cancel();
    await _ampSub?.cancel();
    String? path;
    try {
      path = await _rec.stop();
    } catch (_) {}
    final duration = _startedAt == null ? Duration.zero : DateTime.now().difference(_startedAt!);
    if (!send || path == null || duration.inMilliseconds < 700) {
      if (path != null) unawaited(File(path).delete().catchError((_) => File(path!)));
      widget.onCancel();
      return;
    }
    try {
      final bytes = await File(path).readAsBytes();
      unawaited(File(path).delete().catchError((_) => File(path!)));
      var data = base64Encode(bytes);
      if (_mask) {
        if (mounted) setState(() => _masking = true);
        try {
          data = await backend.maskVoice(data);
        } catch (_) {
          showToast(tr('voice.maskError'), error: true);
          widget.onCancel();
          return;
        }
      }
      final stamp = DateTime.now().toIso8601String().replaceAll(RegExp(r'[:.]'), '-');
      widget.onSend(OutgoingAttachment(
        dataBase64: data,
        fileName: 'voice-$stamp.$_ext',
        mime: _mime,
        kind: 'voice',
        durationMs: duration.inMilliseconds,
        waveform: downsampleLevels(_all, _wavePoints),
      ), _viewOnce);
    } catch (e) {
      notifyError(e);
      widget.onCancel();
    }
  }

  @override
  Widget build(BuildContext context) {
    final t = context.tk;
    return Row(crossAxisAlignment: CrossAxisAlignment.end, children: [
      IconBtn('trash', size: 44, round: true, color: t.danger, tooltip: context.t('voice.cancel'), onPressed: () => _finish(false)),
      const SizedBox(width: 8),
      Expanded(
        child: Container(
          height: 44,
          padding: const EdgeInsets.symmetric(horizontal: 16),
          decoration: BoxDecoration(color: t.sunk, borderRadius: BorderRadius.circular(22)),
          child: Row(children: [
            _RecDot(color: t.danger),
            const SizedBox(width: 10),
            Text(formatDuration(_elapsed.inMilliseconds), style: ts(14, weight: 600, color: t.ink, mono: true)),
            if (_mask) ...[
              const SizedBox(width: 8),
              Semantics(label: context.t('voice.maskOn'), child: AppIcon('shield', size: 16, color: t.accent)),
            ],
            const SizedBox(width: 12),
            if (_masking)
              Expanded(child: Text(context.t('voice.maskProcessing'), maxLines: 1, overflow: TextOverflow.ellipsis, style: ts(13, color: t.muted)))
            else
            Expanded(
              // Only the most recent levels that fit (5 px per bar): the bars must never spill
              // under the buttons next to the field.
              child: SizedBox(
                height: 28,
                child: ClipRect(
                  child: LayoutBuilder(builder: (context, box) {
                    final fit = math.max(0, (box.maxWidth / 5).floor());
                    final shown = _levels.length > fit ? _levels.sublist(_levels.length - fit) : _levels;
                    return Row(mainAxisAlignment: MainAxisAlignment.end, children: [
                      for (final v in shown)
                        Container(
                          width: 3,
                          height: math.max(3.4, v * 28),
                          margin: const EdgeInsets.only(left: 2),
                          decoration: BoxDecoration(color: t.accent, borderRadius: BorderRadius.circular(2)),
                        ),
                    ]);
                  }),
                ),
              ),
            ),
          ]),
        ),
      ),
      const SizedBox(width: 8),
      ViewOnceToggle(
        on: _viewOnce,
        onPressed: () {
          setState(() => _viewOnce = !_viewOnce);
          showToast(tr(_viewOnce ? 'mobile.viewOnce.voiceOn' : 'mobile.viewOnce.off'));
        },
      ),
      const SizedBox(width: 8),
      SizedBox(
        width: 44,
        height: 44,
        child: Tap(
          onTap: () => _finish(true),
          color: t.accent,
          radius: BorderRadius.circular(22),
          semanticLabel: context.t('voice.send'),
          child: Center(child: AppIcon('send', size: 19, color: t.onAccent)),
        ),
      ),
    ]);
  }
}

class _RecDot extends StatefulWidget {
  const _RecDot({required this.color});
  final Color color;

  @override
  State<_RecDot> createState() => _RecDotState();
}

class _RecDotState extends State<_RecDot> with SingleTickerProviderStateMixin {
  late final _c = AnimationController(vsync: this, duration: const Duration(milliseconds: 900))..repeat(reverse: true);

  @override
  void dispose() {
    _c.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) => FadeTransition(
        opacity: Tween(begin: .35, end: 1.0).animate(_c),
        child: Container(width: 10, height: 10, decoration: BoxDecoration(color: widget.color, shape: BoxShape.circle)),
      );
}

// ---------------------------------------------------------------------------
// player
// ---------------------------------------------------------------------------

List<double> _resample(List<double>? wave, int n) {
  if (wave == null || wave.isEmpty) return List.filled(n, 0.3);
  return List.generate(n, (i) => (wave[(i / n * wave.length).floor()]).clamp(0.1, 1.0));
}

class Waveform extends StatelessWidget {
  const Waveform({super.key, required this.wave, required this.progress, required this.on, required this.off, this.onSeek});

  final List<double>? wave;
  final double progress;
  final Color on;
  final Color off;
  final ValueChanged<double>? onSeek;

  @override
  Widget build(BuildContext context) {
    final bars = _resample(wave, _bars);
    return LayoutBuilder(builder: (context, c) {
      void seek(Offset p) => onSeek?.call((p.dx / c.maxWidth).clamp(0.0, 1.0));
      return GestureDetector(
        behavior: HitTestBehavior.opaque,
        onTapDown: onSeek == null ? null : (d) => seek(d.localPosition),
        onHorizontalDragUpdate: onSeek == null ? null : (d) => seek(d.localPosition),
        child: SizedBox(
          height: 28,
          child: ClipRect(
            child: Row(children: [
              for (var i = 0; i < bars.length; i++)
                Container(
                  width: 3,
                  height: 5 + (bars[i] * 21).roundToDouble(),
                  margin: const EdgeInsets.only(right: 2),
                  decoration: BoxDecoration(color: i / _bars < progress ? on : off, borderRadius: BorderRadius.circular(2)),
                ),
            ]),
          ),
        ),
      );
    });
  }
}

/// Only one voice note plays at a time.
_VoicePlayerState? _playing;

class VoicePlayer extends StatefulWidget {
  const VoicePlayer({
    super.key,
    required this.message,
    required this.a,
    required this.playBg,
    required this.playFg,
    required this.waveOn,
    required this.waveOff,
    required this.meta,
  });

  final Message message;
  final Attachment a;
  final Color playBg, playFg, waveOn, waveOff, meta;

  @override
  State<VoicePlayer> createState() => _VoicePlayerState();
}

class _VoicePlayerState extends State<VoicePlayer> {
  AudioPlayer? _player;
  StreamSubscription<Duration>? _posSub;
  StreamSubscription<PlayerState>? _stateSub;
  bool _isPlaying = false;
  Duration _pos = Duration.zero;
  late Duration _duration = Duration(milliseconds: widget.a.durationMs ?? 0);
  double _speed = 1;
  String? _loadedPath;
  File? _copy;

  @override
  void dispose() {
    if (_playing == this) _playing = null;
    _posSub?.cancel();
    _stateSub?.cancel();
    _player?.dispose();
    final copy = _copy;
    if (copy != null) unawaited(discardExport(copy));
    super.dispose();
  }

  Future<AudioPlayer?> _ensure() async {
    final path = widget.a.localPath;
    if (path == null) return null;
    try {
      final p = _player ??= AudioPlayer();
      if (_loadedPath != path) {
        // The player needs a real file: a temporary decrypted copy, removed on dispose.
        final copy = _copy ??= await exportMedia(path, 'voice-${widget.a.id}.${path.split('.').last}');
        final d = await p.setFilePath(copy.path);
        _loadedPath = path;
        if (d != null && d > Duration.zero) setState(() => _duration = d);
        _posSub ??= p.positionStream.listen((pos) {
          if (mounted) setState(() => _pos = pos);
        });
        _stateSub ??= p.playerStateStream.listen((s) {
          if (!mounted) return;
          if (s.processingState == ProcessingState.completed) {
            p.pause();
            p.seek(Duration.zero);
            setState(() {
              _isPlaying = false;
              _pos = Duration.zero;
            });
          } else {
            setState(() => _isPlaying = s.playing);
          }
        });
      }
      return p;
    } catch (e) {
      debugPrint('voice playback unavailable: $e');
      return null;
    }
  }

  Future<void> _toggle() async {
    final p = await _ensure();
    if (p == null) return;
    if (p.playing) {
      await p.pause();
    } else {
      final other = _playing;
      if (other != null && other != this) await other._player?.pause();
      _playing = this;
      await p.setSpeed(_speed);
      unawaited(p.play());
    }
  }

  Future<void> _cycleSpeed() async {
    const speeds = [1.0, 1.5, 2.0];
    final next = speeds[(speeds.indexOf(_speed) + 1) % speeds.length];
    setState(() => _speed = next);
    await _player?.setSpeed(next);
  }

  @override
  Widget build(BuildContext context) {
    final a = widget.a;
    // Erased by the retention period: nothing to play.
    if (a.isExpired) return ExpiredMediaCard(attachments: [a], voice: true, metaColor: widget.meta);
    final has = a.localPath != null;
    final progress = _duration.inMilliseconds > 0 ? (_pos.inMilliseconds / _duration.inMilliseconds).clamp(0.0, 1.0) : 0.0;
    Widget button;
    if (!has) {
      button = a.downloadState == 'downloading'
          ? SizedBox.square(dimension: 18, child: CircularProgressIndicator(strokeWidth: 2, color: widget.playFg))
          : AppIcon('download', size: 19, color: widget.playFg);
    } else {
      button = AppIcon(_isPlaying ? 'pause' : 'play', size: 18, color: widget.playFg);
    }
    final shown = _isPlaying || _pos > Duration.zero ? _pos : _duration;
    return SizedBox(
      width: 250,
      child: Row(children: [
        SizedBox(
          width: 42,
          height: 42,
          child: Tap(
            onTap: has ? _toggle : () => downloadAttachment(widget.message, a),
            color: widget.playBg,
            radius: BorderRadius.circular(21),
            semanticLabel: context.t(!has ? 'message.download' : _isPlaying ? 'voice.pause' : 'voice.play'),
            child: Center(child: button),
          ),
        ),
        const SizedBox(width: 11),
        Expanded(
          child: Column(crossAxisAlignment: CrossAxisAlignment.start, mainAxisSize: MainAxisSize.min, children: [
            Waveform(
              wave: a.waveform,
              progress: progress,
              on: widget.waveOn,
              off: widget.waveOff,
              onSeek: has
                  ? (p) async {
                      final pl = await _ensure();
                      if (pl != null && _duration > Duration.zero) await pl.seek(_duration * p);
                    }
                  : null,
            ),
            const SizedBox(height: 4),
            Row(children: [
              Text(formatDuration(shown.inMilliseconds), style: ts(11.5, color: widget.meta, mono: true)),
              if (has && (_isPlaying || _speed != 1)) ...[
                const SizedBox(width: 8),
                GestureDetector(
                  onTap: _cycleSpeed,
                  child: Container(
                    padding: const EdgeInsets.symmetric(horizontal: 6, vertical: 1),
                    decoration: BoxDecoration(color: widget.meta.withValues(alpha: .18), borderRadius: BorderRadius.circular(999)),
                    child: Text('${_speed == _speed.roundToDouble() ? _speed.toInt() : _speed}×', style: ts(11, weight: 700, color: widget.meta)),
                  ),
                ),
              ],
            ]),
          ]),
        ),
      ]),
    );
  }
}
