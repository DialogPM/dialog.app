// Full-screen call UI drawn above the whole app (port of app/src/features/calls/CallScreen.tsx):
// incoming (accept / decline), outgoing, connecting, active. Video calls show the peer's video
// full-screen and our camera picture-in-picture.

import 'dart:async';

import 'package:flutter/material.dart';
import 'package:flutter_webrtc/flutter_webrtc.dart';

import '../../l10n/i18n.dart';
import '../../services/voice_fx.dart';
import '../../state/call_audio.dart';
import '../../state/calls.dart';
import '../../state/group_calls.dart';
import '../../theme/app_theme.dart';
import '../../util/format.dart';
import '../../widgets/app_icon.dart';
import '../../widgets/ui.dart';
import 'group_call_screen.dart';

const _fg = Color(0xFFF3F7F5);
const _danger = Color(0xFFE5484D);
const _accept = Color(0xFF2FB36D);

/// Shows the call screen on top of [child] while there is a call.
class CallOverlay extends StatelessWidget {
  const CallOverlay({super.key, required this.child});
  final Widget child;

  @override
  Widget build(BuildContext context) {
    // While a call screen is shown nothing underneath may hold the focus: the keyboard of a
    // chat composer would otherwise stay open over the answer / hang-up buttons.
    return Stack(children: [
      SizedBox.expand(child: ListenableBuilder(
        listenable: Listenable.merge([callState, groupCallState]),
        builder: (context, inner) => ExcludeFocus(excluding: callState.value != null || groupCallState.value != null, child: inner!),
        child: child,
      )),
      ValueListenableBuilder<CallState?>(
        valueListenable: callState,
        builder: (context, call, _) => call == null ? const SizedBox.shrink() : Positioned.fill(child: _CallScreen(call: call)),
      ),
      // Group calls (group_call_screen.dart).
      const GroupCallLayer(),
      // Voice effects panel of either call screen.
      const VoiceFxPanelLayer(),
    ]);
  }
}

class _CallScreen extends StatefulWidget {
  const _CallScreen({required this.call});
  final CallState call;

  @override
  State<_CallScreen> createState() => _CallScreenState();
}

class _CallScreenState extends State<_CallScreen> {
  Timer? _ticker;

  @override
  void initState() {
    super.initState();
    _ticker = Timer.periodic(const Duration(milliseconds: 500), (_) {
      if (widget.call.connectedAt != null && mounted) setState(() {});
    });
  }

  @override
  void dispose() {
    _ticker?.cancel();
    super.dispose();
  }

  String _status(BuildContext context) {
    final call = widget.call;
    switch (call.phase) {
      case CallPhase.incoming:
        return context.t(call.video ? 'call.incomingVideo' : 'call.incoming');
      case CallPhase.outgoing:
        return context.t(call.ringing ? 'call.ringing' : 'call.calling');
      case CallPhase.connecting:
        return context.t('call.connecting');
      case CallPhase.active:
        final since = call.connectedAt ?? DateTime.now().millisecondsSinceEpoch;
        return formatDuration(DateTime.now().millisecondsSinceEpoch - since);
    }
  }

  @override
  Widget build(BuildContext context) {
    final call = widget.call;
    final accent = context.tk.accent;
    final incoming = call.phase == CallPhase.incoming;
    final videoMode = call.video && !incoming;
    final remoteVideo = videoMode && call.remoteStream != null && call.remoteCameraOn && call.remoteStream!.getVideoTracks().isNotEmpty;

    final header = Column(mainAxisSize: MainAxisSize.min, children: [
      Row(mainAxisSize: MainAxisSize.min, children: [
        AppIcon(call.video ? 'video' : 'lock', size: 13, color: _fg.withValues(alpha: 0.7)),
        const SizedBox(width: 6),
        Text(context.t('call.encrypted'), style: ts(12, color: _fg.withValues(alpha: 0.7))),
      ]),
      const SizedBox(height: 10),
      Text(call.peerName, maxLines: 1, overflow: TextOverflow.ellipsis, style: ts(remoteVideo ? 19 : 24, weight: 700, color: _fg)),
      const SizedBox(height: 4),
      Text(_status(context), style: ts(14, color: _fg.withValues(alpha: 0.8), mono: call.phase == CallPhase.active)),
      _QualityNotice(call: call),
    ]);

    final avatar = Column(mainAxisSize: MainAxisSize.min, children: [
      Avatar(name: call.peerName, path: call.peerAvatar, seed: call.conversationId, size: 120),
      if (videoMode && call.phase == CallPhase.active && !call.remoteCameraOn) ...[
        const SizedBox(height: 12),
        Text(context.t('call.peerCameraOff'), style: ts(14, color: _fg.withValues(alpha: 0.75))),
      ],
    ]);

    final actions = incoming
        ? [
            CallButton(icon: 'phone', rotate: true, label: context.t('call.decline'), bg: _danger, onTap: () => unawaited(declineCall())),
            CallButton(icon: call.video ? 'video' : 'phone', label: context.t('call.accept'), bg: _accept, onTap: () => unawaited(acceptCall())),
          ]
        : [
            CallButton(
              icon: call.muted ? 'micoff' : 'mic',
              label: context.t(call.muted ? 'call.unmute' : 'call.mute'),
              on: call.muted,
              onTap: toggleMute,
            ),
            if (call.video)
              CallButton(
                icon: call.cameraOn ? 'video' : 'videooff',
                label: context.t(call.cameraOn ? 'call.cameraOff' : 'call.cameraOn'),
                on: !call.cameraOn,
                onTap: call.hasCamera ? toggleCamera : null,
              ),
            if (call.video && call.hasCamera && call.cameraOn)
              CallButton(icon: 'refresh', label: context.t('mobile.call.flip'), onTap: () => unawaited(switchCamera())),
            if (!call.video)
              CallButton(icon: 'speaker', label: context.t('mobile.call.speaker'), on: call.speaker, onTap: () => unawaited(toggleSpeaker())),
            VoiceFxButton(
              effect: call.voiceEffect,
              denoise: call.denoise,
              enabled: call.phase == CallPhase.active,
              onChange: ({effect, denoise}) => unawaited(setVoiceEffect(effect: effect, denoise: denoise)),
            ),
            CallButton(icon: 'phone', rotate: true, label: context.t('call.end'), bg: _danger, onTap: () => unawaited(hangUp())),
          ];

    return Material(
      color: Colors.black,
      child: Stack(fit: StackFit.expand, children: [
        if (remoteVideo)
          CallStreamView(stream: call.remoteStream!, cover: true)
        else
          DecoratedBox(
            decoration: BoxDecoration(
              gradient: LinearGradient(
                begin: Alignment.topCenter,
                end: Alignment.bottomCenter,
                colors: [Color.lerp(accent, const Color(0xFF0D1714), 0.62)!, const Color(0xFF0B1210)],
              ),
            ),
          ),
        if (!remoteVideo) Center(child: Padding(padding: const EdgeInsets.only(bottom: 60), child: avatar)),
        // Top: name and status, over a scrim when video is shown.
        Positioned(
          top: 0,
          left: 0,
          right: 0,
          child: DecoratedBox(
            decoration: BoxDecoration(
              gradient: remoteVideo
                  ? const LinearGradient(begin: Alignment.topCenter, end: Alignment.bottomCenter, colors: [Color(0x99000000), Color(0x00000000)])
                  : null,
            ),
            child: SafeArea(
              bottom: false,
              child: Padding(padding: const EdgeInsets.fromLTRB(20, 24, 20, 36), child: header),
            ),
          ),
        ),
        if (videoMode && call.hasCamera && call.cameraOn && call.localStream != null)
          Positioned(
            right: 16,
            bottom: 150,
            width: 112,
            height: 160,
            child: ClipRRect(
              borderRadius: BorderRadius.circular(14),
              child: DecoratedBox(
                decoration: const BoxDecoration(color: Color(0xFF111111)),
                child: CallStreamView(stream: call.localStream!, mirror: true, cover: true),
              ),
            ),
          ),
        Positioned(
          left: 0,
          right: 0,
          bottom: 0,
          child: DecoratedBox(
            decoration: BoxDecoration(
              gradient: remoteVideo
                  ? const LinearGradient(begin: Alignment.bottomCenter, end: Alignment.topCenter, colors: [Color(0xAA000000), Color(0x00000000)])
                  : null,
            ),
            child: SafeArea(
              top: false,
              child: Padding(
                padding: const EdgeInsets.fromLTRB(12, 40, 12, 28),
                child: Row(mainAxisAlignment: MainAxisAlignment.spaceEvenly, children: actions),
              ),
            ),
          ),
        ),
      ]),
    );
  }
}

/// Renders a MediaStream (video only; audio plays natively).
/// "Weak connection" notice of an active call (state/call_audio.dart), and why our camera stopped.
class _QualityNotice extends StatelessWidget {
  const _QualityNotice({required this.call});
  final CallState call;

  @override
  Widget build(BuildContext context) {
    final weak = call.quality != QualityLevel.good;
    if (call.phase != CallPhase.active || (!weak && !call.autoCameraOff)) return const SizedBox.shrink();
    final text = call.autoCameraOff
        ? context.t('call.quality.cameraStopped')
        : switch (call.qualityWho) {
            QualityWho.me => context.t('call.quality.mine'),
            QualityWho.peer => context.t('call.quality.peer', args: {'name': call.peerName}),
            null => context.t('call.quality.weak'),
          };
    final bad = call.quality == QualityLevel.bad;
    final fg = bad ? const Color(0xFFFFB3B5) : const Color(0xFFFFD27A);
    return Padding(
      padding: const EdgeInsets.only(top: 10, left: 24, right: 24),
      child: Container(
        padding: const EdgeInsets.symmetric(horizontal: 12, vertical: 6),
        decoration: BoxDecoration(
          color: (bad ? const Color(0xFFE5484D) : const Color(0xFFF5A623)).withValues(alpha: 0.2),
          borderRadius: BorderRadius.circular(999),
        ),
        child: Row(mainAxisSize: MainAxisSize.min, children: [
          AppIcon('alert', size: 14, color: fg),
          const SizedBox(width: 6),
          Flexible(child: Text(text, textAlign: TextAlign.center, style: ts(12.5, color: fg))),
        ]),
      ),
    );
  }
}

class CallStreamView extends StatefulWidget {
  const CallStreamView({super.key, required this.stream, this.mirror = false, this.cover = false});
  final MediaStream stream;
  final bool mirror;
  final bool cover;

  @override
  State<CallStreamView> createState() => _CallStreamViewState();
}

class _CallStreamViewState extends State<CallStreamView> {
  final _renderer = RTCVideoRenderer();
  bool _ready = false;

  @override
  void initState() {
    super.initState();
    unawaited(_init());
  }

  Future<void> _init() async {
    await _renderer.initialize();
    if (!mounted) {
      await _renderer.dispose();
      return;
    }
    _renderer.srcObject = widget.stream;
    setState(() => _ready = true);
  }

  @override
  void didUpdateWidget(CallStreamView old) {
    super.didUpdateWidget(old);
    if (_ready && old.stream != widget.stream) _renderer.srcObject = widget.stream;
  }

  @override
  void dispose() {
    if (_ready) _renderer.srcObject = null;
    unawaited(_renderer.dispose());
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    if (!_ready) return const SizedBox.shrink();
    return RTCVideoView(
      _renderer,
      mirror: widget.mirror,
      objectFit: widget.cover ? RTCVideoViewObjectFit.RTCVideoViewObjectFitCover : RTCVideoViewObjectFit.RTCVideoViewObjectFitContain,
    );
  }
}

/// The open effects panel of the call screens (drawn by [VoiceFxPanelLayer]). The call screens
/// sit above the app's navigator, so the panel is part of their own layer, not a route.
final _fxPanel = ValueNotifier<_FxPanelRequest?>(null);

class _FxPanelRequest {
  const _FxPanelRequest(this.effect, this.denoise, this.onChange);
  final String effect;
  final bool denoise;
  final void Function({String? effect, bool? denoise}) onChange;
}

/// "Effects" button of the call screens: live voice effect and AI noise reduction, for this call.
class VoiceFxButton extends StatelessWidget {
  const VoiceFxButton({super.key, required this.effect, required this.denoise, required this.enabled, required this.onChange});

  final String effect;
  final bool denoise;
  final bool enabled;
  final void Function({String? effect, bool? denoise}) onChange;

  @override
  Widget build(BuildContext context) {
    final on = effect != 'none' || denoise;
    final label = effect != 'none'
        ? '${kVoiceEffectEmoji[effect]} ${context.t('voiceFx.$effect')}'
        : denoise
            ? context.t('voiceFx.denoiseShort')
            : context.t('voiceFx.button');
    return CallButton(
      icon: 'sparkles',
      label: label,
      on: on,
      onTap: enabled ? () => _fxPanel.value = _FxPanelRequest(effect, denoise, onChange) : null,
    );
  }
}

/// The effects panel over the call screen (a scrim and a sheet at the bottom).
class VoiceFxPanelLayer extends StatelessWidget {
  const VoiceFxPanelLayer({super.key});

  @override
  Widget build(BuildContext context) {
    return ListenableBuilder(
      listenable: Listenable.merge([_fxPanel, callState, groupCallState]),
      builder: (context, _) {
        final req = _fxPanel.value;
        if (req == null) return const SizedBox.shrink();
        // Closed with the call.
        if (callState.value == null && groupCallState.value == null) {
          WidgetsBinding.instance.addPostFrameCallback((_) => _fxPanel.value = null);
          return const SizedBox.shrink();
        }
        void close() => _fxPanel.value = null;
        Widget row({required Widget leading, required String text, required bool selected, required VoidCallback onTap}) => InkWell(
              onTap: onTap,
              child: Padding(
                padding: const EdgeInsets.symmetric(horizontal: 22, vertical: 13),
                child: Row(children: [
                  SizedBox(width: 28, child: Center(child: leading)),
                  const SizedBox(width: 14),
                  Expanded(child: Text(text, style: ts(16, weight: selected ? 700 : 500, color: _fg))),
                  if (selected) const AppIcon('check', size: 18, color: _accept),
                ]),
              ),
            );
        return Positioned.fill(
          child: Stack(children: [
            Positioned.fill(child: GestureDetector(onTap: close, child: const ColoredBox(color: Color(0x88000000)))),
            Align(
              alignment: Alignment.bottomCenter,
              child: Material(
                color: const Color(0xFF16201C),
                borderRadius: const BorderRadius.vertical(top: Radius.circular(22)),
                child: SafeArea(
                  top: false,
                  child: Column(mainAxisSize: MainAxisSize.min, crossAxisAlignment: CrossAxisAlignment.stretch, children: [
                    const SizedBox(height: 10),
                    row(
                      leading: const AppIcon('mic', size: 20, color: _fg),
                      text: context.t('voiceFx.denoise'),
                      selected: req.denoise,
                      onTap: () {
                        req.onChange(denoise: !req.denoise);
                        _fxPanel.value = _FxPanelRequest(req.effect, !req.denoise, req.onChange);
                      },
                    ),
                    Divider(color: _fg.withValues(alpha: 0.12), height: 12),
                    Padding(
                      padding: const EdgeInsets.fromLTRB(22, 6, 22, 4),
                      child: Text(context.t('voiceFx.title').toUpperCase(), style: ts(12, weight: 700, color: _fg.withValues(alpha: 0.6), letterSpacing: 1)),
                    ),
                    for (final e in kVoiceEffects)
                      row(
                        leading: Text(kVoiceEffectEmoji[e]!, style: const TextStyle(fontSize: 20)),
                        text: context.t('voiceFx.$e'),
                        selected: req.effect == e,
                        onTap: () {
                          req.onChange(effect: e);
                          close();
                        },
                      ),
                    const SizedBox(height: 10),
                  ]),
                ),
              ),
            ),
          ]),
        );
      },
    );
  }
}

class CallButton extends StatelessWidget {
  const CallButton({super.key, required this.icon, required this.label, this.onTap, this.bg, this.on = false, this.rotate = false});

  final String icon;
  final String label;
  final VoidCallback? onTap;
  final Color? bg;
  /// Toggle buttons: highlighted while on (muted, camera off, speaker).
  final bool on;
  final bool rotate;

  @override
  Widget build(BuildContext context) {
    final background = bg ?? (on ? _fg : Colors.white.withValues(alpha: 0.14));
    final fg = bg != null ? Colors.white : (on ? const Color(0xFF0B1210) : _fg);
    Widget glyph = AppIcon(icon, size: 26, color: fg);
    if (rotate) glyph = Transform.rotate(angle: 2.356, child: glyph); // 135°: "hang up"
    return Opacity(
      opacity: onTap == null ? 0.45 : 1,
      child: Semantics(
        button: true,
        label: label,
        child: GestureDetector(
          onTap: onTap,
          behavior: HitTestBehavior.opaque,
          child: SizedBox(
            width: 76,
            child: Column(mainAxisSize: MainAxisSize.min, children: [
              Container(
                width: 62,
                height: 62,
                decoration: BoxDecoration(color: background, shape: BoxShape.circle),
                child: Center(child: glyph),
              ),
              const SizedBox(height: 8),
              Text(label, maxLines: 1, overflow: TextOverflow.ellipsis, textAlign: TextAlign.center, style: ts(12, color: _fg)),
            ]),
          ),
        ),
      ),
    );
  }
}
