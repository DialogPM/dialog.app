// Full-screen viewer for incoming view-once media (core: engine/view_once.rs).
//
// Opening marks the message opened everywhere (`open_view_once`); the media is shown once,
// with screen security forced on, and without save / share / forward / copy. On close the
// decrypted bytes are dropped, a temporary video copy is deleted and `view_once_done` makes
// the core securely delete its file. Leaving the app closes the viewer.
//
// A voice message plays once here: from a decrypted copy in the app's private temporary
// directory (the player needs a file), overwritten and deleted as soon as the viewer closes.
// It starts at once, can be paused but not sought, and the viewer closes when playback ends,
// when the user closes it or when the app goes to the background, whichever comes first.

import 'dart:async';
import 'dart:io';
import 'dart:typed_data';

import 'package:flutter/material.dart';
import 'package:just_audio/just_audio.dart';
import 'package:video_player/video_player.dart';

import '../../app/globals.dart';
import '../../core/backend.dart';
import '../../core/models.dart';
import '../../l10n/i18n.dart';
import '../../services/media.dart';
import '../../services/security.dart';
import '../../theme/app_theme.dart';
import '../../util/format.dart';
import '../../widgets/app_icon.dart';
import '../../widgets/ui.dart';
import 'voice.dart' show Waveform;

Future<void> openViewOnceViewer(BuildContext context, Message message) =>
    Navigator.of(context).push(PageRouteBuilder<void>(
      barrierColor: Colors.black,
      pageBuilder: (_, _, _) => ViewOnceViewer(message: message),
      transitionsBuilder: (_, anim, _, child) => FadeTransition(opacity: anim, child: child),
    ));

class ViewOnceViewer extends StatefulWidget {
  const ViewOnceViewer({super.key, required this.message});
  final Message message;

  @override
  State<ViewOnceViewer> createState() => _ViewOnceViewerState();
}

class _ViewOnceViewerState extends State<ViewOnceViewer> with WidgetsBindingObserver {
  late final Future<void> Function() _releaseSecurity;
  bool _closed = false;
  bool _opened = false; // open_view_once succeeded: view_once_done is owed
  Uint8List? _bytes;
  File? _videoFile;
  VideoPlayerController? _video;
  File? _audioFile;
  AudioPlayer? _audio;
  Attachment? _audioAtt;
  StreamSubscription<Duration>? _audioPos;
  StreamSubscription<PlayerState>? _audioState;
  Duration _audioAt = Duration.zero;
  Duration _audioLen = Duration.zero;
  bool _audioPlaying = false;
  String? _error;

  bool get _isVoice => widget.message.kind == 'voice';

  @override
  void initState() {
    super.initState();
    _releaseSecurity = holdScreenSecurity(); // before anything is shown
    WidgetsBinding.instance.addObserver(this);
    unawaited(_open());
  }

  Future<void> _open() async {
    final id = widget.message.id;
    try {
      final a = await backend.openViewOnce(id);
      _opened = true;
      if (_closed) return await _cleanup();
      final path = a.localPath;
      if (path == null) throw StateError('no media');
      if (_isVoice) {
        // The player needs a real file: a temporary decrypted copy in the app's private
        // temporary directory, overwritten and deleted on close.
        final f = await exportMedia(path, 'voice.${path.split('.').last}');
        _audioFile = f;
        if (_closed) return await _cleanup();
        final p = AudioPlayer();
        _audio = p;
        final d = await p.setFilePath(f.path);
        if (_closed) return await _cleanup();
        _audioLen = d ?? Duration(milliseconds: a.durationMs ?? 0);
        _audioPos = p.positionStream.listen((pos) {
          if (mounted) setState(() => _audioAt = pos);
        });
        _audioState = p.playerStateStream.listen((st) {
          if (st.processingState == ProcessingState.completed) {
            // Played to the end: gone for good.
            _close();
          } else if (mounted) {
            setState(() => _audioPlaying = st.playing);
          }
        });
        if (mounted) setState(() => _audioAtt = a);
        unawaited(p.play());
        return;
      }
      if (a.isVideo || (!a.isImage && widget.message.kind == 'video')) {
        // The player needs a real file: a temporary decrypted copy, deleted on close.
        final f = await exportMedia(path, a.fileName.isEmpty ? 'video.mp4' : a.fileName);
        _videoFile = f;
        if (_closed) return await _cleanup();
        final c = VideoPlayerController.file(f);
        _video = c;
        await c.initialize();
        if (_closed) return await _cleanup();
        await c.play();
        c.addListener(_onVideo);
        if (mounted) setState(() {});
      } else {
        // Straight from the core, not through loadMedia: never kept in the media cache.
        final bytes = await backend.readMedia(path);
        if (_closed) {
          bytes.fillRange(0, bytes.length, 0);
          return await _cleanup();
        }
        if (mounted) setState(() => _bytes = bytes);
      }
    } catch (e) {
      final already = e is CoreError && e.message.contains('already opened');
      if (_closed) return await _cleanup();
      if (mounted) setState(() => _error = tr(already ? 'mobile.viewOnce.alreadyOpened' : 'mobile.viewOnce.openFailed'));
    }
  }

  void _onVideo() {
    if (mounted) setState(() {});
  }

  @override
  void didChangeAppLifecycleState(AppLifecycleState state) {
    // Leaving the app ends the viewing.
    if ((state == AppLifecycleState.paused || state == AppLifecycleState.hidden) && !_closed) _close();
  }

  bool _popped = false;

  void _close() {
    // Once only: playback end and leaving the app may both ask.
    if (_popped || !mounted || !Navigator.of(context).canPop()) return;
    _popped = true;
    Navigator.of(context).pop();
  }

  /// Drops everything shown and lets the core erase the media. Runs once the viewer is gone
  /// (and again, harmlessly, if the opening finishes after that).
  Future<void> _cleanup() async {
    final audio = _audio;
    _audio = null;
    await _audioPos?.cancel();
    await _audioState?.cancel();
    _audioPos = null;
    _audioState = null;
    if (audio != null) {
      try {
        await audio.stop();
      } catch (_) {}
      await audio.dispose();
    }
    final af = _audioFile;
    _audioFile = null;
    if (af != null) await _shred(af);
    final video = _video;
    _video = null;
    if (video != null) {
      video.removeListener(_onVideo);
      await video.dispose();
    }
    final f = _videoFile;
    _videoFile = null;
    if (f != null) await _shred(f);
    final b = _bytes;
    _bytes = null;
    if (b != null) b.fillRange(0, b.length, 0);
    // Decoded frames of the image live in the image cache: drop them.
    PaintingBinding.instance.imageCache
      ..clear()
      ..clearLiveImages();
    if (_opened) {
      _opened = false;
      try {
        await backend.viewOnceDone(widget.message.id);
      } catch (_) {}
    }
  }

  /// Overwrites a decrypted temporary copy with zeros, then deletes it.
  static Future<void> _shred(File f) async {
    try {
      final len = await f.length();
      final raf = await f.open(mode: FileMode.writeOnlyAppend);
      try {
        await raf.setPosition(0);
        const chunk = 64 * 1024;
        final zeros = Uint8List(chunk);
        for (var left = len; left > 0; left -= chunk) {
          await raf.writeFrom(zeros, 0, left < chunk ? left : chunk);
        }
        await raf.flush();
      } finally {
        await raf.close();
      }
    } catch (_) {}
    await discardExport(f);
  }

  Widget _voiceBody(Attachment a) {
    final t = context.tk;
    final len = _audioLen.inMilliseconds;
    final progress = len > 0 ? (_audioAt.inMilliseconds / len).clamp(0.0, 1.0) : 0.0;
    final shown = _audioPlaying || _audioAt > Duration.zero ? _audioAt : _audioLen;
    return Center(
      child: Container(
        width: 320,
        margin: const EdgeInsets.symmetric(horizontal: 20),
        padding: const EdgeInsets.fromLTRB(18, 18, 20, 18),
        decoration: BoxDecoration(color: Colors.white.withValues(alpha: .08), borderRadius: BorderRadius.circular(22)),
        child: Row(children: [
          SizedBox(
            width: 52,
            height: 52,
            child: Tap(
              // Pause and resume only: no seeking, no replay.
              onTap: () {
                final p = _audio;
                if (p == null) return;
                p.playing ? p.pause() : p.play();
              },
              color: t.accent,
              radius: BorderRadius.circular(26),
              semanticLabel: context.t(_audioPlaying ? 'voice.pause' : 'voice.play'),
              child: Center(child: AppIcon(_audioPlaying ? 'pause' : 'play', size: 22, color: t.onAccent)),
            ),
          ),
          const SizedBox(width: 14),
          Expanded(
            child: Column(crossAxisAlignment: CrossAxisAlignment.start, mainAxisSize: MainAxisSize.min, children: [
              Waveform(wave: a.waveform, progress: progress, on: t.accent, off: Colors.white30),
              const SizedBox(height: 6),
              Text(formatDuration(shown.inMilliseconds), style: ts(12, color: Colors.white70, mono: true)),
            ]),
          ),
        ]),
      ),
    );
  }

  @override
  void dispose() {
    _closed = true;
    WidgetsBinding.instance.removeObserver(this);
    unawaited(_cleanup().whenComplete(_releaseSecurity));
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    final m = widget.message;
    final video = _video;
    Widget body;
    if (_error != null) {
      body = Center(
        child: Padding(
          padding: const EdgeInsets.all(24),
          child: Column(mainAxisSize: MainAxisSize.min, children: [
            const AppIcon('eyeoff', size: 36, color: Colors.white60),
            const SizedBox(height: 12),
            Text(_error!, textAlign: TextAlign.center, style: ts(15, color: Colors.white)),
          ]),
        ),
      );
    } else if (_audioAtt != null) {
      body = _voiceBody(_audioAtt!);
    } else if (_bytes != null) {
      body = GestureDetector(
        behavior: HitTestBehavior.opaque,
        onTap: _close,
        child: InteractiveViewer(
          minScale: 1,
          maxScale: 5,
          child: Center(child: SafeMemoryImage(_bytes!, fit: BoxFit.contain)),
        ),
      );
    } else if (video != null && video.value.isInitialized) {
      final v = video.value;
      body = GestureDetector(
        behavior: HitTestBehavior.opaque,
        onTap: () => v.isPlaying ? video.pause() : video.play(),
        child: Stack(fit: StackFit.expand, children: [
          Center(child: AspectRatio(aspectRatio: v.aspectRatio, child: VideoPlayer(video))),
          if (!v.isPlaying)
            Center(
              child: Container(
                width: 64,
                height: 64,
                decoration: const BoxDecoration(color: Color.fromRGBO(0, 0, 0, .45), shape: BoxShape.circle),
                child: const Center(child: AppIcon('play', size: 32, color: Colors.white)),
              ),
            ),
          Positioned(
            left: 16,
            right: 16,
            bottom: 12,
            child: VideoProgressIndicator(video, allowScrubbing: true, padding: const EdgeInsets.symmetric(vertical: 8)),
          ),
        ]),
      );
    } else {
      body = Center(
        child: Column(mainAxisSize: MainAxisSize.min, children: [
          const SizedBox.square(dimension: 28, child: CircularProgressIndicator(strokeWidth: 2.5, color: Colors.white)),
          const SizedBox(height: 12),
          Text(context.t('mobile.viewOnce.loading'), style: ts(13, color: Colors.white60)),
        ]),
      );
    }
    return Scaffold(
      backgroundColor: Colors.black,
      body: SafeArea(
        child: Column(children: [
          Padding(
            padding: const EdgeInsets.fromLTRB(6, 8, 8, 8),
            child: Row(children: [
              IconBtn('x', color: Colors.white, iconSize: 21, tooltip: context.t('common.close'), onPressed: _close),
              const SizedBox(width: 6),
              Avatar(name: m.senderName, seed: m.senderAccountId, size: 36),
              const SizedBox(width: 10),
              Expanded(
                child: Column(crossAxisAlignment: CrossAxisAlignment.start, children: [
                  Text(m.senderName, maxLines: 1, overflow: TextOverflow.ellipsis, style: ts(15, weight: 650, color: Colors.white)),
                  Row(children: [
                    const AppIcon('eye', size: 13, color: Colors.white60),
                    const SizedBox(width: 4),
                    Text(context.t(_isVoice ? 'viewOnce.voice' : 'mobile.viewOnce.title'), style: ts(12, color: Colors.white60)),
                  ]),
                ]),
              ),
            ]),
          ),
          Expanded(child: body),
          if (_bytes != null)
            Padding(
              padding: const EdgeInsets.only(bottom: 10),
              child: Text(context.t('mobile.viewOnce.tapToClose'), style: ts(12, color: Colors.white38)),
            ),
          if (_audioAtt != null)
            Padding(
              padding: const EdgeInsets.fromLTRB(24, 0, 24, 14),
              child: Text(context.t('viewOnce.voiceHint'), textAlign: TextAlign.center, style: ts(12, color: Colors.white38)),
            ),
        ]),
      ),
    );
  }
}
