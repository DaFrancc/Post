// One conversation: its messages with every edit visible, security notices
// in line, who is typing, and the composer.

import 'dart:async';

import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:postern/src/rust/api/client.dart';

import '../state/app_state.dart';
import '../theme.dart';
import '../ui/controls.dart';
import '../ui/format.dart';
import '../ui/markup_text.dart';
import 'dialogs.dart';
import 'home.dart' show Panel;

/// Messages from one sender this close together share one header.
const _groupWithin = Duration(minutes: 7);

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
      if (messages.isNotEmpty && messages.last.id != before) {
        final last = messages.last;
        if (!last.outgoing) _state.stoppedTyping(_id, last.senderUsername);
        unawaited(_state.markRead(_id));
      }
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
    _composer.clear();
    try {
      await _state.client.sendText(conversation: _id, text: text);
      _jumpToNewest();
    } catch (e) {
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
    final typing = _state.typingIn(_id);
    final items = _timeline();
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
                            itemBuilder: (context, i) {
                              if (i == items.length) {
                                return _hasMore
                                    ? const Padding(
                                        padding: EdgeInsets.all(16),
                                        child: Center(child: PSpinner()),
                                      )
                                    : _Beginning(conversation: c);
                              }
                              return _itemView(items[items.length - 1 - i]);
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
      key: ValueKey(m.id),
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

sealed class _Item {
  int get at;
}

class _Msg extends _Item {
  _Msg(this.m, {required this.startsGroup});

  final MessageInfo m;
  final bool startsGroup;

  @override
  int get at => m.sentAtMs;
}

class _Notice extends _Item {
  _Notice(this.n);

  final NoticeInfo n;

  @override
  int get at => n.atMs;
}

/// Where unread messages began when the conversation opened.
class _NewMark extends _Item {
  _NewMark(this.at);

  @override
  final int at;
}

class _Day extends _Item {
  _Day(this.at);

  @override
  final int at;
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
  const _Beginning({required this.conversation});

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
    super.key,
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
  });

  /// It came in while the conversation was open: slide it in.
  final bool arrived;
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
  late final AnimationController _in = AnimationController(vsync: this, duration: PMotion.slow, value: widget.arrived ? 0 : 1);

  @override
  void initState() {
    super.initState();
    if (widget.arrived) _in.forward();
  }

  @override
  void dispose() {
    _in.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    final a = CurvedAnimation(parent: _in, curve: PMotion.curve);
    return FadeTransition(
      opacity: a,
      child: SlideTransition(
        position: Tween(begin: const Offset(0, 0.25), end: Offset.zero).animate(a),
        child: _tile(context),
      ),
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
          _bodyText(m),
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
        child: AnimatedContainer(
          duration: PMotion.fast,
          margin: EdgeInsets.only(top: widget.startsGroup ? 10 : 0),
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
        'This message needs a newer version of Postern.',
        style: TextStyle(fontSize: 14, height: 1.5, fontStyle: FontStyle.italic, color: PColors.textFaint),
      );
    }
    return MarkupText(m.body ?? '', style: PType.text);
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
    required this.onKey,
    required this.onChanged,
    required this.onSend,
  });

  final TextEditingController controller;
  final FocusNode focus;
  final String placeholder;
  final List<String> typing;
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
                          Expanded(
                            child: Text(
                              typing.join(' · '),
                              maxLines: 1,
                              overflow: TextOverflow.ellipsis,
                              style: const TextStyle(fontSize: 12.5, color: PColors.textMuted),
                            ),
                          ),
                        ],
                      ),
                    ),
            ),
          ),
          ListenableBuilder(
            listenable: focus,
            builder: (context, child) => AnimatedContainer(
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
                    child: Theme(
                      data: Theme.of(context).copyWith(visualDensity: VisualDensity.standard),
                      child: TextField(
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
