// Spellchecking, as Discord does it: on by default, in the language of the
// system, with more languages a click away (downloaded on demand from the
// dictionaries Chromium ships, checked against pinned hashes), and a
// personal dictionary of words you added. Settings live in the profile's
// encrypted settings; dictionaries in a folder shared by every profile.

import 'dart:async';
import 'dart:io';
import 'dart:typed_data';

import 'package:flutter/foundation.dart';
import 'package:post/src/rust/api/client.dart';
import 'package:post/src/rust/api/spell.dart';

class Spelling extends ChangeNotifier {
  Spelling(this.client, this.dir) {
    unawaited(_start());
  }

  final Client client;

  /// Where dictionaries are kept.
  final String dir;

  /// Whether to check at all.
  bool enabled = true;

  /// The chosen dictionaries, by code (`en_US`).
  List<String> languages = const [];

  /// Every dictionary on offer, with whether it is downloaded.
  List<SpellLanguage> catalog = const [];

  /// Words you added: never marked.
  Set<String> words = {};

  /// Dictionaries downloading now.
  final downloading = <String>{};

  /// Why the last download failed, if it did.
  String? error;

  Speller? _speller;
  bool _disposed = false;

  /// Whether checks can run: on, with at least one dictionary loaded.
  bool get ready => enabled && _speller != null;

  static const _onKey = 'spell.on';
  static const _languagesKey = 'spell.languages';
  static const _wordsKey = 'spell.words';

  Future<void> _start() async {
    try {
      enabled = await client.setting(key: _onKey) != '0';
      final saved = await client.setting(key: _languagesKey);
      words = {...?(await client.setting(key: _wordsKey))?.split('\n').where((w) => w.isNotEmpty)};
      catalog = await spellLanguages(dir: dir);
      final offered = {for (final l in catalog) l.code};
      languages = saved == null ? [_systemLanguage()] : saved.split(',').where(offered.contains).toList();
      _notify();
      await _load();
    } catch (e) {
      error = '$e';
      _notify();
    }
  }

  /// The dictionary for the system's language: its exact one (en_GB), or
  /// another of the same language (en_US for en_IE), or English.
  String _systemLanguage() {
    final locale = Platform.localeName.split('.').first.replaceAll('-', '_');
    final codes = [for (final l in catalog) l.code];
    if (codes.contains(locale)) return locale;
    final language = locale.split('_').first;
    for (final c in codes) {
      if (c == language || c.startsWith('${language}_')) return c;
    }
    return 'en_US';
  }

  bool installed(String code) => catalog.any((l) => l.code == code && l.installed);

  /// Download what is chosen and missing, then load the chosen dictionaries.
  Future<void> _load() async {
    if (!enabled) {
      _speller = null;
      _notify();
      return;
    }
    for (final code in languages) {
      if (!installed(code)) await download(code);
    }
    _speller = await Speller.load(dir: dir, languages: languages, words: words.toList());
    _notify();
  }

  /// Fetch and install a dictionary. Google learns only that a language
  /// pack was downloaded.
  Future<void> download(String code) async {
    if (!downloading.add(code)) return;
    error = null;
    _notify();
    final http = HttpClient()..connectionTimeout = const Duration(seconds: 15);
    try {
      final urls = await spellUrls(code: code);
      Future<Uint8List> get(String url) async {
        final response = await (await http.getUrl(Uri.parse(url))).close();
        if (response.statusCode != 200) throw HttpException('status ${response.statusCode}', uri: Uri.parse(url));
        final bytes = BytesBuilder(copy: false);
        await response.forEach(bytes.add);
        return bytes.takeBytes();
      }

      final aff = await get(urls[0]);
      final dic = await get(urls[1]);
      await spellInstall(dir: dir, code: code, aff: aff, dic: dic);
      catalog = await spellLanguages(dir: dir);
    } catch (e) {
      error = "Couldn't download that dictionary. Check your connection and try again.";
    } finally {
      http.close(force: true);
      downloading.remove(code);
      _notify();
    }
  }

  /// The misspelled words in `text`, or none when off or not ready.
  Future<List<Misspelling>> check(String text) async {
    final s = _speller;
    if (!enabled || s == null || text.isEmpty) return const [];
    return s.check(text: text);
  }

  Future<List<String>> suggest(String word) async => (await _speller?.suggest(word: word)) ?? const [];

  Future<void> addWord(String word) async {
    if (!words.add(word)) return;
    await _speller?.addWord(word: word);
    await client.setSetting(key: _wordsKey, value: words.join('\n'));
    _notify();
  }

  Future<void> removeWord(String word) async {
    if (!words.remove(word)) return;
    await _speller?.removeWord(word: word);
    await client.setSetting(key: _wordsKey, value: words.isEmpty ? null : words.join('\n'));
    _notify();
  }

  Future<void> setEnabled(bool on) async {
    enabled = on;
    _notify();
    await client.setSetting(key: _onKey, value: on ? null : '0');
    await _load();
  }

  /// Choose a dictionary or stop using it. Choosing one not downloaded
  /// downloads it.
  Future<void> setLanguage(String code, bool on) async {
    languages = on ? [...languages.where((l) => l != code), code] : languages.where((l) => l != code).toList();
    _notify();
    await client.setSetting(key: _languagesKey, value: languages.join(','));
    await _load();
  }

  /// Delete a downloaded dictionary, and stop using it.
  Future<void> remove(String code) async {
    if (languages.contains(code)) await setLanguage(code, false);
    await spellRemove(dir: dir, code: code);
    catalog = await spellLanguages(dir: dir);
    _notify();
  }

  void _notify() {
    if (!_disposed) notifyListeners();
  }

  @override
  void dispose() {
    _disposed = true;
    super.dispose();
  }
}
