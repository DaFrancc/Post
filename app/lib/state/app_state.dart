// What the signed-in app knows, shared by every screen: the conversation
// list, who is typing, presence and people, kept current from the core's
// live events.

import 'dart:async';

import 'package:flutter/foundation.dart';
import 'package:post/src/rust/api/client.dart';
import 'package:post/src/rust/api/error.dart';

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
  AppState(this.client, this.me) {
    unawaited(client.run());
    _events = client.events().listen(_onEvent);
    unawaited(refreshConversations());
    unawaited(_loadOwnPresence());
    unawaited(person(me.username));
    _connectionTimer = Timer.periodic(const Duration(seconds: 4), (_) => _checkConnection());
    unawaited(_checkConnection());
  }

  final Client client;
  final Identity me;

  /// Live conversations, most recent first.
  List<ConversationInfo> conversations = const [];

  /// The conversation on screen, if any.
  String? selected;

  /// Whether the core is connected to the server.
  bool connected = true;

  /// The presence this user chose.
  PresenceChoice ownPresence = PresenceChoice.online;

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
    _typingSweep?.cancel();
    _refreshSoon?.cancel();
    _connectionTimer.cancel();
    super.dispose();
  }
}
