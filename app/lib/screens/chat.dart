// One conversation: its messages with every edit visible, security notices
// in line, who is typing, and the composer.

import 'dart:async';
import 'dart:math' as math;
import 'dart:ui' show lerpDouble;

import 'package:flutter/foundation.dart' show listEquals;
import 'package:flutter/material.dart';
import 'package:flutter/rendering.dart' show RenderEditable;
import 'package:flutter/services.dart';
import 'package:post/src/rust/api/client.dart';

import '../state/app_state.dart';
import '../state/spelling.dart';
import '../theme.dart';
import '../ui/arrival.dart';
import '../ui/controls.dart';
import '../ui/format.dart';
import '../ui/markup.dart';
import '../ui/markup_text.dart';
import 'dialogs.dart';
import 'home.dart' show Panel;
import 'calls.dart' show CallButton, CallStrip;
import 'profile_card.dart';
import 'search.dart' show SearchBox;
import 'settings.dart' show SettingsSection, showSettings;

/// Messages from one sender this close together share one header.
const _groupWithin = Duration(minutes: 7);

/// How long a new message's row takes to open. The composer shrinks back
/// over the same time on the same curve after a send, so the two cancel
/// out rather than the history dipping and climbing.
const _rowOpens = Duration(milliseconds: 390);

/// A new row's whole entrance: it opens, and its contents fade in after.
const _rowEnters = Duration(milliseconds: 520);

/// How many more messages to load when scrolling back.
const _page = 150;

class ChatView extends StatefulWidget {
  const ChatView({super.key, required this.state, required this.conversation, required this.panelOpen, this.onTogglePanel});

  final AppState state;
  final ConversationInfo conversation;
  final bool panelOpen;

  /// Null when there is no room for the side panel.
  final VoidCallback? onTogglePanel;

  @override
  State<ChatView> createState() => _ChatViewState();
}

class _ChatViewState extends State<ChatView> {
  /// Shows formatting as you type, markers dimmed in place.
  final _composer = MarkupEditingController();
  final _composerFocus = FocusNode();
  final _editor = TextEditingController();
  final _editorFocus = FocusNode();
  final _scroll = ScrollController();
  late final StreamSubscription<AppEvent> _events;

  List<MessageInfo> _messages = const [];
  List<NoticeInfo> _notices = const [];
  List<MemberInfo> _members = const [];
  final _versions = <String, List<MessageVersionInfo>>{};
  int _limit = _page;
  bool _loaded = false;

  /// Messages already here when the conversation opened. Others arrived
  /// while it was open, and animate in.
  Set<String>? _initial;

  /// The first message that was unread when the conversation opened: the
  /// "New" line goes above it, and stays while the conversation is open.
  String? _firstUnread;

  // Messages fly into the history from where they were: yours from the
  // composer, others' from their typing line.
  final _composerFieldKey = GlobalKey();
  final _composerBoxKey = GlobalKey();
  final _typingKeys = <String, GlobalKey>{};

  /// Flights waiting for their message to appear.
  final _flights = <_Flight>[];

  /// Each new message's flight, or none, decided once when it first shows.
  final _claimed = <String, _Flight?>{};

  GlobalKey _typingKey(String username) => _typingKeys.putIfAbsent(username, GlobalKey.new);

  /// The global rectangle of a laid-out widget, if it is on screen.
  static Rect? _rectOf(GlobalKey key) {
    final box = key.currentContext?.findRenderObject();
    if (box is! RenderBox || !box.hasSize || !box.attached) return null;
    return box.localToGlobal(Offset.zero) & box.size;
  }

  /// The flight for a message that has just appeared: yours, matched by
  /// text; someone else's, matched by who was typing.
  _Flight? _flightFor(MessageInfo m) {
    if (_initial == null || _initial!.contains(m.id)) return null;
    return _claimed.putIfAbsent(m.id, () {
      // Your message's row starts opening this frame: the composer can
      // shrink back now, alongside it.
      if (m.outgoing) WidgetsBinding.instance.addPostFrameCallback((_) => _releaseComposer());
      final i = _flights.indexWhere((f) => m.outgoing ? (f.outgoing && f.body == m.body) : (!f.outgoing && f.username == m.senderUsername));
      return i < 0 ? null : _flights.removeAt(i);
    });
  }

  /// How each new message arrives, decided once when it first shows: in
  /// its sender's style (yours for your own). A bubble needs somewhere to
  /// fly from; without one it rises instead.
  final _arrivals = <String, Arrival>{};

  Arrival _arrivalFor(MessageInfo m) {
    // Already here when the conversation opened: shown as it is.
    if (_initial == null || _initial!.contains(m.id)) return Arrival.instant;
    return _arrivals.putIfAbsent(m.id, () {
      final a = _state.arrivalOf(m.outgoing ? _state.me.username : m.senderUsername);
      return a == Arrival.bubble && _claimed[m.id] == null ? Arrival.rise : a;
    });
  }

  /// While a sent message is on its way to the history, the composer keeps
  /// the height it had: if it shrank at once, the history would drop into
  /// the room it left and then climb back as the message's row opened.
  /// Released when the row starts opening, so the two move together.
  double? _composerHold;
  Timer? _composerHoldTimer;

  void _holdComposer(double height) {
    _composerHoldTimer?.cancel();
    // In case the message never shows (a send that fails slowly).
    _composerHoldTimer = Timer(const Duration(seconds: 3), _releaseComposer);
    setState(() => _composerHold = height);
  }

  void _releaseComposer() {
    _composerHoldTimer?.cancel();
    _composerHoldTimer = null;
    if (mounted && _composerHold != null) setState(() => _composerHold = null);
  }

  bool _loadingMore = false;
  String? _editing;
  DateTime _lastTyping = DateTime.fromMillisecondsSinceEpoch(0);

  String get _id => widget.conversation.id;
  AppState get _state => widget.state;

  // A message opened from a search result: brought into view and lit for a
  // moment.
  String? _highlight;
  String? _jumpTarget;
  final _highlightKey = GlobalKey();
  Timer? _unhighlight;

  @override
  void initState() {
    super.initState();
    _events = _state.events.listen(_onEvent);
    HardwareKeyboard.instance.addHandler(_typeToCompose);
    HardwareKeyboard.instance.addHandler(_findKey);
    unawaited(_load());
    unawaited(_loadMembers());
    _composerFocus.requestFocus();
    _composer.addListener(_spellSoon);
    _composer.addListener(_mentionTyped);
    _state.spelling.addListener(_spellSoon);
  }

  @override
  void dispose() {
    _events.cancel();
    _composerHoldTimer?.cancel();
    _spellTimer?.cancel();
    _state.spelling.removeListener(_spellSoon);
    HardwareKeyboard.instance.removeHandler(_typeToCompose);
    HardwareKeyboard.instance.removeHandler(_findKey);
    _unhighlight?.cancel();
    _composer.dispose();
    _composerFocus.dispose();
    _editor.dispose();
    _editorFocus.dispose();
    _scroll.dispose();
    super.dispose();
  }

  @override
  void didUpdateWidget(ChatView old) {
    super.didUpdateWidget(old);
    // A search result in this conversation was opened.
    if (_loaded) _takeJump();
  }

  void _takeJump() {
    final id = _state.takeJump(_id);
    if (id != null) WidgetsBinding.instance.addPostFrameCallback((_) => unawaited(_jumpTo(id)));
  }

  /// Ctrl+F: to the search box, its text selected for typing over.
  bool _findKey(KeyEvent e) {
    final keys = HardwareKeyboard.instance;
    if (e is! KeyDownEvent || e.logicalKey != LogicalKeyboardKey.keyF || !keys.isControlPressed || keys.isAltPressed) return false;
    if (!(ModalRoute.of(context)?.isCurrent ?? false)) return false;
    _state.searchFocus.requestFocus();
    return true;
  }

  /// Bring a message into view, centred, and light it for a moment. It may
  /// be older than what is loaded, and the list only builds rows near the
  /// screen: load until it is there, then scroll back until its row is
  /// built, then centre it.
  Future<void> _jumpTo(String id) async {
    bool have() => _messages.any((m) => m.id == id);
    while (!have() && _hasMore && _limit < 20000) {
      _limit += _page * 4;
      await _load();
      if (!mounted) return;
    }
    if (!have()) {
      if (mounted) showToast(context, "That message isn't here any more.");
      return;
    }
    setState(() => _highlight = _jumpTarget = id);
    await WidgetsBinding.instance.endOfFrame;
    for (var i = 0; i < 300 && mounted && _highlightKey.currentContext == null && _scroll.hasClients; i++) {
      final p = _scroll.position;
      final next = math.min(p.pixels + p.viewportDimension * 0.8, p.maxScrollExtent);
      if (next <= p.pixels) break;
      _scroll.jumpTo(next);
      await WidgetsBinding.instance.endOfFrame;
    }
    final row = _highlightKey.currentContext;
    if (row != null && row.mounted) await Scrollable.ensureVisible(row, alignment: 0.5, duration: PMotion.slow, curve: PMotion.curve);
    _unhighlight?.cancel();
    _unhighlight = Timer(const Duration(milliseconds: 1800), () {
      if (mounted) setState(() => _highlight = null);
    });
  }

  void _onEvent(AppEvent e) {
    switch (e.kind) {
      case EventKind.messages when e.conversation == _id:
        unawaited(_load());
      // A call's notices ("Call ended · 3 min") come with call events.
      case EventKind.notice when e.conversation == _id:
      case EventKind.call when e.conversation == _id:
        unawaited(_load());
      case EventKind.profile:
        unawaited(_load());
        unawaited(_loadMembers());
      case EventKind.conversations when e.conversation == _id || e.conversation == null:
      case EventKind.devices:
        unawaited(_loadMembers());
      default:
        break;
    }
  }

  Future<void> _load() async {
    try {
      final before = _messages.isEmpty ? null : _messages.last.id;
      final messages = await _state.client.messages(conversation: _id, limit: _limit);
      final notices = await _state.client.notices(conversation: _id, limit: 50);
      if (!mounted) return;
      // Versions shown stay current through edits.
      for (final id in _versions.keys.toList()) {
        _versions[id] = await _state.client.messageVersions(conversation: _id, messageId: id);
      }
      if (!mounted) return;
      // Messages from people shown typing fly from their typing line, which
      // is about to go: note where it is while it is still there.
      final known = {for (final m in _messages) m.id};
      final arrivedFrom = <String>{};
      if (_initial != null) {
        final typing = {for (final (u, _) in _state.typingUsersIn(_id)) u};
        for (final m in messages.where((m) => !m.outgoing && !known.contains(m.id))) {
          arrivedFrom.add(m.senderUsername);
          final rect = _rectOf(_typingKey(m.senderUsername));
          if (rect != null && typing.contains(m.senderUsername) && m.body != null && _state.arrivalOf(m.senderUsername) == Arrival.bubble) {
            // Its shape starts as a small pill around the typing line.
            final pill = Rect.fromLTRB(rect.left - 8, rect.top - 4, rect.right + 8, rect.bottom + 4);
            _flights.add(
              _Flight(
                body: m.body!,
                outgoing: false,
                username: m.senderUsername,
                from: rect,
                fromStyle: _Composer.typingStyle,
                fromShape: pill,
                fromRadius: pill.height / 2,
              ),
            );
          }
        }
      }
      // A flight whose message never came (a send that failed) is dropped.
      _flights.removeWhere((f) => DateTime.now().difference(f.made) > const Duration(seconds: 5));
      setState(() {
        _messages = messages;
        _notices = notices;
        if (_initial == null) {
          _initial = {for (final m in messages) m.id};
          _firstUnread = _findFirstUnread(messages, widget.conversation.unread);
        }
        _loaded = true;
        _loadingMore = false;
      });
      for (final u in arrivedFrom) {
        _state.stoppedTyping(_id, u);
      }
      // Opened from a search result: now the messages are here, go to it.
      _takeJump();
      if (messages.isNotEmpty && messages.last.id != before) unawaited(_state.markRead(_id));
    } catch (e) {
      if (mounted) setState(() => _loadingMore = false);
    }
  }

  // ---------------------------------------------------------------- typing a mention

  /// The mention being typed in the composer (`@` and what follows it so
  /// far), and the members it could be, best first; empty for no list.
  ({int at, String query})? _mention;
  List<MemberInfo> _mentionChoices = const [];
  int _mentionPick = 0;

  /// Escape closed the list for the mention whose `@` is here; it stays
  /// closed until the cursor leaves that mention.
  int? _mentionClosedAt;

  void _mentionTyped() {
    final v = _composer.value;
    final typed = v.selection.isCollapsed && !v.composing.isValid ? mentionTyped(v.text, v.selection.baseOffset) : null;
    if (typed?.at != _mentionClosedAt) _mentionClosedAt = null;
    final choices = typed == null || typed.at == _mentionClosedAt ? const <MemberInfo>[] : _mentionMatches(typed.query);
    final same = typed == _mention && listEquals([for (final m in choices) m.username], [for (final m in _mentionChoices) m.username]);
    if (same) return;
    setState(() {
      _mention = typed;
      _mentionChoices = choices;
      _mentionPick = 0;
    });
  }

  /// Members (not you) whose username, or a word of whose name, starts
  /// with [q]: usernames matching first, then by name.
  List<MemberInfo> _mentionMatches(String q) {
    bool byName(MemberInfo m) => m.shownName.toLowerCase().split(RegExp(r'\s+')).any((w) => w.startsWith(q));
    final found = _members.where((m) => !m.you && (m.username.startsWith(q) || byName(m))).toList()
      ..sort((a, b) {
        final byUsername = (a.username.startsWith(q) ? 0 : 1) - (b.username.startsWith(q) ? 0 : 1);
        return byUsername != 0 ? byUsername : a.shownName.toLowerCase().compareTo(b.shownName.toLowerCase());
      });
    return found.take(8).toList();
  }

  /// Put the chosen member's mention in place of what was typed, with a
  /// space after it to carry on typing.
  void _pickMention(MemberInfo m) {
    final typed = _mention;
    if (typed == null) return;
    final t = _composer.text;
    // The whole name the cursor is in, up to what follows it too.
    var end = typed.at + 1;
    while (end < t.length && RegExp('[a-z0-9_]').hasMatch(t[end])) {
      end++;
    }
    final spaced = end < t.length && t[end] == ' ';
    final text = t.replaceRange(typed.at, end, '@${m.username}${spaced ? '' : ' '}');
    _composer.value = TextEditingValue(
      text: text,
      selection: TextSelection.collapsed(offset: typed.at + m.username.length + 2),
    );
    _composerFocus.requestFocus();
  }

  /// Keys for the list while it shows: arrows choose, Tab or Enter picks,
  /// Escape closes it.
  KeyEventResult _mentionKey(KeyEvent e) {
    final n = _mentionChoices.length;
    if (n == 0 || HardwareKeyboard.instance.isShiftPressed) return KeyEventResult.ignored;
    switch (e.logicalKey) {
      case LogicalKeyboardKey.arrowDown:
        setState(() => _mentionPick = (_mentionPick + 1) % n);
      case LogicalKeyboardKey.arrowUp:
        setState(() => _mentionPick = (_mentionPick - 1 + n) % n);
      case LogicalKeyboardKey.enter || LogicalKeyboardKey.numpadEnter || LogicalKeyboardKey.tab when e is KeyDownEvent:
        _pickMention(_mentionChoices[_mentionPick]);
      case LogicalKeyboardKey.escape when e is KeyDownEvent:
        _mentionClosedAt = _mention?.at;
        _mentionTyped();
      default:
        return KeyEventResult.ignored;
    }
    return KeyEventResult.handled;
  }

  /// Whether someone else's message mentions the viewer, remembered by its
  /// text so the list's rebuilds do not parse every message again.
  final _mentionCache = <String, (String, bool)>{};
  bool _mentionsYou(MessageInfo m) {
    final body = m.body;
    if (m.outgoing || m.deleted || body == null) return false;
    final known = _mentionCache[m.id];
    if (known != null && known.$1 == body) return known.$2;
    final you = mentionsIn(body).contains(_state.me.username);
    _mentionCache[m.id] = (body, you);
    return you;
  }

  /// The name a mentioned username shows as here: the name members see each
  /// other by in this conversation, else any profile already loaded.
  String _shownName(String username) {
    for (final m in _members) {
      if (m.username == username) return m.shownName;
    }
    return _state.knownName(username);
  }

  Future<void> _loadMembers() async {
    try {
      final m = await _state.client.members(conversation: _id);
      if (!mounted) return;
      setState(() => _members = m);
      _mentionTyped();
    } catch (_) {}
  }

  bool get _hasMore => _messages.length >= _limit;

  /// Unread counts messages from others that are not deleted, so count back
  /// that many of them from the newest.
  static String? _findFirstUnread(List<MessageInfo> messages, int unread) {
    if (unread <= 0) return null;
    var left = unread;
    for (var i = messages.length - 1; i >= 0; i--) {
      final m = messages[i];
      if (m.outgoing || m.deleted) continue;
      if (--left == 0) return m.id;
    }
    return null;
  }

  bool _onScroll(ScrollNotification n) {
    // The list is reversed: its far end is the oldest message.
    if (n.metrics.pixels > n.metrics.maxScrollExtent - 300 && _hasMore && !_loadingMore) {
      _loadingMore = true;
      _limit += _page;
      unawaited(_load());
    }
    return false;
  }

  // ---------------------------------------------------------------- sending

  Future<void> _send() async {
    final text = _composer.text.trim();
    if (text.isEmpty) return;
    // Where the text sits now, inside the field's 10 px of vertical padding:
    // the message flies from here when it appears in the history.
    final field = _rectOf(_composerFieldKey);
    final box = _rectOf(_composerBoxKey);
    if (field != null && box != null && _state.arrivalOf(_state.me.username) == Arrival.bubble) {
      final from = Rect.fromLTRB(field.left, field.top + 10, field.right, field.bottom - 10);
      // Its shape starts as the composer's own, up to the send button, so it
      // looks like the text lifts out of the box it was typed in.
      final shape = Rect.fromLTRB(box.left, box.top, field.right + 2, box.bottom);
      _flights.add(_Flight(body: text, outgoing: true, from: from, fromStyle: _composerText, fromShape: shape, fromRadius: 18));
    }
    if (field != null) _holdComposer(field.height);
    _composer.clear();
    try {
      await _state.client.sendText(conversation: _id, text: text);
      _jumpToNewest();
    } catch (e) {
      _releaseComposer();
      if (!mounted) return;
      // Put it back, so nothing typed is lost.
      if (_composer.text.isEmpty) _composer.text = text;
      showToast(context, errorText(e), error: true);
    }
  }

  void _jumpToNewest() {
    if (_scroll.hasClients) _scroll.animateTo(0, duration: const Duration(milliseconds: 200), curve: Curves.easeOut);
  }

  void _typed(String text) {
    if (text.trim().isEmpty) return;
    final now = DateTime.now();
    // The core sends at most every three seconds; this saves the calls.
    if (now.difference(_lastTyping) < const Duration(milliseconds: 2500)) return;
    _lastTyping = now;
    unawaited(_state.client.sendTyping(conversation: _id).catchError((_) {}));
  }

  /// Typing anywhere in the conversation types into the composer, as it
  /// would in other chat apps, unless something else holds the keyboard.
  /// Sees every key press (a keyboard handler, since with nothing focused
  /// no widget would); acts only when no text field holds the keyboard and
  /// nothing (a dialog, a menu) is open over the conversation.
  bool _typeToCompose(KeyEvent e) {
    if (e is! KeyDownEvent || _editing != null || !widget.conversation.following) return false;
    if (!(ModalRoute.of(context)?.isCurrent ?? false)) return false;
    final primary = FocusManager.instance.primaryFocus?.context;
    if (primary != null && (primary.widget is EditableText || primary.findAncestorStateOfType<EditableTextState>() != null)) return false;
    final keys = HardwareKeyboard.instance;
    final ch = e.character;
    if (ch == null || ch.isEmpty || keys.isControlPressed || keys.isAltPressed || keys.isMetaPressed) return false;
    final code = ch.codeUnitAt(0);
    if (code < 0x20 || code == 0x7f) return false;
    final text = _composer.text + ch;
    _composer.value = TextEditingValue(
      text: text,
      selection: TextSelection.collapsed(offset: text.length),
    );
    _composerFocus.requestFocus();
    _typed(text);
    return true;
  }

  KeyEventResult _composerKey(FocusNode _, KeyEvent e) {
    if (e is! KeyDownEvent && e is! KeyRepeatEvent) return KeyEventResult.ignored;
    final keys = HardwareKeyboard.instance;
    if (keys.isControlPressed && !keys.isAltPressed && !keys.isMetaPressed) {
      final shift = keys.isShiftPressed;
      final Edit Function(Edit)? op = switch (e.logicalKey) {
        LogicalKeyboardKey.keyB when !shift => (e) => toggleWrap(e, markerFor[Mark.bold]!),
        LogicalKeyboardKey.keyI when !shift => (e) => toggleWrap(e, markerFor[Mark.italic]!),
        LogicalKeyboardKey.keyU when !shift => (e) => toggleWrap(e, markerFor[Mark.underline]!),
        LogicalKeyboardKey.keyX when shift => (e) => toggleWrap(e, markerFor[Mark.strike]!),
        LogicalKeyboardKey.keyE when !shift => toggleCode,
        _ => null,
      };
      if (op != null) {
        if (e is KeyDownEvent) _format(op);
        return KeyEventResult.handled;
      }
    }
    if (_mentionKey(e) == KeyEventResult.handled) return KeyEventResult.handled;
    if (e is! KeyDownEvent) return KeyEventResult.ignored;
    final composing = _composer.value.composing.isValid;
    if (e.logicalKey == LogicalKeyboardKey.enter && !HardwareKeyboard.instance.isShiftPressed && !composing) {
      unawaited(_send());
      return KeyEventResult.handled;
    }
    // Up in an empty composer edits your last message.
    if (e.logicalKey == LogicalKeyboardKey.arrowUp && _composer.text.isEmpty) {
      final mine = _messages.lastWhereOrNull((m) => m.outgoing && !m.deleted && !m.unsupported && m.body != null);
      if (mine != null) {
        _startEdit(mine);
        return KeyEventResult.handled;
      }
    }
    return KeyEventResult.ignored;
  }

  // ---------------------------------------------------------------- spelling

  Timer? _spellTimer;
  int _spellRun = 0;

  /// Check the composer a moment after it changes (text, cursor, or the
  /// dictionaries), not on every keystroke.
  void _spellSoon() {
    _spellTimer?.cancel();
    _spellTimer = Timer(const Duration(milliseconds: 150), () => unawaited(_spellNow()));
  }

  Future<void> _spellNow() async {
    final run = ++_spellRun;
    final text = _composer.text;
    final found = await _state.spelling.check(text);
    if (!mounted || run != _spellRun || _composer.text != text) return;
    // The word being typed is left alone until you move on from it.
    final sel = _composer.selection;
    final typingAt = sel.isValid && sel.isCollapsed ? sel.baseOffset : -1;
    final ranges = [
      for (final m in found)
        if (m.end != typingAt) TextRange(start: m.start, end: m.end),
    ];
    if (!listEquals(ranges, _composer.misspelled)) _composer.misspelled = ranges;
  }

  /// Apply a formatting command to the composer's selection (or, with no
  /// selection yet, at its end), keeping what it selects afterwards.
  void _format(Edit Function(Edit) op) {
    final v = _composer.value;
    final sel = v.selection.isValid ? v.selection : TextSelection.collapsed(offset: v.text.length);
    final r = op((text: v.text, start: sel.start, end: sel.end));
    _composer.value = TextEditingValue(
      text: r.text,
      selection: TextSelection(baseOffset: r.start, extentOffset: r.end),
    );
    _composerFocus.requestFocus();
    _typed(r.text);
  }

  // ---------------------------------------------------------------- editing

  void _startEdit(MessageInfo m) {
    setState(() {
      _editing = m.id;
      _editor.text = m.body ?? '';
      _editor.selection = TextSelection.collapsed(offset: _editor.text.length);
    });
    // Take the keyboard from the composer once the editor is built.
    WidgetsBinding.instance.addPostFrameCallback((_) {
      if (mounted && _editing == m.id) _editorFocus.requestFocus();
    });
  }

  void _cancelEdit() {
    setState(() => _editing = null);
    _composerFocus.requestFocus();
  }

  Future<void> _saveEdit(MessageInfo m) async {
    final text = _editor.text.trim();
    if (text.isEmpty) {
      // An empty edit means delete, as in other chat apps; ask first.
      _cancelEdit();
      await _delete(m);
      return;
    }
    if (text == m.body) {
      _cancelEdit();
      return;
    }
    try {
      await _state.client.editMessage(conversation: _id, messageId: m.id, text: text);
      if (mounted) _cancelEdit();
    } catch (e) {
      if (mounted) showToast(context, errorText(e), error: true);
    }
  }

  Future<void> _delete(MessageInfo m) async {
    final ok = await confirm(
      context,
      title: 'Delete message?',
      body: 'It is deleted for everyone, with every earlier version. This cannot be undone.',
      action: 'Delete',
      danger: true,
    );
    if (!ok) return;
    try {
      await _state.client.deleteMessage(conversation: _id, messageId: m.id);
    } catch (e) {
      if (mounted) showToast(context, errorText(e), error: true);
    }
  }

  /// Everything the hover toolbar does, plus more, on a right-click.
  Future<void> _messageMenu(MessageInfo m, Offset at) async {
    final mine = m.outgoing && !m.deleted && !m.unsupported;
    final body = m.body;
    final choice = await showContextMenu<String>(context, at, [
      if (mine && body != null) const PMenuItem(value: 'edit', label: 'Edit', icon: Icons.edit_outlined),
      if (body != null) const PMenuItem(value: 'copy', label: 'Copy text', icon: Icons.copy_rounded),
      if (m.edited && !m.deleted)
        PMenuItem(value: 'versions', label: _versions.containsKey(m.id) ? 'Hide versions' : 'Show every version', icon: Icons.history_rounded),
      if (!m.outgoing) ...[
        const PMenuDivider(),
        const PMenuItem(value: 'dm', label: 'Message', caption: 'Open a DM with them'),
        const PMenuItem(value: 'nickname', label: 'Set nickname', caption: 'Only you see it'),
      ],
      if (mine) ...[const PMenuDivider(), const PMenuItem(value: 'delete', label: 'Delete message', icon: Icons.delete_outline_rounded, danger: true)],
    ]);
    if (!mounted) return;
    switch (choice) {
      case 'edit':
        _startEdit(m);
      case 'copy':
        await copyText(context, body!, what: 'Message copied');
      case 'versions':
        await _toggleVersions(m);
      case 'dm':
        try {
          final dm = await _state.client.startDm(username: m.senderUsername);
          await _state.refreshConversations();
          _state.select(dm.id);
        } catch (e) {
          if (mounted) showToast(context, errorText(e), error: true);
        }
      case 'nickname':
        await setNickname(context, _state, m.senderUsername);
      case 'delete':
        await _delete(m);
    }
  }

  Future<void> _toggleVersions(MessageInfo m) async {
    if (_versions.containsKey(m.id)) {
      setState(() => _versions.remove(m.id));
      return;
    }
    try {
      final v = await _state.client.messageVersions(conversation: _id, messageId: m.id);
      if (mounted) setState(() => _versions[m.id] = v);
    } catch (e) {
      if (mounted) showToast(context, errorText(e), error: true);
    }
  }

  // ---------------------------------------------------------------- layout

  List<_Item> _timeline() {
    final items = <_Item>[];
    var n = 0;
    final notices = _notices;
    void add(_Item item) {
      final prev = items.isEmpty ? null : items.last;
      if (prev == null || !sameDay(prev.at, item.at)) items.add(_Day(item.at));
      items.add(item);
    }

    for (final m in _messages) {
      final at = m.sentAtMs;
      while (n < notices.length && notices[n].atMs < at) {
        add(_Notice(notices[n++]));
      }
      final prev = items.isEmpty ? null : items.last;
      final isFirstUnread = m.id == _firstUnread;
      if (isFirstUnread) add(_NewMark(at));
      final joins =
          !isFirstUnread &&
          prev is _Msg &&
          prev.m.senderUsername == m.senderUsername &&
          sameDay(prev.at, at) &&
          Duration(milliseconds: at - prev.at) < _groupWithin;
      add(_Msg(m, startsGroup: !joins));
    }
    while (n < notices.length) {
      add(_Notice(notices[n++]));
    }
    return items;
  }

  @override
  Widget build(BuildContext context) {
    final c = widget.conversation;
    final typing = _state.typingUsersIn(_id);
    final items = _timeline();
    // Each row's position in the (newest-first) list, by key.
    final positions = <Key, int>{
      for (final (i, item) in items.indexed) item.key: items.length - 1 - i,
      const ValueKey('more'): items.length,
      const ValueKey('beginning'): items.length,
    };
    return Panel(
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          _Header(
            state: _state,
            conversation: c,
            memberCount: _members.length,
            panelOpen: widget.panelOpen,
            onTogglePanel: widget.onTogglePanel,
            search: SearchBox(state: _state, members: _members),
          ),
          CallStrip(state: _state, conversation: c.id),
          Expanded(
            child: !_loaded
                ? const Center(child: PSpinner())
                : Mentions(
                    nameOf: _shownName,
                    you: _state.me.username,
                    onTap: (u, at) => unawaited(showProfileCard(context, _state, u, at: at)),
                    child: Stack(
                      children: [
                        // A message's hover toolbar floats in this overlay:
                        // above every row (the list paints older rows over
                        // newer ones, so it would sit under the row above),
                        // unclipped by its own row, but kept inside the list.
                        Overlay.wrap(
                          child: NotificationListener<ScrollNotification>(
                            onNotification: _onScroll,
                            // Text can be selected and copied with Ctrl+C; a
                            // right-click opens the message's own menu instead
                            // of the selection's. The selection still asks for
                            // its menu on a right-click and requires a builder
                            // (null crashes it), so it gets one that draws
                            // nothing.
                            child: SelectionArea(
                              contextMenuBuilder: (context, selection) => const SizedBox.shrink(),
                              child: ListView.builder(
                                controller: _scroll,
                                reverse: true,
                                padding: const EdgeInsets.fromLTRB(14, 12, 14, 8),
                                itemCount: items.length + 1,
                                // Find each row by its key, not its position: a new
                                // message enters at position 0 (the list runs
                                // newest first) and moves every other row along.
                                findChildIndexCallback: (key) => positions[key],
                                itemBuilder: (context, i) {
                                  if (i == items.length) {
                                    return _hasMore
                                        ? const Padding(
                                            key: ValueKey('more'),
                                            padding: EdgeInsets.all(16),
                                            child: Center(child: PSpinner()),
                                          )
                                        : _Beginning(
                                            key: const ValueKey('beginning'),
                                            conversation: c,
                                            image: c.isGroup || c.peer == null ? null : _state.avatarOf(c.peer!),
                                          );
                                  }
                                  final item = items[items.length - 1 - i];
                                  return KeyedSubtree(key: item.key, child: _itemView(item));
                                },
                              ),
                            ),
                          ),
                        ),
                        Positioned(
                          left: 0,
                          right: 0,
                          bottom: 10,
                          child: _JumpToNewest(scroll: _scroll, onTap: _jumpToNewest),
                        ),
                        // Members to mention, just above the composer.
                        Positioned(
                          left: 14,
                          right: 14,
                          bottom: 4,
                          child: _MentionList(
                            choices: _mentionChoices,
                            picked: _mentionPick,
                            query: _mention?.query ?? '',
                            presenceOf: _state.presenceOf,
                            avatarOf: _state.avatarOf,
                            onHover: (i) => setState(() => _mentionPick = i),
                            onPick: _pickMention,
                          ),
                        ),
                      ],
                    ),
                  ),
          ),
          if (c.following)
            _FormatToolbar(
              controller: _composer,
              focus: _composerFocus,
              fieldKey: _composerFieldKey,
              onFormat: _format,
              child: _Composer(
                controller: _composer,
                focus: _composerFocus,
                placeholder: 'Message ${c.title}',
                typing: typing,
                typingKey: _typingKey,
                fieldKey: _composerFieldKey,
                boxKey: _composerBoxKey,
                holdHeight: _composerHold,
                onKey: _composerKey,
                onChanged: _typed,
                onSend: _send,
                contextMenuBuilder: (context, editable) => _ComposerMenu(
                  editable: editable,
                  misspelled: _composer.misspelled,
                  spelling: _state.spelling,
                  onSpellingSettings: () => unawaited(showSettings(this.context, _state, section: SettingsSection.spelling)),
                ),
              ),
            )
          else
            _LeftBanner(group: c.isGroup),
        ],
      ),
    );
  }

  Widget _itemView(_Item item) => switch (item) {
    _Day(:final at) => _DaySeparator(dayLabel(at)),
    _NewMark() => const _NewLine(),
    _Notice(:final n) => _NoticeLine(n),
    _Msg(:final m, :final startsGroup) => _MessageTile(
      // The message a search result opened carries a key, to find its row.
      // Kept after the light fades, so the row is not rebuilt (and its
      // fade cut short) when it goes out.
      key: m.id == _jumpTarget ? _highlightKey : null,
      highlighted: m.id == _highlight,
      mentionsYou: _mentionsYou(m),
      flight: _flightFor(m),
      arrival: _arrivalFor(m),
      arrived: !(_initial?.contains(m.id) ?? true),
      m: m,
      startsGroup: startsGroup,
      versions: _versions[m.id],
      editing: _editing == m.id,
      editor: _editor,
      editorFocus: _editorFocus,
      onEdit: () => _startEdit(m),
      onDelete: () => _delete(m),
      onToggleVersions: () => _toggleVersions(m),
      onSaveEdit: () => _saveEdit(m),
      onCancelEdit: _cancelEdit,
      onMenu: (at) => _messageMenu(m, at),
      onPerson: (anchor) => unawaited(showProfileCard(anchor, _state, m.senderUsername)),
      receipts: () => _state.client.messageReceipts(conversation: widget.conversation.id, message: m.id),
      avatarOf: _state.avatarOf,
    ),
  };
}

extension<T> on List<T> {
  T? lastWhereOrNull(bool Function(T) test) {
    for (var i = length - 1; i >= 0; i--) {
      if (test(this[i])) return this[i];
    }
    return null;
  }
}

// ---------------------------------------------------------------- timeline items

/// One row of the conversation. Each has a key that stays the same however
/// the rows around it change, so the list keeps its widget (and its state:
/// revealed spoilers, open versions) when messages arrive.
sealed class _Item {
  int get at;
  Key get key;
}

class _Msg extends _Item {
  _Msg(this.m, {required this.startsGroup});

  final MessageInfo m;
  final bool startsGroup;

  @override
  int get at => m.sentAtMs;

  @override
  Key get key => ValueKey('message ${m.id}');
}

class _Notice extends _Item {
  _Notice(this.n);

  final NoticeInfo n;

  @override
  int get at => n.atMs;

  @override
  Key get key => ValueKey('notice ${n.atMs} ${n.text}');
}

/// Where unread messages began when the conversation opened.
class _NewMark extends _Item {
  _NewMark(this.at);

  @override
  final int at;

  @override
  Key get key => const ValueKey('new');
}

class _Day extends _Item {
  _Day(this.at);

  @override
  final int at;

  @override
  Key get key {
    final d = DateTime.fromMillisecondsSinceEpoch(at);
    return ValueKey('day ${d.year}-${d.month}-${d.day}');
  }
}

// ---------------------------------------------------------------- flights

/// A message's text on its way into the history: from the composer when you
/// send, or from the sender's typing line when they do.
class _Flight {
  _Flight({
    required this.body,
    required this.outgoing,
    required this.from,
    required this.fromStyle,
    required this.fromShape,
    required this.fromRadius,
    this.username,
  });

  final String body;
  final bool outgoing;

  /// Whose typing line it leaves (someone else's message).
  final String? username;

  /// Where its text starts, in window coordinates, and how it looks there.
  final Rect from;
  final TextStyle fromStyle;

  /// The shape it carries starts as this: the composer, or a pill around the
  /// typing line.
  final Rect fromShape;
  final double fromRadius;

  final DateTime made = DateTime.now();
}

/// How far along its arc a flight is: eases in from rest, gently, eases out,
/// and carries a couple of percent past its row before settling back (the
/// second control point's 1.15).
const _travel = Cubic(0.62, 0, 0.32, 1.15);

/// The text in flight, drawn above everything, carried in a bubble for the
/// journey: the bubble leaves as the composer (or a pill at the typing
/// line), closes around the text, swings out to the right and back in to the
/// message's place, and dissolves as it lands. Each
/// frame it looks up where its message now sits, so it lands right even if
/// the list moves meanwhile.
class _Flying extends StatefulWidget {
  const _Flying({required this.flight, required this.target, required this.tile, required this.toStyle, required this.onDone});

  final _Flight flight;

  /// Around the message's text, and around its whole row.
  final GlobalKey target;
  final GlobalKey tile;
  final TextStyle toStyle;
  final VoidCallback onDone;

  @override
  State<_Flying> createState() => _FlyingState();
}

class _FlyingState extends State<_Flying> with SingleTickerProviderStateMixin {
  late final AnimationController _c = AnimationController(
    vsync: this,
    // Rising from the typing line, the text also fades in: a little longer.
    duration: Duration(milliseconds: widget.flight.outgoing ? 780 : 840),
  );

  /// The text as drawn last frame: the bubble hugs its size.
  final _text = GlobalKey();

  /// Narrow enough at the sides that, in place, it stays clear of the avatar.
  static const _pad = EdgeInsets.symmetric(horizontal: 11, vertical: 7);

  /// Your bubble carries a touch of the accent; someone else's is neutral.
  static final _mine = Color.alphaBlend(PColors.accent.withValues(alpha: 0.16), PColors.panelRaised);
  static final _mineEdge = PColors.accent.withValues(alpha: 0.45);

  /// The point [u] of the way along a cubic Bézier from [s] to [e] that
  /// bulges [bulge] to the right: it sets off up and to the right, swings
  /// round and comes up into [e] from below and a little to the right. Past
  /// 1, as the spring overshoots, it carries on the way it was heading:
  /// mostly up, so it never drifts over the avatars.
  static Offset _arc(Offset s, Offset e, double bulge, double u) {
    final rise = s.dy - e.dy;
    final p1 = Offset(s.dx + bulge, s.dy - rise * 0.1);
    final p2 = Offset(e.dx + bulge * 0.25, e.dy + rise * 0.55);
    final w = 1 - u;
    return s * (w * w * w) + p1 * (3 * w * w * u) + p2 * (3 * w * u * u) + e * (u * u * u);
  }

  @override
  void initState() {
    super.initState();
    _c.forward().whenComplete(widget.onDone);
  }

  @override
  void dispose() {
    _c.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    return AnimatedBuilder(
      animation: _c,
      builder: (context, _) {
        // An overlay entry fills the overlay, so this widget's own box is the
        // space to place the text in (laid out from the second frame on).
        final box = context.findRenderObject();
        final to = _ChatViewState._rectOf(widget.target);
        final row = _ChatViewState._rectOf(widget.tile);
        if (to == null || row == null || box is! RenderBox || !box.hasSize) return const Stack(children: []);
        Rect local(Rect r) => box.globalToLocal(r.topLeft) & r.size;
        final f = widget.flight;
        final v = _c.value;
        final from = local(f.from);
        final dest = local(to);

        // The path: an arc that swings out into the open space to the right
        // of the messages and comes back in to land. Everything eases in and
        // out: it gets going gently rather than jumping off.
        // The swing grows with the climb, within the room the row has.
        final t = Curves.easeInOutCubic.transform(v);

        // The text is laid out once, as it will sit in the history (its
        // final style and width, so its final line breaks), and only scaled
        // on the way: from the composer's slightly larger size for yours,
        // growing a little into place for someone else's. Re-wrapping it as
        // it shrank once moved words between lines mid-flight.
        final grow = f.outgoing ? lerpDouble((f.fromStyle.fontSize ?? 14) / (widget.toStyle.fontSize ?? 14), 1, t)! : lerpDouble(0.9, 1, t)!;
        final drawn = _text.currentContext?.findRenderObject();
        final laid = drawn is RenderBox && drawn.hasSize ? drawn.size : from.size;
        final size = laid * grow;

        // A longer message swings less: a few words take nearly the full
        // swing, 80 characters half of it, a paragraph barely bows.
        final calm = 1 / (1 + f.body.length / 80);
        final rise = (from.top - dest.top).abs();
        final bulge = math.min((90 + rise * 0.5).clamp(110.0, 220.0), local(row).width * 0.4) * calm;
        final at = _arc(from.topLeft, dest.topLeft, bulge, _travel.transform(v));
        final (x, y) = (at.dx, at.dy);

        // The bubble: closes from where it started around the text as it
        // sets off, rides with it, and once it is nearly in place swells a
        // little as it dissolves, leaving the text where it belongs.
        final hug = _pad.inflateRect(Offset(x, y) & size);
        final gather = Curves.easeInOutCubic.transform((v / 0.4).clamp(0, 1));
        final settle = Curves.easeInOutCubic.transform(((v - 0.72) / 0.28).clamp(0, 1));
        final shape = Rect.lerp(local(f.fromShape), hug, gather)!.inflate(6 * settle);
        final radius = lerpDouble(f.fromRadius, math.min(hug.height / 2, 18), gather)!;
        final fill = f.outgoing ? Color.lerp(PColors.panelRaised, _mine, gather)! : PColors.selected;
        final edge = f.outgoing ? Color.lerp(PColors.focusEdge, _mineEdge, gather)! : PColors.borderStrong;
        // Raised off the page in flight, flat at both ends.
        final lift = math.sin(math.pi * v);

        return Stack(
          children: [
            Positioned.fromRect(
              rect: shape,
              child: IgnorePointer(
                child: Opacity(
                  // Yours starts as the composer itself, so it is there from
                  // the first frame; someone else's grows out of nothing.
                  opacity: (f.outgoing ? 1 : gather) * (1 - settle),
                  child: DecoratedBox(
                    decoration: BoxDecoration(
                      color: fill,
                      border: Border.all(color: edge),
                      borderRadius: BorderRadius.circular(radius),
                      boxShadow: [
                        BoxShadow(
                          color: Colors.black.withValues(alpha: 0.5 * lift),
                          blurRadius: 26 * lift,
                          offset: Offset(0, 10 * lift),
                        ),
                        if (f.outgoing)
                          BoxShadow(
                            color: PColors.accent.withValues(alpha: 0.22 * lift),
                            blurRadius: 30 * lift,
                            spreadRadius: lift,
                          ),
                      ],
                    ),
                  ),
                ),
              ),
            ),
            Positioned(
              left: x,
              top: y,
              child: IgnorePointer(
                // Wrapped exactly as the real text, so nothing moves when they
                // swap; the extra pixel keeps rounding from wrapping the last
                // word.
                child: Opacity(
                  // Someone else's message appears as it rises; the typing
                  // line it leaves fades out on its own.
                  opacity: f.outgoing ? 1 : Curves.easeInOut.transform((v / 0.5).clamp(0, 1)),
                  child: Transform.scale(
                    scale: grow,
                    alignment: Alignment.topLeft,
                    child: KeyedSubtree(
                      key: _text,
                      child: ConstrainedBox(
                        constraints: BoxConstraints(maxWidth: dest.width + 1),
                        child: MarkupText(
                          f.body,
                          style: widget.toStyle.copyWith(color: Color.lerp(f.outgoing ? f.fromStyle.color : widget.toStyle.color, widget.toStyle.color, t)),
                        ),
                      ),
                    ),
                  ),
                ),
              ),
            ),
          ],
        );
      },
    );
  }
}

// ---------------------------------------------------------------- header

class _Header extends StatelessWidget {
  const _Header({
    required this.state,
    required this.conversation,
    required this.memberCount,
    required this.panelOpen,
    required this.search,
    this.onTogglePanel,
  });

  final AppState state;
  final ConversationInfo conversation;
  final int memberCount;
  final bool panelOpen;
  final VoidCallback? onTogglePanel;

  /// The search box.
  final Widget search;

  @override
  Widget build(BuildContext context) {
    final c = conversation;
    final peer = c.peer;
    final subtitle = c.isGroup
        ? (memberCount == 0 ? 'Group' : '$memberCount ${memberCount == 1 ? 'member' : 'members'}')
        : '@$peer · ${presenceLabel(state.presenceOf(peer ?? ''))}';
    return Container(
      constraints: const BoxConstraints(minHeight: PShape.header),
      padding: const EdgeInsets.fromLTRB(22, 8, 12, 8),
      decoration: const BoxDecoration(
        border: Border(bottom: BorderSide(color: PColors.border)),
      ),
      child: Row(
        children: [
          Expanded(
            child: Row(
              children: [
                Flexible(
                  child: Column(
                    crossAxisAlignment: CrossAxisAlignment.start,
                    mainAxisSize: MainAxisSize.min,
                    children: [
                      Text(c.title, maxLines: 1, overflow: TextOverflow.ellipsis, style: PType.heading),
                      const SizedBox(height: 1),
                      AnimatedSwitcher(
                        duration: PMotion.normal,
                        layoutBuilder: (current, previous) => Stack(alignment: Alignment.centerLeft, children: [...previous, ?current]),
                        child: Text(subtitle, key: ValueKey(subtitle), maxLines: 1, overflow: TextOverflow.ellipsis, style: PType.small),
                      ),
                    ],
                  ),
                ),
                const SizedBox(width: 12),
                const _EncryptedBadge(),
              ],
            ),
          ),
          const SizedBox(width: 12),
          search,
          const SizedBox(width: 8),
          if (c.following) ...[CallButton(state: state, conversation: c), const SizedBox(width: 4)],
          if (onTogglePanel != null)
            PIconButton(
              icon: c.isGroup ? Icons.people_alt_outlined : Icons.person_outline_rounded,
              tooltip: panelOpen ? (c.isGroup ? 'Hide members' : 'Hide profile') : (c.isGroup ? 'Show members' : 'Show profile'),
              size: 40,
              selected: panelOpen,
              onPressed: onTogglePanel,
            ),
        ],
      ),
    );
  }
}

class _EncryptedBadge extends StatelessWidget {
  const _EncryptedBadge();

  @override
  Widget build(BuildContext context) {
    return Tooltip(
      message: 'Only the people in this conversation can read it. Keys resist quantum computers.',
      child: Container(
        height: 26,
        padding: const EdgeInsets.symmetric(horizontal: 10),
        decoration: BoxDecoration(color: PColors.accentSoft, borderRadius: BorderRadius.circular(13)),
        child: const Row(
          mainAxisSize: MainAxisSize.min,
          children: [
            Icon(Icons.lock_outline_rounded, size: 13, color: PColors.accentText),
            SizedBox(width: 6),
            Text(
              'End-to-end encrypted',
              style: TextStyle(fontSize: 12, fontWeight: FontWeight.w500, color: PColors.accentText),
            ),
          ],
        ),
      ),
    );
  }
}

// ---------------------------------------------------------------- lines

class _Beginning extends StatelessWidget {
  const _Beginning({super.key, required this.conversation, this.image});

  final ConversationInfo conversation;

  /// The other person's picture, in a DM.
  final PictureRef? image;

  @override
  Widget build(BuildContext context) {
    final c = conversation;
    return Padding(
      padding: const EdgeInsets.fromLTRB(10, 24, 10, 16),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          PAvatar(name: c.title, colorKey: c.isGroup ? c.id : (c.peer ?? c.id), size: 64, square: c.isGroup, image: image),
          const SizedBox(height: 14),
          Text(c.title, style: PType.title.copyWith(fontSize: 24)),
          const SizedBox(height: 6),
          Text(
            c.isGroup
                ? 'This is the start of ${c.title}. Messages are end-to-end encrypted; people added later see only what is sent after they join.'
                : 'This is the start of your conversation with ${c.title}. Compare safety numbers in their profile to be sure it is really them.',
            style: const TextStyle(fontSize: 14, height: 1.5, color: PColors.textMuted),
          ),
        ],
      ),
    );
  }
}

/// While a mention is being typed: the members it could be, the chosen one
/// lit. Arrows (in the composer) or the pointer choose; Tab, Enter or a
/// click picks. It never takes the keyboard from the composer.
class _MentionList extends StatelessWidget {
  const _MentionList({
    required this.choices,
    required this.picked,
    required this.query,
    required this.presenceOf,
    required this.avatarOf,
    required this.onHover,
    required this.onPick,
  });

  final List<MemberInfo> choices;
  final int picked;

  /// What has been typed after the `@`, picked out in each username.
  final String query;
  final String Function(String username) presenceOf;
  final PictureRef? Function(String username) avatarOf;
  final ValueChanged<int> onHover;
  final ValueChanged<MemberInfo> onPick;

  @override
  Widget build(BuildContext context) {
    return AnimatedSwitcher(
      duration: pMenuAnimation.duration!,
      reverseDuration: pMenuAnimation.reverseDuration!,
      switchInCurve: Curves.easeOutQuart,
      switchOutCurve: Curves.easeInCubic,
      transitionBuilder: (child, a) => FadeTransition(
        opacity: a,
        child: SlideTransition(
          position: Tween(begin: const Offset(0, 0.06), end: Offset.zero).animate(a),
          child: child,
        ),
      ),
      layoutBuilder: (current, previous) => Stack(alignment: Alignment.bottomCenter, children: [...previous, ?current]),
      child: choices.isEmpty ? const SizedBox(key: ValueKey('none'), width: double.infinity) : _panel(),
    );
  }

  Widget _panel() {
    return TextFieldTapRegion(
      key: const ValueKey('list'),
      child: Material(
        color: PColors.panelRaised,
        elevation: 12,
        shadowColor: Colors.black,
        shape: RoundedRectangleBorder(
          borderRadius: BorderRadius.circular(14),
          side: const BorderSide(color: PColors.borderStrong),
        ),
        clipBehavior: Clip.antiAlias,
        child: Padding(
          padding: const EdgeInsets.all(6),
          child: Column(
            mainAxisSize: MainAxisSize.min,
            crossAxisAlignment: CrossAxisAlignment.stretch,
            children: [
              const Padding(
                padding: EdgeInsets.fromLTRB(8, 4, 8, 6),
                child: Row(
                  children: [
                    Expanded(
                      child: Text(
                        'MEMBERS',
                        style: TextStyle(fontSize: 11, fontWeight: FontWeight.w700, letterSpacing: 0.6, color: PColors.textFaint),
                      ),
                    ),
                    Text('Tab or Enter to mention · Esc to close', style: TextStyle(fontSize: 11, color: PColors.textFaint)),
                  ],
                ),
              ),
              for (final (i, m) in choices.indexed) _row(i, m),
            ],
          ),
        ),
      ),
    );
  }

  Widget _row(int i, MemberInfo m) {
    final lit = i == picked;
    final matched = m.username.startsWith(query) ? query.length : 0;
    return MouseRegion(
      cursor: SystemMouseCursors.click,
      onEnter: (_) => onHover(i),
      child: GestureDetector(
        onTap: () => onPick(m),
        child: AnimatedContainer(
          duration: PMotion.fast,
          height: 38,
          padding: const EdgeInsets.symmetric(horizontal: 8),
          decoration: BoxDecoration(color: lit ? PColors.active : Colors.transparent, borderRadius: BorderRadius.circular(10)),
          child: Row(
            children: [
              PAvatar(
                name: m.shownName,
                colorKey: m.username,
                size: 26,
                presence: presenceOf(m.username),
                ringColor: lit ? PColors.active : PColors.panelRaised,
                image: avatarOf(m.username),
              ),
              const SizedBox(width: 10),
              Flexible(
                child: Text(
                  m.shownName,
                  maxLines: 1,
                  overflow: TextOverflow.ellipsis,
                  style: TextStyle(fontSize: 14, fontWeight: FontWeight.w500, color: lit ? PColors.textStrong : PColors.text),
                ),
              ),
              const SizedBox(width: 8),
              Text.rich(
                TextSpan(
                  children: [
                    TextSpan(
                      text: '@${m.username.substring(0, matched)}',
                      style: const TextStyle(color: PColors.accentText, fontWeight: FontWeight.w600),
                    ),
                    TextSpan(text: m.username.substring(matched)),
                  ],
                ),
                style: const TextStyle(fontSize: 12.5, color: PColors.textFaint),
              ),
            ],
          ),
        ),
      ),
    );
  }
}

/// Once you have scrolled back a way, a pill to bring you to the newest
/// message.
class _JumpToNewest extends StatelessWidget {
  const _JumpToNewest({required this.scroll, required this.onTap});

  final ScrollController scroll;
  final VoidCallback onTap;

  @override
  Widget build(BuildContext context) {
    return ListenableBuilder(
      listenable: scroll,
      builder: (context, _) {
        // The list is reversed: offset 0 is the newest message.
        final away = scroll.hasClients && scroll.offset > 600;
        return Center(
          child: IgnorePointer(
            ignoring: !away,
            child: AnimatedOpacity(
              opacity: away ? 1 : 0,
              duration: PMotion.normal,
              child: AnimatedSlide(
                offset: away ? Offset.zero : const Offset(0, 0.4),
                duration: PMotion.normal,
                curve: PMotion.curve,
                // Where you are, and the way back.
                child: Container(
                  padding: const EdgeInsets.fromLTRB(16, 5, 5, 5),
                  decoration: BoxDecoration(
                    color: PColors.panelRaised,
                    border: Border.all(color: PColors.borderStrong),
                    borderRadius: BorderRadius.circular(14),
                    boxShadow: const [BoxShadow(color: Color(0x66000000), blurRadius: 16, offset: Offset(0, 6))],
                  ),
                  child: Row(
                    mainAxisSize: MainAxisSize.min,
                    children: [
                      const Text("You're viewing older messages", style: TextStyle(fontSize: 13, color: PColors.textMuted)),
                      const SizedBox(width: 12),
                      FilledButton.icon(
                        onPressed: onTap,
                        icon: const Icon(Icons.arrow_downward_rounded, size: 15),
                        label: const Text('Jump to present'),
                        style: FilledButton.styleFrom(
                          minimumSize: const Size(0, 30),
                          padding: const EdgeInsets.symmetric(horizontal: 12),
                          textStyle: const TextStyle(fontSize: 12.5, fontWeight: FontWeight.w600),
                          shape: RoundedRectangleBorder(borderRadius: BorderRadius.circular(10)),
                        ),
                      ),
                    ],
                  ),
                ),
              ),
            ),
          ),
        );
      },
    );
  }
}

/// "New": a pill and a thin line in the unread colour, above the first
/// message that was unread.
class _NewLine extends StatelessWidget {
  const _NewLine();

  @override
  Widget build(BuildContext context) {
    return Padding(
      padding: const EdgeInsets.fromLTRB(10, 12, 10, 2),
      child: Row(
        children: [
          Container(
            padding: const EdgeInsets.symmetric(horizontal: 7, vertical: 1),
            decoration: BoxDecoration(color: PColors.unread, borderRadius: BorderRadius.circular(6)),
            child: const Text(
              'New',
              style: TextStyle(fontSize: 11, fontWeight: FontWeight.w700, color: PColors.onUnread),
            ),
          ),
          const SizedBox(width: 8),
          Expanded(child: Container(height: 1, color: PColors.unread.withValues(alpha: 0.5))),
        ],
      ),
    );
  }
}

class _DaySeparator extends StatelessWidget {
  const _DaySeparator(this.label);

  final String label;

  @override
  Widget build(BuildContext context) {
    return Padding(
      padding: const EdgeInsets.fromLTRB(10, 14, 10, 6),
      child: Row(
        children: [
          const Expanded(child: Divider(color: PColors.selected, height: 1)),
          Padding(
            padding: const EdgeInsets.symmetric(horizontal: 12),
            child: Text(
              label,
              style: const TextStyle(fontSize: 12, fontWeight: FontWeight.w600, color: PColors.textFaint),
            ),
          ),
          const Expanded(child: Divider(color: PColors.selected, height: 1)),
        ],
      ),
    );
  }
}

class _NoticeLine extends StatelessWidget {
  const _NoticeLine(this.n);

  final NoticeInfo n;

  @override
  Widget build(BuildContext context) {
    // A phone for calls, red when one was missed; a shield for the rest,
    // which are mostly security warnings.
    final (icon, color) = switch (n.kind) {
      NoticeKind.call => (Icons.call_outlined, PColors.online),
      NoticeKind.missedCall => (Icons.phone_missed_outlined, PColors.danger),
      NoticeKind.general => (Icons.shield_outlined, PColors.unverified),
    };
    return Padding(
      padding: const EdgeInsets.symmetric(horizontal: 10, vertical: 8),
      child: Row(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Padding(
            padding: const EdgeInsets.only(top: 1),
            child: Icon(icon, size: 15, color: color),
          ),
          const SizedBox(width: 8),
          Expanded(
            child: Text.rich(
              TextSpan(
                children: [
                  TextSpan(text: n.text),
                  TextSpan(
                    text: '  ${clockTime(n.atMs)}',
                    style: const TextStyle(fontSize: 11.5, color: PColors.textFaint),
                  ),
                ],
              ),
              style: const TextStyle(fontSize: 13, height: 1.45, color: PColors.textMuted),
            ),
          ),
        ],
      ),
    );
  }
}

class _MessageTile extends StatefulWidget {
  const _MessageTile({
    super.key,
    this.highlighted = false,
    this.mentionsYou = false,
    required this.arrived,
    required this.m,
    required this.startsGroup,
    required this.versions,
    required this.editing,
    required this.editor,
    required this.editorFocus,
    required this.onEdit,
    required this.onDelete,
    required this.onToggleVersions,
    required this.onSaveEdit,
    required this.onCancelEdit,
    required this.onMenu,
    required this.onPerson,
    required this.arrival,
    required this.receipts,
    required this.avatarOf,
    this.flight,
  });

  /// Who has this message (yours), for the ticks' hover card.
  final Future<MessageReceiptsInfo> Function() receipts;

  /// Someone's profile picture, if it is here.
  final PictureRef? Function(String username) avatarOf;

  /// The sender's avatar or name was clicked: given what was clicked, to
  /// show their card beside it.
  final ValueChanged<BuildContext> onPerson;

  /// How it arrives, if it is new: [Arrival.bubble] with a [flight], or
  /// played in place on its text.
  final Arrival arrival;

  /// It came in while the conversation was open: slide it in.
  final bool arrived;

  /// Opened from a search: lit for a moment.
  final bool highlighted;

  /// Someone else's message that mentions the viewer: warmed, with a bar.
  final bool mentionsYou;

  /// Where its text flies in from, if it is new and came from the composer
  /// or a typing line.
  final _Flight? flight;
  final MessageInfo m;
  final bool startsGroup;
  final List<MessageVersionInfo>? versions;
  final bool editing;
  final TextEditingController editor;
  final FocusNode editorFocus;
  final VoidCallback onEdit;
  final VoidCallback onDelete;
  final VoidCallback onToggleVersions;
  final VoidCallback onSaveEdit;
  final VoidCallback onCancelEdit;

  /// Right-clicked here (window coordinates).
  final ValueChanged<Offset> onMenu;

  @override
  State<_MessageTile> createState() => _MessageTileState();
}

class _MessageTileState extends State<_MessageTile> with TickerProviderStateMixin {
  // The toolbar floats in the list's overlay, outside the row, so the
  // pointer moving onto it leaves the row; over either counts as hovering.
  bool _overRow = false;
  bool _overBar = false;
  bool get _hover => _overRow || _overBar;

  final _barLink = LayerLink();
  final _bar = OverlayPortalController();
  late final AnimationController _barIn = AnimationController(vsync: this, duration: PMotion.fast)
    ..addStatusListener((s) {
      if (s == AnimationStatus.dismissed && _bar.isShowing) _bar.hide();
    });

  /// The row's top is near the list's: the toolbar sits inside the row
  /// rather than over its top edge, where the list would cut it off.
  bool _barInside = false;
  ScrollPosition? _watching;

  void _setOver({bool? row, bool? bar}) {
    if (!mounted) return;
    final was = _hover;
    setState(() {
      _overRow = row ?? _overRow;
      _overBar = bar ?? _overBar;
      if (_hover) _barInside = _nearTop();
    });
    if (_hover == was) return;
    // Rows move under a still pointer as the list scrolls.
    _watching?.removeListener(_replace);
    _watching = null;
    if (_hover) {
      _watching = Scrollable.maybeOf(context)?.position?..addListener(_replace);
      _bar.show();
      _barIn.forward();
    } else {
      _barIn.reverse();
    }
  }

  void _replace() {
    final inside = _nearTop();
    if (inside != _barInside) setState(() => _barInside = inside);
  }

  bool _nearTop() {
    final row = _rowKey.currentContext?.findRenderObject();
    final view = Scrollable.maybeOf(context)?.context.findRenderObject();
    if (row is! RenderBox || view is! RenderBox || !row.attached || !view.attached) return false;
    return row.localToGlobal(Offset.zero, ancestor: view).dy < 16;
  }

  // Instant means no animation at all: not even the row opening.
  late final AnimationController _in = AnimationController(
    vsync: this,
    duration: _rowEnters,
    value: widget.arrived && widget.arrival != Arrival.instant ? 0 : 1,
  );

  /// Around the message's text, and its row: where a flight lands.
  final _bodyKey = GlobalKey();
  final _rowKey = GlobalKey();

  /// False while the text is still flying in; the real text shows once it lands.
  late bool _landed = widget.flight == null;
  OverlayEntry? _flying;

  @override
  void initState() {
    super.initState();
    if (widget.arrived) _in.forward();
    // Laid out first, so the flight knows where to land.
    if (widget.flight != null) WidgetsBinding.instance.addPostFrameCallback((_) => _launch());
  }

  void _launch() {
    if (!mounted) return;
    final flight = widget.flight!;
    if (MediaQuery.disableAnimationsOf(context) || _ChatViewState._rectOf(_bodyKey) == null) {
      setState(() => _landed = true);
      return;
    }
    _flying = OverlayEntry(
      builder: (_) => _Flying(
        flight: flight,
        target: _bodyKey,
        tile: _rowKey,
        toStyle: PType.text,
        onDone: () {
          _flying?.remove();
          _flying = null;
          if (mounted) setState(() => _landed = true);
        },
      ),
    );
    // The navigator's overlay: the list's own would clip the flight to it.
    Navigator.of(context).overlay!.insert(_flying!);
  }

  @override
  void dispose() {
    _flying?.remove();
    _watching?.removeListener(_replace);
    _barIn.dispose();
    _in.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    // A new message makes room for itself: its row opens up, easing in and
    // out, so the messages above glide up rather than jump; then what is in
    // the row fades in (a flying message's text arrives on its own).
    final open = CurvedAnimation(
      parent: _in,
      curve: Interval(0, _rowOpens.inMicroseconds / _rowEnters.inMicroseconds, curve: Curves.easeInOutCubic),
    );
    final fade = CurvedAnimation(
      parent: _in,
      curve: const Interval(0.3, 1, curve: Curves.easeInOut),
    );
    return SizeTransition(
      sizeFactor: open,
      alignment: Alignment.topLeft,
      child: FadeTransition(opacity: fade, child: _tile(context)),
    );
  }

  Widget _tile(BuildContext context) {
    final m = widget.m;
    final mine = m.outgoing && !m.deleted && !m.unsupported;
    final actions = <Widget>[
      if (mine && m.body != null && !widget.editing) PIconButton(icon: Icons.edit_outlined, tooltip: 'Edit', size: 30, iconSize: 16, onPressed: widget.onEdit),
      if (m.edited && !m.deleted)
        PIconButton(
          icon: Icons.history_rounded,
          tooltip: widget.versions == null ? 'Show every version' : 'Hide versions',
          size: 30,
          iconSize: 16,
          onPressed: widget.onToggleVersions,
        ),
      if (m.body != null)
        PIconButton(
          icon: Icons.copy_rounded,
          tooltip: 'Copy text',
          size: 30,
          iconSize: 15,
          onPressed: () => copyText(context, m.body!, what: 'Message copied'),
        ),
      if (mine)
        PIconButton(
          icon: Icons.delete_outline_rounded,
          tooltip: 'Delete for everyone',
          size: 30,
          iconSize: 16,
          color: PColors.danger,
          onPressed: widget.onDelete,
        ),
    ];

    final body = Column(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        if (widget.startsGroup)
          Padding(
            padding: const EdgeInsets.only(bottom: 2),
            child: Row(
              crossAxisAlignment: CrossAxisAlignment.baseline,
              textBaseline: TextBaseline.alphabetic,
              children: [
                Flexible(
                  child: _PersonLink(
                    onTap: widget.onPerson,
                    child: Text(m.sender, maxLines: 1, overflow: TextOverflow.ellipsis, style: PType.name),
                  ),
                ),
                const SizedBox(width: 8),
                Text(messageTime(m.sentAtMs), style: const TextStyle(fontSize: 12, color: Color(0xFF7C829A))),
              ],
            ),
          ),
        if (widget.editing)
          _InlineEditor(controller: widget.editor, focus: widget.editorFocus, onSave: widget.onSaveEdit, onCancel: widget.onCancelEdit)
        else
          // Hidden, but laid out, while its text flies in.
          Opacity(
            opacity: _landed ? 1 : 0,
            child: KeyedSubtree(key: _bodyKey, child: _bodyText(m)),
          ),
        if (m.failed)
          const Padding(
            padding: EdgeInsets.only(top: 3),
            child: Text('Not sent: the server refused it.', style: TextStyle(fontSize: 12, color: PColors.dangerText)),
          ),
        if (m.edited && !m.deleted && !widget.editing)
          Padding(
            padding: const EdgeInsets.only(top: 4),
            child: _VersionsChip(count: m.versions, open: widget.versions != null, onTap: widget.onToggleVersions),
          ),
        if (widget.versions != null && !m.deleted) _VersionList(widget.versions!),
      ],
    );

    final showBar = actions.isNotEmpty && !widget.editing;
    return MouseRegion(
      onEnter: (_) => _setOver(row: true),
      onExit: (_) => _setOver(row: false),
      child: PRightClick(
        onMenu: widget.editing ? null : widget.onMenu,
        // The space above is outside the row's box, so a flight settles
        // into the row's shape exactly.
        child: AnimatedPadding(
          duration: PMotion.fast,
          padding: EdgeInsets.only(top: widget.startsGroup ? 10 : 0),
          child: OverlayPortal(
            controller: _bar,
            overlayChildBuilder: (context) => showBar ? _toolbar(actions) : const SizedBox.shrink(),
            child: CompositedTransformTarget(
              link: _barLink,
              child: AnimatedContainer(
                key: _rowKey,
                // Lit at once, fading slowly back.
                duration: widget.highlighted ? PMotion.fast : const Duration(milliseconds: 900),
                curve: PMotion.curve,
                padding: const EdgeInsets.fromLTRB(10, 3, 10, 3),
                decoration: BoxDecoration(
                  color: switch ((widget.highlighted, widget.mentionsYou, _hover || widget.editing)) {
                    (true, _, _) => const Color(0xFF1A2142),
                    (_, true, final hover) => hover ? PColors.mentionRowHover : PColors.mentionRow,
                    (_, _, true) => PColors.messageHover,
                    _ => PColors.panel,
                  },
                  borderRadius: BorderRadius.circular(12),
                ),
                child: Stack(
                  clipBehavior: Clip.none,
                  children: [
                    // Opened from a search: an accent bar down its left
                    // edge; mentioning you: an amber one, which stays.
                    Positioned(
                      left: -10,
                      top: -3,
                      bottom: -3,
                      child: AnimatedOpacity(
                        opacity: widget.highlighted || widget.mentionsYou ? 1 : 0,
                        duration: widget.highlighted ? PMotion.fast : const Duration(milliseconds: 900),
                        child: AnimatedContainer(
                          duration: const Duration(milliseconds: 900),
                          width: 3,
                          decoration: BoxDecoration(
                            color: widget.highlighted ? PColors.accent : PColors.mentionBar,
                            borderRadius: const BorderRadius.horizontal(left: Radius.circular(12)),
                          ),
                        ),
                      ),
                    ),
                    Opacity(
                      opacity: m.pending ? 0.55 : 1,
                      child: Row(
                        crossAxisAlignment: CrossAxisAlignment.start,
                        children: [
                          SizedBox(
                            width: 40,
                            child: widget.startsGroup
                                ? Padding(
                                    padding: const EdgeInsets.only(top: 2),
                                    child: _PersonLink(
                                      onTap: widget.onPerson,
                                      child: PAvatar(name: m.sender, colorKey: m.senderUsername, size: 40, image: widget.avatarOf(m.senderUsername)),
                                    ),
                                  )
                                : (_hover
                                      ? Padding(
                                          padding: const EdgeInsets.only(top: 4),
                                          child: Text(
                                            clockTime(m.sentAtMs),
                                            textAlign: TextAlign.center,
                                            style: const TextStyle(fontSize: 10.5, color: PColors.textFaint),
                                          ),
                                        )
                                      : null),
                          ),
                          const SizedBox(width: 14),
                          Expanded(child: body),
                          // Your messages' ticks, level with their first line.
                          if (m.outgoing && !m.deleted && !m.failed)
                            Padding(
                              padding: EdgeInsets.only(left: 10, top: widget.startsGroup ? 25 : 4),
                              child: _Ticks(m, load: widget.receipts, avatarOf: widget.avatarOf),
                            ),
                        ],
                      ),
                    ),
                  ],
                ),
              ),
            ),
          ),
        ),
      ),
    );
  }

  /// The hover toolbar, over the row's top-right corner: half above it,
  /// over whatever is there.
  Widget _toolbar(List<Widget> actions) {
    // The overlay gives its children its whole size; the toolbar takes its own.
    return Align(
      alignment: Alignment.topLeft,
      child: CompositedTransformFollower(
        link: _barLink,
        showWhenUnlinked: false,
        targetAnchor: Alignment.topRight,
        followerAnchor: Alignment.topRight,
        offset: Offset(-14, _barInside ? 4 : (widget.startsGroup ? -11 : -15)),
        child: IgnorePointer(
          ignoring: !_hover,
          child: MouseRegion(
            onEnter: (_) => _setOver(bar: true),
            onExit: (_) => _setOver(bar: false),
            child: FadeTransition(
              opacity: _barIn,
              child: SlideTransition(
                position: Tween(begin: const Offset(0, 0.15), end: Offset.zero).animate(CurvedAnimation(parent: _barIn, curve: PMotion.curve)),
                child: Container(
                  padding: const EdgeInsets.all(3),
                  decoration: BoxDecoration(
                    color: PColors.panelRaised,
                    border: Border.all(color: PColors.borderStrong),
                    borderRadius: BorderRadius.circular(12),
                    boxShadow: const [BoxShadow(color: Color(0x66000000), blurRadius: 12, offset: Offset(0, 4))],
                  ),
                  child: Row(mainAxisSize: MainAxisSize.min, children: actions),
                ),
              ),
            ),
          ),
        ),
      ),
    );
  }

  Widget _bodyText(MessageInfo m) {
    if (m.deleted) {
      return const Text(
        'Message deleted',
        style: TextStyle(fontSize: 14, height: 1.5, fontStyle: FontStyle.italic, color: PColors.textFaint),
      );
    }
    if (m.unsupported) {
      return const Text(
        'This message needs a newer version of Post.',
        style: TextStyle(fontSize: 14, height: 1.5, fontStyle: FontStyle.italic, color: PColors.textFaint),
      );
    }
    // In-place styles play on the text once the row has begun to open.
    return ArrivalText(
      m.body ?? '',
      style: PType.text,
      arrival: widget.arrival,
      play: widget.arrived && widget.arrival.inPlace,
      delay: const Duration(milliseconds: 140),
    );
  }
}

/// Read receipts' blue, a little brighter than the accent so it reads at a
/// glance against the grey of the other ticks.
const _readBlue = Color(0xFF5FB8FF);

/// Where one of your messages has got to: a clock while it waits to send,
/// one tick when the server has it, two when everyone else has it, two blue
/// when everyone has read it. Hovering says who, person by person.
class _Ticks extends StatefulWidget {
  const _Ticks(this.m, {required this.load, required this.avatarOf});

  final MessageInfo m;
  final Future<MessageReceiptsInfo> Function() load;
  final PictureRef? Function(String username) avatarOf;

  @override
  State<_Ticks> createState() => _TicksState();
}

class _TicksState extends State<_Ticks> {
  MessageReceiptsInfo? _who;
  bool _hover = false;

  String get _label {
    final m = widget.m;
    return m.pending ? 'Sending' : (m.read ? 'Read' : (m.delivered ? 'Delivered' : 'Sent'));
  }

  Future<void> _load() async {
    if (widget.m.pending) return;
    try {
      final who = await widget.load();
      if (mounted) setState(() => _who = who);
    } catch (_) {
      // The card says only the summary.
    }
  }

  @override
  void didUpdateWidget(_Ticks old) {
    super.didUpdateWidget(old);
    // A tick moved on while the card is open: say who now.
    if (_hover && (old.m.read != widget.m.read || old.m.delivered != widget.m.delivered || old.m.pending != widget.m.pending)) {
      unawaited(_load());
    }
  }

  /// A sentence for when there is no one to list, or null when there is.
  String? _why(MessageReceiptsInfo? who) {
    if (widget.m.pending) return 'Waiting to reach the server.';
    if (who == null) return null;
    if (who.tooLarge) return 'The server has it. Groups this large have no receipts.';
    if (who.people.isEmpty) return 'The server has it. No one else is here.';
    return null;
  }

  /// The brief overview, on hover: for each stage, up to three faces and how
  /// many more.
  InlineSpan _card() {
    const strong = TextStyle(fontWeight: FontWeight.w600, color: PColors.textStrong);
    final who = _who;
    final why = _why(who);
    if (who == null || why != null) {
      return TextSpan(
        children: [
          TextSpan(text: _label, style: strong),
          if (why != null) TextSpan(text: '\n$why'),
        ],
      );
    }
    return WidgetSpan(
      child: Column(
        mainAxisSize: MainAxisSize.min,
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          for (final stage in _ReceiptStage.values)
            if (stage.of(who.people) case final people when people.isNotEmpty)
              Padding(
                padding: const EdgeInsets.symmetric(vertical: 3),
                child: Row(
                  mainAxisSize: MainAxisSize.min,
                  children: [
                    Tooltip(
                      message: stage.title,
                      child: Icon(stage.icon, size: 15, color: stage.color),
                    ),
                    const SizedBox(width: 10),
                    _Faces(people, avatarOf: widget.avatarOf),
                    const SizedBox(width: 8),
                    Text('${people.length} ${stage.counted}', style: const TextStyle(fontSize: 12, color: PColors.textMuted)),
                  ],
                ),
              ),
          const SizedBox(height: 4),
          const Text('Click for details', style: TextStyle(fontSize: 11.5, color: PColors.textFaint)),
        ],
      ),
    );
  }

  /// The detailed overview, on click: everyone, stage by stage.
  Future<void> _details() async {
    Tooltip.dismissAllToolTips();
    await _load();
    if (!mounted) return;
    final who = _who;
    final why = _why(who);
    final m = widget.m;
    await showDialog<void>(
      context: context,
      builder: (context) => PDialog(
        title: 'Message info',
        width: 420,
        child: ConstrainedBox(
          constraints: BoxConstraints(maxHeight: MediaQuery.sizeOf(context).height * 0.6),
          child: SingleChildScrollView(
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.stretch,
              children: [
                // Which message: its text, and when it was sent.
                Container(
                  padding: const EdgeInsets.fromLTRB(14, 10, 14, 10),
                  decoration: BoxDecoration(
                    color: PColors.sunken,
                    border: Border.all(color: PColors.border),
                    borderRadius: BorderRadius.circular(12),
                  ),
                  child: Column(
                    crossAxisAlignment: CrossAxisAlignment.start,
                    children: [
                      Text(
                        plainText(m.body ?? ''),
                        maxLines: 3,
                        overflow: TextOverflow.ellipsis,
                        style: const TextStyle(fontSize: 13.5, height: 1.45, color: PColors.textBody),
                      ),
                      const SizedBox(height: 4),
                      Text('Sent ${messageTime(m.sentAtMs)}', style: PType.small),
                    ],
                  ),
                ),
                if (why != null || who == null) ...[
                  const SizedBox(height: 14),
                  Text(why ?? _label, style: const TextStyle(fontSize: 13.5, height: 1.45, color: PColors.textMuted)),
                ],
                if (who != null && why == null)
                  for (final stage in _ReceiptStage.values)
                    if (stage.of(who.people) case final people when people.isNotEmpty) ...[
                      const SizedBox(height: 18),
                      Row(
                        children: [
                          Icon(stage.icon, size: 15, color: stage.color),
                          const SizedBox(width: 8),
                          Text('${stage.title.toUpperCase()} · ${people.length}', style: PType.label),
                        ],
                      ),
                      const SizedBox(height: 8),
                      for (final p in people)
                        Padding(
                          padding: const EdgeInsets.symmetric(vertical: 5),
                          child: Row(
                            children: [
                              PAvatar(name: p.shownName, colorKey: p.username, size: 32, image: widget.avatarOf(p.username)),
                              const SizedBox(width: 12),
                              Expanded(
                                child: Column(
                                  crossAxisAlignment: CrossAxisAlignment.start,
                                  children: [
                                    Text(
                                      p.shownName,
                                      maxLines: 1,
                                      overflow: TextOverflow.ellipsis,
                                      style: const TextStyle(fontSize: 13.5, color: PColors.textStrong),
                                    ),
                                    Text('@${p.username}', maxLines: 1, overflow: TextOverflow.ellipsis, style: PType.small),
                                  ],
                                ),
                              ),
                            ],
                          ),
                        ),
                    ],
                if (who != null && who.readReceiptsOff && why == null) ...[
                  const SizedBox(height: 14),
                  const Text(
                    'Your read receipts are off, so you don’t see who has read it.',
                    style: TextStyle(fontSize: 12.5, height: 1.45, color: PColors.textFaint),
                  ),
                ],
              ],
            ),
          ),
        ),
      ),
    );
  }

  @override
  Widget build(BuildContext context) {
    final m = widget.m;
    final (icon, color) = m.pending
        ? (Icons.schedule_rounded, PColors.textFaint)
        : m.read
        ? (Icons.done_all_rounded, _readBlue)
        : m.delivered
        ? (Icons.done_all_rounded, PColors.textFaint)
        : (Icons.done_rounded, PColors.textFaint);
    final label = _label;
    return MouseRegion(
      cursor: SystemMouseCursors.click,
      onEnter: (_) {
        _hover = true;
        unawaited(_load());
      },
      onExit: (_) => _hover = false,
      child: GestureDetector(
        onTap: () => unawaited(_details()),
        child: Tooltip(
          richMessage: _card(),
          // Long enough to fetch who, short enough not to feel slow.
          waitDuration: const Duration(milliseconds: 250),
          preferBelow: false,
          textStyle: const TextStyle(fontSize: 12.5, height: 1.5, color: PColors.textBody),
          decoration: BoxDecoration(
            color: PColors.panelRaised,
            border: Border.all(color: PColors.borderStrong),
            borderRadius: BorderRadius.circular(10),
          ),
          padding: const EdgeInsets.symmetric(horizontal: 12, vertical: 8),
          child: AnimatedSwitcher(
            duration: PMotion.normal,
            transitionBuilder: (child, a) => FadeTransition(
              opacity: a,
              child: ScaleTransition(scale: Tween(begin: 0.7, end: 1.0).animate(a), child: child),
            ),
            child: Icon(icon, key: ValueKey(label), size: 15, color: color, semanticLabel: label),
          ),
        ),
      ),
    );
  }
}

/// How far people have got with a message, furthest first, as the ticks
/// show it.
enum _ReceiptStage {
  read(ReceiptState.read, Icons.done_all_rounded, _readBlue, 'Read', 'read'),
  delivered(ReceiptState.delivered, Icons.done_all_rounded, PColors.textFaint, 'Delivered', 'delivered'),
  waiting(ReceiptState.waiting, Icons.done_rounded, PColors.textFaint, 'Not delivered yet', 'waiting');

  const _ReceiptStage(this.state, this.icon, this.color, this.title, this.counted);

  final ReceiptState state;
  final IconData icon;
  final Color color;

  /// The stage's name, and the word after a count of people at it.
  final String title;
  final String counted;

  List<PersonReceiptInfo> of(List<PersonReceiptInfo> people) => people.where((p) => p.state == state).toList();
}

/// Up to three faces, overlapping, and how many more.
class _Faces extends StatelessWidget {
  const _Faces(this.people, {required this.avatarOf});

  final List<PersonReceiptInfo> people;
  final PictureRef? Function(String username) avatarOf;

  static const _size = 22.0;
  static const _step = 15.0;

  @override
  Widget build(BuildContext context) {
    final shown = people.take(3).toList();
    final more = people.length - shown.length;
    return Row(
      mainAxisSize: MainAxisSize.min,
      children: [
        SizedBox(
          width: _size + _step * (shown.length - 1),
          height: _size,
          child: Stack(
            children: [
              for (final (i, p) in shown.indexed)
                Positioned(
                  left: _step * i,
                  child: Container(
                    // A ring of the card's colour, so each face reads as cut
                    // out of the one behind.
                    padding: const EdgeInsets.all(1.5),
                    decoration: const BoxDecoration(color: PColors.panelRaised, shape: BoxShape.circle),
                    child: PAvatar(name: p.shownName, colorKey: p.username, size: _size - 3, image: avatarOf(p.username)),
                  ),
                ),
            ],
          ),
        ),
        if (more > 0) ...[
          const SizedBox(width: 6),
          Text(
            '+$more',
            style: const TextStyle(fontSize: 12, fontWeight: FontWeight.w600, color: PColors.textMuted),
          ),
        ],
      ],
    );
  }
}

/// A sender's avatar or name: a click shows their card beside it. The name
/// underlines on hover, so it reads as something to click.
class _PersonLink extends StatefulWidget {
  const _PersonLink({required this.onTap, required this.child});

  final ValueChanged<BuildContext> onTap;
  final Widget child;

  @override
  State<_PersonLink> createState() => _PersonLinkState();
}

class _PersonLinkState extends State<_PersonLink> {
  bool _hover = false;

  @override
  Widget build(BuildContext context) {
    return MouseRegion(
      cursor: SystemMouseCursors.click,
      onEnter: (_) => setState(() => _hover = true),
      onExit: (_) => setState(() => _hover = false),
      child: GestureDetector(
        onTap: () => widget.onTap(context),
        child: DefaultTextStyle.merge(
          style: TextStyle(decoration: _hover ? TextDecoration.underline : TextDecoration.none),
          child: widget.child,
        ),
      ),
    );
  }
}

class _VersionsChip extends StatelessWidget {
  const _VersionsChip({required this.count, required this.open, required this.onTap});

  final int count;
  final bool open;
  final VoidCallback onTap;

  @override
  Widget build(BuildContext context) {
    return MouseRegion(
      cursor: SystemMouseCursors.click,
      child: GestureDetector(
        onTap: onTap,
        child: Container(
          padding: const EdgeInsets.symmetric(horizontal: 8, vertical: 2),
          decoration: BoxDecoration(color: open ? PColors.selected : PColors.panelRaised, borderRadius: BorderRadius.circular(8)),
          child: Text('edited · $count versions', style: const TextStyle(fontSize: 12, color: PColors.textMuted)),
        ),
      ),
    );
  }
}

class _VersionList extends StatelessWidget {
  const _VersionList(this.versions);

  final List<MessageVersionInfo> versions;

  @override
  Widget build(BuildContext context) {
    return Container(
      margin: const EdgeInsets.only(top: 6),
      padding: const EdgeInsets.fromLTRB(14, 10, 14, 10),
      decoration: BoxDecoration(
        color: PColors.sunken,
        border: Border.all(color: PColors.selected),
        borderRadius: BorderRadius.circular(12),
      ),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          for (final (i, v) in versions.indexed)
            Padding(
              padding: EdgeInsets.only(top: i == 0 ? 0 : 8),
              child: Row(
                crossAxisAlignment: CrossAxisAlignment.start,
                children: [
                  SizedBox(
                    width: 92,
                    child: Text(
                      i == 0 ? 'Original ${clockTime(v.atMs)}' : (v.pending ? 'Sending…' : messageTime(v.atMs)),
                      style: const TextStyle(fontSize: 12, height: 1.5, color: Color(0xFF7C829A)),
                    ),
                  ),
                  Expanded(
                    child: Text(v.body, style: const TextStyle(fontSize: 13, height: 1.5, color: Color(0xFFB9BDCE))),
                  ),
                ],
              ),
            ),
        ],
      ),
    );
  }
}

class _InlineEditor extends StatelessWidget {
  const _InlineEditor({required this.controller, required this.focus, required this.onSave, required this.onCancel});

  final TextEditingController controller;
  final FocusNode focus;
  final VoidCallback onSave;
  final VoidCallback onCancel;

  @override
  Widget build(BuildContext context) {
    return Column(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        const SizedBox(height: 4),
        Focus(
          onKeyEvent: (_, e) {
            if (e is! KeyDownEvent) return KeyEventResult.ignored;
            if (e.logicalKey == LogicalKeyboardKey.escape) {
              onCancel();
              return KeyEventResult.handled;
            }
            if (e.logicalKey == LogicalKeyboardKey.enter && !HardwareKeyboard.instance.isShiftPressed && !controller.value.composing.isValid) {
              onSave();
              return KeyEventResult.handled;
            }
            return KeyEventResult.ignored;
          },
          child: TextField(
            controller: controller,
            focusNode: focus,
            minLines: 1,
            maxLines: 10,
            style: PType.text.copyWith(color: PColors.text),
            decoration: InputDecoration(
              isDense: true,
              fillColor: PColors.panelRaised,
              contentPadding: const EdgeInsets.symmetric(horizontal: 12, vertical: 10),
              enabledBorder: OutlineInputBorder(
                borderRadius: BorderRadius.circular(12),
                borderSide: const BorderSide(color: PColors.borderStrong),
              ),
            ),
          ),
        ),
        const SizedBox(height: 6),
        DefaultTextStyle.merge(
          style: const TextStyle(fontSize: 12, color: PColors.textFaint),
          child: Wrap(
            children: [
              const Text('escape to '),
              _Link('cancel', onCancel),
              const Text(' · enter to '),
              _Link('save', onSave),
              const Text(' · every version stays visible'),
            ],
          ),
        ),
      ],
    );
  }
}

class _Link extends StatelessWidget {
  const _Link(this.text, this.onTap);

  final String text;
  final VoidCallback onTap;

  @override
  Widget build(BuildContext context) {
    return MouseRegion(
      cursor: SystemMouseCursors.click,
      child: GestureDetector(
        onTap: onTap,
        child: Text(text, style: const TextStyle(color: PColors.accentText)),
      ),
    );
  }
}

// ---------------------------------------------------------------- composer

const _composerText = TextStyle(
  fontFamily: PType.body,
  fontSize: 14.5,
  height: 20 / 14.5,
  leadingDistribution: TextLeadingDistribution.even,
  color: PColors.text,
);

/// Formatting buttons floating just above the composer's selection, while
/// there is one: bold, italic, underline, strikethrough, spoiler, quote and
/// code, lit for the styles already there.
class _FormatToolbar extends StatefulWidget {
  const _FormatToolbar({required this.controller, required this.focus, required this.fieldKey, required this.onFormat, required this.child});

  final TextEditingController controller;
  final FocusNode focus;
  final GlobalKey fieldKey;
  final void Function(Edit Function(Edit)) onFormat;
  final Widget child;

  @override
  State<_FormatToolbar> createState() => _FormatToolbarState();
}

class _FormatToolbarState extends State<_FormatToolbar> {
  final _portal = OverlayPortalController();

  @override
  void initState() {
    super.initState();
    widget.controller.addListener(_update);
    widget.focus.addListener(_update);
  }

  @override
  void dispose() {
    widget.controller.removeListener(_update);
    widget.focus.removeListener(_update);
    super.dispose();
  }

  bool get _wanted {
    final sel = widget.controller.selection;
    return widget.focus.hasFocus && sel.isValid && !sel.isCollapsed;
  }

  void _update() {
    // After the next frame's layout, so the selection's place on screen is
    // known; and make sure there is a next frame.
    WidgetsBinding.instance.ensureVisualUpdate();
    WidgetsBinding.instance.addPostFrameCallback((_) {
      if (!mounted) return;
      if (_wanted) {
        _portal.show();
        setState(() {});
      } else if (_portal.isShowing) {
        _portal.hide();
      }
    });
  }

  /// The composer's text, as laid out.
  RenderEditable? _editable() {
    RenderEditable? found;
    void visit(RenderObject o) {
      if (found != null) return;
      if (o is RenderEditable) {
        found = o;
        return;
      }
      o.visitChildren(visit);
    }

    final root = widget.fieldKey.currentContext?.findRenderObject();
    if (root != null) visit(root);
    return found;
  }

  @override
  Widget build(BuildContext context) {
    return OverlayPortal(controller: _portal, overlayChildBuilder: _bar, child: widget.child);
  }

  Widget _bar(BuildContext context) {
    final editable = _editable();
    final overlay = Overlay.of(context).context.findRenderObject();
    final sel = widget.controller.selection;
    if (editable == null || !editable.attached || overlay is! RenderBox || !sel.isValid || sel.isCollapsed) return const SizedBox.shrink();
    // Centred over the selection's first line, just above it.
    final ends = editable.getEndpointsForSelection(sel);
    final lineHeight = editable.preferredLineHeight;
    final first = ends.first.point;
    final last = ends.last.point;
    final sameLine = (first.dy - last.dy).abs() < 1;
    final x = sameLine ? (first.dx + last.dx) / 2 : first.dx;
    final anchor = overlay.globalToLocal(editable.localToGlobal(Offset(x, first.dy - lineHeight)));
    final text = widget.controller.text;
    final marks = marksAt(text, sel.start, sel.end);
    final lineStart = sel.start == 0 ? 0 : text.lastIndexOf('\n', sel.start - 1) + 1;
    final quoted = text.startsWith('> ', lineStart);
    const width = 7 * 32.0 + 2 * 9 + 8;
    final left = (anchor.dx - width / 2).clamp(8.0, math.max(8.0, overlay.size.width - width - 8)).toDouble();
    return Positioned(
      left: left,
      top: anchor.dy - 44,
      // Taps here count as taps in the field: they leave it focused, with
      // its selection.
      child: TextFieldTapRegion(
        child: TweenAnimationBuilder<double>(
          tween: Tween(begin: 0, end: 1),
          duration: PMotion.fast,
          curve: PMotion.curve,
          builder: (context, v, child) => Opacity(
            opacity: v,
            child: Transform.translate(offset: Offset(0, 4 * (1 - v)), child: child),
          ),
          child: Container(
            height: 38,
            padding: const EdgeInsets.symmetric(horizontal: 4),
            decoration: BoxDecoration(
              color: PColors.panelRaised,
              border: Border.all(color: PColors.borderStrong),
              borderRadius: BorderRadius.circular(12),
              boxShadow: const [BoxShadow(color: Color(0x66000000), blurRadius: 14, offset: Offset(0, 4))],
            ),
            child: Row(
              mainAxisSize: MainAxisSize.min,
              children: [
                for (final (mark, icon, tip) in const [
                  (Mark.bold, Icons.format_bold_rounded, 'Bold (Ctrl+B)'),
                  (Mark.italic, Icons.format_italic_rounded, 'Italic (Ctrl+I)'),
                  (Mark.underline, Icons.format_underlined_rounded, 'Underline (Ctrl+U)'),
                  (Mark.strike, Icons.format_strikethrough_rounded, 'Strikethrough (Ctrl+Shift+X)'),
                  (Mark.spoiler, Icons.visibility_off_outlined, 'Spoiler'),
                ])
                  _FormatButton(icon: icon, tooltip: tip, on: marks.contains(mark), onTap: () => widget.onFormat((e) => toggleWrap(e, markerFor[mark]!))),
                const _FormatDivider(),
                _FormatButton(icon: Icons.format_quote_rounded, tooltip: 'Quote', on: quoted, onTap: () => widget.onFormat(toggleQuote)),
                _FormatButton(icon: Icons.code_rounded, tooltip: 'Code (Ctrl+E)', on: false, onTap: () => widget.onFormat(toggleCode)),
              ],
            ),
          ),
        ),
      ),
    );
  }
}

class _FormatDivider extends StatelessWidget {
  const _FormatDivider();

  @override
  Widget build(BuildContext context) => Container(width: 1, height: 20, margin: const EdgeInsets.symmetric(horizontal: 4), color: PColors.borderStrong);
}

class _FormatButton extends StatefulWidget {
  const _FormatButton({required this.icon, required this.tooltip, required this.on, required this.onTap});

  final IconData icon;
  final String tooltip;
  final bool on;
  final VoidCallback onTap;

  @override
  State<_FormatButton> createState() => _FormatButtonState();
}

class _FormatButtonState extends State<_FormatButton> {
  bool _hover = false;

  @override
  Widget build(BuildContext context) {
    return Tooltip(
      message: widget.tooltip,
      waitDuration: const Duration(milliseconds: 500),
      child: MouseRegion(
        cursor: SystemMouseCursors.click,
        onEnter: (_) => setState(() => _hover = true),
        onExit: (_) => setState(() => _hover = false),
        child: GestureDetector(
          onTap: widget.onTap,
          child: AnimatedContainer(
            duration: PMotion.fast,
            width: 32,
            height: 30,
            decoration: BoxDecoration(
              color: widget.on ? PColors.active : (_hover ? PColors.hoverStrong : Colors.transparent),
              borderRadius: BorderRadius.circular(8),
            ),
            child: Icon(widget.icon, size: 18, color: widget.on ? PColors.accentText : (_hover ? PColors.textStrong : PColors.textMuted)),
          ),
        ),
      ),
    );
  }
}

/// The composer's right-click menu, in the app's menu style: corrections
/// for a misspelled word and adding it to your dictionary, then the editing
/// commands with their shortcuts, then spelling's own switch and settings.
class _ComposerMenu extends StatefulWidget {
  const _ComposerMenu({required this.editable, required this.misspelled, required this.spelling, required this.onSpellingSettings});

  final EditableTextState editable;
  final List<TextRange> misspelled;
  final Spelling spelling;
  final VoidCallback onSpellingSettings;

  @override
  State<_ComposerMenu> createState() => _ComposerMenuState();
}

class _ComposerMenuState extends State<_ComposerMenu> {
  /// The misspelled word clicked, if one was.
  late final TextRange? _word = _findWord();
  List<String>? _suggestions;

  /// Opens upward: fixed once chosen.
  bool? _up;

  /// Under the pointer: a right-click on Linux leaves the cursor where it
  /// was, so the click's own position says which word was meant.
  TextRange? _findWord() {
    final e = widget.editable;
    final text = e.textEditingValue.text;
    final at = e.renderEditable.getPositionForPoint(e.contextMenuAnchors.primaryAnchor).offset;
    for (final r in widget.misspelled) {
      if (r.end <= text.length && r.start <= at && at <= r.end) return r;
    }
    return null;
  }

  @override
  void initState() {
    super.initState();
    final w = _word;
    if (w != null) {
      unawaited(
        widget.spelling.suggest(w.textInside(widget.editable.textEditingValue.text)).then((s) {
          if (mounted) setState(() => _suggestions = s);
        }),
      );
    }
  }

  void _close() => widget.editable.hideToolbar();

  void _replace(TextRange r, String with_) {
    final v = widget.editable.textEditingValue;
    widget.editable.userUpdateTextEditingValue(
      TextEditingValue(
        text: v.text.replaceRange(r.start, r.end, with_),
        selection: TextSelection.collapsed(offset: r.start + with_.length),
      ),
      SelectionChangedCause.toolbar,
    );
    _close();
  }

  @override
  Widget build(BuildContext context) {
    final e = widget.editable;
    final s = widget.spelling;
    final word = _word;
    final text = e.textEditingValue.text;
    final overlay = Overlay.of(context).context.findRenderObject()! as RenderBox;
    final anchor = overlay.globalToLocal(e.contextMenuAnchors.primaryAnchor);
    final rows = <Widget>[
      if (word != null) ...[
        if (_suggestions == null)
          const PMenuRow(label: 'Looking for corrections…', onTap: null)
        else if (_suggestions!.isEmpty)
          const PMenuRow(label: 'No corrections', onTap: null)
        else
          for (final sug in _suggestions!) PMenuRow(label: sug, strong: true, onTap: () => _replace(word, sug)),
        PMenuRow(
          label: 'Add to dictionary',
          onTap: () {
            unawaited(s.addWord(word.textInside(text)));
            _close();
          },
        ),
        const _MenuLine(),
      ],
      PMenuRow(label: 'Cut', shortcut: 'Ctrl+X', onTap: e.cutEnabled ? () => e.cutSelection(SelectionChangedCause.toolbar) : null),
      PMenuRow(label: 'Copy', shortcut: 'Ctrl+C', onTap: e.copyEnabled ? () => e.copySelection(SelectionChangedCause.toolbar) : null),
      PMenuRow(label: 'Paste', shortcut: 'Ctrl+V', onTap: e.pasteEnabled ? () => unawaited(e.pasteText(SelectionChangedCause.toolbar)) : null),
      PMenuRow(
        label: 'Select all',
        shortcut: 'Ctrl+A',
        onTap: e.selectAllEnabled && text.isNotEmpty
            ? () {
                e.selectAll(SelectionChangedCause.toolbar);
                _close();
              }
            : null,
      ),
      const _MenuLine(),
      PMenuRow(
        label: 'Check spelling',
        checked: s.enabled,
        onTap: () {
          unawaited(s.setEnabled(!s.enabled));
          _close();
        },
      ),
      PMenuRow(
        label: 'Spelling settings…',
        onTap: () {
          _close();
          widget.onSpellingSettings();
        },
      ),
    ];
    final theme = PopupMenuTheme.of(context);
    // Which way it opens, decided once from its full height (known from its
    // rows) rather than from its size while it grows: a growing menu that
    // fits below at first would flip above partway through.
    // Decided on the first frame, allowing for all five corrections, which
    // arrive a moment later.
    final up = _up ??= () {
      final shown = rows.fold<double>(0, (h, r) => h + (r is _MenuLine ? _MenuLine.height : 32));
      final corrections = word != null && _suggestions == null ? 4 * 32 : 0;
      return anchor.dy + 12 + shown + corrections > overlay.size.height - _MenuAtPointer.margin;
    }();
    return CustomSingleChildLayout(
      delegate: _MenuAtPointer(anchor, up: up),
      // Clicks in the menu count as clicks in the field: it keeps focus.
      child: TextFieldTapRegion(
        // Opening as the other menus do: quickly, with a long ease-out.
        child: ConstrainedBox(
          constraints: const BoxConstraints(minWidth: 200, maxWidth: 280),
          child: IntrinsicWidth(
            child: TweenAnimationBuilder<double>(
              tween: Tween(begin: 0, end: 1),
              duration: pMenuAnimation.duration!,
              curve: pMenuAnimation.curve!,
              builder: (context, v, child) => Opacity(
                opacity: (v * 3).clamp(0.0, 1.0),
                // The panel itself grows (it clips its entries), unrolling
                // away from the pointer: opening up, its bottom stays put
                // and its top rises.
                child: Material(
                  color: theme.color,
                  shape: theme.shape,
                  elevation: theme.elevation ?? 8,
                  clipBehavior: Clip.antiAlias,
                  child: Align(alignment: up ? Alignment.bottomLeft : Alignment.topLeft, widthFactor: 1, heightFactor: v, child: child),
                ),
              ),
              child: Padding(
                padding: const EdgeInsets.all(6),
                child: Column(mainAxisSize: MainAxisSize.min, crossAxisAlignment: CrossAxisAlignment.stretch, children: rows),
              ),
            ),
          ),
        ),
      ),
    );
  }
}

/// A menu with its corner at the pointer, as the app's other menus: to the
/// right, or the left where there is no room; below, or above when [up]
/// (decided beforehand from its full height); always inside the window.
class _MenuAtPointer extends SingleChildLayoutDelegate {
  _MenuAtPointer(this.at, {required this.up});

  final Offset at;
  final bool up;
  static const margin = 8.0;

  @override
  BoxConstraints getConstraintsForChild(BoxConstraints constraints) => constraints.loosen();

  @override
  Offset getPositionForChild(Size size, Size child) {
    var x = at.dx + child.width > size.width - margin ? at.dx - child.width : at.dx;
    var y = up ? at.dy - child.height : at.dy;
    x = x.clamp(margin, math.max(margin, size.width - child.width - margin));
    y = y.clamp(margin, math.max(margin, size.height - child.height - margin));
    return Offset(x, y);
  }

  @override
  bool shouldRelayout(_MenuAtPointer old) => old.at != at || old.up != up;
}

class _MenuLine extends StatelessWidget {
  const _MenuLine();

  /// 4 above, the line, 4 below.
  static const height = 9.0;

  @override
  Widget build(BuildContext context) => const Padding(
    padding: EdgeInsets.symmetric(horizontal: 6, vertical: 4),
    child: Divider(height: 1, thickness: 1, color: PColors.borderStrong),
  );
}

class _Composer extends StatelessWidget {
  const _Composer({
    required this.controller,
    required this.focus,
    required this.placeholder,
    required this.typing,
    required this.typingKey,
    required this.fieldKey,
    required this.boxKey,
    required this.holdHeight,
    required this.onKey,
    required this.onChanged,
    required this.onSend,
    required this.contextMenuBuilder,
  });

  static const typingStyle = TextStyle(fontFamily: PType.body, fontSize: 12.5, color: PColors.textMuted);

  final TextEditingController controller;
  final FocusNode focus;
  final String placeholder;

  /// Who is typing, and what to show for each.
  final List<(String, String)> typing;

  /// A key for each typing person's text, so their message can fly from it.
  final GlobalKey Function(String username) typingKey;

  /// On the text field, so a sent message can fly from it.
  final GlobalKey fieldKey;

  /// On the rounded box around it: a sent message's shape starts as this.
  final GlobalKey boxKey;

  /// The field's height to keep for now, while a sent message is on its way.
  final double? holdHeight;
  final FocusOnKeyEventCallback onKey;
  final ValueChanged<String> onChanged;
  final VoidCallback onSend;

  /// Its right-click menu.
  final EditableTextContextMenuBuilder contextMenuBuilder;

  @override
  Widget build(BuildContext context) {
    return Padding(
      padding: const EdgeInsets.fromLTRB(18, 0, 18, 16),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          ConstrainedBox(
            constraints: const BoxConstraints(minHeight: 26),
            child: AnimatedSwitcher(
              duration: PMotion.normal,
              transitionBuilder: (child, a) => FadeTransition(
                opacity: a,
                child: SlideTransition(
                  position: Tween(begin: const Offset(0, 0.3), end: Offset.zero).animate(a),
                  child: child,
                ),
              ),
              child: typing.isEmpty
                  ? const SizedBox(key: ValueKey('quiet'), width: double.infinity)
                  : Padding(
                      key: const ValueKey('typing'),
                      padding: const EdgeInsets.fromLTRB(6, 4, 6, 4),
                      child: Row(
                        children: [
                          const _TypingDots(),
                          const SizedBox(width: 8),
                          for (final (i, (user, text)) in typing.indexed) ...[
                            if (i > 0) const Text(' · ', style: typingStyle),
                            Flexible(
                              child: Text(text, key: typingKey(user), maxLines: 1, overflow: TextOverflow.ellipsis, style: typingStyle),
                            ),
                          ],
                        ],
                      ),
                    ),
            ),
          ),
          ListenableBuilder(
            listenable: focus,
            builder: (context, child) => AnimatedContainer(
              key: boxKey,
              duration: PMotion.normal,
              curve: PMotion.curve,
              constraints: const BoxConstraints(minHeight: 52),
              padding: const EdgeInsets.fromLTRB(16, 6, 6, 6),
              decoration: BoxDecoration(
                color: PColors.panelRaised,
                border: Border.all(color: focus.hasFocus ? PColors.focusEdge : const Color(0xFF23263A)),
                borderRadius: BorderRadius.circular(18),
                boxShadow: [BoxShadow(color: PColors.accent.withValues(alpha: focus.hasFocus ? 0.10 : 0), blurRadius: 20, spreadRadius: 1)],
              ),
              child: child,
            ),
            child: Row(
              crossAxisAlignment: CrossAxisAlignment.end,
              children: [
                Expanded(
                  child: Focus(
                    onKeyEvent: onKey,
                    // One line is 20px of text in 10px of padding: exactly as
                    // tall as the send button, so the two centre together.
                    // Desktop's compact density would quietly take 4px off
                    // each side, so it is switched off for this field.
                    //
                    // Its height eases between line counts, so sending a few
                    // lines lets the box (and the messages above it) settle
                    // back down rather than snap.
                    // Shrinking after a send, it moves exactly as the new
                    // message's row opens, so the history only ever rises.
                    child: AnimatedSize(
                      duration: _rowOpens,
                      curve: Curves.easeInOutCubic,
                      alignment: Alignment.bottomLeft,
                      child: ConstrainedBox(
                        constraints: BoxConstraints(minHeight: holdHeight ?? 0),
                        child: Theme(
                          data: Theme.of(context).copyWith(visualDensity: VisualDensity.standard),
                          child: TextField(
                            key: fieldKey,
                            controller: controller,
                            focusNode: focus,
                            minLines: 1,
                            maxLines: 8,
                            onChanged: onChanged,
                            textCapitalization: TextCapitalization.sentences,
                            textAlignVertical: TextAlignVertical.center,
                            // The hint takes the text's metrics, or it sits apart.
                            style: _composerText,
                            // Every line exactly 20px, whatever formatting
                            // it holds (inline code is smaller), so the box
                            // never changes height for a style.
                            strutStyle: StrutStyle.fromTextStyle(_composerText, forceStrutHeight: true),
                            contextMenuBuilder: contextMenuBuilder,
                            decoration: InputDecoration(
                              hintText: placeholder,
                              hintStyle: _composerText.copyWith(color: PColors.textFaint),
                              isDense: true,
                              filled: false,
                              contentPadding: const EdgeInsets.symmetric(vertical: 10),
                              border: InputBorder.none,
                              enabledBorder: InputBorder.none,
                              focusedBorder: InputBorder.none,
                            ),
                          ),
                        ),
                      ),
                    ),
                  ),
                ),
                const SizedBox(width: 8),
                ListenableBuilder(
                  listenable: controller,
                  builder: (context, _) {
                    final ready = controller.text.trim().isNotEmpty;
                    return Tooltip(
                      message: 'Send (Enter). Shift+Enter for a new line.',
                      waitDuration: const Duration(milliseconds: 600),
                      child: MouseRegion(
                        cursor: ready ? SystemMouseCursors.click : SystemMouseCursors.basic,
                        child: GestureDetector(
                          onTap: ready ? onSend : null,
                          child: AnimatedContainer(
                            duration: PMotion.normal,
                            curve: PMotion.curve,
                            width: 40,
                            height: 40,
                            decoration: BoxDecoration(
                              color: ready ? null : PColors.selected,
                              gradient: ready ? PColors.brand : null,
                              borderRadius: BorderRadius.circular(14),
                              boxShadow: [
                                BoxShadow(
                                  color: PColors.accent.withValues(alpha: ready ? 0.35 : 0),
                                  blurRadius: 14,
                                  offset: const Offset(0, 4),
                                ),
                              ],
                            ),
                            child: AnimatedScale(
                              scale: ready ? 1 : 0.85,
                              duration: PMotion.normal,
                              curve: ready ? Curves.easeOutBack : PMotion.curve,
                              child: Icon(Icons.arrow_upward_rounded, size: 20, color: ready ? PColors.onAccent : PColors.textFaint),
                            ),
                          ),
                        ),
                      ),
                    );
                  },
                ),
              ],
            ),
          ),
        ],
      ),
    );
  }
}

class _TypingDots extends StatefulWidget {
  const _TypingDots();

  @override
  State<_TypingDots> createState() => _TypingDotsState();
}

class _TypingDotsState extends State<_TypingDots> with SingleTickerProviderStateMixin {
  late final AnimationController _c = AnimationController(vsync: this, duration: const Duration(milliseconds: 1200))..repeat();

  @override
  void dispose() {
    _c.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    return AnimatedBuilder(
      animation: _c,
      builder: (context, _) => Row(
        mainAxisSize: MainAxisSize.min,
        children: [
          for (var i = 0; i < 3; i++) ...[
            if (i > 0) const SizedBox(width: 3),
            Builder(
              builder: (context) {
                // Each dot rises in turn.
                final t = ((_c.value - i * 0.12) % 1.0);
                final up = t < 0.4 ? (t < 0.2 ? t / 0.2 : (0.4 - t) / 0.2) : 0.0;
                return Transform.translate(
                  offset: Offset(0, -3 * up),
                  child: Opacity(
                    opacity: 0.5 + 0.5 * up,
                    child: Container(
                      width: 6,
                      height: 6,
                      decoration: const BoxDecoration(color: PColors.textMuted, shape: BoxShape.circle),
                    ),
                  ),
                );
              },
            ),
          ],
        ],
      ),
    );
  }
}

class _LeftBanner extends StatelessWidget {
  const _LeftBanner({required this.group});

  final bool group;

  @override
  Widget build(BuildContext context) {
    return Container(
      margin: const EdgeInsets.fromLTRB(18, 8, 18, 16),
      padding: const EdgeInsets.all(14),
      decoration: BoxDecoration(
        color: PColors.sunken,
        border: Border.all(color: PColors.border),
        borderRadius: BorderRadius.circular(16),
      ),
      child: Text(
        group
            ? 'You are no longer in this group. What was sent while you were stays here; someone in it can add you back.'
            : 'This device is no longer in this conversation. What it received stays here.',
        style: const TextStyle(fontSize: 13, height: 1.5, color: PColors.textMuted),
      ),
    );
  }
}
