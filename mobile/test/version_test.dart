import 'dart:io';

import 'package:chatapp/app/globals.dart';
import 'package:chatapp/util/version.dart';
import 'package:flutter_test/flutter_test.dart';

void main() {
  test('kAppVersion is the version of pubspec.yaml', () {
    final line = File('pubspec.yaml').readAsLinesSync().firstWhere((l) => l.startsWith('version:'));
    expect(kAppVersion, line.substring('version:'.length).trim().split('+').first);
  });

  test('plain versions are compared numerically, part by part', () {
    expect(parseVersion('1.0.1'), [1, 0, 1]);
    for (final bad in ['', '1.0', '1.0.0.0', 'v1.0.0', '1.0.x', '1.0.12345', ' 1.0.0', null]) {
      expect(parseVersion(bad), isNull, reason: '$bad');
    }
    expect(isOlderVersion('1.0.1', '1.0.2'), isTrue);
    expect(isOlderVersion('1.0.9', '1.0.10'), isTrue);
    expect(isOlderVersion('0.99.99', '1.0.0'), isTrue);
    expect(isOlderVersion('1.0.1', '1.0.1'), isFalse);
    expect(isOlderVersion('1.0.2', '1.0.1'), isFalse);
    // Never an update for something that is not a version.
    expect(isOlderVersion('1.0.1', 'latest'), isFalse);
    expect(isOlderVersion('1.0.1', null), isFalse);
  });
}
