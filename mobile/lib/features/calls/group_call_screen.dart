// Group call UI (port of app/src/features/calls/GroupCallScreen.tsx): ringing (join / decline),
// then a grid of participants with video tiles or avatars and a speaking ring, plus the
// "call in progress" banner shown in the group conversation.

import 'dart:async';

import 'package:flutter/material.dart';
import 'package:flutter_webrtc/flutter_webrtc.dart';

import '../../core/backend.dart';
import '../../l10n/i18n.dart';
import '../../state/call_settings.dart' show callSettings;
import '../../state/calls.dart' show callState;
import '../../state/group_calls.dart';
import '../../theme/app_theme.dart';
import '../../util/format.dart';
import '../../widgets/app_icon.dart';
import '../../widgets/ui.dart';
import 'call_screen.dart';

const _fg = Color(0xFFF3F7F5);
const _danger = Color(0xFFE5484D);
const _accept = Color(0xFF2FB36D);
const _speakingColor = Color(0xFF2FB36D);

/// The group call screen, drawn above the app while this device rings for or is in a call.
class GroupCallLayer extends StatelessWidget {
  const GroupCallLayer({super.key});

  @override
  Widget build(BuildContext context) {
    return ValueListenableBuilder<GroupCallUi?>(
      valueListenable: groupCallState,
      builder: (context, call, _) => call == null ? const SizedBox.shrink() : Positioned.fill(child: _GroupCallScreen(call: call)),
    );
  }
}

class _GroupCallScreen extends StatefulWidget {
  const _GroupCallScreen({required this.call});
  final GroupCallUi call;

  @override
  State<_GroupCallScreen> createState() => _GroupCallScreenState();
}

class _GroupCallScreenState extends State<_GroupCallScreen> {
  Timer? _ticker;

  @override
  void initState() {
    super.initState();
    _ticker = Timer.periodic(const Duration(milliseconds: 500), (_) {
      if (widget.call.joinedAt != null && mounted) setState(() {});
    });
  }

  @override
  void dispose() {
    _ticker?.cancel();
    super.dispose();
  }

  Widget _background(BuildContext context) => DecoratedBox(
        decoration: BoxDecoration(
          gradient: LinearGradient(
            begin: Alignment.topCenter,
            end: Alignment.bottomCenter,
            colors: [Color.lerp(context.tk.accent, const Color(0xFF0D1714), 0.62)!, const Color(0xFF0B1210)],
          ),
        ),
      );

  Widget _header(BuildContext context, String status, {bool mono = false}) {
    final call = widget.call;
    return Column(mainAxisSize: MainAxisSize.min, children: [
      Row(mainAxisSize: MainAxisSize.min, children: [
        AppIcon(call.video ? 'video' : 'lock', size: 13, color: _fg.withValues(alpha: 0.7)),
        const SizedBox(width: 6),
        Text(context.t('call.encrypted'), style: ts(12, color: _fg.withValues(alpha: 0.7))),
      ]),
      const SizedBox(height: 8),
      Text(call.title, maxLines: 1, overflow: TextOverflow.ellipsis, style: ts(20, weight: 700, color: _fg)),
      const SizedBox(height: 4),
      Text(status, style: ts(14, color: _fg.withValues(alpha: 0.8), mono: mono)),
    ]);
  }

  Widget _incoming(BuildContext context) {
    final call = widget.call;
    return Material(
      color: Colors.black,
      child: Stack(fit: StackFit.expand, children: [
        _background(context),
        SafeArea(
          child: Column(children: [
            Padding(
              padding: const EdgeInsets.fromLTRB(20, 24, 20, 0),
              child: _header(context, context.t(call.video ? 'groupCall.incomingVideo' : 'groupCall.incoming')),
            ),
            Expanded(child: Center(child: Avatar(name: call.title, path: call.avatar, seed: call.conversationId, size: 120, group: true))),
            Padding(
              padding: const EdgeInsets.fromLTRB(12, 0, 12, 28),
              child: Row(mainAxisAlignment: MainAxisAlignment.spaceEvenly, children: [
                CallButton(icon: 'phone', rotate: true, label: context.t('call.decline'), bg: _danger, onTap: () => unawaited(declineGroupCall())),
                CallButton(
                  icon: call.video ? 'video' : 'phone',
                  label: context.t('groupCall.join'),
                  bg: _accept,
                  onTap: () => unawaited(joinGroupCall(call.conversationId)),
                ),
              ]),
            ),
          ]),
        ),
      ]),
    );
  }

  @override
  Widget build(BuildContext context) {
    final call = widget.call;
    if (call.phase == GroupCallPhase.incoming) return _incoming(context);
    final peers = call.peers.values.toList();
    final count = peers.length + 1;
    final status = call.phase == GroupCallPhase.active && call.joinedAt != null
        ? '${formatDuration(DateTime.now().millisecondsSinceEpoch - call.joinedAt!)} · '
            '${context.t('groupCall.participants', args: {'count': count, 'max': call.maxParticipants})}'
        : context.t('call.connecting');
    final tiles = <Widget>[
      _Tile(
        name: context.t('common.you'),
        seed: 'me',
        stream: call.hasCamera && call.cameraOn ? call.localStream : null,
        mirror: true,
        speaking: call.speaking,
        muted: call.muted,
        connected: true,
      ),
      for (final p in peers)
        _Tile(
          name: p.name,
          avatar: p.avatar,
          seed: p.accountId,
          stream: call.video && p.showsVideo ? p.stream : null,
          speaking: p.speaking,
          connected: p.connected,
        ),
    ];
    final columns = count <= 2 ? 1 : 2;
    return Material(
      color: Colors.black,
      child: Stack(fit: StackFit.expand, children: [
        _background(context),
        SafeArea(
          child: Column(children: [
            Padding(padding: const EdgeInsets.fromLTRB(20, 16, 20, 8), child: _header(context, status, mono: call.joinedAt != null)),
            Expanded(
              child: LayoutBuilder(builder: (context, box) {
                final rows = (tiles.length / columns).ceil();
                final width = (box.maxWidth - 24 - (columns - 1) * 8) / columns;
                final height = rows == 0 ? box.maxHeight : ((box.maxHeight - 16 - (rows - 1) * 8) / rows).clamp(120.0, 600.0);
                return GridView.count(
                  padding: const EdgeInsets.symmetric(horizontal: 12, vertical: 8),
                  crossAxisCount: columns,
                  mainAxisSpacing: 8,
                  crossAxisSpacing: 8,
                  childAspectRatio: width / height,
                  children: tiles,
                );
              }),
            ),
            if (call.phase == GroupCallPhase.active && peers.isEmpty)
              Padding(
                padding: const EdgeInsets.only(bottom: 6),
                child: Text(context.t('groupCall.waiting'), style: ts(14, color: _fg.withValues(alpha: 0.75))),
              ),
            Padding(
              padding: const EdgeInsets.fromLTRB(8, 12, 8, 24),
              child: Row(mainAxisAlignment: MainAxisAlignment.spaceEvenly, children: [
                CallButton(
                  icon: call.muted ? 'micoff' : 'mic',
                  label: context.t(call.muted ? 'call.unmute' : 'call.mute'),
                  on: call.muted,
                  onTap: toggleGroupMute,
                ),
                if (call.video)
                  CallButton(
                    icon: call.cameraOn ? 'video' : 'videooff',
                    label: context.t(call.cameraOn ? 'call.cameraOff' : 'call.cameraOn'),
                    on: !call.cameraOn,
                    onTap: call.hasCamera ? toggleGroupCamera : null,
                  ),
                if (call.video && call.hasCamera && call.cameraOn)
                  CallButton(icon: 'refresh', label: context.t('mobile.call.flip'), onTap: () => unawaited(switchGroupCamera())),
                if (!call.video)
                  CallButton(icon: 'speaker', label: context.t('mobile.call.speaker'), on: call.speaker, onTap: () => unawaited(toggleGroupSpeaker())),
                VoiceFxButton(
                  effect: call.voiceEffect,
                  denoise: call.denoise,
                  enabled: call.phase == GroupCallPhase.active,
                  onChange: ({effect, denoise}) => unawaited(setGroupVoiceEffect(effect: effect, denoise: denoise)),
                ),
                CallButton(icon: 'phone', rotate: true, label: context.t('groupCall.leave'), bg: _danger, onTap: () => unawaited(leaveGroupCall())),
              ]),
            ),
          ]),
        ),
      ]),
    );
  }
}

class _Tile extends StatelessWidget {
  const _Tile({
    required this.name,
    this.avatar,
    required this.seed,
    this.stream,
    this.mirror = false,
    required this.speaking,
    this.muted = false,
    required this.connected,
  });

  final String name;
  final String? avatar;
  final String seed;
  final MediaStream? stream;
  final bool mirror;
  final bool speaking;
  final bool muted;
  final bool connected;

  @override
  Widget build(BuildContext context) {
    return Container(
      decoration: BoxDecoration(
        color: const Color(0xFF121A17),
        borderRadius: BorderRadius.circular(16),
        border: Border.all(color: speaking ? _speakingColor : Colors.transparent, width: 3),
      ),
      clipBehavior: Clip.antiAlias,
      child: Stack(fit: StackFit.expand, children: [
        if (stream != null)
          CallStreamView(stream: stream!, mirror: mirror, cover: true)
        else
          Center(child: Avatar(name: name, path: avatar, seed: seed, size: 64)),
        Positioned(
          left: 8,
          bottom: 8,
          right: 8,
          child: Align(
            alignment: Alignment.centerLeft,
            child: Container(
              padding: const EdgeInsets.symmetric(horizontal: 8, vertical: 3),
              decoration: BoxDecoration(color: Colors.black.withValues(alpha: 0.55), borderRadius: BorderRadius.circular(8)),
              child: Row(mainAxisSize: MainAxisSize.min, children: [
                Flexible(child: Text(name, maxLines: 1, overflow: TextOverflow.ellipsis, style: ts(12.5, color: _fg))),
                if (muted) ...[const SizedBox(width: 6), const AppIcon('micoff', size: 13, color: _fg)],
                if (!connected) ...[
                  const SizedBox(width: 6),
                  Text(context.t('call.connecting'), style: ts(11.5, color: _fg.withValues(alpha: 0.7))),
                ],
              ]),
            ),
          ),
        ),
      ]),
    );
  }
}

/// "Call in progress — Join" at the top of a group conversation.
class GroupCallBanner extends StatefulWidget {
  const GroupCallBanner({super.key, required this.conversationId});
  final String conversationId;

  @override
  State<GroupCallBanner> createState() => _GroupCallBannerState();
}

class _GroupCallBannerState extends State<GroupCallBanner> {
  @override
  void initState() {
    super.initState();
    unawaited(refreshGroupCall(widget.conversationId));
  }

  @override
  void didUpdateWidget(GroupCallBanner old) {
    super.didUpdateWidget(old);
    if (old.conversationId != widget.conversationId) unawaited(refreshGroupCall(widget.conversationId));
  }

  @override
  Widget build(BuildContext context) {
    final t = context.tk;
    return ValueListenableBuilder<Map<String, GroupCallInfo>>(
      valueListenable: groupCallsOngoing,
      builder: (context, ongoing, _) => ValueListenableBuilder<GroupCallUi?>(
        valueListenable: groupCallState,
        builder: (context, mine, _) => ValueListenableBuilder<CallSettings>(
          valueListenable: callSettings,
          builder: (context, settings, _) {
          final info = ongoing[widget.conversationId];
          if (info == null || info.ended || info.joined || mine?.conversationId == widget.conversationId) {
            return const SizedBox.shrink();
          }
          final count = info.participants.length;
          final full = count >= info.maxParticipants;
          final busy = callState.value != null || mine != null;
          // Calls turned off in Settings → Privacy: the call is shown but cannot be joined.
          final off = !settings.allowCalls;
          return Material(
            color: t.accent.withValues(alpha: 0.12),
            child: InkWell(
              onTap: full || busy || off
                  ? null
                  : () => unawaited(joinGroupCall(widget.conversationId, video: info.video && count < videoSuggestedMax)),
              child: Padding(
                padding: const EdgeInsets.symmetric(horizontal: 16, vertical: 10),
                child: Row(children: [
                  AppIcon(info.video ? 'video' : 'phone', size: 17, color: t.accentText),
                  const SizedBox(width: 10),
                  Expanded(
                    child: Column(crossAxisAlignment: CrossAxisAlignment.start, children: [
                      Text(
                        full ? context.t('groupCall.bannerFull') : context.t('groupCall.banner', count: count),
                        style: ts(14, color: t.ink),
                      ),
                      if (off) Text(context.t('call.callsOffBanner'), style: ts(12.5, color: t.muted)),
                    ]),
                  ),
                  if (!full && !off) Text(context.t('groupCall.join'), style: ts(14, weight: 700, color: t.accentText)),
                ]),
              ),
            ),
          );
          },
        ),
      ),
    );
  }
}
