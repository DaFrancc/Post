// One conversation: its messages with every edit visible, security notices
// in line, who is typing, and the composer.

import 'dart:async';
import 'dart:math' as math;
import 'dart:ui' show lerpDouble;

import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:post/src/rust/api/client.dart';

import '../state/app_state.dart';
import '../theme.dart';
import '../ui/arrival.dart';
import '../ui/controls.dart';
import '../ui/format.dart';
import '../ui/markup_text.dart';
import 'dialogs.dart';
import 'home.dart' show Panel;

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
  final _composer = TextEditingController();
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

  @override
  void initState() {
    super.initState();
    _events = _state.events.listen(_onEvent);
    HardwareKeyboard.instance.addHandler(_typeToCompose);
    unawaited(_load());
    unawaited(_loadMembers());
    _composerFocus.requestFocus();
  }

  @override
  void dispose() {
    _events.cancel();
    _composerHoldTimer?.cancel();
    HardwareKeyboard.instance.removeHandler(_typeToCompose);
    _composer.dispose();
    _composerFocus.dispose();
    _editor.dispose();
    _editorFocus.dispose();
    _scroll.dispose();
    super.dispose();
  }

  void _onEvent(AppEvent e) {
    switch (e.kind) {
      case EventKind.messages when e.conversation == _id:
        unawaited(_load());
      case EventKind.notice when e.conversation == _id:
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
      if (messages.isNotEmpty && messages.last.id != before) unawaited(_state.markRead(_id));
    } catch (e) {
      if (mounted) setState(() => _loadingMore = false);
    }
  }

  Future<void> _loadMembers() async {
    try {
      final m = await _state.client.members(conversation: _id);
      if (mounted) setState(() => _members = m);
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
          _Header(state: _state, conversation: c, memberCount: _members.length, panelOpen: widget.panelOpen, onTogglePanel: widget.onTogglePanel),
          Expanded(
            child: !_loaded
                ? const Center(child: PSpinner())
                : Stack(
                    children: [
                      NotificationListener<ScrollNotification>(
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
                                    : _Beginning(key: const ValueKey('beginning'), conversation: c);
                              }
                              final item = items[items.length - 1 - i];
                              return KeyedSubtree(key: item.key, child: _itemView(item));
                            },
                          ),
                        ),
                      ),
                      Positioned(
                        left: 0,
                        right: 0,
                        bottom: 10,
                        child: _JumpToNewest(scroll: _scroll, onTap: _jumpToNewest),
                      ),
                    ],
                  ),
          ),
          if (c.following)
            _Composer(
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
  const _Header({required this.state, required this.conversation, required this.memberCount, required this.panelOpen, this.onTogglePanel});

  final AppState state;
  final ConversationInfo conversation;
  final int memberCount;
  final bool panelOpen;
  final VoidCallback? onTogglePanel;

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
  const _Beginning({super.key, required this.conversation});

  final ConversationInfo conversation;

  @override
  Widget build(BuildContext context) {
    final c = conversation;
    return Padding(
      padding: const EdgeInsets.fromLTRB(10, 24, 10, 16),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          PAvatar(name: c.title, colorKey: c.isGroup ? c.id : (c.peer ?? c.id), size: 64, square: c.isGroup),
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
                child: MouseRegion(
                  cursor: SystemMouseCursors.click,
                  child: GestureDetector(
                    onTap: onTap,
                    child: Container(
                      padding: const EdgeInsets.fromLTRB(14, 7, 10, 7),
                      decoration: BoxDecoration(
                        color: PColors.panelRaised,
                        border: Border.all(color: PColors.borderStrong),
                        borderRadius: BorderRadius.circular(20),
                        boxShadow: const [BoxShadow(color: Color(0x66000000), blurRadius: 16, offset: Offset(0, 6))],
                      ),
                      child: const Row(
                        mainAxisSize: MainAxisSize.min,
                        children: [
                          Text(
                            'Jump to newest',
                            style: TextStyle(fontSize: 13, fontWeight: FontWeight.w500, color: PColors.text),
                          ),
                          SizedBox(width: 6),
                          Icon(Icons.arrow_downward_rounded, size: 16, color: PColors.accentText),
                        ],
                      ),
                    ),
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
    return Padding(
      padding: const EdgeInsets.symmetric(horizontal: 10, vertical: 8),
      child: Row(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          const Padding(
            padding: EdgeInsets.only(top: 1),
            child: Icon(Icons.shield_outlined, size: 15, color: PColors.unverified),
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
    required this.arrival,
    this.flight,
  });

  /// How it arrives, if it is new: [Arrival.bubble] with a [flight], or
  /// played in place on its text.
  final Arrival arrival;

  /// It came in while the conversation was open: slide it in.
  final bool arrived;

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

class _MessageTileState extends State<_MessageTile> with SingleTickerProviderStateMixin {
  bool _hover = false;
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
    Overlay.of(context).insert(_flying!);
  }

  @override
  void dispose() {
    _flying?.remove();
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
                  child: Text(m.sender, maxLines: 1, overflow: TextOverflow.ellipsis, style: PType.name),
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

    return MouseRegion(
      onEnter: (_) => setState(() => _hover = true),
      onExit: (_) => setState(() => _hover = false),
      child: PRightClick(
        onMenu: widget.editing ? null : widget.onMenu,
        // The space above is outside the row's box, so a flight settles
        // into the row's shape exactly.
        child: AnimatedPadding(
          duration: PMotion.fast,
          padding: EdgeInsets.only(top: widget.startsGroup ? 10 : 0),
          child: AnimatedContainer(
            key: _rowKey,
            duration: PMotion.fast,
            padding: const EdgeInsets.fromLTRB(10, 3, 10, 3),
            decoration: BoxDecoration(color: _hover || widget.editing ? PColors.messageHover : PColors.panel, borderRadius: BorderRadius.circular(12)),
            child: Stack(
              clipBehavior: Clip.none,
              children: [
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
                                child: PAvatar(name: m.sender, colorKey: m.senderUsername, size: 40),
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
                    ],
                  ),
                ),
                if (actions.isNotEmpty && !widget.editing)
                  Positioned(
                    right: 4,
                    top: widget.startsGroup ? -14 : -18,
                    child: IgnorePointer(
                      ignoring: !_hover,
                      child: AnimatedOpacity(
                        opacity: _hover ? 1 : 0,
                        duration: PMotion.fast,
                        child: AnimatedSlide(
                          offset: _hover ? Offset.zero : const Offset(0, 0.15),
                          duration: PMotion.fast,
                          curve: PMotion.curve,
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
              ],
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
