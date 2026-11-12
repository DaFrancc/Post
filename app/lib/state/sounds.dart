// Sounds: which sound plays for what, from a sound pack the user chooses,
// with any one sound swapped for a file of their own.
//
// Post ships its own default pack (assets/sounds, made by
// tool/make_sounds.py). A pack is any folder of sound files: a `pack.json`
// may name a file for each sound ({"sounds": {"message": "pop.flac"}}); else
// files are matched by name, so `call_ringing.flac` or `ring_incoming.wav`
// both ring. Sounds the pack lacks fall back to the default.

import 'dart:async';
import 'dart:convert';
import 'dart:io';

import 'package:flutter/foundation.dart';
import 'package:flutter_soloud/flutter_soloud.dart';

/// Something Post makes a sound for.
enum SoundEvent {
  ringIncoming('ring_incoming', 'Incoming call', 'Rings while someone calls you', ['call_ringing', 'ringing', 'ringtone']),
  ringOutgoing('ring_outgoing', 'Calling', 'While you wait for someone to answer', ['call_calling', 'calling', 'ringback']),
  join('join', 'Someone joins a call', 'When someone joins a call you are in', ['user_join', 'joined']),
  leave('leave', 'Someone leaves a call', 'When someone leaves a call you are in', ['user_leave', 'left']),
  disconnect('disconnect', 'You leave a call', 'When your call ends', ['call_end', 'hangup', 'hang_up']),
  mute('mute', 'Mute', 'When you mute your microphone', []),
  unmute('unmute', 'Unmute', 'When you unmute it', []),
  message('message', 'Message', 'A new message in another conversation', ['new_message', 'notification']),
  mention('mention', 'Mention', 'A message that mentions you', ['mentioned']);

  const SoundEvent(this.key, this.title, this.description, this.aliases);

  /// Its name in packs and settings, and its default file's name.
  final String key;
  final String title;
  final String description;

  /// Other names a pack's files may use for it.
  final List<String> aliases;

  /// Whether a file's name (no folder, no extension) means this sound: one
  /// of its names, perhaps followed by digits (`message1`, `mention3`).
  bool matches(String name) {
    final n = name.toLowerCase();
    return [key, ...aliases].any((a) => n == a || (n.startsWith(a) && RegExp(r'^\d+$').hasMatch(n.substring(a.length))));
  }
}

/// Sound files a pack (or the user) can use.
const soundExtensions = ['wav', 'flac', 'ogg', 'mp3', 'opus'];

class Sounds extends ChangeNotifier {
  Sounds({required this.readSetting, required this.writeSetting});

  final Future<String?> Function(String key) readSetting;
  final Future<void> Function(String key, String? value) writeSetting;

  /// The chosen pack's folder; null for Post's own.
  String? pack;

  /// Files the user chose for single sounds, over the pack.
  final Map<SoundEvent, String> custom = {};

  /// Sounds turned off.
  final Set<SoundEvent> off = {};

  /// 0 to 1.
  double volume = 0.8;

  /// Where each sound comes from now: a file path, or null for the default.
  final Map<SoundEvent, String?> _files = {};
  final Map<SoundEvent, AudioSource> _loaded = {};
  final Map<SoundEvent, SoundHandle> _looping = {};
  bool _ready = false;

  static const _packKey = 'sound.pack';
  static const _volumeKey = 'sound.volume';
  String _customKey(SoundEvent e) => 'sound.${e.key}.file';
  String _offKey(SoundEvent e) => 'sound.${e.key}.off';

  /// Start the audio engine and read the settings. Sounds are a nicety: if
  /// there is no audio device, Post carries on silently.
  Future<void> start() async {
    try {
      pack = await readSetting(_packKey);
      volume = double.tryParse(await readSetting(_volumeKey) ?? '') ?? volume;
      for (final e in SoundEvent.values) {
        final file = await readSetting(_customKey(e));
        if (file != null) custom[e] = file;
        if (await readSetting(_offKey(e)) == '1') off.add(e);
      }
    } catch (_) {}
    try {
      await SoLoud.instance.init(channels: Channels.stereo);
      SoLoud.instance.setGlobalVolume(volume);
      _ready = true;
    } catch (e) {
      debugPrint('sounds: no audio ($e)');
    }
    await _reload();
  }

  /// The file a pack has for each sound, by `pack.json` or by name.
  static Map<SoundEvent, String> readPack(String folder) {
    final out = <SoundEvent, String>{};
    final dir = Directory(folder);
    if (!dir.existsSync()) return out;
    final manifest = File('$folder/pack.json');
    if (manifest.existsSync()) {
      try {
        final named = (jsonDecode(manifest.readAsStringSync()) as Map<String, dynamic>)['sounds'] as Map<String, dynamic>? ?? {};
        for (final e in SoundEvent.values) {
          final f = named[e.key];
          // Only files inside the pack: no paths out of it.
          if (f is String && !f.contains('/') && File('$folder/$f').existsSync()) out[e] = '$folder/$f';
        }
      } catch (_) {}
    }
    final files = dir.listSync().whereType<File>().where((f) => soundExtensions.contains(f.path.split('.').last.toLowerCase())).toList()
      ..sort((a, b) => a.path.compareTo(b.path));
    for (final e in SoundEvent.values) {
      if (out.containsKey(e)) continue;
      for (final f in files) {
        final name = f.uri.pathSegments.last;
        if (e.matches(name.substring(0, name.lastIndexOf('.')))) {
          out[e] = f.path;
          break;
        }
      }
    }
    return out;
  }

  /// Where a sound comes from now: the user's file, the pack's, or null for
  /// Post's own.
  String? fileFor(SoundEvent e) => _files[e];

  Future<void> _reload() async {
    final fromPack = pack == null ? const <SoundEvent, String>{} : readPack(pack!);
    for (final e in SoundEvent.values) {
      final file = custom[e] ?? fromPack[e];
      _files[e] = file != null && File(file).existsSync() ? file : null;
    }
    await stopAll();
    if (_ready) {
      for (final s in _loaded.values) {
        unawaited(SoLoud.instance.disposeSource(s).catchError((_) {}));
      }
      _loaded.clear();
    }
    notifyListeners();
  }

  Future<AudioSource?> _source(SoundEvent e) async {
    if (!_ready) return null;
    final loaded = _loaded[e];
    if (loaded != null) return loaded;
    try {
      final file = _files[e];
      final source = file != null ? await SoLoud.instance.loadFile(file) : await SoLoud.instance.loadAsset('assets/sounds/${e.key}.wav');
      return _loaded[e] = source;
    } catch (err) {
      debugPrint('sounds: ${e.key}: $err');
      return null;
    }
  }

  /// Play a sound once.
  Future<void> play(SoundEvent e, {bool evenIfOff = false}) async {
    if (off.contains(e) && !evenIfOff) return;
    final s = await _source(e);
    if (s != null) SoLoud.instance.play(s);
  }

  /// Play a sound over and over until [stop].
  Future<void> loop(SoundEvent e) async {
    if (off.contains(e) || _looping.containsKey(e)) return;
    final s = await _source(e);
    if (s == null || _looping.containsKey(e)) return;
    _looping[e] = SoLoud.instance.play(s, looping: true);
  }

  Future<void> stop(SoundEvent e) async {
    final h = _looping.remove(e);
    if (h != null) await SoLoud.instance.stop(h).catchError((_) {});
  }

  Future<void> stopAll() async {
    for (final e in _looping.keys.toList()) {
      await stop(e);
    }
  }

  // ---------------------------------------------------------------- settings

  Future<void> choosePack(String? folder) async {
    pack = folder;
    await writeSetting(_packKey, folder);
    await _reload();
  }

  Future<void> chooseFile(SoundEvent e, String? file) async {
    if (file == null) {
      custom.remove(e);
    } else {
      custom[e] = file;
    }
    await writeSetting(_customKey(e), file);
    await _reload();
  }

  Future<void> setOn(SoundEvent e, bool on) async {
    on ? off.remove(e) : off.add(e);
    if (!on) await stop(e);
    notifyListeners();
    await writeSetting(_offKey(e), on ? null : '1');
  }

  void setVolume(double v) {
    volume = v.clamp(0, 1).toDouble();
    if (_ready) SoLoud.instance.setGlobalVolume(volume);
    notifyListeners();
  }

  /// Keep the volume once the slider lets go.
  Future<void> saveVolume() => writeSetting(_volumeKey, volume.toStringAsFixed(2));

  @override
  void dispose() {
    unawaited(stopAll());
    super.dispose();
  }
}
