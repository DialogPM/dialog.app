// Settings → Privacy → Anonymized voice: the switch (prefs.voiceMask, used by the voice
// recorder) and a 5 second sample to hear the result. The sample is recorded and masked in
// temporary files that are deleted right after; it is never sent.

import 'dart:async';
import 'dart:convert';
import 'dart:io';

import 'package:flutter/material.dart';
import 'package:flutter_riverpod/flutter_riverpod.dart';
import 'package:just_audio/just_audio.dart';
import 'package:record/record.dart';

import '../../app/globals.dart';
import '../../l10n/i18n.dart';
import '../../services/media.dart';
import '../../state/prefs.dart';
import '../../theme/app_theme.dart';
import '../../widgets/toast.dart';
import '../../widgets/ui.dart';

const _sample = Duration(seconds: 5);

enum _Step { idle, recording, processing }

class VoiceMaskCard extends ConsumerStatefulWidget {
  const VoiceMaskCard({super.key});

  @override
  ConsumerState<VoiceMaskCard> createState() => _VoiceMaskCardState();
}

class _VoiceMaskCardState extends ConsumerState<VoiceMaskCard> {
  final _player = AudioPlayer();
  var _step = _Step.idle;
  String? _original;
  String? _masked;

  @override
  void dispose() {
    unawaited(_player.dispose());
    _deleteSamples();
    super.dispose();
  }

  void _deleteSamples() {
    for (final p in [_original, _masked]) {
      if (p != null) unawaited(File(p).delete().then((_) {}, onError: (_) {}));
    }
    _original = _masked = null;
  }

  Future<void> _tryIt() async {
    final rec = AudioRecorder();
    try {
      if (!await rec.hasPermission()) {
        showToast(tr('mobile.micDenied'), error: true);
        return;
      }
      await _player.stop();
      setState(() {
        _deleteSamples();
        _step = _Step.recording;
      });
      final path = await appTempPath('voice-sample.wav');
      await rec.start(const RecordConfig(encoder: AudioEncoder.wav, numChannels: 1, sampleRate: 16000), path: path);
      await Future<void>.delayed(_sample);
      final recorded = await rec.stop() ?? path;
      _original = recorded;
      if (mounted) setState(() => _step = _Step.processing);
      final masked = base64Decode(await backend.maskVoice(base64Encode(await File(recorded).readAsBytes())));
      final out = await appTempPath('voice-sample-masked.wav');
      await File(out).writeAsBytes(masked, flush: true);
      _masked = out;
      if (mounted) unawaited(_play(out));
    } catch (_) {
      showToast(tr('voice.maskError'), error: true);
    } finally {
      unawaited(rec.dispose());
      if (mounted) setState(() => _step = _Step.idle);
    }
  }

  Future<void> _play(String path) async {
    try {
      await _player.stop();
      await _player.setFilePath(path);
      await _player.play();
    } catch (_) {}
  }

  @override
  Widget build(BuildContext context) {
    final t = context.tk;
    final on = ref.watch(prefsProvider.select((p) => p.voiceMask));
    final status = switch (_step) {
      _Step.recording => context.t('voice.maskRecording'),
      _Step.processing => context.t('voice.maskProcessing'),
      _Step.idle => context.t('voice.maskTryHint'),
    };
    return Column(crossAxisAlignment: CrossAxisAlignment.stretch, children: [
      Padding(padding: const EdgeInsets.only(left: 4, bottom: 8), child: Eyebrow(context.t('voice.maskSection'))),
      ListCard(children: [
        ToggleRow(
          padding: const EdgeInsets.symmetric(horizontal: 16, vertical: 15),
          label: context.t('voice.mask'),
          sub: context.t('voice.maskHint'),
          on: on,
          onToggle: () => ref.read(prefsProvider.notifier).update((x) => x.copyWith(voiceMask: !x.voiceMask)),
        ),
        Padding(
          padding: const EdgeInsets.fromLTRB(16, 14, 16, 14),
          child: Column(crossAxisAlignment: CrossAxisAlignment.stretch, children: [
            Text(context.t('voice.maskTry'), style: ts(14.5, weight: 600, color: t.ink)),
            const SizedBox(height: 2),
            Text(status, style: ts(13, color: t.muted)),
            const SizedBox(height: 12),
            Btn(
              label: context.t('voice.maskTryButton'),
              icon: 'mic',
              kind: BtnKind.secondary,
              height: 44,
              busy: _step != _Step.idle,
              onPressed: _step == _Step.idle ? _tryIt : null,
            ),
            if (_original != null && _masked != null && _step == _Step.idle) ...[
              const SizedBox(height: 8),
              Row(children: [
                Expanded(
                  child: Btn(label: context.t('voice.maskOriginal'), icon: 'play', kind: BtnKind.secondary, height: 44, onPressed: () => _play(_original!)),
                ),
                const SizedBox(width: 8),
                Expanded(child: Btn(label: context.t('voice.maskMasked'), icon: 'play', height: 44, onPressed: () => _play(_masked!))),
              ]),
            ],
          ]),
        ),
      ]),
    ]);
  }
}
