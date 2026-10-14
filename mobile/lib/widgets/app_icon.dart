// Icon set of the Dialog design (docs/design/Icon.dc.html), drawn with a CustomPainter from the
// same SVG path data: 24x24 viewBox, round caps and joins, stroke 1.8 by default. A "*" prefix
// draws a dot (stroke 2.8), a "#" prefix a filled shape. A few extra icons (marked below) replace
// the Tabler icons used by the desktop app, drawn in the same style.

import 'package:flutter/widgets.dart';
import 'package:path_parsing/path_parsing.dart';

const Map<String, List<String>> kIconPaths = {
  'search': ['M11 4a7 7 0 1 0 0 14a7 7 0 0 0 0-14z', 'M20 20l-3.5-3.5'],
  'edit': ['M12 20h9', 'M16.5 3.5a2.1 2.1 0 0 1 3 3L7 19l-4 1 1-4z'],
  'more': ['*M12 5h0', '*M12 12h0', '*M12 19h0'],
  'back': ['M19 12H5', 'M12 19l-7-7 7-7'],
  'x': ['M18 6L6 18', 'M6 6l12 12'],
  'clip': ['M21.4 11.1l-9.2 9.2a6 6 0 0 1-8.5-8.5l9.2-9.2a4 4 0 0 1 5.7 5.7l-9.2 9.2a2 2 0 0 1-2.8-2.8l8.5-8.5'],
  'sparkles': ['M12 3l1.6 4.4L18 9l-4.4 1.6L12 15l-1.6-4.4L6 9l4.4-1.6z', 'M19 15l.8 2.2L22 18l-2.2.8L19 21l-.8-2.2L16 18l2.2-.8z', 'M5 3l.6 1.4L7 5l-1.4.6L5 7l-.6-1.4L3 5l1.4-.6z'],
  'smile': ['M12 3a9 9 0 1 0 0 18a9 9 0 0 0 0-18z', 'M8 14s1.5 2 4 2 4-2 4-2', '*M9 9.5h0', '*M15 9.5h0'],
  'mic': ['M12 2a3 3 0 0 0-3 3v7a3 3 0 0 0 6 0V5a3 3 0 0 0-3-3z', 'M19 10v2a7 7 0 0 1-14 0v-2', 'M12 19v3'],
  'send': ['M22 2L11 13', 'M22 2l-7 20-4-9-9-4z'],
  'lock': ['M5 11h14v10H5z', 'M8 11V7a4 4 0 0 1 8 0v4'],
  'shield': ['M12 22s8-4 8-10V5l-8-3-8 3v7c0 6 8 10 8 10z', 'M9 12l2 2 4-4'],
  'qr': ['M3 3h7v7H3z', 'M14 3h7v7h-7z', 'M3 14h7v7H3z', 'M14 14h3v3h-3z', 'M21 14v.01', 'M14 21h.01', 'M17 17h4v4'],
  'gear': [
    'M12 9a3 3 0 1 0 0 6a3 3 0 0 0 0-6z',
    'M19.4 15a1.7 1.7 0 0 0 .3 1.8l.1.1a2 2 0 1 1-2.8 2.8l-.1-.1a1.7 1.7 0 0 0-1.8-.3 1.7 1.7 0 0 0-1 1.5V21a2 2 0 1 1-4 0v-.1a1.7 1.7 0 0 0-1.1-1.5 1.7 1.7 0 0 0-1.8.3l-.1.1a2 2 0 1 1-2.8-2.8l.1-.1a1.7 1.7 0 0 0 .3-1.8 1.7 1.7 0 0 0-1.5-1H3a2 2 0 1 1 0-4h.1a1.7 1.7 0 0 0 1.5-1.1 1.7 1.7 0 0 0-.3-1.8l-.1-.1a2 2 0 1 1 2.8-2.8l.1.1a1.7 1.7 0 0 0 1.8.3H9a1.7 1.7 0 0 0 1-1.5V3a2 2 0 1 1 4 0v.1a1.7 1.7 0 0 0 1 1.5 1.7 1.7 0 0 0 1.8-.3l.1-.1a2 2 0 1 1 2.8 2.8l-.1.1a1.7 1.7 0 0 0-.3 1.8V9a1.7 1.7 0 0 0 1.5 1H21a2 2 0 1 1 0 4h-.1a1.7 1.7 0 0 0-1.5 1z',
  ],
  'chat': ['M21 12a8 8 0 0 1-11.6 7.1L4 20l1-4.6A8 8 0 1 1 21 12z'],
  'check': ['M5 12l5 5L20 7'],
  'checks': ['M2 12.5l5 5L17.5 7', 'M13 16.5l1 1L22 7'],
  'pin': ['M12 17v5', 'M9 3h6l-1 5 3 3v2H7v-2l3-3z'],
  'mute': ['M8.7 3.2A6 6 0 0 1 18 8c0 3 .7 5 1.4 6.3', 'M6.3 6.3A6 6 0 0 0 6 8c0 7-3 9-3 9h14', 'M10.3 21a1.9 1.9 0 0 0 3.4 0', 'M2 2l20 20'],
  'bell': ['M6 8a6 6 0 0 1 12 0c0 7 3 9 3 9H3s3-2 3-9', 'M10.3 21a1.9 1.9 0 0 0 3.4 0'],
  'play': ['#M8 5.5v13l10.5-6.5z'],
  'pause': ['#M7 5h3.5v14H7z', '#M13.5 5H17v14h-3.5z'],
  'mappin': ['M12 22s7-6.2 7-12a7 7 0 0 0-14 0c0 5.8 7 12 7 12z', 'M12 7.5a2.5 2.5 0 1 0 0 5a2.5 2.5 0 0 0 0-5z'],
  'external': ['M15 3h6v6', 'M10 14L21 3', 'M18 13v6a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2V8a2 2 0 0 1 2-2h6'],
  'globe': ['M12 3a9 9 0 1 0 0 18a9 9 0 0 0 0-18z', 'M3 12h18', 'M12 3a14 14 0 0 1 0 18', 'M12 3a14 14 0 0 0 0 18'],
  'server': ['M3 4h18v7H3z', 'M3 13h18v7H3z', '*M7 7.5h0', '*M7 16.5h0'],
  'copy': ['M9 9h11v11H9z', 'M5 15H4V4h11v1'],
  'camera': ['M3 8h4l2-3h6l2 3h4v12H3z', 'M12 10a3.5 3.5 0 1 0 0 7a3.5 3.5 0 0 0 0-7z'],
  // Disappearing messages (badge on chat avatars).
  'timer': ['M12 3a9 9 0 1 1-8.6 6.3', '*M4.6 6.4h0', '*M7.6 3.9h0', 'M12 8v4.5l3 1.8'],
  'clock': ['M12 3a9 9 0 1 0 0 18a9 9 0 0 0 0-18z', 'M12 7v5l3 2'],
  'chevron': ['M9 6l6 6-6 6'],
  'moon': ['M21 12.8A9 9 0 1 1 11.2 3a7 7 0 0 0 9.8 9.8z'],
  'sun': [
    'M12 8a4 4 0 1 0 0 8a4 4 0 0 0 0-8z',
    'M12 2v2',
    'M12 20v2',
    'M4.9 4.9l1.4 1.4',
    'M17.7 17.7l1.4 1.4',
    'M2 12h2',
    'M20 12h2',
    'M4.9 19.1l1.4-1.4',
    'M17.7 6.3l1.4-1.4',
  ],
  'devices': ['M4 5h13v9H4z', 'M2 18h11', 'M17 9h5v11h-5z'],
  'key': ['M8 15a4 4 0 1 0 0-8a4 4 0 0 0 0 8z', 'M12 11h9', 'M18 11v3', 'M21 11v2'],
  'palette': [
    'M12 3a9 9 0 0 0 0 18c1.4 0 2-1 2-2 0-1.5-1-1.8-1-3 0-1 .8-2 2-2h2a4 4 0 0 0 4-4c0-4-4-7-9-7z',
    '*M7.5 11h0',
    '*M10 7h0',
    '*M15 7.5h0',
  ],
  'lang': ['M4 5h9', 'M8.5 3v2', 'M11 5c-1 4-4 7-7 8', 'M6 9c1 2 3 3.5 5 4', 'M13 21l4-9 4 9', 'M14.5 18h5'],
  'info': ['M12 3a9 9 0 1 0 0 18a9 9 0 0 0 0-18z', 'M12 11v5', '*M12 8h0'],
  'logout': ['M9 21H5a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h4', 'M16 17l5-5-5-5', 'M21 12H9'],
  'users': ['M9 11a4 4 0 1 0 0-8a4 4 0 0 0 0 8z', 'M2 21v-1a6 6 0 0 1 12 0v1', 'M16 3.5a4 4 0 0 1 0 7.5', 'M22 21v-1a6 6 0 0 0-4-5.6'],
  'image': ['M3 4h18v16H3z', 'M8.5 8a1.5 1.5 0 1 0 0 3a1.5 1.5 0 0 0 0-3z', 'M21 15l-5-5L5 20'],
  'forward': ['M15 5l6 6-6 6', 'M21 11H10a6 6 0 0 0-6 6v2'],
  'user': ['M12 11a4 4 0 1 0 0-8a4 4 0 0 0 0 8z', 'M4 21v-1a8 8 0 0 1 16 0v1'],
  'trash': ['M3 6h18', 'M8 6V4h8v2', 'M6 6l1 15h10l1-15'],
  'eye': ['M2 12s3.5-7 10-7 10 7 10 7-3.5 7-10 7S2 12 2 12z', 'M12 9a3 3 0 1 0 0 6a3 3 0 0 0 0-6z'],
  'eyeoff': [
    'M3 3l18 18',
    'M10.6 5.1A10 10 0 0 1 12 5c6.5 0 10 7 10 7a17 17 0 0 1-3 3.8',
    'M6.6 6.6A17 17 0 0 0 2 12s3.5 7 10 7a9.7 9.7 0 0 0 5.4-1.6',
    'M9.9 9.9a3 3 0 0 0 4.2 4.2',
  ],
  'plug': ['M9 2v6', 'M15 2v6', 'M6 8h12v3a6 6 0 0 1-12 0z', 'M12 17v5'],
  'okcircle': ['M12 3a9 9 0 1 0 0 18a9 9 0 0 0 0-18z', 'M8 12l3 3 5-5'],
  'panel': ['M3 4h18v16H3z', 'M15 4v16'],
  'plus': ['M12 5v14', 'M5 12h14'],
  'scan': ['M3 8V5a2 2 0 0 1 2-2h3', 'M16 3h3a2 2 0 0 1 2 2v3', 'M21 16v3a2 2 0 0 1-2 2h-3', 'M8 21H5a2 2 0 0 1-2-2v-3', 'M7 12h10'],
  'file': ['M14 3H6v18h12V7z', 'M14 3v4h4'],
  'alert': ['M12 3a9 9 0 1 0 0 18a9 9 0 0 0 0-18z', 'M12 7.5v5', '*M12 16h0'],
  'share': ['M12 3v13', 'M7 8l5-5 5 5', 'M5 14v7h14v-7'],
  // --- extra icons (replacements for the Tabler icons of the desktop app) ---
  'archive': ['M3 4h18v4H3z', 'M5 8v11h14V8', 'M10 12h4'],
  'unarchive': ['M3 4h18v4H3z', 'M5 8v11h14V8', 'M12 17v-5', 'M9.5 14.5L12 12l2.5 2.5'],
  'link': ['M10 14a4.5 4.5 0 0 0 6.4 0l3-3a4.5 4.5 0 0 0-6.4-6.4l-1 1', 'M14 10a4.5 4.5 0 0 0-6.4 0l-3 3a4.5 4.5 0 0 0 6.4 6.4l1-1'],
  'star': ['M12 3.5l2.6 5.3 5.8.8-4.2 4.1 1 5.8-5.2-2.7-5.2 2.7 1-5.8-4.2-4.1 5.8-.8z'],
  'staroff': ['M12 3.5l2.6 5.3 5.8.8-4.2 4.1 1 5.8-5.2-2.7-5.2 2.7 1-5.8-4.2-4.1 5.8-.8z', 'M3 3l18 18'],
  'pinoff': ['M12 17v5', 'M9 3h6l-1 5 3 3v2H7v-2l3-3z', 'M3 3l18 18'],
  'ban': ['M12 3a9 9 0 1 0 0 18a9 9 0 0 0 0-18z', 'M5.7 5.7l12.6 12.6'],
  'eraser': ['M19 20H8.5l-4.2-4.3a1 1 0 0 1 0-1.4l9.8-9.8a1 1 0 0 1 1.4 0l4.2 4.3a1 1 0 0 1 0 1.4L12.5 20', 'M18 13.3L10.7 6'],
  'download': ['M4 17v2a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2v-2', 'M7 11l5 5 5-5', 'M12 4v12'],
  'refresh': ['M20 11A8.1 8.1 0 0 0 4.5 9', 'M4 5v4h4', 'M4 13a8.1 8.1 0 0 0 15.5 2', 'M20 19v-4h-4'],
  'reply': ['M9 14L4 9l5-5', 'M4 9h10.5a5.5 5.5 0 0 1 0 11H13'],
  // Calls (same paths as the desktop icon set).
  'video': ['M3 6.5h12v11H3z', 'M15 10.5l6-3.5v10l-6-3.5'],
  'videooff': ['M3 3l18 18', 'M7 6.5h8v8', 'M15 17.5H3v-11', 'M15 10.5l6-3.5v10l-6-3.5'],
  'micoff': ['M3 3l18 18', 'M9 9v3a3 3 0 0 0 5.1 2.1', 'M15 9.3V5a3 3 0 0 0-5.9-.7', 'M19 10v2a7 7 0 0 1-1.2 3.9', 'M5 10v2a7 7 0 0 0 11.5 5.4', 'M12 19v3'],
  'speaker': ['M4 9h4l5-4v14l-5-4H4z', 'M16.5 8.5a5 5 0 0 1 0 7', 'M19 6a8.5 8.5 0 0 1 0 12'],
  'phone': ['M5 4h4l2 5-2.5 1.5a11 11 0 0 0 5 5L15 13l5 2v4a2 2 0 0 1-2 2A16 16 0 0 1 3 6a2 2 0 0 1 2-2'],
  'crown': ['M12 6l4 6 5-4-2 10H5L3 8l5 4z'],
  'shieldoff': ['M12 22s8-4 8-10V5l-8-3-8 3v7c0 6 8 10 8 10z', 'M3 3l18 18'],
  'userminus': ['M9 11a4 4 0 1 0 0-8a4 4 0 0 0 0 8z', 'M2 21v-1a6 6 0 0 1 12 0v1', 'M16 11h6'],
  'movie': ['M4 4h16v16H4z', 'M8 4v16', 'M16 4v16', 'M4 8h4', 'M4 16h4', 'M4 12h16', 'M16 8h4', 'M16 16h4'],
  'locate': ['M12 8a4 4 0 1 0 0 8a4 4 0 0 0 0-8z', 'M12 2v3', 'M12 19v3', 'M2 12h3', 'M19 12h3'],
  'book': ['M6 3h12a2 2 0 0 1 2 2v14a2 2 0 0 1-2 2H6z', 'M10 16h6', 'M13 11a2 2 0 1 0 0-4a2 2 0 0 0 0 4z', 'M4 8h3', 'M4 12h3', 'M4 16h3'],
  'wifioff': ['M3 3l18 18', 'M8.5 16.5a5 5 0 0 1 7 0', 'M5 12.9a10 10 0 0 1 5.2-2.8', 'M2 8.8a15 15 0 0 1 4.2-2.7', 'M13.8 10.2a10 10 0 0 1 5.2 2.7', 'M10.6 5.1A15 15 0 0 1 22 8.8', '*M12 20h0'],
  'fingerprint': ['M12 11v3', 'M8 11a4 4 0 0 1 8 0v2a8 8 0 0 1-1 4', 'M5 13v-2a7 7 0 0 1 14 0', 'M12 17v4', 'M8.5 15a8 8 0 0 0 .5 5'],
};

enum _Kind { stroke, dot, fill }

class _Part {
  final _Kind kind;
  final Path path;
  final Offset dot;
  const _Part(this.kind, this.path, [this.dot = Offset.zero]);
}

class _PathBuilder extends PathProxy {
  final Path path = Path();
  @override
  void close() => path.close();
  @override
  void cubicTo(double x1, double y1, double x2, double y2, double x3, double y3) => path.cubicTo(x1, y1, x2, y2, x3, y3);
  @override
  void lineTo(double x, double y) => path.lineTo(x, y);
  @override
  void moveTo(double x, double y) => path.moveTo(x, y);
}

final Map<String, List<_Part>> _cache = {};
final _dotRe = RegExp(r'M\s*([-\d.]+)[ ,]+([-\d.]+)');

List<_Part> _parts(String name) => _cache.putIfAbsent(name, () {
      final out = <_Part>[];
      for (final raw in kIconPaths[name] ?? const <String>[]) {
        final kind = raw.startsWith('*') ? _Kind.dot : raw.startsWith('#') ? _Kind.fill : _Kind.stroke;
        final d = kind == _Kind.stroke ? raw : raw.substring(1);
        if (kind == _Kind.dot) {
          final m = _dotRe.firstMatch(d);
          out.add(_Part(kind, Path(), Offset(double.parse(m!.group(1)!), double.parse(m.group(2)!))));
          continue;
        }
        final b = _PathBuilder();
        writeSvgPathDataToPath(d, b);
        out.add(_Part(kind, b.path));
      }
      return out;
    });

/// A design icon. The color defaults to the ambient [IconTheme] / [DefaultTextStyle] color.
class AppIcon extends StatelessWidget {
  const AppIcon(this.name, {super.key, this.size = 20, this.color, this.strokeWidth = 1.8, this.semanticLabel});

  final String name;
  final double size;
  final Color? color;
  final double strokeWidth;
  final String? semanticLabel;

  @override
  Widget build(BuildContext context) {
    final c = color ?? IconTheme.of(context).color ?? DefaultTextStyle.of(context).style.color ?? const Color(0xFF000000);
    final icon = SizedBox.square(
      dimension: size,
      child: CustomPaint(painter: _IconPainter(name, c, strokeWidth)),
    );
    if (semanticLabel == null) return ExcludeSemantics(child: icon);
    return Semantics(label: semanticLabel, image: true, child: icon);
  }
}

class _IconPainter extends CustomPainter {
  _IconPainter(this.name, this.color, this.strokeWidth);

  final String name;
  final Color color;
  final double strokeWidth;

  @override
  void paint(Canvas canvas, Size size) {
    canvas.save();
    canvas.scale(size.width / 24, size.height / 24);
    final stroke = Paint()
      ..color = color
      ..style = PaintingStyle.stroke
      ..strokeWidth = strokeWidth
      ..strokeCap = StrokeCap.round
      ..strokeJoin = StrokeJoin.round
      ..isAntiAlias = true;
    final fill = Paint()
      ..color = color
      ..style = PaintingStyle.fill
      ..isAntiAlias = true;
    for (final p in _parts(name)) {
      switch (p.kind) {
        case _Kind.stroke:
          canvas.drawPath(p.path, stroke);
        case _Kind.fill:
          canvas.drawPath(p.path, fill);
        case _Kind.dot:
          canvas.drawCircle(p.dot, 1.4, fill);
      }
    }
    canvas.restore();
  }

  @override
  bool shouldRepaint(_IconPainter old) => old.name != name || old.color != color || old.strokeWidth != strokeWidth;
}
