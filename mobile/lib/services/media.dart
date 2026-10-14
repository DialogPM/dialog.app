// Media files (attachments, avatars) are encrypted at rest by the core. Images are decrypted
// into memory only; a player, the share sheet or "save as" that needs a real file gets a
// temporary decrypted copy in the app's private cache, removed after use and at every start.

import 'dart:async';
import 'dart:io';
import 'dart:math';
import 'dart:typed_data';

import 'dart:ui' as ui;

import 'package:flutter/material.dart';
import 'package:path_provider/path_provider.dart';

import '../app/globals.dart';
import 'security.dart';

const _maxCacheBytes = 64 * 1024 * 1024;
// Map literals keep insertion order: the first key is the least recently used.
final _cache = <String, Uint8List>{};
final _inflight = <String, Future<Uint8List?>>{};
var _cacheBytes = 0;

/// Decrypted bytes of a media file (null when missing or unreadable).
Future<Uint8List?> loadMedia(String path) {
  final hit = _cache.remove(path);
  if (hit != null) {
    _cache[path] = hit; // most recently used last
    return Future.value(hit);
  }
  return _inflight[path] ??= () async {
    try {
      final bytes = await backend.readMedia(path);
      _cache[path] = bytes;
      _cacheBytes += bytes.length;
      while (_cacheBytes > _maxCacheBytes && _cache.length > 1) {
        final oldest = _cache.keys.first;
        _cacheBytes -= _cache.remove(oldest)!.length;
      }
      return bytes;
    } catch (_) {
      // Not one of our media files (e.g. a file the user just picked): read it as it is.
      final f = File(path);
      return f.existsSync() ? f.readAsBytes() : null;
    } finally {
      unawaited(Future(() => _inflight.remove(path)));
    }
  }();
}

// ---------------------------------------------------------------------------
// Decoding limits ("decompression bombs")
// ---------------------------------------------------------------------------

/// Images larger than this are never decoded (like Signal: a few KB of PNG can describe a
/// 50 000 x 50 000 image that takes gigabytes once decoded).
const kMaxImagePixels = 40 * 1000 * 1000;
const kMaxImageSide = 16384;

/// Decoded images are never larger than this on their longest side (enough for the lightbox).
const kMaxDisplaySide = 4096;

/// Size read from the image header only (nothing is decoded); null when unreadable.
Future<({int width, int height})?> imageHeaderSize(Uint8List bytes) async {
  ui.ImmutableBuffer? buffer;
  ui.ImageDescriptor? desc;
  try {
    buffer = await ui.ImmutableBuffer.fromUint8List(bytes);
    desc = await ui.ImageDescriptor.encoded(buffer);
    return (width: desc.width, height: desc.height);
  } catch (_) {
    return null;
  } finally {
    desc?.dispose();
    buffer?.dispose();
  }
}

bool imageSizeAllowed(int width, int height) =>
    width > 0 && height > 0 && width <= kMaxImageSide && height <= kMaxImageSide && width * height <= kMaxImagePixels;

// Rejected images are stored as 0 x 0.
final _checked = Expando<({int width, int height})>();
final _checking = Expando<Future<({int width, int height})?>>();

/// Header size of [bytes] when the image may be decoded, null otherwise (cached per buffer).
Future<({int width, int height})?> safeImageSize(Uint8List bytes) {
  if (_checking[bytes] == null) {
    _checking[bytes] = imageHeaderSize(bytes).then((s) {
      final ok = s != null && imageSizeAllowed(s.width, s.height) ? s : null;
      _checked[bytes] = ok ?? (width: 0, height: 0);
      return ok;
    });
  }
  return _checking[bytes]!;
}

/// Image.memory for bytes received from others: the header is checked first (size limits
/// above) and the image is decoded at most at [kMaxDisplaySide] (or [cacheWidth]).
class SafeMemoryImage extends StatefulWidget {
  const SafeMemoryImage(this.bytes, {super.key, this.fit, this.width, this.height, this.cacheWidth, this.placeholder});
  final Uint8List bytes;
  final BoxFit? fit;
  final double? width;
  final double? height;
  final int? cacheWidth;
  final Widget? placeholder;

  @override
  State<SafeMemoryImage> createState() => _SafeMemoryImageState();
}

class _SafeMemoryImageState extends State<SafeMemoryImage> {
  ({int width, int height})? _size;
  bool _done = false;

  @override
  void initState() {
    super.initState();
    _check();
  }

  @override
  void didUpdateWidget(SafeMemoryImage old) {
    super.didUpdateWidget(old);
    if (!_same(old.bytes, widget.bytes)) _check();
  }

  // Thumbnails are decoded from base64 at every build: equal bytes keep the checked size.
  static bool _same(Uint8List a, Uint8List b) {
    if (identical(a, b)) return true;
    if (a.length != b.length || a.length > 256 * 1024) return false;
    for (var i = 0; i < a.length; i++) {
      if (a[i] != b[i]) return false;
    }
    return true;
  }

  void _check() {
    final bytes = widget.bytes;
    final known = _checked[bytes];
    if (known != null) {
      _size = known.width == 0 ? null : known;
      _done = true;
      return;
    }
    _done = false;
    unawaited(safeImageSize(bytes).then((s) {
      if (mounted && identical(widget.bytes, bytes)) {
        setState(() {
          _size = s;
          _done = true;
        });
      }
    }));
  }

  @override
  Widget build(BuildContext context) {
    final s = _size;
    if (!_done) return widget.placeholder ?? SizedBox(width: widget.width, height: widget.height);
    if (s == null) {
      return SizedBox(
        width: widget.width,
        height: widget.height,
        child: const Center(child: Icon(Icons.broken_image_outlined, color: Color(0x80808080))),
      );
    }
    int? cw = widget.cacheWidth;
    int? ch;
    if (cw == null && (s.width > kMaxDisplaySide || s.height > kMaxDisplaySide)) {
      if (s.width >= s.height) {
        cw = kMaxDisplaySide;
      } else {
        ch = kMaxDisplaySide;
      }
    }
    return Image.memory(widget.bytes,
        fit: widget.fit, width: widget.width, height: widget.height, cacheWidth: cw, cacheHeight: ch, gaplessPlayback: true);
  }
}

/// Image from an encrypted media file.
class MediaImage extends StatefulWidget {
  const MediaImage(this.path, {super.key, this.fit, this.width, this.height, this.cacheWidth, this.placeholder});
  final String path;
  final BoxFit? fit;
  final double? width;
  final double? height;
  final int? cacheWidth;
  final Widget? placeholder;

  @override
  State<MediaImage> createState() => _MediaImageState();
}

class _MediaImageState extends State<MediaImage> {
  Uint8List? _bytes;

  @override
  void initState() {
    super.initState();
    _load();
  }

  @override
  void didUpdateWidget(MediaImage old) {
    super.didUpdateWidget(old);
    if (old.path != widget.path) _load();
  }

  void _load() {
    final path = widget.path;
    unawaited(loadMedia(path).then((b) {
      if (mounted && widget.path == path) setState(() => _bytes = b);
    }));
  }

  @override
  Widget build(BuildContext context) {
    final b = _bytes;
    if (b == null) return widget.placeholder ?? SizedBox(width: widget.width, height: widget.height);
    return SafeMemoryImage(b,
        fit: widget.fit, width: widget.width, height: widget.height, cacheWidth: widget.cacheWidth, placeholder: widget.placeholder);
  }
}

// ---------------------------------------------------------------------------
// Temporary files
// ---------------------------------------------------------------------------

String? _tempRoot;

/// The app's own folder for temporary plaintext files (picked-file copies, contact exports,
/// voice drafts, decrypted copies). Emptied at every start and when the app locks. On iOS its
/// files are protected while the device is locked (NSFileProtectionCompleteUnlessOpen).
Future<Directory> appTempDir() async {
  final root = _tempRoot ??= '${(await getTemporaryDirectory()).path}/dialog-tmp';
  final dir = Directory(root);
  if (!dir.existsSync()) {
    dir.createSync(recursive: true);
    await protectDirectory(dir.path, 'completeUnlessOpen');
  }
  return dir;
}

/// A new unique path inside [appTempDir] for a file named [name].
Future<String> appTempPath(String name) async {
  final safe = name.replaceAll(RegExp(r'[/\\:*?"<>|]'), '_');
  return '${(await appTempDir()).path}/${DateTime.now().microsecondsSinceEpoch}-${safe.isEmpty ? 'file' : safe}';
}

/// Deletes [path] if it is a temporary copy (ours or one made by a file / image picker plugin),
/// never a file of the user's.
Future<void> deleteTempCopy(String path) async {
  try {
    final f = File(path);
    if (!f.existsSync()) return;
    final real = f.resolveSymbolicLinksSync();
    final roots = <String>[];
    for (final r in await _tempRoots()) {
      try {
        roots.add(Directory(r).resolveSymbolicLinksSync());
      } catch (_) {}
    }
    if (!roots.any((r) => real.startsWith('$r/'))) return;
    await File(real).delete();
  } catch (_) {}
}

/// The platform's temporary directories (iOS: Library/Caches and tmp).
Future<Set<String>> _tempRoots() async => {(await getTemporaryDirectory()).path, Directory.systemTemp.path};

/// Removes the app's temporary files: everything at startup, files older than [olderThan] when
/// the app locks (a file just picked for the send screen is kept). Only our own folder and the
/// copies made by the picker plugins are touched, never the engine's caches.
Future<void> wipeAppTemp({Duration? olderThan}) async {
  final now = DateTime.now();
  bool old(FileSystemEntity e) {
    if (olderThan == null) return true;
    try {
      return now.difference(e.statSync().modified) > olderThan;
    } catch (_) {
      return true;
    }
  }

  Future<void> wipe(FileSystemEntity e) async {
    try {
      if (old(e)) await e.delete(recursive: true);
    } catch (_) {}
  }

  for (final r in await _tempRoots()) {
    try {
      final dir = Directory(r);
      if (!dir.existsSync()) continue;
      for (final e in dir.listSync(followLinks: false)) {
        final name = e.uri.pathSegments.where((s) => s.isNotEmpty).last;
        if (name == 'dialog-tmp' && e is Directory) {
          for (final f in e.listSync(followLinks: false)) {
            // Decrypted copies in use (player, share sheet) are removed by their owner.
            if (olderThan != null && f.uri.pathSegments.where((s) => s.isNotEmpty).last == 'media-open') continue;
            await wipe(f);
          }
        } else if (name == 'media-open' && olderThan == null) {
          await wipe(e);
        } else if (name == 'file_picker' ||
            name.startsWith('image_picker') ||
            // Files of older versions, written straight into the temporary directory.
            RegExp(r'^(pick-\d+-|contacts-\d+\.vcf$|voice-\d+\.m4a$)').hasMatch(name)) {
          await wipe(e);
        }
      }
    } catch (_) {}
  }
}

Future<Directory> _exportRoot() async => Directory('${(await appTempDir()).path}/media-open');

/// Decrypted copies in use (player, viewer, share sheet): the sweep leaves them alone.
final _openExports = <String>{};

/// A temporary decrypted copy of a media file, for code that needs a real file.
/// Delete it with [discardExport] when done. With [inUse] false (handed to another app that
/// reads it at once) the sweep removes it after two minutes.
Future<File> exportMedia(String path, String fileName, {bool inUse = true}) async {
  final dir = Directory('${(await _exportRoot()).path}/${Random.secure().nextInt(1 << 32).toRadixString(16)}');
  await dir.create(recursive: true);
  if (inUse) _openExports.add(dir.path);
  final safe = fileName.replaceAll(RegExp(r'[/\\:*?"<>|]'), '_');
  final dest = File('${dir.path}/${safe.isEmpty ? 'file' : safe}');
  await backend.exportMedia(path, dest.path);
  return dest;
}

Future<void> discardExport(File f) async {
  _openExports.remove(f.parent.path);
  try {
    await f.parent.delete(recursive: true);
  } catch (_) {}
}


// ---------------------------------------------------------------------------
// Media check (the app's side of crates/chat-core/src/engine/retention.rs)
// ---------------------------------------------------------------------------

/// Decrypted copies not in use are removed after this long.
const kStaleCopyAge = Duration(minutes: 2);

/// Removes plaintext media this app wrote outside the core's folders: decrypted copies
/// (`media-open`) older than [kStaleCopyAge] that are not in use, and anything in our temporary
/// folder or the picker plugins' caches (picked copies, voice drafts, contact exports) older than
/// the retention period. Also drops decrypted bytes cached in memory for files that are gone.
/// Returns the number of files removed.
Future<int> sweepTempMedia(Duration retention) async {
  final now = DateTime.now();
  var removed = 0;
  Duration age(FileSystemEntity e) {
    try {
      return now.difference(e.statSync().modified);
    } catch (_) {
      return Duration.zero;
    }
  }

  Future<void> remove(FileSystemEntity e) async {
    try {
      if (e is Directory) {
        removed += e.listSync(recursive: true, followLinks: false).whereType<File>().length;
      } else {
        removed++;
      }
      await e.delete(recursive: true);
    } catch (_) {}
  }

  String nameOf(FileSystemEntity e) => e.uri.pathSegments.where((s) => s.isNotEmpty).last;

  // Newest file inside a folder (a copy folder holds one file).
  Duration folderAge(Directory d) {
    var newest = age(d);
    try {
      for (final f in d.listSync(recursive: true, followLinks: false)) {
        final a = age(f);
        if (a < newest) newest = a;
      }
    } catch (_) {}
    return newest;
  }

  try {
    final ours = await appTempDir();
    for (final e in ours.listSync(followLinks: false)) {
      if (e is Directory && nameOf(e) == 'media-open') {
        for (final copy in e.listSync(followLinks: false)) {
          if (_openExports.contains(copy.path)) continue;
          if ((copy is Directory ? folderAge(copy) : age(copy)) > kStaleCopyAge) await remove(copy);
        }
      } else if ((e is Directory ? folderAge(e) : age(e)) > retention) {
        await remove(e);
      }
    }
  } catch (_) {}
  for (final r in await _tempRoots()) {
    try {
      final dir = Directory(r);
      if (!dir.existsSync()) continue;
      for (final e in dir.listSync(followLinks: false)) {
        final name = nameOf(e);
        final pluginCache = name == 'file_picker' || name.startsWith('image_picker');
        final legacy = RegExp(r'^(pick-\d+-|contacts-\d+\.vcf$|voice-\d+\.m4a$)').hasMatch(name);
        if (name == 'media-open' && e is Directory) {
          // Copies of older versions, straight in the temporary directory.
          if (folderAge(e) > kStaleCopyAge) await remove(e);
        } else if (pluginCache && e is Directory) {
          for (final f in e.listSync(followLinks: false)) {
            if ((f is Directory ? folderAge(f) : age(f)) > retention) await remove(f);
          }
        } else if ((pluginCache || legacy) && age(e) > retention) {
          await remove(e);
        }
      }
    } catch (_) {}
  }
  // Decrypted bytes of media that was erased stay out of memory too.
  for (final path in _cache.keys.toList()) {
    if (!File(path).existsSync()) _cacheBytes -= _cache.remove(path)!.length;
  }
  return removed;
}
