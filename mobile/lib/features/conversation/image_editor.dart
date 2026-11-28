// Image editor: draw on an image, put text on it, crop it and turn it, then hand the result back
// (an image about to be sent) or send it into the conversation it came from (an image opened
// from the lightbox). Everything happens on the device, on a decrypted copy in memory: the
// original attachment is never touched, and closing the editor changes and sends nothing.
//
// The model and the painting are in image_edit.dart.

import 'dart:async';
import 'dart:io';
import 'dart:typed_data';
import 'dart:ui' as ui;

import 'package:flutter/material.dart';

import '../../app/globals.dart';
import '../../core/models.dart';
import '../../l10n/i18n.dart';
import '../../services/media.dart';
import '../../state/actions.dart';
import '../../theme/app_theme.dart';
import '../../util/format.dart';
import '../../widgets/dialogs.dart';
import '../../widgets/toast.dart';
import '../../widgets/ui.dart';
import 'image_edit.dart';

/// The result of an edit: a PNG at the original's resolution, and the caption typed in send mode.
typedef EditedImage = ({Uint8List bytes, int width, int height, String caption});

/// The editor never works on more pixels than this on the longest side (memory, and the size a
/// GPU can render in one piece). Sent pictures are reduced further by the core anyway.
const kMaxEditSide = 4096;

/// Opens the editor on [bytes]. Without [onSend] the edited image is returned (null when closed
/// or unchanged). With [onSend] the editor shows a caption field and a send button for
/// [sendTo]; it stays open when [onSend] returns false and returns the image once it was sent.
Future<EditedImage?> openImageEditor(
  BuildContext context, {
  required Uint8List bytes,
  String? sendTo,
  Future<bool> Function(EditedImage edited)? onSend,
}) {
  return Navigator.of(context).push(PageRouteBuilder<EditedImage>(
    pageBuilder: (_, _, _) => ImageEditorPage(bytes: bytes, sendTo: sendTo, onSend: onSend),
    transitionsBuilder: (_, anim, _, child) => FadeTransition(opacity: anim, child: child),
  ));
}

/// Still images the editor can open (an animation would lose its frames).
bool isEditableImage(Attachment a) => a.isImage && !a.mime.startsWith('image/gif') && !a.mime.startsWith('image/svg');

/// True when an edited copy of an image of this conversation can be sent back into it: the same
/// cases in which the conversation shows its composer (conversation_page.dart).
bool canSendEdited(String conversationId) {
  final s = appData;
  final conv = s.conversations[conversationId];
  if (conv == null || conv.isRequest) return false;
  final bot = conv.bot;
  if (bot != null && (bot.state == 'none' || bot.deleted)) return false;
  if (!conv.isGroup) return !(conv.peerAccountId != null ? (s.contacts[conv.peerAccountId]?.blocked ?? conv.blocked) : conv.blocked);
  if (conv.left) return false;
  return !(s.groups[conv.id]?.settings.onlyAdminsSend ?? false) || conv.isAdmin;
}

/// File name of an edited copy: the original's, as a PNG.
String editedFileName(String original) {
  final dot = original.lastIndexOf('.');
  final base = dot > 0 ? original.substring(0, dot) : original;
  return '${base.isEmpty ? 'image' : base}.png';
}

/// Opens an image of a conversation in the editor and sends the result into that conversation,
/// as a reply to [message]. It goes out like any other picture (same command, encrypted by the
/// core). Returns true when something was sent.
Future<bool> editAndSendImage(BuildContext context, Message message, Attachment attachment) async {
  final path = attachment.localPath;
  if (path == null) return false;
  final bytes = await loadMedia(path);
  if (bytes == null || !context.mounted) {
    showToast(tr('annotate.loadFailed'), error: true);
    return false;
  }
  final name = editedFileName(attachment.fileName);
  final sent = await openImageEditor(
    context,
    bytes: bytes,
    sendTo: appData.conversations[message.conversationId]?.title ?? '',
    onSend: (edited) async {
      final max = appData.serverInfo?.maxAttachmentBytes ?? 0;
      if (max > 0 && edited.bytes.length > max) {
        showToast(tr('composer.tooBig', args: {'name': name, 'max': formatBytes(max, currentLanguage)}), error: true);
        return false;
      }
      // The core reads attachments from a file: a temporary copy, removed once it was read.
      final file = File(await appTempPath('edit-$name'));
      try {
        await file.writeAsBytes(edited.bytes, flush: true);
        final m = await attempt(() => backend.sendMessage(
              conversationId: message.conversationId,
              text: edited.caption.isEmpty ? null : edited.caption,
              replyTo: message.id,
              attachments: [
                OutgoingAttachment(path: file.path, fileName: name, mime: 'image/png', kind: 'image', width: edited.width, height: edited.height),
              ],
            ));
        if (m == null) return false;
        store.upsertMessage(m);
        return true;
      } finally {
        unawaited(deleteTempCopy(file.path));
      }
    },
  );
  return sent != null;
}

enum _Tool { pen, text }

/// "Rotate left", without the desktop keyboard shortcut the shared string ends with.
String _rotateLabel(BuildContext context) => context.t('annotate.rotateLeft').replaceFirst(RegExp(r'\s*[(（].*$'), '');

class ImageEditorPage extends StatefulWidget {
  const ImageEditorPage({super.key, required this.bytes, this.sendTo, this.onSend});
  final Uint8List bytes;
  final String? sendTo;
  final Future<bool> Function(EditedImage edited)? onSend;

  @override
  State<ImageEditorPage> createState() => ImageEditorPageState();
}

@visibleForTesting
class ImageEditorPageState extends State<ImageEditorPage> {
  ui.Image? _image;
  bool _failed = false;
  bool _busy = false;

  // Undo / redo walk this list.
  final _history = <EditState>[const EditState()];
  int _index = 0;

  _Tool _tool = _Tool.pen;
  Color _color = kEditColors.first.value;
  String _sizeKey = 'medium';

  // What the finger on the image is doing.
  int? _pointer;
  Offset _down = Offset.zero;
  bool _moved = false;
  PenStroke? _draft;
  ({int index, TextMark orig, TextMark now})? _moving;

  // Crop mode: the frame being adjusted, in pixels of the image as shown.
  Rect? _frame;
  String _aspectKey = 'free';
  ({CropHandle handle, Rect start})? _cropDrag;

  final _caption = TextEditingController();

  EditState get state => _history[_index];
  Size get _full => Size(_image!.width.toDouble(), _image!.height.toDouble());
  Size get _shown => state.outputSize(_full);
  double get _factor => kEditSizes.firstWhere((s) => s.key == _sizeKey).factor;
  double? get _aspect => kEditAspects.firstWhere((a) => a.key == _aspectKey).value;
  // Line and text sizes follow the kept part of the image.
  double get _baseWidth => state.crop?.width ?? _full.width;
  bool get _sending => widget.onSend != null;

  @override
  void initState() {
    super.initState();
    unawaited(_decode());
  }

  @override
  void dispose() {
    _image?.dispose();
    _caption.dispose();
    super.dispose();
  }

  Future<void> _decode() async {
    try {
      final size = await imageHeaderSize(widget.bytes);
      if (size == null || !imageSizeAllowed(size.width, size.height)) throw StateError('unsupported image');
      // Full resolution, unless the image is larger than what can be edited in one piece.
      final big = size.width > kMaxEditSide || size.height > kMaxEditSide;
      final codec = await ui.instantiateImageCodec(
        widget.bytes,
        targetWidth: big && size.width >= size.height ? kMaxEditSide : null,
        targetHeight: big && size.height > size.width ? kMaxEditSide : null,
      );
      final frame = await codec.getNextFrame();
      codec.dispose();
      if (!mounted) return frame.image.dispose();
      setState(() => _image = frame.image);
    } catch (_) {
      if (mounted) setState(() => _failed = true);
    }
  }

  // ------------------------------------------------------------------ history

  void _push(EditState next) {
    if (identical(next, state)) return;
    setState(() {
      _history.removeRange(_index + 1, _history.length);
      _history.add(next);
      _index++;
    });
  }

  void _undo() => setState(() => _index = (_index - 1).clamp(0, _history.length - 1));
  void _redo() => setState(() => _index = (_index + 1).clamp(0, _history.length - 1));

  void _rotate() {
    if (_image == null || _pointer != null) return;
    _push(state.rotated(-1, _full));
    // Crop mode: the frame starts over on the turned image.
    if (_frame != null) setState(() => _frame = aspectRect(_shown, _aspect));
  }

  // ------------------------------------------------------------------ crop

  void _startCrop() {
    if (_image == null) return;
    setState(() => _frame = aspectRect(_shown, _aspect));
  }

  void _pickAspect(String key) => setState(() {
        _aspectKey = key;
        _frame = aspectRect(_shown, _aspect);
      });

  void _applyCrop() {
    final f = _frame;
    if (f == null) return;
    setState(() => _frame = null);
    _push(state.cropped(f, _full));
  }

  // ------------------------------------------------------------------ pointer

  Size _textBox(TextMark m) {
    final tp = m.painter;
    final s = tp.size;
    tp.dispose();
    return s;
  }

  /// Index of the topmost text under a point of the image, or -1.
  int _textAt(Offset p) {
    final shapes = state.shapes;
    for (var i = shapes.length - 1; i >= 0; i--) {
      final s = shapes[i];
      if (s is TextMark && s.hit(p, _textBox(s))) return i;
    }
    return -1;
  }

  /// [local] (widget pixels) as a point of the image; [fit] = widget pixels per image pixel.
  Offset _toImage(Offset local, double fit) {
    final s = _shown;
    return Offset((local.dx / fit).clamp(0, s.width).toDouble(), (local.dy / fit).clamp(0, s.height).toDouble());
  }

  void _onDown(PointerDownEvent e, double fit) {
    if (_pointer != null || _busy) return;
    final p = _toImage(e.localPosition, fit);
    _pointer = e.pointer;
    _down = p;
    _moved = false;
    final frame = _frame;
    if (frame != null) {
      final h = cropHandleAt(frame, p, 30 / fit);
      _cropDrag = h == null ? null : (handle: h, start: frame);
      return;
    }
    if (_tool == _Tool.text) {
      final hit = _textAt(p);
      if (hit >= 0) {
        final m = state.shapes[hit] as TextMark;
        setState(() => _moving = (index: hit, orig: m, now: m));
      }
      return;
    }
    setState(() => _draft = PenStroke(color: _color, width: strokeFor(_baseWidth, _factor), points: [p]));
  }

  void _onMove(PointerMoveEvent e, double fit) {
    if (e.pointer != _pointer) return;
    final p = _toImage(e.localPosition, fit);
    if ((p - _down).distance * fit > 6) _moved = true;
    final drag = _cropDrag;
    if (_frame != null) {
      if (drag != null) {
        final s = _shown;
        final min = [s.width, s.height, 40 / fit > 16 ? 40 / fit : 16.0].reduce((a, b) => a < b ? a : b);
        setState(() => _frame = dragCrop(drag.start, drag.handle, p - _down, s, _aspect, min: min));
      }
      return;
    }
    final mv = _moving;
    if (mv != null) {
      setState(() => _moving = (index: mv.index, orig: mv.orig, now: mv.orig.shifted(p - _down)));
      return;
    }
    final d = _draft;
    if (d != null) setState(() => _draft = d.withPoint(p));
  }

  void _onUp(PointerEvent e) {
    if (e.pointer != _pointer) return;
    _pointer = null;
    _cropDrag = null;
    if (_frame != null) return;
    final mv = _moving;
    final d = _draft;
    setState(() {
      _moving = null;
      _draft = null;
    });
    if (e is PointerCancelEvent) return;
    if (mv != null) {
      if (_moved) {
        _push(state.replacing(mv.index, mv.now));
      } else {
        unawaited(_editText(mv.index));
      }
    } else if (d != null) {
      _push(state.withShape(d));
    } else if (_tool == _Tool.text && !_moved) {
      unawaited(_addText(_down));
    }
  }

  // ------------------------------------------------------------------ text

  Future<String?> _askText(String initial) =>
      showTextEdit(context, title: context.t('annotate.addText'), initial: initial, maxLength: 200, allowEmpty: initial.isNotEmpty);

  /// New text, centred on [at] (the middle of the image from the toolbar button).
  Future<void> _addText(Offset at) async {
    final text = await _askText('');
    if (text == null || text.isEmpty || !mounted) return;
    var m = TextMark(color: _color, size: fontFor(_baseWidth, _factor), pos: at, text: text);
    final box = _textBox(m);
    final s = _shown;
    // Inside the image where it fits.
    final maxX = s.width > box.width ? s.width - box.width : 0.0;
    final maxY = s.height > box.height ? s.height - box.height : 0.0;
    m = m.shifted(Offset((at.dx - box.width / 2).clamp(0.0, maxX) - at.dx, (at.dy - box.height / 2).clamp(0.0, maxY) - at.dy));
    _push(state.withShape(m));
  }

  /// A tap on a text: change it, or clear it to remove it.
  Future<void> _editText(int index) async {
    final m = state.shapes[index] as TextMark;
    final text = await _askText(m.text);
    if (text == null || text == m.text || !mounted) return;
    if (text.isEmpty) {
      _push(EditState(shapes: [...state.shapes]..removeAt(index), turns: state.turns, crop: state.crop));
    } else {
      _push(state.replacing(index, TextMark(color: m.color, size: m.size, pos: m.pos, text: text, turns: m.turns)));
    }
  }

  // ------------------------------------------------------------------ result

  Future<void> _finish() async {
    final image = _image;
    if (image == null || _busy || _frame != null) return;
    // Nothing to save. Sending still goes through an export: the result is a file of its own.
    if (state.isEmpty && !_sending) return Navigator.of(context).pop();
    setState(() => _busy = true);
    try {
      final out = await exportEdit(image, state);
      final edited = (bytes: out.bytes, width: out.width, height: out.height, caption: _caption.text.trim());
      if (_sending && !await widget.onSend!(edited)) return;
      if (mounted) Navigator.of(context).pop(edited);
    } catch (_) {
      showToast(tr('annotate.saveFailed'), error: true);
    } finally {
      if (mounted) setState(() => _busy = false);
    }
  }

  // ------------------------------------------------------------------ UI

  @override
  Widget build(BuildContext context) {
    final image = _image;
    final cropping = _frame != null;
    return Scaffold(
      backgroundColor: Colors.black,
      resizeToAvoidBottomInset: true,
      body: SafeArea(
        child: Column(children: [
          _topBar(context, cropping),
          Expanded(
            child: image == null
                ? Center(
                    child: _failed
                        ? Text(context.t('annotate.loadFailed'), style: ts(14, color: Colors.white70))
                        : const CircularProgressIndicator(color: Colors.white54, strokeWidth: 2.5),
                  )
                : Padding(
                    padding: const EdgeInsets.all(12),
                    child: LayoutBuilder(builder: (context, box) {
                      final s = _shown;
                      final fit = [box.maxWidth / s.width, box.maxHeight / s.height].reduce((a, b) => a < b ? a : b);
                      final shapes = state;
                      final mv = _moving;
                      return Center(
                        child: Listener(
                          key: const ValueKey('editor-surface'),
                          behavior: HitTestBehavior.opaque,
                          onPointerDown: (e) => _onDown(e, fit),
                          onPointerMove: (e) => _onMove(e, fit),
                          onPointerUp: _onUp,
                          onPointerCancel: _onUp,
                          child: CustomPaint(
                            size: Size(s.width * fit, s.height * fit),
                            painter: _EditPainter(
                              image: image,
                              state: mv == null ? shapes : shapes.replacing(mv.index, mv.now),
                              scale: fit,
                              draft: _draft,
                              frame: _frame,
                            ),
                          ),
                        ),
                      );
                    }),
                  ),
          ),
          if (image != null) cropping ? _cropBar(context) : _toolBar(context),
          if (image != null && !cropping && _sending) _sendBar(context),
          const SizedBox(height: 6),
        ]),
      ),
    );
  }

  Widget _iconBtn(IconData icon, String tooltip, VoidCallback? onPressed, {bool active = false, Key? key}) {
    final accent = context.tk.accent;
    return IconButton(
      key: key,
      onPressed: onPressed,
      tooltip: tooltip,
      icon: Icon(icon, size: 23),
      color: Colors.white,
      disabledColor: Colors.white30,
      style: IconButton.styleFrom(backgroundColor: active ? accent : Colors.white.withValues(alpha: .1), foregroundColor: active ? context.tk.onAccent : Colors.white),
    );
  }

  Widget _topBar(BuildContext context, bool cropping) {
    final ready = _image != null && !_busy;
    return Padding(
      padding: const EdgeInsets.fromLTRB(6, 6, 6, 4),
      child: Row(children: [
        _iconBtn(Icons.close, context.t(cropping ? 'common.cancel' : 'common.close'), cropping ? () => setState(() => _frame = null) : () => Navigator.of(context).pop(),
            key: const ValueKey('editor-close')),
        const Spacer(),
        if (cropping) ...[
          _iconBtn(Icons.rotate_90_degrees_ccw, _rotateLabel(context), ready ? _rotate : null),
          _iconBtn(Icons.check, context.t('annotate.cropApply'), _applyCrop, active: true, key: const ValueKey('editor-crop-apply')),
        ] else ...[
          _iconBtn(Icons.undo, context.t('annotate.undo'), ready && _index > 0 ? _undo : null, key: const ValueKey('editor-undo')),
          _iconBtn(Icons.redo, context.t('annotate.redo'), ready && _index < _history.length - 1 ? _redo : null),
          const SizedBox(width: 8),
          _iconBtn(Icons.crop_rotate, context.t('annotate.crop'), ready ? _startCrop : null, key: const ValueKey('editor-crop')),
          _iconBtn(Icons.rotate_90_degrees_ccw, _rotateLabel(context), ready ? _rotate : null, key: const ValueKey('editor-rotate')),
          _iconBtn(Icons.title, context.t('annotate.text'), ready ? () => setState(() => _tool = _Tool.text) : null,
              active: _tool == _Tool.text, key: const ValueKey('editor-text')),
          _iconBtn(Icons.edit, context.t('annotate.pen'), ready ? () => setState(() => _tool = _Tool.pen) : null,
              active: _tool == _Tool.pen, key: const ValueKey('editor-pen')),
          if (!_sending) ...[
            const SizedBox(width: 8),
            _busy
                ? const Padding(padding: EdgeInsets.all(12), child: SizedBox.square(dimension: 22, child: CircularProgressIndicator(strokeWidth: 2.5, color: Colors.white)))
                : _iconBtn(Icons.check, context.t('annotate.save'), _image != null ? _finish : null, active: true, key: const ValueKey('editor-save')),
          ],
        ],
      ]),
    );
  }

  /// Colours and thickness of the pen / text.
  Widget _toolBar(BuildContext context) {
    return SizedBox(
      height: 48,
      child: Center(
        child: SingleChildScrollView(
          scrollDirection: Axis.horizontal,
          padding: const EdgeInsets.symmetric(horizontal: 10),
          child: Row(mainAxisSize: MainAxisSize.min, children: [
            for (final c in kEditColors)
              Semantics(
                button: true,
                selected: _color == c.value,
                label: context.t('annotate.color', args: {'name': context.t('annotate.colors.${c.key}')}),
                child: GestureDetector(
                  onTap: () => setState(() => _color = c.value),
                  child: Container(
                    width: 30,
                    height: 30,
                    margin: const EdgeInsets.symmetric(horizontal: 6),
                    decoration: BoxDecoration(
                      color: c.value,
                      shape: BoxShape.circle,
                      border: Border.all(color: _color == c.value ? Colors.white : Colors.white38, width: _color == c.value ? 3 : 1.5),
                    ),
                  ),
                ),
              ),
            Container(width: 1, height: 24, margin: const EdgeInsets.symmetric(horizontal: 8), color: Colors.white24),
            for (final (i, s) in kEditSizes.indexed)
              Semantics(
                button: true,
                selected: _sizeKey == s.key,
                label: context.t('annotate.size', args: {'name': context.t('annotate.sizes.${s.key}')}),
                child: GestureDetector(
                  behavior: HitTestBehavior.opaque,
                  onTap: () => setState(() => _sizeKey = s.key),
                  child: Container(
                    width: 38,
                    height: 38,
                    margin: const EdgeInsets.symmetric(horizontal: 2),
                    decoration: BoxDecoration(shape: BoxShape.circle, color: _sizeKey == s.key ? Colors.white24 : Colors.transparent),
                    child: Center(
                      child: Container(width: 6.0 + i * 6, height: 6.0 + i * 6, decoration: const BoxDecoration(color: Colors.white, shape: BoxShape.circle)),
                    ),
                  ),
                ),
              ),
          ]),
        ),
      ),
    );
  }

  Widget _cropBar(BuildContext context) {
    final t = context.tk;
    return SizedBox(
      height: 48,
      child: Center(
        child: SingleChildScrollView(
          scrollDirection: Axis.horizontal,
          padding: const EdgeInsets.symmetric(horizontal: 10),
          child: Row(mainAxisSize: MainAxisSize.min, children: [
            for (final a in kEditAspects)
              Padding(
                padding: const EdgeInsets.symmetric(horizontal: 3),
                child: Tap(
                  onTap: () => _pickAspect(a.key),
                  color: _aspectKey == a.key ? t.accent : Colors.white.withValues(alpha: .1),
                  radius: BorderRadius.circular(18),
                  padding: const EdgeInsets.symmetric(horizontal: 14, vertical: 8),
                  child: Text(
                    a.key == 'free' || a.key == 'square' ? context.t('annotate.aspect.${a.key}') : a.key,
                    style: ts(13.5, weight: 600, color: _aspectKey == a.key ? t.onAccent : Colors.white),
                  ),
                ),
              ),
          ]),
        ),
      ),
    );
  }

  /// Send mode: who gets it, a caption, the send button.
  Widget _sendBar(BuildContext context) {
    final t = context.tk;
    return Padding(
      padding: const EdgeInsets.fromLTRB(10, 4, 10, 2),
      child: Row(crossAxisAlignment: CrossAxisAlignment.end, children: [
        Expanded(
          child: Container(
            constraints: const BoxConstraints(minHeight: 46),
            padding: const EdgeInsets.symmetric(horizontal: 16),
            decoration: BoxDecoration(color: const Color(0xFF242628), borderRadius: BorderRadius.circular(23)),
            child: TextField(
              key: const ValueKey('editor-caption'),
              enableIMEPersonalizedLearning: imePersonalizedLearning,
              controller: _caption,
              enabled: !_busy,
              minLines: 1,
              maxLines: 3,
              textCapitalization: TextCapitalization.sentences,
              style: ts(15, color: Colors.white),
              cursorColor: t.accent,
              decoration: InputDecoration(
                isDense: true,
                border: InputBorder.none,
                hintText: context.t('composer.caption'),
                hintStyle: ts(15, color: Colors.white54),
                helperText: widget.sendTo == null || widget.sendTo!.isEmpty ? null : context.t('composer.sendTo', args: {'name': widget.sendTo}),
                helperStyle: ts(11.5, color: Colors.white54),
                helperMaxLines: 1,
                contentPadding: const EdgeInsets.only(top: 12, bottom: 4),
              ),
            ),
          ),
        ),
        const SizedBox(width: 8),
        SizedBox.square(
          dimension: 48,
          child: Material(
            color: t.accent,
            shape: const CircleBorder(),
            child: InkWell(
              key: const ValueKey('editor-send'),
              customBorder: const CircleBorder(),
              onTap: _busy ? null : _finish,
              child: Center(
                child: _busy
                    ? SizedBox.square(dimension: 20, child: CircularProgressIndicator(strokeWidth: 2.5, color: t.onAccent))
                    : Tooltip(message: context.t('composer.send'), child: Icon(Icons.send, size: 21, color: t.onAccent)),
              ),
            ),
          ),
        ),
      ]),
    );
  }
}

class _EditPainter extends CustomPainter {
  _EditPainter({required this.image, required this.state, required this.scale, this.draft, this.frame});
  final ui.Image image;
  final EditState state;
  final double scale;
  final PenStroke? draft;
  final Rect? frame;

  @override
  void paint(Canvas canvas, Size size) {
    paintEdit(canvas, image, state, scale: scale, extra: draft, quality: FilterQuality.medium);
    final f = frame;
    if (f == null) return;
    // Crop mode: everything outside the frame dimmed, corner handles, rule of thirds.
    final r = Rect.fromLTRB(f.left * scale, f.top * scale, f.right * scale, f.bottom * scale);
    canvas.drawPath(
      Path.combine(PathOperation.difference, Path()..addRect(Offset.zero & size), Path()..addRect(r)),
      Paint()..color = const Color(0x99000000),
    );
    final thin = Paint()
      ..color = const Color(0x73FFFFFF)
      ..strokeWidth = 1;
    for (var i = 1; i < 3; i++) {
      canvas.drawLine(Offset(r.left + r.width * i / 3, r.top), Offset(r.left + r.width * i / 3, r.bottom), thin);
      canvas.drawLine(Offset(r.left, r.top + r.height * i / 3), Offset(r.right, r.top + r.height * i / 3), thin);
    }
    canvas.drawRect(
      r,
      Paint()
        ..color = Colors.white
        ..style = PaintingStyle.stroke
        ..strokeWidth = 1.2,
    );
    final thick = Paint()
      ..color = Colors.white
      ..style = PaintingStyle.stroke
      ..strokeWidth = 4
      ..strokeCap = StrokeCap.square;
    final arm = [22.0, r.width / 3, r.height / 3].reduce((a, b) => a < b ? a : b);
    for (final (c, dx, dy) in [(r.topLeft, 1, 1), (r.topRight, -1, 1), (r.bottomLeft, 1, -1), (r.bottomRight, -1, -1)]) {
      canvas.drawPath(
        Path()
          ..moveTo(c.dx + dx * arm, c.dy)
          ..lineTo(c.dx, c.dy)
          ..lineTo(c.dx, c.dy + dy * arm),
        thick,
      );
    }
  }

  @override
  bool shouldRepaint(_EditPainter old) =>
      old.image != image || old.state != state || old.scale != scale || old.draft != draft || old.frame != frame;
}
