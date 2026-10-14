// OpenStreetMap maps (flutter_map): the small map of location bubbles and the location picker.

import 'dart:async';

import 'package:flutter/material.dart';
import 'package:flutter_map/flutter_map.dart';
import 'package:geolocator/geolocator.dart';
import 'package:latlong2/latlong.dart';

import '../../core/models.dart';
import '../../l10n/i18n.dart';
import '../../state/actions.dart';
import '../../theme/app_theme.dart';
import '../../widgets/app_icon.dart';
import '../../widgets/ui.dart';

/// Map tiles. The public OpenStreetMap servers have a usage policy; production builds should
/// point to their own tile server: `--dart-define=CHATAPP_TILE_URL=https://tiles.example.org/{z}/{x}/{y}.png`.
const _tileUrl = String.fromEnvironment('CHATAPP_TILE_URL', defaultValue: 'https://tile.openstreetmap.org/{z}/{x}/{y}.png');
const _userAgent = 'pm.dialog.chat';

/// The design's pin: a white disc with the "mappin" icon.
class MapPin extends StatelessWidget {
  const MapPin({super.key});

  @override
  Widget build(BuildContext context) => Container(
    width: 40,
    height: 40,
    decoration: BoxDecoration(
      color: context.tk.pane,
      shape: BoxShape.circle,
      boxShadow: const [BoxShadow(color: Color.fromRGBO(0, 0, 0, .15), blurRadius: 12, offset: Offset(0, 4))],
    ),
    child: const Center(child: AppIcon('mappin', size: 22, color: Color(0xFFE24B3B))),
  );
}

/// Grid background shown under the tiles (--map-bg).
class _MapGrid extends CustomPainter {
  const _MapGrid(this.base);
  final Color base;

  @override
  void paint(Canvas canvas, Size size) {
    canvas.drawRect(Offset.zero & size, Paint()..color = base);
    final p = Paint()
      ..color = const Color.fromRGBO(128, 128, 128, .16)
      ..strokeWidth = 1;
    for (double x = 21.5; x < size.width; x += 22) {
      canvas.drawLine(Offset(x, 0), Offset(x, size.height), p);
    }
    for (double y = 21.5; y < size.height; y += 22) {
      canvas.drawLine(Offset(0, y), Offset(size.width, y), p);
    }
  }

  @override
  bool shouldRepaint(_MapGrid old) => old.base != base;
}

/// Small, non-interactive map for a location bubble.
class LocationMap extends StatelessWidget {
  const LocationMap({super.key, required this.lat, required this.lng});
  final double lat;
  final double lng;

  @override
  Widget build(BuildContext context) {
    final t = context.tk;
    final center = LatLng(lat, lng);
    return CustomPaint(
      painter: _MapGrid(t.mapBg),
      child: FlutterMap(
        options: MapOptions(
          initialCenter: center,
          initialZoom: 15,
          interactionOptions: const InteractionOptions(flags: InteractiveFlag.none),
          backgroundColor: Colors.transparent,
        ),
        children: [
          TileLayer(urlTemplate: _tileUrl, userAgentPackageName: _userAgent),
          MarkerLayer(
            markers: [Marker(point: center, width: 40, height: 40, child: const MapPin())],
          ),
        ],
      ),
    );
  }
}

Future<LatLng?> currentPosition() async {
  try {
    if (!await Geolocator.isLocationServiceEnabled()) return null;
    var perm = await Geolocator.checkPermission();
    if (perm == LocationPermission.denied) perm = await Geolocator.requestPermission();
    if (perm == LocationPermission.denied || perm == LocationPermission.deniedForever) return null;
    final pos = await Geolocator.getCurrentPosition(
      locationSettings: const LocationSettings(accuracy: LocationAccuracy.high, timeLimit: Duration(seconds: 10)),
    );
    return LatLng(pos.latitude, pos.longitude);
  } catch (_) {
    return null;
  }
}

class LocationPickerPage extends StatefulWidget {
  const LocationPickerPage({super.key, required this.conversationId});
  final String conversationId;

  @override
  State<LocationPickerPage> createState() => _LocationPickerPageState();
}

class _LocationPickerPageState extends State<LocationPickerPage> {
  final _map = MapController();
  final _name = TextEditingController();
  LatLng? _point;
  bool _locating = false;
  bool _sending = false;
  String? _geoError;
  // Tiles come from a third-party server that would learn the area (and our IP): the map is
  // loaded only when asked. "Use my location" works without it.
  bool _showMap = false;

  @override
  void initState() {
    super.initState();
    // Try the current position once when the picker opens.
    currentPosition().then((p) {
      if (!mounted || p == null) return;
      setState(() => _point ??= p);
      if (_showMap) _map.move(p, 15);
    });
  }

  @override
  void dispose() {
    _name.dispose();
    super.dispose();
  }

  Future<void> _locate() async {
    setState(() {
      _locating = true;
      _geoError = null;
    });
    final p = await currentPosition();
    if (!mounted) return;
    setState(() {
      _locating = false;
      if (p == null) {
        _geoError = context.t('location.unavailable');
      } else {
        _point = p;
      }
    });
    if (p != null && _showMap) _map.move(p, 15);
  }

  Future<void> _send() async {
    final p = _point;
    if (p == null) return;
    setState(() => _sending = true);
    final name = _name.text.trim();
    await sendMessage(
      conversationId: widget.conversationId,
      location: Location(lat: p.latitude, lng: p.longitude, name: name.isEmpty ? null : name),
    );
    if (mounted) Navigator.of(context).pop();
  }

  @override
  Widget build(BuildContext context) {
    final t = context.tk;
    final p = _point;
    return PaneScaffold(
      header: PaneHeader(title: context.t('location.title')),
      body: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          Padding(
            padding: const EdgeInsets.fromLTRB(16, 12, 16, 10),
            child: Text(context.t('mobile.locationHint'), style: ts(13, color: t.muted, height: 1.45)),
          ),
          Expanded(
            child: !_showMap
                ? CustomPaint(
                    painter: _MapGrid(t.mapBg),
                    child: Center(
                      child: Padding(
                        padding: const EdgeInsets.all(24),
                        child: Column(
                          mainAxisSize: MainAxisSize.min,
                          children: [
                            AppIcon('mappin', size: 26, color: t.muted),
                            const SizedBox(height: 10),
                            Text(
                              context.t('mobile.mapPrivacyHint'),
                              textAlign: TextAlign.center,
                              style: ts(13, color: t.muted, height: 1.45),
                            ),
                            const SizedBox(height: 14),
                            Btn(
                              label: context.t('location.showMap'),
                              kind: BtnKind.secondary,
                              height: 42,
                              expand: false,
                              onPressed: () => setState(() => _showMap = true),
                            ),
                          ],
                        ),
                      ),
                    ),
                  )
                : CustomPaint(
                    painter: _MapGrid(t.mapBg),
                    child: FlutterMap(
                      mapController: _map,
                      options: MapOptions(
                        initialCenter: p ?? const LatLng(48.2, 12.5),
                        initialZoom: p == null ? 4 : 15,
                        backgroundColor: Colors.transparent,
                        onTap: (_, ll) => setState(() => _point = ll),
                      ),
                      children: [
                        TileLayer(urlTemplate: _tileUrl, userAgentPackageName: _userAgent),
                        if (p != null)
                          MarkerLayer(
                            markers: [Marker(point: p, width: 40, height: 40, child: const MapPin())],
                          ),
                        const RichAttributionWidget(attributions: [TextSourceAttribution('OpenStreetMap contributors')]),
                      ],
                    ),
                  ),
          ),
          Padding(
            padding: EdgeInsets.fromLTRB(16, 12, 16, 12 + MediaQuery.of(context).padding.bottom),
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.stretch,
              children: [
                Btn(label: context.t('location.useCurrent'), icon: 'locate', kind: BtnKind.soft, height: 44, busy: _locating, onPressed: _locate),
                if (_geoError != null) ...[const SizedBox(height: 8), Text(_geoError!, style: ts(13, color: t.sysFg))],
                const SizedBox(height: 12),
                AppField(controller: _name, label: context.t('location.name'), hint: context.t('location.namePlaceholder'), maxLength: 80),
                const SizedBox(height: 12),
                Row(
                  children: [
                    Expanded(
                      child: Text(
                        p != null ? '${p.latitude.toStringAsFixed(5)}, ${p.longitude.toStringAsFixed(5)}' : context.t('location.noPoint'),
                        style: ts(12, color: t.muted, mono: p != null),
                      ),
                    ),
                    Btn(label: context.t('location.send'), icon: 'send', height: 44, expand: false, busy: _sending, onPressed: p == null ? null : _send),
                  ],
                ),
              ],
            ),
          ),
        ],
      ),
    );
  }
}
