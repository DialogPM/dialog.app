import 'package:chatapp/services/user_links.dart';
import 'package:flutter_test/flutter_test.dart';

void main() {
  test('a username link leads to its name', () {
    expect(usernameOfLink('https://dialog.pm/@maria'), 'maria');
    expect(usernameOfLink('https://dialog.pm/@Maria.Pop_1/'), 'maria.pop_1');
    expect(usernameOfLink('https://DIALOG.PM/@maria?lang=ro'), 'maria');
    expect(usernameOfLink('dialog://u/maria.01'), 'maria.01');
    expect(userLink('maria'), 'https://dialog.pm/@maria');
    expect(usernameOfLink(userLink('maria.01')), 'maria.01');
  });

  test('anything else is not a username link', () {
    for (final link in [
      'http://dialog.pm/@maria', // not https
      'https://dialog.pm.evil.example/@maria',
      'https://evil.example/@maria',
      'https://dialog.pm/maria',
      'https://dialog.pm/@maria/extra',
      'https://dialog.pm/@ma', // too short
      'https://dialog.pm/@1maria', // must start with a letter
      'https://dialog.pm/@maria..pop',
      'https://dialog.pm/@maria%20pop',
      'https://dialog.pm/@${'a' * 33}',
      'dialog://x/maria',
      'dialog://u/maria/extra',
      'chatapp://u?s=x#abc',
      'not a link',
      '',
    ]) {
      expect(usernameOfLink(link), isNull, reason: link);
    }
  });
}
