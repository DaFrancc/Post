import 'dart:io';

import 'package:flutter_test/flutter_test.dart';
import 'package:post/state/sounds.dart';

void main() {
  test('files are matched to sounds by name', () {
    expect(SoundEvent.message.matches('message1'), isTrue);
    expect(SoundEvent.mention.matches('mention3'), isTrue);
    expect(SoundEvent.ringIncoming.matches('call_ringing'), isTrue);
    expect(SoundEvent.ringOutgoing.matches('call_calling'), isTrue);
    expect(SoundEvent.join.matches('user_join'), isTrue);
    expect(SoundEvent.mute.matches('mute'), isTrue);
    // Not a near miss: a themed variant, or a name that merely starts alike.
    expect(SoundEvent.mute.matches('halloween_mute'), isFalse);
    expect(SoundEvent.message.matches('messages_archive'), isFalse);
    expect(SoundEvent.mute.matches('unmute'), isFalse);
  });

  test('a pack folder: by name, or by pack.json, and nothing outside it', () {
    final dir = Directory.systemTemp.createTempSync('post-pack-');
    addTearDown(() => dir.deleteSync(recursive: true));
    for (final name in ['call_ringing.flac', 'message1.flac', 'halloween_message1.flac', 'mute.flac', 'unmute.flac', 'notes.txt']) {
      File('${dir.path}/$name').writeAsStringSync('');
    }
    var pack = Sounds.readPack(dir.path);
    expect(pack[SoundEvent.ringIncoming], endsWith('/call_ringing.flac'));
    expect(pack[SoundEvent.message], endsWith('/message1.flac'));
    expect(pack[SoundEvent.mute], endsWith('/mute.flac'));
    expect(pack[SoundEvent.unmute], endsWith('/unmute.flac'));
    expect(pack.containsKey(SoundEvent.join), isFalse, reason: 'missing: the default plays');

    // pack.json wins, and cannot point outside the folder.
    File('${dir.path}/pack.json').writeAsStringSync('{"sounds": {"message": "halloween_message1.flac", "join": "../../etc/passwd"}}');
    pack = Sounds.readPack(dir.path);
    expect(pack[SoundEvent.message], endsWith('/halloween_message1.flac'));
    expect(pack.containsKey(SoundEvent.join), isFalse);
  });
}
