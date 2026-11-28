import 'dart:typed_data';
import 'dart:ui' as ui;

import 'package:chatapp/features/conversation/image_edit.dart';
import 'package:chatapp/features/conversation/image_editor.dart';
import 'package:chatapp/l10n/i18n.dart';
import 'package:chatapp/theme/app_theme.dart';
import 'package:chatapp/theme/tokens.dart';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';

/// A [w] × [h] image: left half red, right half blue.
Future<ui.Image> twoHalves(int w, int h) async {
  final recorder = ui.PictureRecorder();
  final canvas = Canvas(recorder);
  canvas.drawRect(Rect.fromLTWH(0, 0, w / 2, h.toDouble()), Paint()..color = const Color(0xFFFF0000));
  canvas.drawRect(Rect.fromLTWH(w / 2, 0, w / 2, h.toDouble()), Paint()..color = const Color(0xFF0000FF));
  return recorder.endRecording().toImage(w, h);
}

Future<Uint8List> png(ui.Image image) async => (await image.toByteData(format: ui.ImageByteFormat.png))!.buffer.asUint8List();

Future<Color> pixel(ui.Image image, int x, int y) async {
  final data = (await image.toByteData(format: ui.ImageByteFormat.rawRgba))!;
  final i = (y * image.width + x) * 4;
  return Color.fromARGB(data.getUint8(i + 3), data.getUint8(i), data.getUint8(i + 1), data.getUint8(i + 2));
}

void main() {
  const image = Size(400, 300);

  test('the result keeps the resolution of the original, turned and cropped', () {
    expect(const EditState().outputSize(const Size(4032, 3024)), const Size(4032, 3024));
    expect(const EditState(turns: 1).outputSize(const Size(4032, 3024)), const Size(3024, 4032));
    expect(const EditState(turns: 3, crop: Rect.fromLTWH(10, 20, 1000, 500)).outputSize(const Size(4032, 3024)), const Size(500, 1000));
  });

  test('a crop frame drawn on the turned image maps back onto the original', () {
    const whole = Rect.fromLTWH(0, 0, 400, 300);
    const frame = Rect.fromLTWH(10, 20, 100, 50);
    expect(cropToSource(frame, 0, whole), frame);
    // Turned clockwise the image is 300 × 400: its left edge is the original's bottom edge.
    expect(cropToSource(frame, 1, whole), const Rect.fromLTWH(20, 190, 50, 100));
    expect(cropToSource(frame, 2, whole), const Rect.fromLTWH(290, 230, 100, 50));
    expect(cropToSource(frame, 3, whole), const Rect.fromLTWH(330, 10, 50, 100));
    // A second crop is relative to the first one.
    expect(cropToSource(frame, 0, const Rect.fromLTWH(100, 100, 200, 150)), const Rect.fromLTWH(110, 120, 100, 50));
    // Never outside the image.
    expect(cropToSource(const Rect.fromLTWH(-5, -5, 1000, 1000), 0, whole), whole);
  });

  test('what was drawn keeps its place across crop and rotation', () {
    const mark = PenStroke(color: Color(0xFFFFFFFF), width: 4, points: [Offset(150, 120)]);
    var st = const EditState().withShape(mark).cropped(const Rect.fromLTWH(100, 100, 200, 150), image);
    expect(st.crop, const Rect.fromLTWH(100, 100, 200, 150));
    expect((st.shapes.single as PenStroke).points.single, const Offset(50, 20));
    st = st.rotated(1, image);
    expect(st.outputSize(image), const Size(150, 200));
    expect((st.shapes.single as PenStroke).points.single, const Offset(130, 50));
    // Four quarter turns bring everything back.
    for (var i = 0; i < 3; i++) {
      st = st.rotated(1, image);
    }
    expect(st.turns, 0);
    expect((st.shapes.single as PenStroke).points.single, const Offset(50, 20));
    // Cropping to the whole image is not a change.
    expect(identical(st.cropped(Offset.zero & st.outputSize(image), image), st), isTrue);
  });

  test('the crop frame stays inside the image and keeps its ratio', () {
    const start = Rect.fromLTWH(100, 100, 200, 100);
    expect(dragCrop(start, CropHandle.bottomRight, const Offset(50, 20), image, null), const Rect.fromLTWH(100, 100, 250, 120));
    expect(dragCrop(start, CropHandle.topLeft, const Offset(-500, -500), image, null), const Rect.fromLTWH(0, 0, 300, 200));
    expect(dragCrop(start, CropHandle.topLeft, const Offset(500, 500), image, null), const Rect.fromLTWH(284, 184, 16, 16));
    expect(dragCrop(start, CropHandle.move, const Offset(1000, -1000), image, null), const Rect.fromLTWH(200, 0, 200, 100));
    expect(dragCrop(start, CropHandle.bottomRight, const Offset(1000, 0), image, 1), const Rect.fromLTWH(100, 100, 200, 200));
    expect(aspectRect(image, 1), const Rect.fromLTWH(50, 0, 300, 300));
    expect(aspectRect(image, null), Offset.zero & image);
    expect(cropHandleAt(start, const Offset(105, 104), 20), CropHandle.topLeft);
    expect(cropHandleAt(start, const Offset(200, 150), 20), CropHandle.move);
    expect(cropHandleAt(start, const Offset(10, 10), 20), isNull);
  });

  test('a text is found under the finger, turned or not', () {
    const text = TextMark(color: Color(0xFFFFFFFF), size: 40, pos: Offset(100, 100), text: 'hello');
    const box = Size(120, 46);
    expect(text.hit(const Offset(150, 120), box), isTrue);
    expect(text.hit(const Offset(300, 120), box), isFalse);
    // After a clockwise quarter turn the text runs downwards from its corner.
    final turned = TextMark(color: text.color, size: 40, pos: text.pos, text: 'hello', turns: 1);
    expect(turned.hit(const Offset(80, 160), box), isTrue);
    expect(turned.hit(const Offset(180, 110), box), isFalse);
  });

  test('pen and text sizes follow the image', () {
    expect(strokeFor(4000), 20);
    expect(strokeFor(4000, 0.5), 10);
    expect(strokeFor(100, 0.5), 2);
    expect(fontFor(100), 18);
  });

  testWidgets('the edit is baked at full size: crop, turn and strokes', (tester) async {
    await tester.runAsync(() async {
      final src = await twoHalves(400, 300);
      // Untouched: the same pixels, the same size.
      final same = await renderEdit(src, const EditState());
      expect((same.width, same.height), (400, 300));
      expect(await pixel(same, 100, 150), const Color(0xFFFF0000));
      expect(await pixel(same, 300, 150), const Color(0xFF0000FF));

      // Turned clockwise: the red (left) half is now on top.
      final turned = await renderEdit(src, const EditState(turns: 1));
      expect((turned.width, turned.height), (300, 400));
      expect(await pixel(turned, 150, 100), const Color(0xFFFF0000));
      expect(await pixel(turned, 150, 300), const Color(0xFF0000FF));

      // Cropped to the blue half, with a green stroke across it.
      var st = const EditState().cropped(const Rect.fromLTWH(200, 0, 200, 300), const Size(400, 300));
      st = st.withShape(const PenStroke(color: Color(0xFF00FF00), width: 20, points: [Offset(0, 150), Offset(200, 150)]));
      final out = await exportEdit(src, st);
      expect((out.width, out.height), (200, 300));
      final codec = await ui.instantiateImageCodec(out.bytes);
      final decoded = (await codec.getNextFrame()).image;
      expect((decoded.width, decoded.height), (200, 300));
      expect(await pixel(decoded, 100, 20), const Color(0xFF0000FF));
      expect(await pixel(decoded, 100, 150), const Color(0xFF00FF00));
    });
  });

  group('editor screen', () {
    Future<List<EditedImage>> pumpEditor(WidgetTester tester, {required bool send, bool sendOk = true}) async {
      final sent = <EditedImage>[];
      late Uint8List bytes;
      await tester.runAsync(() async => bytes = await png(await twoHalves(400, 300)));
      EditedImage? result;
      await tester.pumpWidget(MaterialApp(
        theme: buildTheme(ChatTokens.build(Direction.a, dark: true)),
        home: L10n(
          lang: 'en',
          child: Builder(
            builder: (context) => TextButton(
              onPressed: () async {
                result = await openImageEditor(
                  context,
                  bytes: bytes,
                  sendTo: send ? 'Ana' : null,
                  onSend: send
                      ? (e) async {
                          sent.add(e);
                          return sendOk;
                        }
                      : null,
                );
                if (result != null && !send) sent.add(result!);
              },
              child: const Text('open'),
            ),
          ),
        ),
      ));
      await tester.tap(find.text('open'));
      await tester.pump();
      // The image is decoded outside the fake clock.
      await tester.runAsync(() => Future<void>.delayed(const Duration(milliseconds: 300)));
      await tester.pumpAndSettle();
      expect(find.byKey(const ValueKey('editor-surface')), findsOneWidget);
      return sent;
    }

    Future<void> draw(WidgetTester tester) async {
      final c = tester.getCenter(find.byKey(const ValueKey('editor-surface')));
      final g = await tester.startGesture(c - const Offset(40, 0));
      await g.moveTo(c);
      await g.moveTo(c + const Offset(40, 0));
      await g.up();
      await tester.pump();
    }

    ImageEditorPageState editor(WidgetTester tester) => tester.state<ImageEditorPageState>(find.byType(ImageEditorPage));

    testWidgets('closing after drawing sends nothing and returns nothing', (tester) async {
      final sent = await pumpEditor(tester, send: true);
      await draw(tester);
      expect(editor(tester).state.shapes, hasLength(1));
      await tester.tap(find.byKey(const ValueKey('editor-close')));
      await tester.pumpAndSettle();
      expect(find.byType(ImageEditorPage), findsNothing);
      expect(sent, isEmpty);
    });

    testWidgets('undo removes the stroke; rotate and crop are steps too', (tester) async {
      await pumpEditor(tester, send: false);
      await draw(tester);
      await tester.tap(find.byKey(const ValueKey('editor-rotate')));
      await tester.pump();
      expect(editor(tester).state.turns, 3);
      expect(editor(tester).state.outputSize(const Size(400, 300)), const Size(300, 400));
      await tester.tap(find.byKey(const ValueKey('editor-crop')));
      await tester.pump();
      await tester.tap(find.text('Square'));
      await tester.pump();
      await tester.tap(find.byKey(const ValueKey('editor-crop-apply')));
      await tester.pump();
      expect(editor(tester).state.outputSize(const Size(400, 300)), const Size(300, 300));
      for (var i = 0; i < 3; i++) {
        await tester.tap(find.byKey(const ValueKey('editor-undo')));
        await tester.pump();
      }
      expect(editor(tester).state.isEmpty, isTrue);
    });

    testWidgets('send hands over a full-size image and the caption', (tester) async {
      final sent = await pumpEditor(tester, send: true);
      await draw(tester);
      await tester.enterText(find.byKey(const ValueKey('editor-caption')), '  my suggestion ');
      await tester.tap(find.byKey(const ValueKey('editor-send')));
      await tester.pump();
      await tester.runAsync(() => Future<void>.delayed(const Duration(milliseconds: 500)));
      await tester.pumpAndSettle();
      expect(sent, hasLength(1));
      expect((sent.single.width, sent.single.height), (400, 300));
      expect(sent.single.caption, 'my suggestion');
      expect(find.byType(ImageEditorPage), findsNothing);
    });

    testWidgets('a failed send keeps the editor open with the edit', (tester) async {
      final sent = await pumpEditor(tester, send: true, sendOk: false);
      await draw(tester);
      await tester.tap(find.byKey(const ValueKey('editor-send')));
      await tester.pump();
      await tester.runAsync(() => Future<void>.delayed(const Duration(milliseconds: 500)));
      await tester.pumpAndSettle();
      expect(sent, hasLength(1));
      expect(find.byType(ImageEditorPage), findsOneWidget);
      expect(editor(tester).state.shapes, hasLength(1));
    });

    testWidgets('saving an untouched image returns nothing', (tester) async {
      final saved = await pumpEditor(tester, send: false);
      await tester.tap(find.byKey(const ValueKey('editor-save')));
      await tester.pumpAndSettle();
      expect(find.byType(ImageEditorPage), findsNothing);
      expect(saved, isEmpty);
    });
  });

  test('edited copies are PNG files named after the original', () {
    expect(editedFileName('IMG_1234.jpg'), 'IMG_1234.png');
    expect(editedFileName('photo'), 'photo.png');
    expect(editedFileName('.hidden'), '.hidden.png');
  });
}
