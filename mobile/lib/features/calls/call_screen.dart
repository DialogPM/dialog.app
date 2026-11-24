// Full-screen call UI drawn above the whole app (port of app/src/features/calls/CallScreen.tsx):
// incoming (accept / decline), outgoing, connecting, active. Video calls show the peer's video
// full-screen and our camera picture-in-picture; the peer's shared screen is shown whole, with
// pinch-to-zoom (text on a computer screen is small on a phone).

import 'dart:async';

import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:flutter_webrtc/flutter_webrtc.dart';

import '../../app/globals.dart';
import '../../l10n/i18n.dart';
import '../../services/voice_fx.dart';
import '../../state/call_audio.dart';
import '../../state/calls.dart';
import '../../state/group_calls.dart';
import '../../theme/app_theme.dart';
import '../../util/format.dart';
import '../../widgets/app_icon.dart';
import '../../widgets/official_badge.dart';
import '../../widgets/ui.dart';
import 'group_call_screen.dart';

const _fg = Color(0xFFF3F7F5);
const _danger = Color(0xFFE5484D);
const _accept = Color(0xFF2FB36D);

/// Shows the call screen on top of [child] while there is a call.
class CallOverlay extends StatelessWidget {
  const CallOverlay({super.key, required this.child});
  final Widget child;

  /// Height of the ongoing-call bar below the status bar.
  static const barHeight = 52.0;

  @override
  Widget build(BuildContext context) {
    // While a call screen is shown nothing underneath may hold the focus: the keyboard of a
    // chat composer would otherwise stay open over the answer / hang-up buttons. A minimized
    // call leaves the app usable, pushed down below the ongoing-call bar (the same widgets
    // either way, so the app keeps its state when the call is minimized or reopened).
    return ListenableBuilder(
      listenable: Listenable.merge([callState, groupCallState, callMinimized]),
      child: child,
      builder: (context, inner) {
        final call = callState.value;
        final mini = call != null && callMinimized.value && call.phase != CallPhase.incoming;
        final full = call != null && !mini;
        final mq = MediaQuery.of(context);
        final top = mini ? mq.padding.top + barHeight : 0.0;
        return Stack(children: [
          Positioned.fill(
            child: ExcludeFocus(
              excluding: full || groupCallState.value != null,
              child: Padding(
                padding: EdgeInsets.only(top: top),
                child: MediaQuery(data: mini ? mq.removePadding(removeTop: true) : mq, child: inner!),
              ),
            ),
          ),
          if (full) Positioned.fill(child: _CallScreen(call: call)),
          if (mini) Positioned(top: 0, left: 0, right: 0, height: top, child: _OngoingCallBar(call: call)),
          // Group calls (group_call_screen.dart).
          const GroupCallLayer(),
          // Voice effects panel of either call screen.
          const VoiceFxPanelLayer(),
        ]);
      },
    );
  }
}

/// Bar on top of the app while a call is minimized (like WhatsApp's): who, how long, and
/// mute / speaker / hang up. Tapping it reopens the call screen.
class _OngoingCallBar extends StatefulWidget {
  const _OngoingCallBar({required this.call});
  final CallState call;

  @override
  State<_OngoingCallBar> createState() => _OngoingCallBarState();
}

class _OngoingCallBarState extends State<_OngoingCallBar> {
  Timer? _ticker;

  @override
  void initState() {
    super.initState();
    _ticker = Timer.periodic(const Duration(seconds: 1), (_) {
      if (mounted) setState(() {});
    });
  }

  @override
  void dispose() {
    _ticker?.cancel();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    final call = widget.call;
    final status = call.phase == CallPhase.active && call.connectedAt != null
        ? formatDuration(DateTime.now().millisecondsSinceEpoch - call.connectedAt!)
        : context.t(call.phase == CallPhase.connecting ? 'call.connecting' : (call.ringing ? 'call.ringing' : 'call.calling'));
    Widget icon(String name, VoidCallback onTap, {Color? bg, bool rotate = false, String? label}) => Semantics(
          button: true,
          label: label,
          child: InkResponse(
            onTap: onTap,
            radius: 22,
            child: Container(
              width: 36,
              height: 36,
              decoration: BoxDecoration(color: bg ?? Colors.white.withValues(alpha: 0.14), shape: BoxShape.circle),
              alignment: Alignment.center,
              child: Transform.rotate(angle: rotate ? 2.36 : 0, child: AppIcon(name, size: 18, color: _fg)),
            ),
          ),
        );
    return Material(
      color: Color.lerp(context.tk.accent, const Color(0xFF0B1210), 0.55),
      child: SafeArea(
        bottom: false,
        child: SizedBox(
          height: CallOverlay.barHeight,
          child: Row(children: [
            Expanded(
              child: InkWell(
                onTap: () => callMinimized.value = false,
                child: Padding(
                  padding: const EdgeInsets.symmetric(horizontal: 16),
                  child: Row(children: [
                    AppIcon(call.video ? 'video' : 'phone', size: 18, color: _fg),
                    const SizedBox(width: 10),
                    Flexible(
                      child: Column(mainAxisAlignment: MainAxisAlignment.center, crossAxisAlignment: CrossAxisAlignment.start, children: [
                        Text(call.peerName, maxLines: 1, overflow: TextOverflow.ellipsis, style: ts(14.5, weight: 700, color: _fg)),
                        Text('$status · ${context.t('mobile.call.tapToReturn')}',
                            maxLines: 1, overflow: TextOverflow.ellipsis, style: ts(12, color: _fg.withValues(alpha: 0.8))),
                      ]),
                    ),
                  ]),
                ),
              ),
            ),
            icon(call.muted ? 'micoff' : 'mic', toggleMute, bg: call.muted ? Colors.white.withValues(alpha: 0.32) : null,
                label: context.t(call.muted ? 'call.unmute' : 'call.mute')),
            const SizedBox(width: 8),
            if (!call.video) ...[
              icon('speaker', () => unawaited(toggleSpeaker()), bg: call.speaker ? Colors.white.withValues(alpha: 0.32) : null,
                  label: context.t('mobile.call.speaker')),
              const SizedBox(width: 8),
            ],
            icon('phone', () => unawaited(hangUp()), bg: _danger, rotate: true, label: context.t('call.end')),
            const SizedBox(width: 12),
          ]),
        ),
      ),
    );
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
  /// Full screen for the peer's video or screen: landscape, nothing else drawn.
  bool _fullscreen = false;
  /// Where the user dragged our camera's picture-in-picture (top-left corner); null = default.
  Offset? _pip;

  static const _pipSize = Size(112, 160);
  static const _pipMargin = 16.0;
  /// Clear of the header on top and the buttons at the bottom.
  static const _pipTop = 110.0;
  static const _pipBottom = 150.0;

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
    if (_fullscreen) unawaited(_applyFullscreen(false));
    super.dispose();
  }

  Future<void> _setFullscreen(bool on) async {
    if (_fullscreen == on) return;
    setState(() => _fullscreen = on);
    await _applyFullscreen(on);
  }

  static Future<void> _applyFullscreen(bool on) async {
    try {
      // Landscape for a computer screen, whatever the phone's rotation lock says; back to
      // the phone's own rotation afterwards.
      await SystemChrome.setPreferredOrientations(on ? const [DeviceOrientation.landscapeLeft, DeviceOrientation.landscapeRight] : const []);
      await SystemChrome.setEnabledSystemUIMode(on ? SystemUiMode.immersiveSticky : SystemUiMode.edgeToEdge);
    } catch (e) {
      debugPrint('fullscreen: $e');
    }
  }

  /// Our camera, draggable; dropped, it snaps to the nearest corner. Kept on screen when the
  /// phone rotates.
  Widget _pipView(BuildContext context, Widget child) {
    final size = MediaQuery.sizeOf(context);
    final pad = MediaQuery.paddingOf(context);
    final left = _pipMargin;
    final right = size.width - _pipMargin - _pipSize.width;
    final top = pad.top + _pipTop;
    final bottom = size.height - _pipBottom - _pipSize.height;
    final raw = _pip ?? Offset(right, bottom);
    final pos = Offset(raw.dx.clamp(left, right), raw.dy.clamp(top, bottom));
    return Positioned(
      left: pos.dx,
      top: pos.dy,
      width: _pipSize.width,
      height: _pipSize.height,
      child: GestureDetector(
        onPanUpdate: (d) => setState(() => _pip = pos + d.delta),
        onPanEnd: (_) => setState(() {
          final c = pos + Offset(_pipSize.width / 2, _pipSize.height / 2);
          _pip = Offset(c.dx < size.width / 2 ? left : right, c.dy < size.height / 2 ? top : bottom);
        }),
        child: ClipRRect(
          borderRadius: BorderRadius.circular(14),
          child: DecoratedBox(decoration: const BoxDecoration(color: Color(0xFF111111)), child: child),
        ),
      ),
    );
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
    // The peer's video is its screen (shown whole) or its camera.
    final screenMode = !incoming && call.remoteScreen;
    final remoteVideo = videoMode && !screenMode && call.remoteStream != null && call.remoteCameraOn && call.remoteStream!.getVideoTracks().isNotEmpty;
    final fullVideo = remoteVideo || screenMode;

    final header = Column(mainAxisSize: MainAxisSize.min, children: [
      Row(mainAxisSize: MainAxisSize.min, children: [
        AppIcon(call.video ? 'video' : 'lock', size: 13, color: _fg.withValues(alpha: 0.7)),
        const SizedBox(width: 6),
        Text(context.t('call.encrypted'), style: ts(12, color: _fg.withValues(alpha: 0.7))),
      ]),
      const SizedBox(height: 10),
      Row(mainAxisSize: MainAxisSize.min, children: [
        Flexible(child: Text(call.peerName, maxLines: 1, overflow: TextOverflow.ellipsis, style: ts(fullVideo ? 19 : 24, weight: 700, color: _fg))),
        // Who is calling: only the operator-signed badge says "official", never the name.
        OfficialBadge(_official(call), size: fullVideo ? 17 : 20, gap: 8),
      ]),
      const SizedBox(height: 4),
      Text(_status(context), style: ts(14, color: _fg.withValues(alpha: 0.8), mono: call.phase == CallPhase.active)),
      _QualityNotice(call: call),
      if (call.screenAsked) ...[
        const SizedBox(height: 6),
        Text(context.t('call.askedScreen', args: {'name': call.peerName}), style: ts(13, color: _fg.withValues(alpha: 0.75))),
      ],
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

    if (_fullscreen && fullVideo) {
      // Only the peer's screen (or camera) and a way out.
      return Material(
        color: Colors.black,
        child: Stack(fit: StackFit.expand, children: [
          InteractiveViewer(
            minScale: 1,
            maxScale: 5,
            child: CallStreamView(stream: call.remoteStream!, version: call.streamVersion),
          ),
          Positioned(
            top: 0,
            right: 0,
            child: SafeArea(
              child: Padding(
                padding: const EdgeInsets.all(6),
                child: IconButton(
                  tooltip: context.t('mobile.call.exitFullscreen'),
                  onPressed: () => unawaited(_setFullscreen(false)),
                  icon: const AppIcon('fullscreenexit', size: 26, color: _fg),
                ),
              ),
            ),
          ),
        ]),
      );
    }
    // The video went away while in full screen: back to the normal layout (after this build).
    if (_fullscreen) WidgetsBinding.instance.addPostFrameCallback((_) => unawaited(_setFullscreen(false)));

    return Material(
      color: Colors.black,
      child: Stack(fit: StackFit.expand, children: [
        if (screenMode)
          // Pinch to zoom into the shared screen.
          InteractiveViewer(
            minScale: 1,
            maxScale: 5,
            child: CallStreamView(stream: call.remoteStream!, version: call.streamVersion),
          )
        else if (remoteVideo)
          CallStreamView(stream: call.remoteStream!, cover: true, version: call.streamVersion)
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
        if (!fullVideo) Center(child: Padding(padding: const EdgeInsets.only(bottom: 60), child: avatar)),
        // Top: name and status, over a scrim when video is shown.
        Positioned(
          top: 0,
          left: 0,
          right: 0,
          child: DecoratedBox(
            decoration: BoxDecoration(
              gradient: fullVideo
                  ? const LinearGradient(begin: Alignment.topCenter, end: Alignment.bottomCenter, colors: [Color(0x99000000), Color(0x00000000)])
                  : null,
            ),
            child: SafeArea(
              bottom: false,
              child: Padding(padding: const EdgeInsets.fromLTRB(20, 24, 20, 36), child: header),
            ),
          ),
        ),
        if (!incoming)
          Positioned(
            top: 0,
            left: 0,
            child: SafeArea(
              bottom: false,
              child: Padding(
                padding: const EdgeInsets.all(6),
                child: IconButton(
                  tooltip: context.t('mobile.call.minimize'),
                  onPressed: () => callMinimized.value = true,
                  icon: Transform.rotate(angle: 1.5708, child: const AppIcon('chevron', size: 26, color: _fg)),
                ),
              ),
            ),
          ),
        // Top right: full screen (landscape) for the peer's video or screen, and ask the peer
        // (a desktop) to share its screen.
        if (!incoming)
          Positioned(
            top: 0,
            right: 0,
            child: SafeArea(
              bottom: false,
              child: Padding(
                padding: const EdgeInsets.all(6),
                child: Row(mainAxisSize: MainAxisSize.min, children: [
                  if (fullVideo)
                    IconButton(
                      tooltip: context.t('mobile.call.fullscreen'),
                      onPressed: () => unawaited(_setFullscreen(true)),
                      icon: const AppIcon('fullscreen', size: 26, color: _fg),
                    ),
                  IconButton(
                    tooltip: context.t('call.askScreen'),
                    onPressed: call.screenAsked || call.remoteScreenOn || call.phase == CallPhase.outgoing ? null : askForScreen,
                    icon: AppIcon('screen', size: 26, color: _fg.withValues(alpha: call.screenAsked || call.remoteScreenOn ? 0.45 : 1)),
                  ),
                ]),
              ),
            ),
          ),
        if (screenMode)
          Positioned(
            left: 16,
            bottom: 150,
            child: DecoratedBox(
              decoration: BoxDecoration(color: Colors.black.withValues(alpha: 0.55), borderRadius: BorderRadius.circular(999)),
              child: Padding(
                padding: const EdgeInsets.symmetric(horizontal: 10, vertical: 6),
                child: Row(mainAxisSize: MainAxisSize.min, children: [
                  const AppIcon('screen', size: 14, color: _fg),
                  const SizedBox(width: 6),
                  Text(context.t('call.peerScreen', args: {'name': call.peerName}), style: ts(12.5, color: _fg)),
                ]),
              ),
            ),
          ),
        if (videoMode && call.hasCamera && call.cameraOn && call.localStream != null)
          _pipView(context, CallStreamView(stream: call.localStream!, mirror: true, cover: true)),
        Positioned(
          left: 0,
          right: 0,
          bottom: 0,
          child: DecoratedBox(
            decoration: BoxDecoration(
              gradient: fullVideo
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
String? _official(CallState call) {
  final conv = appData.conversations[call.conversationId];
  if (conv != null) return conversationOfficial(conv, appData.contacts);
  return appData.contacts[call.peerAccountId]?.official;
}

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
  const CallStreamView({super.key, required this.stream, this.mirror = false, this.cover = false, this.version = 0});
  final MediaStream stream;
  final bool mirror;
  final bool cover;
  /// Changes when a track was added to the same stream: the renderer binds it again.
  final int version;

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
    if (_ready && (old.stream != widget.stream || old.version != widget.version)) _renderer.srcObject = widget.stream;
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
