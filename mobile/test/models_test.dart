import 'dart:async';
import 'dart:convert';

import 'package:chatapp/core/backend.dart';
import 'package:chatapp/core/models.dart';
import 'package:flutter_test/flutter_test.dart';

void main() {
  group('models', () {
    test('key transparency status parses', () {
      final o = KtOverview.fromJson(jsonDecode('''{
        "enabled": true, "tree_size": 12, "checked_at": 1700000000000, "own_status": "alert", "monitored": 12,
        "alerts": [{"kind": "split_view", "account_id": "a1", "detail": "x", "at": 5, "evidence": {"ours": {}}}]
      }''') as Map<String, dynamic>);
      expect(o.enabled, true);
      expect(o.treeSize, 12);
      expect(o.ownStatus, 'alert');
      expect(o.alerts.single.kind, 'split_view');
      expect(o.alerts.single.accountId, 'a1');
      final c = KtContactStatus.fromJson({'account_id': 'b', 'status': 'verified', 'tree_size': null, 'checked_at': 3});
      expect(c.status, 'verified');
      expect(c.treeSize, null);
    });

    test('Conversation parses all fields', () {
      final c = Conversation.fromJson(jsonDecode('''{
        "id": "c1", "kind": "group", "title": "Familie", "avatar_path": null,
        "peer_account_id": null, "peer_number": null, "group_id": "g1", "member_count": 3,
        "is_admin": true, "left": false,
        "last_message": {"id": "m1", "kind": "image", "text": null, "sender_name": "Mama",
          "outgoing": false, "status": "delivered", "sent_at": 1700000000000, "deleted": false},
        "unread_count": 2, "muted_until": -1, "archived": false, "pinned": true,
        "disappearing_seconds": 86400, "blocked": false, "draft": "hi", "updated_at": 1700000000001
      }''') as Map<String, dynamic>);
      expect(c.isGroup, isTrue);
      expect(c.memberCount, 3);
      expect(c.lastMessage!.kind, 'image');
      expect(c.lastMessage!.senderName, 'Mama');
      expect(c.isMuted(), isTrue);
      expect(c.pinned, isTrue);
      expect(c.draft, 'hi');
      expect(c.sortTime, 1700000000000);
    });

    test('mute expiry', () {
      Conversation conv(int? until) => Conversation.fromJson({'id': 'x', 'muted_until': until});
      expect(conv(null).isMuted(), isFalse);
      expect(conv(-1).isMuted(), isTrue);
      expect(conv(2000).isMuted(1000), isTrue);
      expect(conv(2000).isMuted(3000), isFalse);
    });

    test('Message with attachments, reply, reactions, location and system event', () {
      final m = Message.fromJson(jsonDecode('''{
        "id": "m1", "conversation_id": "c1", "sender_account_id": "a1", "sender_name": "Maria",
        "outgoing": false, "kind": "voice", "text": null,
        "attachments": [{"id": "at1", "mime": "audio/ogg", "file_name": "v.ogg", "size": 1234,
          "width": null, "height": null, "duration_ms": 9000, "waveform": [0.1, 0.5, 1],
          "thumbnail": null, "local_path": "/tmp/v.ogg", "download_state": "done", "progress": null}],
        "location": {"lat": 45.1, "lng": 25.5, "name": "Cabana", "address": null},
        "reply_to": {"id": "m0", "sender_name": "You", "kind": "location", "text": "Cabana", "thumbnail": null},
        "reactions": [{"emoji": "👍", "account_id": "a2", "sender_name": "Tu", "is_me": true}],
        "status": "read", "sent_at": 1, "received_at": 2, "edited_at": 3, "deleted": false,
        "forwarded": true, "expires_at": null,
        "system": {"type": "group_renamed", "actor_name": "You", "target_names": [], "value": "X"}
      }''') as Map<String, dynamic>);
      expect(m.attachments.single.waveform, [0.1, 0.5, 1.0]);
      expect(m.attachments.single.durationMs, 9000);
      expect(m.location!.lat, 45.1);
      expect(m.replyTo!.senderName, 'You');
      expect(m.reactions.single.isMe, isTrue);
      expect(m.editedAt, 3);
      expect(m.forwarded, isTrue);
      expect(m.system!.type, 'group_renamed');
    });

    test('AppState with pending mnemonic and AccountDetails with username', () {
      final s = AppState.fromJson({
        'status': 'ready',
        'account': {'account_id': 'a', 'number': '1234567812345678', 'device_id': 1, 'server_url': 'http://x'},
        'default_server_url': 'http://x',
        'pending_mnemonic': ['alpha', 'beta'],
      });
      expect(s.ready, isTrue);
      expect(s.account!.number, '1234567812345678');
      expect(s.pendingMnemonic, ['alpha', 'beta']);
      final d = AccountDetails.fromJson({
        'account': {'account_id': 'a', 'number': '1', 'device_id': 1, 'server_url': 's'},
        'username': 'maria',
        'phone': null,
        'phone_verified': false,
        'discoverable_by_phone': false,
        'privacy': {'last_seen': 'nobody', 'read_receipts': false, 'typing_indicators': true},
        'backup_enabled': true,
      });
      expect(d.username, 'maria');
      expect(d.privacy.lastSeen, 'nobody');
      expect(d.privacy.readReceipts, isFalse);
      expect(d.copyWith(clearUsername: true).username, isNull);
    });

    test('GroupInfo, SafetyNumber, FoundAccount', () {
      final g = GroupInfo.fromJson({
        'conversation_id': 'c',
        'group_id': 'g',
        'name': 'N',
        'description': '',
        'members': [
          {'account_id': 'a', 'number': '1', 'display_name': 'A', 'role': 'admin', 'is_me': true},
        ],
        'my_role': 'admin',
        'settings': {'only_admins_send': true, 'only_admins_edit': false},
        'created_by': 'a',
        'created_at': 5,
      });
      expect(g.members.single.isAdmin, isTrue);
      expect(g.settings.onlyAdminsSend, isTrue);
      expect(g.settings.copyWith(onlyAdminsEdit: true).toJson(), {
        'only_admins_send': true,
        'only_admins_edit': true,
        'disappearing_seconds': 0,
        'hide_members': false,
        'member_names': 'number',
      });
      final sn = SafetyNumber.fromJson({'digits': '12345 67890 11111', 'qr_payload': 'x', 'verified': false});
      expect(sn.groups, ['12345', '67890', '11111']);
      final f = FoundAccount.fromJson({'account_id': 'a', 'number': '1', 'username': 'u', 'display_name': '@u', 'is_me': false});
      expect(f.username, 'u');
    });

    test('request encoding omits null optional fields', () {
      const a = OutgoingAttachment(dataBase64: 'AA==', fileName: 'v.ogg', mime: 'audio/ogg', kind: 'voice', durationMs: 900);
      expect(a.toJson(), {'data_base64': 'AA==', 'file_name': 'v.ogg', 'mime': 'audio/ogg', 'kind': 'voice', 'duration_ms': 900});
      expect(const ConversationSettingsPatch(setMuted: true).toJson(), {'muted_until': null});
      expect(const ConversationSettingsPatch(pinned: true).toJson(), {'pinned': true});
    });
  });

  group('errors', () {
    test('parseError splits code and message', () {
      final e = parseError('invalid_password: wrong password');
      expect(e.code, 'invalid_password');
      expect(e.message, 'wrong password');
    });

    test('parseError unwraps bridge exceptions', () {
      final e = parseError('AnyhowException(different_server: this contact uses another server)');
      expect(e.code, 'different_server');
      expect(e.message, 'this contact uses another server');
    });

    test('parseError falls back to internal', () {
      final e = parseError('Something: odd');
      expect(e.code, 'internal');
      expect(e.message, 'Something: odd');
    });
  });

  group('backend', () {
    test('sends JSON requests and decodes results', () async {
      final calls = <(String, String)>[];
      final b = Backend((cmd, req) async {
        calls.add((cmd, req));
        if (cmd == 'get_state') return '{"status":"no_account","account":null,"default_server_url":"http://s"}';
        if (cmd == 'mark_read') return 'null';
        throw 'not_found: nope';
      }, const Stream.empty());
      final s = await b.getState();
      expect(s.status, 'no_account');
      await b.markRead('c1');
      expect(calls[0], ('get_state', 'null'));
      expect(calls[1].$1, 'mark_read');
      expect(jsonDecode(calls[1].$2), {'conversation_id': 'c1'});
      await expectLater(b.getProfile(), throwsA(isA<CoreError>().having((e) => e.code, 'code', 'not_found')));
    });

    test('typed event streams', () async {
      final ctrl = StreamController<String>();
      final b = Backend((_, _) async => 'null', ctrl.stream);
      final typing = b.onTyping.first;
      final conn = b.onConnection.first;
      ctrl.add('{"event":"typing","payload":{"conversation_id":"c","account_id":"a","sender_name":"M","typing":true}}');
      ctrl.add('not json');
      ctrl.add('{"event":"connection","payload":"online"}');
      expect((await typing).senderName, 'M');
      expect(await conn, 'online');
      await ctrl.close();
    });

    test('media retention and expired attachments parse', () {
      final r = MediaRetention.fromJson(jsonDecode(
              '{"days": 7, "options": [1, 7, 14, 30], "max_days": 30, "last_check": {"removed": 3, "checked_at": 99}}')
          as Map<String, dynamic>);
      expect(r.days, 7);
      expect(r.options, [1, 7, 14, 30]);
      expect(r.lastCheck?.removed, 3);
      expect(MediaRetention.fromJson(const {}).days, 30, reason: 'an older core: the longest period');
      final a = Attachment.fromJson(const {
        'id': 'm-0', 'mime': 'image/jpeg', 'file_name': '', 'size': 10, 'download_state': 'expired', 'expired_at': 5
      });
      expect(a.isExpired, true);
      expect(a.expiredAt, 5);
      expect(Attachment.fromJson(const {'id': 'm-1', 'mime': 'image/jpeg', 'size': 1, 'download_state': 'done'}).isExpired, false);
      final p = MessagePreview.fromJson(const {'id': 'x', 'kind': 'voice', 'media_expired': true});
      expect(p.mediaExpired, true);
    });
  });
}
