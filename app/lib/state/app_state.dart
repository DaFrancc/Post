// What the signed-in app knows, shared by every screen: the conversation
// list, who is typing, presence and people, kept current from the core's
// live events.

import 'dart:async';

import 'package:flutter/widgets.dart';
import 'package:post/src/rust/api/client.dart';
import 'package:post/src/rust/api/error.dart';

import '../ui/arrival.dart';
import 'search_spec.dart';
import 'spelling.dart';

/// One sentence to show for a failed call. Screens throw [ArgumentError]
/// for their own checks, with the sentence as its message.
String errorText(Object e) => switch (e) {
  PostError(:final message) => message,
  ArgumentError(:final message) => '$message',
  _ => 'Something went wrong. Try again.',
};

/// How long a typing notice shows without a new one (design section 7).
const typingShownFor = Duration(seconds: 6);

class AppState extends ChangeNotifier {
  AppState(this.client, this.me, {required String dictionaries}) : spelling = Spelling(client, dictionaries) {
    unawaited(client.run());
    _events = client.events().listen(_onEvent);
    unawaited(refreshConversations());
    unawaited(_loadOwnPresence());
    unawaited(person(me.username));
    unawaited(_loadSettings());
    _connectionTimer = Timer.periodic(const Duration(seconds: 4), (_) => _checkConnection());
    unawaited(_checkConnection());
  }

  final Client client;
  final Identity me;

  /// Spellchecking in the composer.
  final Spelling spelling;

  /// Live conversations, most recent first.
  List<ConversationInfo> conversations = const [];

  /// The conversation on screen, if any.
  String? selected;

  /// Whether the core is connected to the server.
  bool connected = true;

  /// The presence this user chose.
  PresenceChoice ownPresence = PresenceChoice.online;

  /// Whether other people's messages arrive in the styles they chose; off,
  /// they all simply rise in. Your own always arrive in yours.
  bool showOthersArrivals = true;
  static const _othersArrivalsKey = 'arrival.others';

  final _people = <String, PersonInfo>{};
  final _peopleLoading = <String>{};
  final _presence = <String, String>{};
  final _presenceLoading = <String>{};
  final _typing = <String, Map<String, ({String text, DateTime until})>>{};

  final _relay = StreamController<AppEvent>.broadcast();
  late final StreamSubscription<AppEvent> _events;
  Timer? _typingSweep;
  Timer? _refreshSoon;
  late final Timer _connectionTimer;

  /// Every live event, for screens that show one conversation.
  Stream<AppEvent> get events => _relay.stream;

  ConversationInfo? get current {
    final id = selected;
    if (id == null) return null;
    for (final c in conversations) {
      if (c.id == id) return c;
    }
    return null;
  }

  ConversationInfo? conversationById(String id) {
    for (final c in conversations) {
      if (c.id == id) return c;
    }
    return null;
  }

  // ---------------------------------------------------------------- events

  void _onEvent(AppEvent e) {
    switch (e.kind) {
      case EventKind.conversations:
      case EventKind.messages:
        _refreshListSoon();
      case EventKind.profile:
        final u = e.username;
        if (u != null) {
          _people.remove(u);
          unawaited(person(u));
        }
        _refreshListSoon();
      case EventKind.typing:
        final (c, u, t) = (e.conversation, e.username, e.text);
        if (c != null && u != null && t != null) {
          (_typing[c] ??= {})[u] = (text: t, until: DateTime.now().add(typingShownFor));
          _typingSweep ??= Timer.periodic(const Duration(seconds: 1), (_) => _sweepTyping());
          notifyListeners();
        }
      case EventKind.presence:
        final (u, t) = (e.username, e.text);
        if (u != null && t != null) {
          _presence[u] = t;
          notifyListeners();
        }
      case EventKind.devices:
      case EventKind.notice:
        break;
    }
    _relay.add(e);
  }

  /// Many events arrive together during a sync; re-read the list once.
  void _refreshListSoon() {
    _refreshSoon?.cancel();
    _refreshSoon = Timer(const Duration(milliseconds: 80), () => unawaited(refreshConversations()));
  }

  void _sweepTyping() {
    final now = DateTime.now();
    var changed = false;
    for (final m in _typing.values) {
      final before = m.length;
      m.removeWhere((_, v) => v.until.isBefore(now));
      changed |= m.length != before;
    }
    _typing.removeWhere((_, m) => m.isEmpty);
    if (_typing.isEmpty) {
      _typingSweep?.cancel();
      _typingSweep = null;
    }
    if (changed) notifyListeners();
  }

  Future<void> _checkConnection() async {
    final now = await client.isConnected();
    if (now != connected) {
      connected = now;
      notifyListeners();
    }
  }

  // ---------------------------------------------------------------- conversations

  Future<void> refreshConversations() async {
    try {
      conversations = await client.conversations();
      notifyListeners();
    } catch (_) {
      // The list stays as it was; the next event tries again.
    }
  }

  // ---------------------------------------------------------------- search

  /// The search box: its chips and the words typed after them. Kept here so
  /// it survives opening a result in another conversation.
  List<SearchFilter> searchFilters = const [];
  String searchText = '';

  /// The search box, for Ctrl+F.
  final searchFocus = FocusNode(debugLabel: 'search');

  /// The search whose results show; null when none do.
  SearchSpec? searchShown;

  /// Search every conversation rather than the one on screen; oldest
  /// first; which page of results.
  bool searchAll = false;
  bool searchOldest = false;
  int searchPage = 0;

  /// Searches run, newest first, for the box to offer again.
  List<SearchSpec> searchHistory = const [];
  static const _historyKey = 'search.history';

  void editSearch({List<SearchFilter>? filters, String? text}) {
    if (filters != null) searchFilters = filters;
    if (text != null) searchText = text;
    notifyListeners();
  }

  /// Show results for what is in the box (or [spec]), from the first page.
  void runSearch([SearchSpec? spec]) {
    final s = spec ?? SearchSpec(words: searchText.trim(), filters: searchFilters);
    if (s.isEmpty) return;
    searchFilters = s.filters;
    searchText = s.words;
    searchShown = s;
    searchPage = 0;
    searchHistory = [s, ...searchHistory.where((h) => h != s)].take(8).toList();
    unawaited(client.setSetting(key: _historyKey, value: searchHistory.join('\n')).catchError((_) {}));
    notifyListeners();
  }

  void setSearchView({bool? all, bool? oldest, int? page}) {
    if (all != null) searchAll = all;
    if (oldest != null) searchOldest = oldest;
    searchPage = page ?? (all != null || oldest != null ? 0 : searchPage);
    notifyListeners();
  }

  void closeSearch() {
    searchFilters = const [];
    searchText = '';
    searchShown = null;
    notifyListeners();
  }

  void clearSearchHistory() {
    searchHistory = const [];
    unawaited(client.setSetting(key: _historyKey, value: null).catchError((_) {}));
    notifyListeners();
  }

  Future<void> _loadSearchHistory() async {
    try {
      final saved = await client.setting(key: _historyKey);
      searchHistory = [
        for (final line in saved?.split('\n') ?? const <String>[])
          if (!SearchSpec.parse(line).isEmpty) SearchSpec.parse(line),
      ];
    } catch (_) {}
  }

  /// A message to bring into view (and flash) once its conversation shows.
  ({String conversation, String message})? _jump;

  /// Open a message's conversation and scroll to the message.
  void jumpTo(String conversation, String message) {
    _jump = (conversation: conversation, message: message);
    if (selected == conversation) {
      notifyListeners();
    } else {
      select(conversation);
    }
  }

  /// The message to jump to in this conversation, if there is one: given
  /// once.
  String? takeJump(String conversation) {
    final j = _jump;
    if (j == null || j.conversation != conversation) return null;
    _jump = null;
    return j.message;
  }

  /// Show a conversation (or none) and mark it read.
  void select(String? id) {
    if (selected == id) return;
    selected = id;
    notifyListeners();
    if (id != null) unawaited(markRead(id));
  }

  /// Mark a conversation read. Cheap when nothing is unread: the core sends
  /// nothing unless a read position moved.
  Future<void> markRead(String id) async {
    try {
      await client.markRead(conversation: id);
    } catch (_) {
      // Unread counts are a convenience; try again on the next message.
    }
  }

  /// The typing notices showing in a conversation.
  /// Who is typing in a conversation, and the notice to show for each.
  List<(String, String)> typingUsersIn(String conversation) => [
    for (final e in _typing[conversation]?.entries ?? const <MapEntry<String, ({String text, DateTime until})>>[]) (e.key, e.value.text),
  ];

  List<String> typingIn(String conversation) => _typing[conversation]?.values.map((v) => v.text).toList() ?? const [];

  /// Someone sent a message, so they have stopped typing.
  void stoppedTyping(String conversation, String username) {
    if (_typing[conversation]?.remove(username) != null) notifyListeners();
  }

  // ---------------------------------------------------------------- people

  /// What is known of someone, or null until it has loaded (listeners are
  /// told when it has).
  PersonInfo? personNow(String username) {
    final p = _people[username];
    if (p == null) unawaited(person(username));
    return p;
  }

  Future<PersonInfo?> person(String username) async {
    final known = _people[username];
    if (known != null) return known;
    if (!_peopleLoading.add(username)) return null;
    try {
      final p = await client.person(username: username);
      _people[username] = p;
      notifyListeners();
      return p;
    } catch (_) {
      return null;
    } finally {
      _peopleLoading.remove(username);
    }
  }

  /// The name this user shows as.
  String get myName => _people[me.username]?.shownName ?? me.username;

  /// Forget what is known of someone and load it again, after it changed
  /// here (your own profile, saved).
  Future<void> reloadPerson(String username) async {
    _people.remove(username);
    await person(username);
  }

  /// How a message from [username] arrives on this screen.
  Arrival arrivalOf(String username) {
    if (username != me.username && !showOthersArrivals) return Arrival.rise;
    return Arrival.parse(personNow(username)?.arrival);
  }

  Future<void> _loadSettings() async {
    try {
      showOthersArrivals = await client.setting(key: _othersArrivalsKey) != '0';
    } catch (_) {}
    await _loadSearchHistory();
    notifyListeners();
  }

  /// Everyone this device knows of, for suggesting names: people in DMs,
  /// and anyone whose profile has loaded.
  List<String> get knownPeople => {for (final c in conversations) ?c.peer, ..._people.keys}.toList();

  Future<void> setShowOthersArrivals(bool show) async {
    showOthersArrivals = show;
    notifyListeners();
    await client.setSetting(key: _othersArrivalsKey, value: show ? null : '0');
  }

  /// Someone's presence word (online, away, dnd, offline), loading it the
  /// first time it is asked for.
  String presenceOf(String username) {
    if (username == me.username) return ownPresenceWord;
    final p = _presence[username];
    if (p != null) return p;
    if (_presenceLoading.add(username)) unawaited(_loadPresence(username));
    return 'offline';
  }

  Future<void> _loadPresence(String username) async {
    try {
      final p = await client.presence(username: username);
      _presence[username] ??= switch (p) {
        PresenceState.online => 'online',
        PresenceState.away => 'away',
        PresenceState.doNotDisturb => 'dnd',
        PresenceState.offline => 'offline',
      };
      notifyListeners();
    } catch (_) {
      _presenceLoading.remove(username);
    }
  }

  String get ownPresenceWord => switch (ownPresence) {
    PresenceChoice.online => 'online',
    PresenceChoice.away => 'away',
    PresenceChoice.doNotDisturb => 'dnd',
    PresenceChoice.invisible => 'offline',
  };

  Future<void> _loadOwnPresence() async {
    try {
      ownPresence = await client.ownPresence();
      notifyListeners();
    } catch (_) {}
  }

  Future<void> setOwnPresence(PresenceChoice choice) async {
    final before = ownPresence;
    ownPresence = choice;
    notifyListeners();
    try {
      await client.setPresence(choice: choice);
    } catch (e) {
      ownPresence = before;
      notifyListeners();
      rethrow;
    }
  }

  @override
  void dispose() {
    _events.cancel();
    _relay.close();
    spelling.dispose();
    searchFocus.dispose();
    _typingSweep?.cancel();
    _refreshSoon?.cancel();
    _connectionTimer.cancel();
    super.dispose();
  }
}
