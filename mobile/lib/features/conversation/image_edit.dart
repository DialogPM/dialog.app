// The image editor's model (image_editor.dart): pen strokes and texts drawn over an image, a
// crop and quarter turns. Everything is kept as data, in pixels of the image as currently shown
// (cropped, then turned), and only baked into a new image when the user saves or sends: the same
// painting code draws the on-screen preview (scaled) and the full-size result.
//
// The desktop app has the same model in app/src/features/conversation/annotate.ts.

import 'dart:math' as math;
import 'dart:typed_data';
import 'dart:ui' as ui;

import 'package:flutter/painting.dart';

/// Colours offered by the editor (the same as on desktop).
const kEditColors = <({String key, Color value})>[
  (key: 'red', value: Color(0xFFEF4444)),
  (key: 'blue', value: Color(0xFF3B82F6)),
  (key: 'green', value: Color(0xFF22C55E)),
  (key: 'black', value: Color(0xFF111111)),
  (key: 'white', value: Color(0xFFFFFFFF)),
];

/// Line thickness (and text size) choices: a factor applied to [strokeFor] / [fontFor].
const kEditSizes = <({String key, double factor})>[
  (key: 'thin', factor: 0.5),
  (key: 'medium', factor: 1),
  (key: 'thick', factor: 2.4),
];

/// Crop aspect ratios (width / height); null = free.
const kEditAspects = <({String key, double? value})>[
  (key: 'free', value: null),
  (key: 'square', value: 1),
  (key: '4:3', value: 4 / 3),
  (key: '3:4', value: 3 / 4),
  (key: '16:9', value: 16 / 9),
];

/// Stroke width in image pixels: 0.5% of the width, so it looks alike on small and large images.
double strokeFor(double imageWidth, [double factor = 1]) => math.max(2, (imageWidth * 0.005 * factor).roundToDouble());

/// Text size in image pixels (larger than on desktop: a phone shows the image much smaller).
double fontFor(double imageWidth, [double factor = 1]) => math.max(18, (imageWidth * 0.05 * factor).roundToDouble());

sealed class EditShape {
  const EditShape();

  /// The shape moved by [d].
  EditShape shifted(Offset d);

  /// The shape after a quarter turn of the image: [dir] 1 = clockwise, -1 = counter-clockwise;
  /// [size] is the image's size before the turn.
  EditShape turned(int dir, Size size);
}

Offset _turnPoint(Offset p, int dir, Size s) => dir == 1 ? Offset(s.height - p.dy, p.dx) : Offset(p.dy, s.width - p.dx);

class PenStroke extends EditShape {
  const PenStroke({required this.color, required this.width, required this.points});
  final Color color;
  final double width;
  final List<Offset> points;

  PenStroke withPoint(Offset p) => PenStroke(color: color, width: width, points: [...points, p]);

  @override
  PenStroke shifted(Offset d) => PenStroke(color: color, width: width, points: [for (final p in points) p + d]);

  @override
  PenStroke turned(int dir, Size size) => PenStroke(color: color, width: width, points: [for (final p in points) _turnPoint(p, dir, size)]);
}

class TextMark extends EditShape {
  const TextMark({required this.color, required this.size, required this.pos, required this.text, this.turns = 0});
  final Color color;
  final double size;

  /// Top-left corner of the text (before its own rotation around that corner).
  final Offset pos;
  final String text;

  /// Clockwise quarter turns, following those of the image.
  final int turns;

  TextPainter get painter => TextPainter(
        text: TextSpan(
          text: text,
          style: TextStyle(
            color: color,
            fontSize: size,
            fontWeight: FontWeight.w700,
            height: 1.15,
            // A dark (or light) halo keeps the text readable on any background.
            shadows: [Shadow(color: color == const Color(0xFF111111) ? const Color(0xD9FFFFFF) : const Color(0xBF000000), blurRadius: size / 10)],
          ),
        ),
        textDirection: TextDirection.ltr,
      )..layout();

  @override
  TextMark shifted(Offset d) => TextMark(color: color, size: size, pos: pos + d, text: text, turns: turns);

  @override
  TextMark turned(int dir, Size size) =>
      TextMark(color: color, size: this.size, pos: _turnPoint(pos, dir, size), text: text, turns: (turns + dir + 4) % 4);

  /// True when [p] falls on the text, whose laid-out size is [box].
  bool hit(Offset p, Size box) {
    // Into the text's own frame: origin at its top-left corner.
    final a = -turns * math.pi / 2;
    final d = p - pos;
    final l = Offset(d.dx * math.cos(a) - d.dy * math.sin(a), d.dx * math.sin(a) + d.dy * math.cos(a));
    final pad = size * 0.35;
    return l.dx >= -pad && l.dx <= box.width + pad && l.dy >= -pad && l.dy <= box.height + pad;
  }
}

/// One undo step.
class EditState {
  const EditState({this.shapes = const [], this.turns = 0, this.crop});
  final List<EditShape> shapes;

  /// Clockwise quarter turns of the image (0-3).
  final int turns;

  /// Region of the original (unrotated) image that is kept; null = all of it.
  final Rect? crop;

  bool get isEmpty => shapes.isEmpty && turns == 0 && crop == null;

  /// Size of the edited image in pixels: the crop (or the whole image), turned.
  Size outputSize(Size image) {
    final s = crop?.size ?? image;
    return turns.isOdd ? Size(s.height, s.width) : s;
  }

  EditState withShape(EditShape s) => EditState(shapes: [...shapes, s], turns: turns, crop: crop);

  EditState replacing(int index, EditShape s) =>
      EditState(shapes: [for (var i = 0; i < shapes.length; i++) i == index ? s : shapes[i]], turns: turns, crop: crop);

  /// A quarter turn of the image and of everything drawn on it.
  EditState rotated(int dir, Size image) {
    final before = outputSize(image);
    return EditState(shapes: [for (final s in shapes) s.turned(dir, before)], turns: (turns + dir + 4) % 4, crop: crop);
  }

  /// Keeps only [frame] (a rectangle of the image as shown); the shapes move along.
  EditState cropped(Rect frame, Size image) {
    final shown = outputSize(image);
    // Whole pixels: a quarter turn maps them onto whole pixels of the original.
    final l = frame.left.round().clamp(0, shown.width.toInt() - 1).toDouble();
    final t = frame.top.round().clamp(0, shown.height.toInt() - 1).toDouble();
    final w = frame.width.round().clamp(1, (shown.width - l).toInt()).toDouble();
    final h = frame.height.round().clamp(1, (shown.height - t).toInt()).toDouble();
    if (w == shown.width && h == shown.height) return this;
    final f = Rect.fromLTWH(l, t, w, h);
    return EditState(
      shapes: [for (final s in shapes) s.shifted(-f.topLeft)],
      turns: turns,
      crop: cropToSource(f, turns, crop ?? (Offset.zero & image)),
    );
  }
}

/// Turns a crop frame drawn on the image as shown (cropped to [crop], then turned [turns] quarter
/// turns clockwise) into a crop of the original, unrotated image.
Rect cropToSource(Rect frame, int turns, Rect crop) {
  // A point of the shown image back in the (cropped) source.
  Offset back(Offset p) => switch (turns % 4) {
        1 => Offset(p.dy, crop.height - p.dx),
        2 => Offset(crop.width - p.dx, crop.height - p.dy),
        3 => Offset(crop.width - p.dy, p.dx),
        _ => p,
      };
  final r = Rect.fromPoints(back(frame.topLeft), back(frame.bottomRight)).intersect(Offset.zero & crop.size);
  return r.shift(crop.topLeft);
}

/// The largest centred rectangle of [aspect] inside [area] (all of it when free).
Rect aspectRect(Size area, double? aspect) {
  if (aspect == null) return Offset.zero & area;
  var w = area.width;
  var h = w / aspect;
  if (h > area.height) {
    h = area.height;
    w = h * aspect;
  }
  return Rect.fromLTWH((area.width - w) / 2, (area.height - h) / 2, w, h);
}

enum CropHandle { topLeft, topRight, bottomLeft, bottomRight, move }

/// The crop frame after dragging [handle] by [d] from [start], kept inside [area], at least
/// [min] on each side and, with [aspect], at that width / height ratio.
Rect dragCrop(Rect start, CropHandle handle, Offset d, Size area, double? aspect, {double min = 16}) {
  if (handle == CropHandle.move) {
    return Rect.fromLTWH(
      (start.left + d.dx).clamp(0, area.width - start.width).toDouble(),
      (start.top + d.dy).clamp(0, area.height - start.height).toDouble(),
      start.width,
      start.height,
    );
  }
  final left = handle == CropHandle.topLeft || handle == CropHandle.bottomLeft;
  final top = handle == CropHandle.topLeft || handle == CropHandle.topRight;
  // The opposite corner stays put.
  final ax = left ? start.right : start.left;
  final ay = top ? start.bottom : start.top;
  final mx = ((left ? start.left : start.right) + d.dx).clamp(0, area.width).toDouble();
  final my = ((top ? start.top : start.bottom) + d.dy).clamp(0, area.height).toDouble();
  final maxW = left ? ax : area.width - ax;
  final maxH = top ? ay : area.height - ay;
  var w = math.min(maxW, math.max(min, left ? ax - mx : mx - ax));
  var h = math.min(maxH, math.max(min, top ? ay - my : my - ay));
  if (aspect != null) {
    h = w / aspect;
    if (h > maxH) {
      h = maxH;
      w = h * aspect;
    }
  }
  return Rect.fromLTWH(left ? ax - w : ax, top ? ay - h : ay, w, h);
}

/// The handle of [frame] under [p] (within [reach]), the frame itself when inside it, else null.
CropHandle? cropHandleAt(Rect frame, Offset p, double reach) {
  final corners = {
    CropHandle.topLeft: frame.topLeft,
    CropHandle.topRight: frame.topRight,
    CropHandle.bottomLeft: frame.bottomLeft,
    CropHandle.bottomRight: frame.bottomRight,
  };
  CropHandle? best;
  var bestD = reach;
  corners.forEach((h, c) {
    final d = (c - p).distance;
    if (d <= bestD) {
      best = h;
      bestD = d;
    }
  });
  return best ?? (frame.contains(p) ? CropHandle.move : null);
}

/// Paints the edited image at the origin: [scale] canvas units per image pixel (1 for the
/// full-size result). [extra] is a shape still being drawn.
void paintEdit(Canvas canvas, ui.Image image, EditState st, {double scale = 1, EditShape? extra, FilterQuality quality = FilterQuality.high}) {
  final full = Size(image.width.toDouble(), image.height.toDouble());
  final src = st.crop ?? (Offset.zero & full);
  final out = st.outputSize(full);
  canvas.save();
  canvas.scale(scale);
  canvas.clipRect(Offset.zero & out);
  // The image, turned around the centre of the output.
  canvas.save();
  canvas.translate(out.width / 2, out.height / 2);
  canvas.rotate(st.turns * math.pi / 2);
  canvas.drawImageRect(
    image,
    src,
    Rect.fromCenter(center: Offset.zero, width: src.width, height: src.height),
    Paint()..filterQuality = quality,
  );
  canvas.restore();
  for (final s in [...st.shapes, ?extra]) {
    switch (s) {
      case PenStroke():
        final paint = Paint()
          ..color = s.color
          ..strokeWidth = s.width
          ..strokeCap = StrokeCap.round
          ..strokeJoin = StrokeJoin.round
          ..isAntiAlias = true;
        if (s.points.length == 1) {
          canvas.drawCircle(s.points.first, s.width / 2, paint..style = PaintingStyle.fill);
        } else if (s.points.isNotEmpty) {
          final path = Path()..moveTo(s.points.first.dx, s.points.first.dy);
          for (final p in s.points.skip(1)) {
            path.lineTo(p.dx, p.dy);
          }
          canvas.drawPath(path, paint..style = PaintingStyle.stroke);
        }
      case TextMark():
        canvas.save();
        canvas.translate(s.pos.dx, s.pos.dy);
        canvas.rotate(s.turns * math.pi / 2);
        final tp = s.painter;
        tp.paint(canvas, Offset.zero);
        tp.dispose();
        canvas.restore();
    }
  }
  canvas.restore();
}

/// Bakes the edit into a new image at the resolution of [image] (never the screen's).
Future<ui.Image> renderEdit(ui.Image image, EditState st) async {
  final out = st.outputSize(Size(image.width.toDouble(), image.height.toDouble()));
  final recorder = ui.PictureRecorder();
  paintEdit(Canvas(recorder), image, st);
  final picture = recorder.endRecording();
  try {
    return await picture.toImage(out.width.round(), out.height.round());
  } finally {
    picture.dispose();
  }
}

/// The edit as a PNG file (only pixels: no metadata of the original survives).
Future<({Uint8List bytes, int width, int height})> exportEdit(ui.Image image, EditState st) async {
  final result = await renderEdit(image, st);
  try {
    final data = await result.toByteData(format: ui.ImageByteFormat.png);
    if (data == null) throw StateError('image export failed');
    return (bytes: data.buffer.asUint8List(data.offsetInBytes, data.lengthInBytes), width: result.width, height: result.height);
  } finally {
    result.dispose();
  }
}
