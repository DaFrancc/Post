// Spike (design section 10, "Calls"): does LiveKit's Flutter SDK do
// end-to-end encrypted audio on Linux desktop, with a key per sender?
//
// Three participants join one room on a local LiveKit server in dev mode:
// Alice publishes her microphone, encrypted with her own key; Bob holds
// Alice's key; Eve holds a wrong one. Bob must decrypt, Eve must not.
//
//   podman run --rm -d --name livekit-spike --network host \
//     docker.io/livekit/livekit-server:v1.13.11 --dev --bind 127.0.0.1 --node-ip 127.0.0.1
//   flutter test integration_test/e2ee_test.dart -d linux

import 'dart:async';
import 'dart:convert';
import 'dart:math';
import 'dart:typed_data';

import 'package:crypto/crypto.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:integration_test/integration_test.dart';
import 'package:livekit_client/livekit_client.dart';

const url = 'ws://127.0.0.1:7880';

/// A LiveKit access token (HS256 JWT) from the dev server's key.
String token(String identity, String room) {
  String b64(Object o) => base64Url.encode(utf8.encode(jsonEncode(o))).replaceAll('=', '');
  final now = DateTime.now().millisecondsSinceEpoch ~/ 1000;
  final head = b64({'alg': 'HS256', 'typ': 'JWT'});
  final body = b64({
    'iss': 'devkey',
    'sub': identity,
    'nbf': now - 10,
    'exp': now + 600,
    'video': {'room': room, 'roomJoin': true, 'canPublish': true, 'canSubscribe': true},
  });
  final sig = base64Url.encode(Hmac(sha256, utf8.encode('secret')).convert(utf8.encode('$head.$body')).bytes).replaceAll('=', '');
  return '$head.$body.$sig';
}

Uint8List randomKey() => Uint8List.fromList(List.generate(32, (_) => Random.secure().nextInt(256)));

/// Join `room` as `identity`, holding `keys` (participant -> key).
Future<(Room, List<TrackE2EEStateEvent>)> join(String room, String identity, Map<String, Uint8List> keys) async {
  final provider = await BaseKeyProvider.create(sharedKey: false);
  for (final e in keys.entries) {
    await provider.setRawKey(e.value, participantId: e.key, keyIndex: 0);
  }
  final r = Room(roomOptions: RoomOptions(encryption: E2EEOptions(keyProvider: provider)));
  final states = <TrackE2EEStateEvent>[];
  r.events.on<TrackE2EEStateEvent>((e) {
    // ignore: avoid_print
    print('[$identity] ${e.participant.identity}: ${e.state}');
    states.add(e);
  });
  await r.connect(url, token(identity, room));
  return (r, states);
}

Future<bool> waitFor(bool Function() cond, {Duration timeout = const Duration(seconds: 20)}) async {
  final end = DateTime.now().add(timeout);
  while (DateTime.now().isBefore(end)) {
    if (cond()) return true;
    await Future<void>.delayed(const Duration(milliseconds: 200));
  }
  return cond();
}

void main() {
  IntegrationTestWidgetsFlutterBinding.ensureInitialized();

  testWidgets('per-sender encrypted audio on Linux', (t) async {
    final room = 'spike-${Random().nextInt(1 << 30)}';
    final alice = randomKey(), bob = randomKey(), eve = randomKey();

    final (a, _) = await join(room, 'alice', {'alice': alice});
    final (b, bobStates) = await join(room, 'bob', {'bob': bob, 'alice': alice});
    final (e, eveStates) = await join(room, 'eve', {'eve': eve, 'alice': randomKey()});

    final devices = await Hardware.instance.enumerateDevices();
    // ignore: avoid_print
    print('devices: ${devices.map((d) => '${d.kind}:${d.label} [${d.deviceId}]').toList()}');
    final mic = devices.where((d) => d.kind == 'audioinput').firstOrNull;
    await a.localParticipant!.setMicrophoneEnabled(true, audioCaptureOptions: AudioCaptureOptions(deviceId: mic?.deviceId));
    // ignore: avoid_print
    print('alice publishing: ${a.localParticipant!.audioTrackPublications.map((p) => '${p.sid} encrypted=${p.encryptionType}')}');

    // State events can be lost (the SDK listens only after the cryptor
    // runs), so judge by the audio: samples decoded versus concealed.
    bool hears(Room r) => r.remoteParticipants.values.any((p) => p.identity == 'alice' && p.audioTrackPublications.any((t) => t.subscribed));
    expect(await waitFor(() => hears(b) && hears(e)), isTrue, reason: 'Bob and Eve subscribe to Alice');
    final track = b.remoteParticipants.values.firstWhere((p) => p.identity == 'alice').audioTrackPublications.first;
    await Future<void>.delayed(const Duration(seconds: 8));

    Future<Map<String, num>> audio(Room r) async {
      // The receiver's own stats come back empty on Linux; the whole
      // subscriber connection's do not. Alice's is its only audio track.
      // ignore: invalid_use_of_internal_member
      final stats = [...?await r.engine.subscriber?.pc.getStats(), ...?await r.engine.publisher?.pc.getStats()];
      // ignore: avoid_print
      print('${r.localParticipant!.identity} stats: ${stats.length} reports: ${stats.map((s) => s.type).toSet()} '
          'inbound: ${stats.where((s) => s.type == 'inbound-rtp').map((s) => s.values).toList()}');
      final inbound = stats.firstWhere((s) => s.type == 'inbound-rtp' && s.values['kind'] == 'audio').values;
      num n(String k) => (inbound[k] as num?) ?? -1;
      return {
        'packets': n('packetsReceived'),
        'bytes': n('bytesReceived'),
        'samples': n('totalSamplesReceived'),
        'concealed': n('concealedSamples'),
        'energy': n('totalAudioEnergy'),
      };
    }

    // ignore: invalid_use_of_internal_member
    final sent = [...?await a.engine.publisher?.pc.getStats(), ...?await a.engine.subscriber?.pc.getStats()];
    // ignore: avoid_print
    print('alice stats: ${sent.map((s) => s.type).toSet()} outbound: ${sent.where((s) => s.type == 'outbound-rtp' || s.type == 'media-source').map((s) => s.values).toList()}');
    // ignore: avoid_print
    print('alice transport: ${sent.where((s) => s.type == 'transport' || s.type == 'candidate-pair').map((s) => '${s.type} ${s.values['dtlsState'] ?? s.values['state']} nominated=${s.values['nominated']} bytesSent=${s.values['bytesSent']}').toList()}');
    // ignore: invalid_use_of_internal_member
    print('alice pc states: publisher ${a.engine.publisher?.pc.connectionState} subscriber ${a.engine.subscriber?.pc.connectionState}');
    final bobAudio = await audio(b), eveAudio = await audio(e);
    double concealed(Map<String, num> s) => s['samples']! <= 0 ? 1 : s['concealed']! / s['samples']!;
    // ignore: avoid_print
    print('bob: $bobAudio  concealed ${(concealed(bobAudio) * 100).toStringAsFixed(1)}%  states ${bobStates.map((s) => s.state)}');
    // ignore: avoid_print
    print('eve: $eveAudio  concealed ${(concealed(eveAudio) * 100).toStringAsFixed(1)}%  states ${eveStates.map((s) => s.state)}');

    expect(track.encryptionType, EncryptionType.kGcm, reason: 'the server saw an encrypted track');
    expect(bobAudio['packets']! > 100 && eveAudio['packets']! > 100, isTrue, reason: 'both receive Alice\'s packets');
    expect(concealed(bobAudio), lessThan(0.2), reason: 'Bob, with Alice\'s key, decodes her audio');
    expect(concealed(eveAudio), greaterThan(0.8), reason: 'Eve, with the wrong key, gets nothing but concealment');

    for (final r in [a, b, e]) {
      await r.disconnect();
      await r.dispose();
    }
  });
}
