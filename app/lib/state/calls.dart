// Calls in the app (design section 10): which calls are live, the one
// ringing, and the one this device is in.
//
// The core decides everything about a call: who is in it, its tokens, and
// its media keys, which come from the conversation's encryption. This side
// joins LiveKit's room with them and installs the keys it is given. LiveKit
// only ever relays frames encrypted under those keys.

import 'dart:async';

import 'package:flutter/foundation.dart';
import 'package:flutter/services.dart';
import 'package:livekit_client/livekit_client.dart' as lk;
import 'package:post/src/rust/api/client.dart';
import 'package:post/src/rust/api/error.dart';

/// How long a DM call this device started waits for an answer.
const ringTimeout = Duration(seconds: 60);

/// After installing a new epoch's keys, how long before this device sends
/// under them, so everyone has them first (design section 10).
const _keySwitchGrace = Duration(seconds: 2);

/// The call this device is in.
class ActiveCall {
  ActiveCall._(this.join, this.conversation, this.room, this.keys);

  final CallJoinInfo join;
  final String conversation;
  final lk.Room room;
  final lk.BaseKeyProvider keys;

  String get id => join.call.id;
  String get participantId => join.participantId;

  /// When this device joined.
  final DateTime since = DateTime.now();

  bool muted = false;

  /// The key-ring slot the keys were last installed in.
  int keyIndex = -1;

  /// Participant IDs speaking right now.
  Set<String> speaking = {};

  /// Connected to LiveKit's room (not just announced in the conversation).
  bool connected = false;
}

class CallController extends ChangeNotifier {
  CallController(this.client, {required this.readSetting, required this.writeSetting});

  final Client client;
  final Future<String?> Function(String key) readSetting;
  final Future<void> Function(String key, String? value) writeSetting;

  /// The live calls, everywhere.
  List<CallInfo> calls = const [];

  /// The call this device is in, if any.
  ActiveCall? active;

  /// Joining or leaving is under way.
  bool busy = false;

  /// Why the last call action failed.
  String? error;

  /// Microphones, and the one chosen. On Linux the microphone must be named,
  /// or WebRTC records nothing (spikes/calls-e2ee/RESULTS.md).
  List<lk.MediaDevice> microphones = const [];
  String? microphone;
  static const _micKey = 'call.mic';

  Timer? _ringTimer;
  Timer? _ringSound;
  Timer? _ringCheck;
  Timer? _keyRefresh;
  lk.EventsListener<lk.RoomEvent>? _listener;
  bool _leaving = false;

  /// The call ringing for the user, if any.
  CallInfo? get ringing {
    for (final c in calls) {
      if (c.ringing && !c.joined) return c;
    }
    return null;
  }

  /// The live call in a conversation, if there is one.
  CallInfo? callIn(String conversation) {
    for (final c in calls) {
      if (c.conversation == conversation && !c.ended) return c;
    }
    return null;
  }

  /// The call this device is in, as the core sees it now.
  CallInfo? get activeInfo => active == null ? null : calls.where((c) => c.id == active!.id).firstOrNull ?? active!.join.call;

  // ---------------------------------------------------------------- events

  /// A call event: read the calls again, and keys if this device is in one.
  Future<void> onCallEvent() async {
    await refresh();
    await _installKeys();
  }

  /// The conversation of the active call changed: its epoch may have moved
  /// on, and with it every key.
  Future<void> onConversationChanged(String? conversation) async {
    if (active != null && (conversation == null || conversation == active!.conversation)) await _installKeys();
  }

  Future<void> refresh() async {
    try {
      calls = await client.calls();
    } catch (_) {
      return;
    }
    final a = active;
    // The call ended without this device hanging up: close the room.
    if (a != null && !_leaving && !calls.any((c) => c.id == a.id)) {
      await _close();
    }
    _updateRingSound();
    // A call rings for a minute; look again then, so it stops on time.
    _ringCheck?.cancel();
    if (ringing != null) _ringCheck = Timer(const Duration(seconds: 61), () => unawaited(refresh()));
    notifyListeners();
  }

  void _updateRingSound() {
    // A gentle repeat while something rings. Desktops play their own alert
    // sound; a ringtone of Post's own can come later.
    if (ringing != null && active == null) {
      _ringSound ??= Timer.periodic(const Duration(seconds: 2), (_) => SystemSound.play(SystemSoundType.alert));
    } else {
      _ringSound?.cancel();
      _ringSound = null;
    }
  }

  // ---------------------------------------------------------------- actions

  /// Start a call, ringing `ring` (usernames), or with nobody named everyone
  /// in a DM or small group.
  Future<void> start(String conversation, {List<String> ring = const []}) => _run(() async {
    final join = await client.startCall(conversation: conversation, video: false, ring: ring);
    await _enter(join, conversation);
    // A DM call nobody answers stops after a minute ("no one answered").
    _ringTimer = Timer(ringTimeout, () {
      final info = activeInfo;
      if (active?.id == join.call.id && info != null && info.participants.length <= 1) unawaited(hangUp());
    });
  });

  Future<void> answer(CallInfo call) => _run(() async {
    if (active != null) await _leaveActive();
    final join = await client.joinCall(conversation: call.conversation, call: call.id);
    await _enter(join, call.conversation);
  });

  /// Ring people into the call this device is in.
  Future<void> ringInto(List<String> usernames) => _run(() async {
    final a = active;
    if (a == null) return;
    await client.ringCall(conversation: a.conversation, call: a.id, usernames: usernames);
  });

  Future<void> decline(CallInfo call) => _run(() async {
    await client.declineCall(conversation: call.conversation, call: call.id);
    await refresh();
  });

  Future<void> hangUp() => _run(_leaveActive);

  void dismissError() {
    error = null;
    notifyListeners();
  }

  Future<void> setMuted(bool muted) async {
    final a = active;
    if (a == null) return;
    a.muted = muted;
    notifyListeners();
    await a.room.localParticipant?.setMicrophoneEnabled(!muted, audioCaptureOptions: _capture());
  }

  Future<void> chooseMicrophone(String deviceId) async {
    microphone = deviceId;
    notifyListeners();
    unawaited(writeSetting(_micKey, deviceId));
    final a = active;
    if (a != null && !a.muted) {
      // Republish from the new device.
      await a.room.localParticipant?.setMicrophoneEnabled(false);
      await a.room.localParticipant?.setMicrophoneEnabled(true, audioCaptureOptions: _capture());
    }
  }

  Future<void> loadMicrophones() async {
    try {
      final devices = await lk.Hardware.instance.enumerateDevices();
      microphones = devices.where((d) => d.kind == 'audioinput').toList();
      final saved = await readSetting(_micKey);
      microphone = microphones.any((d) => d.deviceId == saved) ? saved : microphones.firstOrNull?.deviceId;
      notifyListeners();
    } catch (_) {}
  }

  Future<void> _run(Future<void> Function() action) async {
    if (busy) return;
    busy = true;
    error = null;
    notifyListeners();
    try {
      await action();
    } catch (e) {
      error = e is PostError ? e.message : 'The call could not connect. Try again.';
      debugPrint('call: $e');
    } finally {
      busy = false;
      await refresh();
    }
  }

  lk.AudioCaptureOptions _capture() => lk.AudioCaptureOptions(deviceId: microphone);

  /// Join LiveKit's room with this device's token and the call's keys.
  Future<void> _enter(CallJoinInfo join, String conversation) async {
    if (microphones.isEmpty) await loadMicrophones();
    final keys = await lk.BaseKeyProvider.create(sharedKey: false);
    // Speech needs far less than the host's cap allows; stay under both.
    final bitrate = (join.maxBitrateKbps * 1000).clamp(16000, 48000);
    final room = lk.Room(
      roomOptions: lk.RoomOptions(
        encryption: lk.E2EEOptions(keyProvider: keys),
        defaultAudioPublishOptions: lk.AudioPublishOptions(encoding: lk.AudioEncoding(maxBitrate: bitrate)),
      ),
    );
    final a = ActiveCall._(join, conversation, room, keys);
    active = a;
    notifyListeners();
    await _installKeys();
    _listener = room.createListener()
      ..on<lk.ActiveSpeakersChangedEvent>((e) {
        a.speaking = {for (final p in e.speakers) p.identity};
        notifyListeners();
      })
      ..on<lk.ParticipantConnectedEvent>((_) => notifyListeners())
      ..on<lk.ParticipantDisconnectedEvent>((_) => notifyListeners())
      ..on<lk.RoomDisconnectedEvent>((_) {
        // Dropped, not hung up: say so and leave properly.
        if (!_leaving && active == a) {
          error = 'The call dropped.';
          unawaited(hangUp());
        }
      });
    try {
      await room.connect(join.url, join.token);
      a.connected = true;
      await room.localParticipant?.setMicrophoneEnabled(true, audioCaptureOptions: _capture());
      await room.e2eeManager?.setKeyIndex(a.keyIndex);
    } catch (e) {
      // Announced but could not connect: leave, so nobody waits for us.
      await _leaveActive();
      rethrow;
    }
    // A safety net under the events: keys move on with the epoch.
    _keyRefresh = Timer.periodic(const Duration(seconds: 15), (_) => unawaited(_installKeys()));
    notifyListeners();
  }

  /// Install the call's keys for this epoch; when the epoch has moved on,
  /// switch this device's sending to the new slot after a short grace.
  Future<void> _installKeys() async {
    final a = active;
    if (a == null) return;
    final CallKeySet set;
    try {
      set = await client.callKeys(conversation: a.conversation, call: a.id);
    } catch (_) {
      return;
    }
    for (final k in set.keys) {
      await a.keys.setRawKey(Uint8List.fromList(k.key), participantId: k.participantId, keyIndex: set.keyIndex);
    }
    if (set.keyIndex != a.keyIndex) {
      final first = a.keyIndex < 0;
      a.keyIndex = set.keyIndex;
      if (!first && a.connected) {
        Timer(_keySwitchGrace, () {
          if (active == a && a.keyIndex == set.keyIndex) unawaited(a.room.e2eeManager?.setKeyIndex(set.keyIndex));
        });
      }
    }
  }

  Future<void> _leaveActive() async {
    final a = active;
    if (a == null) return;
    _leaving = true;
    try {
      await client.leaveCall(conversation: a.conversation, call: a.id);
    } catch (_) {
      // Leaving the room still matters.
    } finally {
      await _close();
      _leaving = false;
    }
  }

  Future<void> _close() async {
    final a = active;
    active = null;
    _ringTimer?.cancel();
    _keyRefresh?.cancel();
    // Off the screen at once; LiveKit can take a moment to let go.
    notifyListeners();
    final listener = _listener;
    _listener = null;
    await listener?.dispose();
    if (a != null) {
      try {
        await a.room.disconnect();
        await a.room.dispose();
      } catch (_) {}
    }
  }

  bool _disposed = false;

  @override
  void notifyListeners() {
    if (!_disposed) super.notifyListeners();
  }

  @override
  void dispose() {
    _disposed = true;
    _ringTimer?.cancel();
    _ringSound?.cancel();
    _ringCheck?.cancel();
    _keyRefresh?.cancel();
    unawaited(_close());
    super.dispose();
  }
}
