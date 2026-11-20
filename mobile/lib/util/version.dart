// Comparison of plain "major.minor.patch" versions (the app's own and the ones a server
// announces), like app/src/lib/version.ts.

final _plain = RegExp(r'^(\d{1,4})\.(\d{1,4})\.(\d{1,4})$');

/// The three numbers of a plain version, or null when [v] is anything else.
List<int>? parseVersion(String? v) {
  final m = _plain.firstMatch(v ?? '');
  return m == null ? null : [int.parse(m.group(1)!), int.parse(m.group(2)!), int.parse(m.group(3)!)];
}

/// Whether version [a] is older than [b]; false when either is not a plain version.
bool isOlderVersion(String? a, String? b) {
  final x = parseVersion(a), y = parseVersion(b);
  if (x == null || y == null) return false;
  for (var i = 0; i < 3; i++) {
    if (x[i] != y[i]) return x[i] < y[i];
  }
  return false;
}
