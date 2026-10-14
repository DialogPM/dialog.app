// QR code display (white card, like the design's "Cod QR" modal) and the camera scanner with a
// paste fallback.

import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:image_picker/image_picker.dart';
import 'package:mobile_scanner/mobile_scanner.dart';
import 'package:qr_flutter/qr_flutter.dart';

import '../app/globals.dart';
import '../l10n/i18n.dart';
import '../theme/app_theme.dart';
import 'app_icon.dart';
import 'ui.dart';

class QrCodeView extends StatelessWidget {
  const QrCodeView({super.key, required this.data, this.size = 214});

  final String data;
  final double size;

  @override
  Widget build(BuildContext context) {
    final t = context.tk;
    const ink = Color(0xFF0F2A22);
    return Container(
      padding: const EdgeInsets.all(18),
      decoration: BoxDecoration(
        color: Colors.white,
        borderRadius: BorderRadius.circular(22),
        border: Border.all(color: t.line),
        boxShadow: const [BoxShadow(color: Color.fromRGBO(0, 0, 0, .08), blurRadius: 30, offset: Offset(0, 10))],
      ),
      child: _Qr(data: data, size: size, ink: ink),
    );
  }
}

class _Qr extends StatelessWidget {
  const _Qr({required this.data, required this.size, required this.ink});
  final String data;
  final double size;
  final Color ink;

  @override
  Widget build(BuildContext context) => QrImageView(
        data: data,
        size: size,
        padding: EdgeInsets.zero,
        backgroundColor: Colors.white,
        errorCorrectionLevel: QrErrorCorrectLevel.M,
        eyeStyle: QrEyeStyle(eyeShape: QrEyeShape.square, color: ink),
        dataModuleStyle: QrDataModuleStyle(dataModuleShape: QrDataModuleShape.square, color: ink),
      );
}

/// Scanner: camera preview (Android / iOS) with the design's corner frame, "from image", and a
/// field to paste the link. Calls [onResult] with the decoded text.
class QrScanner extends StatefulWidget {
  const QrScanner({super.key, required this.onResult, this.hint, this.pasteLabel, this.placeholder, this.submitLabel, this.busy = false});

  final ValueChanged<String> onResult;
  final String? hint;
  final String? pasteLabel;
  final String? placeholder;
  final String? submitLabel;
  final bool busy;

  @override
  State<QrScanner> createState() => _QrScannerState();
}

class _QrScannerState extends State<QrScanner> with SingleTickerProviderStateMixin {
  MobileScannerController? _controller;
  final _paste = TextEditingController();
  late final AnimationController _line = AnimationController(vsync: this, duration: const Duration(milliseconds: 1800))..repeat(reverse: true);
  bool _done = false;
  String? _error;

  @override
  void initState() {
    super.initState();
    if (isMobilePlatform) _controller = MobileScannerController(formats: const [BarcodeFormat.qrCode]);
  }

  @override
  void dispose() {
    _controller?.dispose();
    _paste.dispose();
    _line.dispose();
    super.dispose();
  }

  void _emit(String value) {
    final v = value.trim();
    if (v.isEmpty || _done) return;
    _done = true;
    widget.onResult(v);
    Future<void>.delayed(const Duration(seconds: 2), () => _done = false);
  }

  Future<void> _fromImage() async {
    final c = _controller;
    if (c == null) return;
    final img = await ImagePicker().pickImage(source: ImageSource.gallery);
    if (img == null) return;
    final res = await c.analyzeImage(img.path);
    final v = res?.barcodes.map((b) => b.rawValue).whereType<String>().firstOrNull;
    if (v == null) {
      setState(() => _error = tr('qr.notFound'));
    } else {
      _emit(v);
    }
  }

  @override
  Widget build(BuildContext context) {
    final t = context.tk;
    final c = _controller;
    Widget frame = Container(
      width: 280,
      height: 280,
      decoration: BoxDecoration(color: const Color(0xFF0B0F10), borderRadius: BorderRadius.circular(24)),
      clipBehavior: Clip.antiAlias,
      child: Stack(fit: StackFit.expand, children: [
        if (c != null)
          MobileScanner(
            controller: c,
            onDetect: (cap) {
              final v = cap.barcodes.map((b) => b.rawValue).whereType<String>().firstOrNull;
              if (v != null) _emit(v);
            },
            errorBuilder: (context, e) => Center(
              child: Padding(
                padding: const EdgeInsets.all(24),
                child: Text(
                  e.errorCode == MobileScannerErrorCode.permissionDenied ? context.t('mobile.cameraDenied') : context.t('qr.noCamera'),
                  textAlign: TextAlign.center,
                  style: ts(13, color: Colors.white70, height: 1.45),
                ),
              ),
            ),
          )
        else
          Center(
            child: Padding(
              padding: const EdgeInsets.all(40),
              child: Text(context.t('mobile.scanUnsupported'), textAlign: TextAlign.center, style: ts(13, color: Colors.white70, height: 1.45)),
            ),
          ),
        ..._corners(),
        if (c != null)
          AnimatedBuilder(
            animation: _line,
            builder: (_, _) => Positioned(
              left: 52,
              right: 52,
              top: 60 + 160 * _line.value,
              child: Container(
                height: 2,
                decoration: BoxDecoration(color: t.accent, boxShadow: [BoxShadow(color: t.accent, blurRadius: 16)]),
              ),
            ),
          ),
        if (widget.busy) Container(color: Colors.black54, child: const Center(child: CircularProgressIndicator(color: Colors.white))),
      ]),
    );
    return Column(mainAxisSize: MainAxisSize.min, crossAxisAlignment: CrossAxisAlignment.stretch, children: [
      Center(child: frame),
      const SizedBox(height: 14),
      Text(widget.hint ?? context.t('qr.scanHint'), textAlign: TextAlign.center, style: ts(13.5, color: t.muted, height: 1.45)),
      if (c != null) ...[
        const SizedBox(height: 10),
        Center(child: ChipBtn(label: context.t('qr.fromImage'), icon: 'image', tone: 'sunk', onPressed: _fromImage)),
      ],
      if (_error != null) ...[const SizedBox(height: 8), Text(_error!, textAlign: TextAlign.center, style: ts(13, color: t.danger))],
      const SizedBox(height: 14),
      Row(children: [
        Expanded(child: Divider(color: t.line)),
        Padding(padding: const EdgeInsets.symmetric(horizontal: 10), child: Text(widget.pasteLabel ?? context.t('qr.orPaste'), style: ts(12.5, color: t.faint))),
        Expanded(child: Divider(color: t.line)),
      ]),
      const SizedBox(height: 10),
      Row(children: [
        Expanded(
          child: AppField(
            controller: _paste,
            hint: widget.placeholder ?? 'chatapp://add?…',
            mono: true,
            fontSize: 13,
            onSubmitted: _emit,
            suffix: IconButton(
              tooltip: context.t('mobile.paste'),
              onPressed: () async {
                final d = await Clipboard.getData(Clipboard.kTextPlain);
                if (d?.text != null) setState(() => _paste.text = d!.text!.trim());
              },
              icon: AppIcon('copy', size: 17, color: t.muted),
            ),
          ),
        ),
        const SizedBox(width: 8),
        Btn(label: widget.submitLabel ?? context.t('common.add'), height: 48, expand: false, onPressed: widget.busy ? null : () => _emit(_paste.text)),
      ]),
    ]);
  }

  List<Widget> _corners() => [const Positioned.fill(child: CustomPaint(painter: _CornersPainter()))];
}

/// The four white corner brackets of the scanner frame (36px, 3px, radius 12, inset 40).
class _CornersPainter extends CustomPainter {
  const _CornersPainter();

  @override
  void paint(Canvas canvas, Size size) {
    final p = Paint()
      ..color = Colors.white
      ..style = PaintingStyle.stroke
      ..strokeWidth = 3;
    const inset = 40.0 + 1.5, len = 36.0 - 1.5, r = 12.0;
    final w = size.width, h = size.height;
    Path corner(double x, double y, double dx, double dy) => Path()
      ..moveTo(x, y + dy * len)
      ..lineTo(x, y + dy * r)
      ..quadraticBezierTo(x, y, x + dx * r, y)
      ..lineTo(x + dx * len, y);
    canvas.drawPath(corner(inset, inset, 1, 1), p);
    canvas.drawPath(corner(w - inset, inset, -1, 1), p);
    canvas.drawPath(corner(inset, h - inset, 1, -1), p);
    canvas.drawPath(corner(w - inset, h - inset, -1, -1), p);
  }

  @override
  bool shouldRepaint(_CornersPainter oldDelegate) => false;
}
