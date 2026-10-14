import 'package:chatapp/util/format.dart';
import 'package:flutter_test/flutter_test.dart';

// Same cases as crates/chat-core/src/util.rs (password_policy): the app and the core must agree.
void main() {
  test('password rules', () {
    for (final weak in ['short1A!', 'alllowercaseletters', 'Password123!', 'Parola2024!!', 'aaaaaaaaaaaaaaaa', '123456789012!', 'Abcdef12345!']) {
      expect(passwordProblems(weak), isNotEmpty, reason: weak);
    }
    for (final ok in ['Mere-Pere-2026', 'corect horse battery staple', 'Ploaie de vara 77', 'x9!kQ2#vLm4p']) {
      expect(passwordProblems(ok), isEmpty, reason: ok);
    }
  });
}
