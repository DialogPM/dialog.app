// Picking files / photos, saving and sharing decrypted attachments.

import 'dart:async';
import 'dart:io';

import 'package:file_picker/file_picker.dart';
import 'package:flutter/foundation.dart';
import 'package:image_picker/image_picker.dart';
import 'package:mime/mime.dart';
import 'package:share_plus/share_plus.dart';

import '../app/globals.dart';
import '../state/actions.dart';
import 'media.dart';

class PickedFile {
  final String path;
  final String name;
  final String mime;
  final int? size;

  const PickedFile({required this.path, required this.name, required this.mime, this.size});

  bool get isImage => mime.startsWith('image/');
  bool get isVideo => mime.startsWith('video/');

  static PickedFile fromPath(String path, {String? name}) {
    final n = name ?? path.split(Platform.pathSeparator).last;
    int? size;
    try {
      size = File(path).lengthSync();
    } catch (_) {}
    return PickedFile(path: path, name: n, mime: lookupMimeType(n) ?? lookupMimeType(path) ?? 'application/octet-stream', size: size);
  }
}

Future<String> _tempCopy(String name, Uint8List bytes) async {
  final f = File(await appTempPath('pick-$name'));
  await f.writeAsBytes(bytes, flush: true);
  return f.path;
}

/// Removes the temporary copies of picked files once they were sent or dropped (the files the
/// user picked in place are left alone).
void discardPicked(Iterable<PickedFile> files) {
  for (final f in files) {
    unawaited(deleteTempCopy(f.path));
  }
}

/// Picks files. [kind]: "any" | "media" | "image" | "contacts" (.vcf / .csv).
Future<List<PickedFile>> pickFiles({String kind = 'any', bool multiple = false}) async {
  try {
    if (isMobilePlatform && (kind == 'media' || kind == 'image')) {
      final picker = ImagePicker();
      if (kind == 'image' && !multiple) {
        final f = await picker.pickImage(source: ImageSource.gallery);
        return f == null ? const [] : [PickedFile.fromPath(f.path, name: f.name)];
      }
      final list = kind == 'image' ? await picker.pickMultiImage() : await picker.pickMultipleMedia();
      return list.map((f) => PickedFile.fromPath(f.path, name: f.name)).toList();
    }
    final type = switch (kind) { 'media' => FileType.media, 'image' => FileType.image, 'contacts' => FileType.custom, _ => FileType.any };
    final ext = kind == 'contacts' ? const ['vcf', 'csv'] : null;
    final files = multiple
        ? await FilePicker.pickFiles(type: type, allowedExtensions: ext)
        : [?(await FilePicker.pickFile(type: type, allowedExtensions: ext))];
    final out = <PickedFile>[];
    for (final f in files) {
      final path = f.path ?? await _tempCopy(f.name, await f.readAsBytes());
      out.add(PickedFile.fromPath(path, name: f.name));
    }
    return out;
  } catch (e) {
    debugPrint('pick failed: $e');
    notifyError(e);
    return const [];
  }
}

/// Takes a photo with the camera (Android / iOS).
Future<PickedFile?> takePhoto() async {
  try {
    final f = await ImagePicker().pickImage(source: ImageSource.camera, maxWidth: 2560, maxHeight: 2560, imageQuality: 88);
    return f == null ? null : PickedFile.fromPath(f.path, name: f.name);
  } catch (e) {
    notifyError(e);
    return null;
  }
}

/// Lets the user choose where to save a decrypted file (system "save as" / document picker).
Future<bool> saveFileAs(String localPath, String fileName, String mime) async {
  final bytes = await loadMedia(localPath);
  if (bytes == null) return false;
  final uri = await FilePicker.saveFile(fileName: fileName, bytes: bytes, mimeType: mime);
  return uri != null;
}

/// Shares a media file: the share sheet needs a real file, so a temporary decrypted copy is
/// made and removed once the sheet is closed.
Future<void> shareFile(String localPath, String fileName, String mime) async {
  final copy = await exportMedia(localPath, fileName);
  try {
    await SharePlus.instance.share(ShareParams(files: [XFile(copy.path, mimeType: mime, name: fileName)]));
  } finally {
    // The receiving app may still be reading it for a moment.
    unawaited(Future.delayed(const Duration(minutes: 2), () => discardExport(copy)));
  }
}

Future<void> shareText(String text, {String? subject}) async {
  await SharePlus.instance.share(ShareParams(text: text, subject: subject));
}
