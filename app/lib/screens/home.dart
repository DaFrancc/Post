// The signed-in app: a rail, the conversation list, the open conversation
// and, when there is room, its members or the other person's profile.

import 'dart:async';

import 'package:flutter/material.dart';
import 'package:postern/src/rust/api/client.dart';

import '../state/app_state.dart';
import '../theme.dart';
import '../ui/controls.dart';
import '../ui/format.dart';
import 'chat.dart';
import 'dialogs.dart';
import 'settings.dart';
import 'side_panel.dart';

/// Below this width the side panel does not fit beside the chat.
const _panelMinWidth = 1100.0;

class HomeScreen extends StatefulWidget {
  const HomeScreen({super.key, required this.state});

  final AppState state;

  @override
  State<HomeScreen> createState() => _HomeScreenState();
}

class _HomeScreenState extends State<HomeScreen> {
  bool _panel = true;
  late final StreamSubscription<AppEvent> _notices;

  @override
  void initState() {
    super.initState();
    // Security notices about no conversation in particular pop up; those
    // about one show in it.
    _notices = widget.state.events.where((e) => e.kind == EventKind.notice).listen((e) {
      if (!mounted) return;
      final t = e.text;
      if (t != null && (e.conversation == null || e.conversation != widget.state.selected)) showToast(context, t, error: true);
    });
  }

  @override
  void dispose() {
    _notices.cancel();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    final state = widget.state;
    return ListenableBuilder(
      listenable: state,
      builder: (context, _) => LayoutBuilder(
        builder: (context, box) {
          final c = state.current;
          final roomForPanel = box.maxWidth >= _panelMinWidth;
          final showPanel = _panel && roomForPanel && c != null;
          return Padding(
            padding: const EdgeInsets.fromLTRB(PShape.gap, 0, PShape.gap, PShape.gap),
            child: Row(
              crossAxisAlignment: CrossAxisAlignment.stretch,
              children: [
                _Rail(state: state),
                const SizedBox(width: PShape.gap),
                SizedBox(width: 272, child: _ConversationList(state: state)),
                const SizedBox(width: PShape.gap),
                Expanded(
                  child: AnimatedSwitcher(
                    duration: PMotion.normal,
                    switchInCurve: PMotion.curve,
                    switchOutCurve: Curves.easeIn,
                    transitionBuilder: _fadeRise,
                    child: c == null
                        ? _Start(key: const ValueKey('start'), state: state)
                        : ChatView(
                            key: ValueKey(c.id),
                            state: state,
                            conversation: c,
                            panelOpen: showPanel,
                            onTogglePanel: roomForPanel ? () => setState(() => _panel = !_panel) : null,
                          ),
                  ),
                ),
                // Slides open and shut; within it, conversations cross-fade.
                AnimatedSwitcher(
                  duration: PMotion.slow,
                  switchInCurve: PMotion.curve,
                  switchOutCurve: PMotion.curve,
                  transitionBuilder: (child, a) => SizeTransition(
                    axis: Axis.horizontal,
                    alignment: Alignment.centerLeft,
                    sizeFactor: a,
                    child: FadeTransition(opacity: a, child: child),
                  ),
                  child: showPanel
                      ? Row(
                          key: const ValueKey('panel'),
                          children: [
                            const SizedBox(width: PShape.gap),
                            SizedBox(
                              width: 268,
                              child: AnimatedSwitcher(
                                duration: PMotion.normal,
                                child: SidePanel(key: ValueKey('panel-${c.id}'), state: state, conversation: c),
                              ),
                            ),
                          ],
                        )
                      : const SizedBox(key: ValueKey('none')),
                ),
              ],
            ),
          );
        },
      ),
    );
  }
}

/// Fade in while rising a few pixels: how a new view arrives.
Widget _fadeRise(Widget child, Animation<double> a) => FadeTransition(
  opacity: a,
  child: SlideTransition(
    position: Tween(begin: const Offset(0, 0.012), end: Offset.zero).animate(a),
    child: child,
  ),
);

/// A rounded panel, the unit the layout is built from.
class Panel extends StatelessWidget {
  const Panel({super.key, required this.child, this.padding = EdgeInsets.zero});

  final Widget child;
  final EdgeInsets padding;

  @override
  Widget build(BuildContext context) {
    return Container(
      padding: padding,
      clipBehavior: Clip.antiAlias,
      decoration: BoxDecoration(
        color: PColors.panel,
        border: Border.all(color: PColors.border),
        borderRadius: BorderRadius.circular(PShape.panelRadius),
      ),
      child: child,
    );
  }
}

// ---------------------------------------------------------------- rail

class _Rail extends StatelessWidget {
  const _Rail({required this.state});

  final AppState state;

  @override
  Widget build(BuildContext context) {
    final groups = state.conversations.where((c) => c.isGroup && c.following).toList();
    // DMs with something unread come up under Home, so they are in reach
    // from any conversation; they leave once read.
    final unreadDms = state.conversations.where((c) => !c.isGroup && c.following && c.unread > 0 && c.id != state.selected).toList();
    return SizedBox(
      width: 64,
      child: Column(
        children: [
          _RailButton(
            label: 'Home',
            selected: state.selected == null,
            onTap: () => state.select(null),
            child: Image.asset('assets/brand/logo.png', width: 48, height: 48, filterQuality: FilterQuality.medium),
          ),
          AnimatedSize(
            duration: PMotion.normal,
            curve: PMotion.curve,
            alignment: Alignment.topCenter,
            child: Column(
              children: [
                for (final d in unreadDms)
                  Padding(
                    padding: const EdgeInsets.only(top: 10),
                    child: _RailButton(
                      key: ValueKey(d.id),
                      label: d.title,
                      selected: false,
                      round: true,
                      badge: d.unread,
                      color: PColors.avatarFor(d.peer ?? d.id),
                      onTap: () => state.select(d.id),
                      child: Text(
                        initials(d.title),
                        style: const TextStyle(fontFamily: PType.body, fontWeight: FontWeight.w600, fontSize: 15, color: PColors.onAvatar),
                      ),
                    ),
                  ),
              ],
            ),
          ),
          const SizedBox(height: 10),
          Container(
            width: 28,
            height: 2,
            decoration: BoxDecoration(color: PColors.selected, borderRadius: BorderRadius.circular(2)),
          ),
          const SizedBox(height: 10),
          Expanded(
            child: ScrollConfiguration(
              behavior: ScrollConfiguration.of(context).copyWith(scrollbars: false),
              child: ListView(
                padding: EdgeInsets.zero,
                children: [
                  for (final g in groups)
                    Padding(
                      padding: const EdgeInsets.only(bottom: 10),
                      child: _RailButton(
                        label: g.title,
                        selected: state.selected == g.id,
                        unread: g.unread > 0,
                        badge: state.selected == g.id ? 0 : g.unread,
                        color: PColors.avatarFor(g.id),
                        onTap: () => state.select(g.id),
                        child: Text(
                          initials(g.title),
                          style: const TextStyle(fontFamily: PType.display, fontWeight: FontWeight.w600, fontSize: 15, color: PColors.onAvatar),
                        ),
                      ),
                    ),
                  Center(child: _NewGroupButton(onTap: () => newGroup(context, state))),
                ],
              ),
            ),
          ),
          const SizedBox(height: 10),
          PIconButton(
            icon: Icons.verified_user_outlined,
            tooltip: 'Devices and security',
            size: 48,
            iconSize: 20,
            onPressed: () => showSettings(context, state, section: SettingsSection.devices),
          ),
          // Centred on the card with your name, across the gap.
          const SizedBox(height: 13),
        ],
      ),
    );
  }
}

/// A rail entry: round at rest, squarer on hover or when selected, with a
/// pill on the left edge for unread or selected.
class _RailButton extends StatefulWidget {
  const _RailButton({
    super.key,
    required this.label,
    required this.selected,
    required this.onTap,
    required this.child,
    this.color,
    this.unread = false,
    this.badge = 0,
    this.round = false,
  });

  final String label;
  final bool selected;
  final bool unread;

  /// An unread count to show on the corner; 0 for none.
  final int badge;

  /// Stays round on hover (a person, rather than a group).
  final bool round;
  final Color? color;
  final VoidCallback onTap;
  final Widget child;

  @override
  State<_RailButton> createState() => _RailButtonState();
}

class _RailButtonState extends State<_RailButton> {
  bool _hover = false;
  bool _down = false;

  @override
  Widget build(BuildContext context) {
    final square = !widget.round && (widget.selected || _hover);
    final pill = widget.selected ? 36.0 : (_hover ? 18.0 : (widget.unread ? 8.0 : 0.0));
    final glow = widget.color ?? PColors.accent;
    return Tooltip(
      message: widget.label,
      preferBelow: false,
      waitDuration: const Duration(milliseconds: 300),
      child: MouseRegion(
        cursor: SystemMouseCursors.click,
        onEnter: (_) => setState(() => _hover = true),
        onExit: (_) => setState(() => _hover = false),
        child: GestureDetector(
          onTapDown: (_) => setState(() => _down = true),
          onTapUp: (_) => setState(() => _down = false),
          onTapCancel: () => setState(() => _down = false),
          onTap: widget.onTap,
          child: SizedBox(
            width: 64,
            height: 48,
            child: Stack(
              alignment: Alignment.center,
              children: [
                Positioned(
                  left: 0,
                  child: AnimatedContainer(
                    duration: PMotion.normal,
                    curve: PMotion.curve,
                    width: 4,
                    height: pill,
                    decoration: BoxDecoration(
                      gradient: widget.selected ? PColors.brand : null,
                      color: widget.selected ? null : PColors.text,
                      borderRadius: const BorderRadius.horizontal(right: Radius.circular(4)),
                    ),
                  ),
                ),
                AnimatedScale(
                  scale: _down ? 0.94 : 1,
                  duration: PMotion.fast,
                  child: AnimatedContainer(
                    duration: PMotion.normal,
                    curve: PMotion.curve,
                    width: 48,
                    height: 48,
                    clipBehavior: Clip.antiAlias,
                    alignment: Alignment.center,
                    decoration: BoxDecoration(
                      color: widget.color,
                      borderRadius: BorderRadius.circular(square ? 15 : 24),
                      boxShadow: [
                        BoxShadow(
                          color: glow.withValues(alpha: widget.selected ? 0.35 : (_hover ? 0.22 : 0)),
                          blurRadius: 16,
                          offset: const Offset(0, 4),
                        ),
                      ],
                    ),
                    child: widget.child,
                  ),
                ),
                // The count sits on the corner, ringed in the ground colour so
                // it reads as cut out of the icon.
                Positioned(
                  right: 4,
                  bottom: -2,
                  child: AnimatedScale(
                    scale: widget.badge > 0 ? 1 : 0,
                    duration: PMotion.normal,
                    curve: widget.badge > 0 ? Curves.easeOutBack : Curves.easeIn,
                    child: Container(
                      constraints: const BoxConstraints(minWidth: 22),
                      height: 22,
                      padding: const EdgeInsets.symmetric(horizontal: 5),
                      alignment: Alignment.center,
                      decoration: BoxDecoration(
                        color: PColors.unread,
                        borderRadius: BorderRadius.circular(11),
                        border: Border.all(color: PColors.ground, width: 3),
                      ),
                      child: Text(
                        widget.badge > 99 ? '99+' : '${widget.badge}',
                        style: const TextStyle(fontSize: 10.5, fontWeight: FontWeight.w700, color: PColors.onUnread),
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
}

class _NewGroupButton extends StatefulWidget {
  const _NewGroupButton({required this.onTap});

  final VoidCallback onTap;

  @override
  State<_NewGroupButton> createState() => _NewGroupButtonState();
}

class _NewGroupButtonState extends State<_NewGroupButton> {
  bool _hover = false;

  @override
  Widget build(BuildContext context) {
    return Tooltip(
      message: 'New group',
      preferBelow: false,
      child: MouseRegion(
        cursor: SystemMouseCursors.click,
        onEnter: (_) => setState(() => _hover = true),
        onExit: (_) => setState(() => _hover = false),
        child: GestureDetector(
          onTap: widget.onTap,
          child: AnimatedContainer(
            duration: const Duration(milliseconds: 150),
            width: 48,
            height: 48,
            decoration: BoxDecoration(
              color: _hover ? PColors.accentSoft : Colors.transparent,
              border: Border.all(color: _hover ? PColors.accent : const Color(0xFF34384F)),
              borderRadius: BorderRadius.circular(_hover ? 15 : 16),
            ),
            child: Icon(Icons.add_rounded, size: 22, color: _hover ? PColors.accentText : PColors.textFaint),
          ),
        ),
      ),
    );
  }
}

// ---------------------------------------------------------------- conversation list

class _ConversationList extends StatefulWidget {
  const _ConversationList({required this.state});

  final AppState state;

  @override
  State<_ConversationList> createState() => _ConversationListState();
}

class _ConversationListState extends State<_ConversationList> {
  final _search = TextEditingController();
  String _query = '';

  @override
  void dispose() {
    _search.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    final state = widget.state;
    final q = _query.trim().toLowerCase();
    bool matches(ConversationInfo c) => q.isEmpty || c.title.toLowerCase().contains(q) || (c.peer?.contains(q) ?? false);
    final dms = state.conversations.where((c) => !c.isGroup && matches(c)).toList();
    final groups = state.conversations.where((c) => c.isGroup && matches(c)).toList();
    return Panel(
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          ConstrainedBox(
            constraints: const BoxConstraints(minHeight: PShape.header),
            child: Padding(
              padding: const EdgeInsets.fromLTRB(20, 8, 14, 8),
              child: Row(
                children: [
                  const Expanded(child: Text('Messages', style: PType.title)),
                  PIconButton(icon: Icons.edit_outlined, tooltip: 'New message', onPressed: () => startDm(context, state)),
                ],
              ),
            ),
          ),
          AnimatedSize(
            duration: PMotion.normal,
            curve: PMotion.curve,
            child: state.connected
                ? const SizedBox(width: double.infinity)
                : const Padding(
                    padding: EdgeInsets.fromLTRB(20, 0, 16, 8),
                    child: Row(
                      children: [
                        PSpinner(size: 11),
                        SizedBox(width: 8),
                        Text('Connecting to the server…', style: TextStyle(fontSize: 12, color: PColors.away)),
                      ],
                    ),
                  ),
          ),
          Padding(
            padding: const EdgeInsets.fromLTRB(14, 0, 14, 6),
            child: TextField(
              controller: _search,
              onChanged: (v) => setState(() => _query = v),
              style: const TextStyle(fontSize: 14, color: PColors.text),
              decoration: InputDecoration(
                hintText: 'Search',
                isDense: true,
                contentPadding: const EdgeInsets.symmetric(vertical: 11),
                prefixIcon: const Icon(Icons.search_rounded, size: 18, color: PColors.textFaint),
                prefixIconConstraints: const BoxConstraints(minWidth: 40, minHeight: 40),
                suffixIconConstraints: const BoxConstraints(minWidth: 36, minHeight: 36),
                suffixIcon: _query.isEmpty
                    ? null
                    : PIconButton(
                        icon: Icons.close_rounded,
                        tooltip: 'Clear',
                        size: 28,
                        iconSize: 15,
                        onPressed: () => setState(() {
                          _search.clear();
                          _query = '';
                        }),
                      ),
                enabledBorder: OutlineInputBorder(
                  borderRadius: BorderRadius.circular(PShape.controlRadius),
                  borderSide: const BorderSide(color: PColors.selected),
                ),
              ),
            ),
          ),
          Expanded(
            child: state.conversations.isEmpty
                ? const _EmptyList()
                : ListView(
                    padding: const EdgeInsets.fromLTRB(8, 0, 8, 8),
                    children: [
                      if (dms.isNotEmpty) ...[const _SectionLabel('Direct messages'), for (final c in dms) _ConversationRow(state: state, c: c)],
                      if (groups.isNotEmpty) ...[const _SectionLabel('Groups'), for (final c in groups) _ConversationRow(state: state, c: c)],
                      if (dms.isEmpty && groups.isEmpty)
                        const Padding(
                          padding: EdgeInsets.all(20),
                          child: Text('Nothing matches.', style: PType.small, textAlign: TextAlign.center),
                        ),
                    ],
                  ),
          ),
          _YouCard(state: state),
        ],
      ),
    );
  }
}

class _EmptyList extends StatelessWidget {
  const _EmptyList();

  @override
  Widget build(BuildContext context) {
    return const Padding(
      padding: EdgeInsets.all(24),
      child: Text(
        'No conversations yet. Start one with the pencil above, or make a group with the + on the left.',
        style: TextStyle(fontSize: 13, height: 1.5, color: PColors.textFaint),
        textAlign: TextAlign.center,
      ),
    );
  }
}

class _SectionLabel extends StatelessWidget {
  const _SectionLabel(this.text);

  final String text;

  @override
  Widget build(BuildContext context) {
    return Padding(
      padding: const EdgeInsets.fromLTRB(12, 14, 12, 6),
      child: Text(text.toUpperCase(), style: PType.label),
    );
  }
}

/// What the list shows under a conversation's name.
String _preview(ConversationInfo c) {
  final m = c.lastMessage;
  if (m == null) return c.isGroup ? 'New group' : 'No messages yet';
  final body = m.deleted
      ? 'Message deleted'
      : m.unsupported
      ? 'A message this version cannot show'
      : (m.body ?? '').replaceAll('\n', ' ');
  final who = m.outgoing ? 'You: ' : (c.isGroup ? '${m.sender}: ' : '');
  return '$who$body';
}

class _ConversationRow extends StatefulWidget {
  const _ConversationRow({required this.state, required this.c});

  final AppState state;
  final ConversationInfo c;

  @override
  State<_ConversationRow> createState() => _ConversationRowState();
}

class _ConversationRowState extends State<_ConversationRow> {
  bool _hover = false;

  Future<void> _menu(Offset at) async {
    final c = widget.c;
    final state = widget.state;
    final peer = c.peer;
    final choice = await showContextMenu<String>(context, at, [
      const PMenuItem(value: 'open', label: 'Open'),
      if (c.unread > 0) const PMenuItem(value: 'read', label: 'Mark as read'),
      if (peer != null) const PMenuItem(value: 'nickname', label: 'Set nickname', caption: 'Only you see it'),
      if (c.isGroup && c.following) const PMenuItem(value: 'myname', label: 'Your name in this group'),
      if (c.isGroup && c.following) ...[const PMenuDivider(), const PMenuItem(value: 'leave', label: 'Leave group', danger: true)],
      if (peer != null) ...[const PMenuDivider(), const PMenuItem(value: 'copy', label: 'Copy username', icon: Icons.alternate_email_rounded)],
    ]);
    if (!mounted) return;
    switch (choice) {
      case 'open':
        state.select(c.id);
      case 'read':
        await state.markRead(c.id);
      case 'nickname':
        await setNickname(context, state, peer!);
      case 'myname':
        await setConversationName(context, state, c, '');
      case 'leave':
        await leaveGroup(context, state, c);
      case 'copy':
        await copyText(context, peer!, what: 'Username copied');
    }
  }

  @override
  Widget build(BuildContext context) {
    final c = widget.c;
    final state = widget.state;
    final selected = state.selected == c.id;
    final unread = c.unread > 0 && !selected;
    final typing = state.typingIn(c.id);
    final bg = selected ? PColors.active : (_hover ? PColors.hover : PColors.panel);
    return MouseRegion(
      cursor: SystemMouseCursors.click,
      onEnter: (_) => setState(() => _hover = true),
      onExit: (_) => setState(() => _hover = false),
      child: GestureDetector(
        onTap: () => state.select(c.id),
        onSecondaryTapUp: (d) => _menu(d.globalPosition),
        child: AnimatedContainer(
          duration: PMotion.fast,
          constraints: const BoxConstraints(minHeight: 52),
          margin: const EdgeInsets.only(bottom: 2),
          padding: const EdgeInsets.symmetric(horizontal: 10, vertical: 8),
          decoration: BoxDecoration(
            color: bg,
            border: Border.all(color: selected ? PColors.activeEdge : bg),
            borderRadius: BorderRadius.circular(PShape.rowRadius),
          ),
          child: Opacity(
            opacity: c.following ? 1 : 0.55,
            child: Row(
              children: [
                c.isGroup
                    ? PAvatar(name: c.title, colorKey: c.id, square: true)
                    : PAvatar(name: c.title, colorKey: c.peer ?? c.id, presence: state.presenceOf(c.peer ?? ''), ringColor: bg),
                const SizedBox(width: 12),
                Expanded(
                  child: Column(
                    crossAxisAlignment: CrossAxisAlignment.start,
                    children: [
                      Text(
                        c.title,
                        maxLines: 1,
                        overflow: TextOverflow.ellipsis,
                        style: TextStyle(
                          fontSize: 14,
                          fontWeight: unread ? FontWeight.w600 : FontWeight.w500,
                          color: selected || unread ? PColors.textStrong : const Color(0xFFC9CDDC),
                        ),
                      ),
                      const SizedBox(height: 2),
                      AnimatedSwitcher(
                        duration: PMotion.fast,
                        layoutBuilder: (current, previous) => Stack(alignment: Alignment.centerLeft, children: [...previous, ?current]),
                        child: Text(
                          typing.isNotEmpty ? typing.first : (c.following ? _preview(c) : (c.isGroup ? 'You left this group' : 'No longer updated')),
                          key: ValueKey(typing.isNotEmpty),
                          maxLines: 1,
                          overflow: TextOverflow.ellipsis,
                          style: TextStyle(fontSize: 12.5, color: typing.isNotEmpty ? PColors.accentText : PColors.textFaint),
                        ),
                      ),
                    ],
                  ),
                ),
                AnimatedScale(
                  scale: unread ? 1 : 0,
                  duration: PMotion.normal,
                  curve: unread ? Curves.easeOutBack : Curves.easeIn,
                  child: unread ? Padding(padding: const EdgeInsets.only(left: 8), child: _Badge(c.unread)) : const SizedBox(),
                ),
              ],
            ),
          ),
        ),
      ),
    );
  }
}

class _Badge extends StatelessWidget {
  const _Badge(this.count);

  final int count;

  @override
  Widget build(BuildContext context) {
    return Container(
      constraints: const BoxConstraints(minWidth: 20),
      height: 20,
      padding: const EdgeInsets.symmetric(horizontal: 6),
      alignment: Alignment.center,
      decoration: BoxDecoration(color: PColors.unread, borderRadius: BorderRadius.circular(10)),
      child: Text(
        count > 99 ? '99+' : '$count',
        style: const TextStyle(fontSize: 11.5, fontWeight: FontWeight.w700, color: PColors.onUnread),
      ),
    );
  }
}

// ---------------------------------------------------------------- you

class _YouCard extends StatelessWidget {
  const _YouCard({required this.state});

  final AppState state;

  @override
  Widget build(BuildContext context) {
    final note = state.personNow(state.me.username)?.note;
    final status = presenceChoiceLabel(state.ownPresence);
    return Container(
      margin: const EdgeInsets.fromLTRB(8, 0, 8, 8),
      padding: const EdgeInsets.all(6),
      decoration: BoxDecoration(
        color: PColors.sunken,
        border: Border.all(color: PColors.border),
        borderRadius: BorderRadius.circular(16),
      ),
      child: Row(
        children: [
          Expanded(
            child: Builder(
              builder: (anchor) => _HoverRow(
                tooltip: 'Set your status',
                onTap: () => _presenceMenu(anchor),
                child: Row(
                  children: [
                    PAvatar(name: state.myName, colorKey: state.me.username, presence: state.ownPresenceWord, ringColor: PColors.sunken),
                    const SizedBox(width: 10),
                    Expanded(
                      child: Column(
                        crossAxisAlignment: CrossAxisAlignment.start,
                        children: [
                          Text(state.myName, maxLines: 1, overflow: TextOverflow.ellipsis, style: PType.name),
                          Text(
                            note == null ? status : '$status · $note',
                            maxLines: 1,
                            overflow: TextOverflow.ellipsis,
                            style: const TextStyle(fontSize: 12, color: PColors.textFaint),
                          ),
                        ],
                      ),
                    ),
                  ],
                ),
              ),
            ),
          ),
          PIconButton(icon: Icons.settings_outlined, tooltip: 'Settings', onPressed: () => showSettings(context, state)),
        ],
      ),
    );
  }

  Future<void> _presenceMenu(BuildContext anchor) async {
    final box = anchor.findRenderObject() as RenderBox;
    final overlay = Overlay.of(anchor).context.findRenderObject() as RenderBox;
    final at = box.localToGlobal(Offset.zero, ancestor: overlay);
    final choice = await showMenu<PresenceChoice>(
      context: anchor,
      position: RelativeRect.fromRect(Rect.fromLTWH(at.dx, at.dy - 8, box.size.width, 0), Offset.zero & overlay.size),
      constraints: const BoxConstraints(minWidth: 270, maxWidth: 270),
      items: [
        for (final p in PresenceChoice.values)
          PopupMenuItem(
            value: p,
            padding: const EdgeInsets.symmetric(horizontal: 8, vertical: 2),
            child: _PresenceOption(choice: p, checked: p == state.ownPresence),
          ),
      ],
    );
    if (choice == null) return;
    try {
      await state.setOwnPresence(choice);
    } catch (e) {
      if (anchor.mounted) showToast(anchor, errorText(e), error: true);
    }
  }
}

String presenceChoiceLabel(PresenceChoice p) => switch (p) {
  PresenceChoice.online => 'Online',
  PresenceChoice.away => 'Away',
  PresenceChoice.doNotDisturb => 'Do not disturb',
  PresenceChoice.invisible => 'Invisible',
};

class _PresenceOption extends StatelessWidget {
  const _PresenceOption({required this.choice, required this.checked});

  final PresenceChoice choice;
  final bool checked;

  @override
  Widget build(BuildContext context) {
    final (word, help) = switch (choice) {
      PresenceChoice.online => ('online', 'Shown as online.'),
      PresenceChoice.away => ('away', 'Shown as away. Set by itself after 10 minutes idle.'),
      PresenceChoice.doNotDisturb => ('dnd', 'Shown as do not disturb. Your notifications are silenced.'),
      PresenceChoice.invisible => ('offline', 'Shown as offline. You still get everything.'),
    };
    return Container(
      padding: const EdgeInsets.all(8),
      decoration: BoxDecoration(color: checked ? PColors.active : null, borderRadius: BorderRadius.circular(12)),
      child: Row(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Padding(padding: const EdgeInsets.only(top: 4), child: PresenceDot(word)),
          const SizedBox(width: 10),
          Expanded(
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
                Text(presenceChoiceLabel(choice), style: PType.name),
                const SizedBox(height: 2),
                Text(help, style: const TextStyle(fontSize: 12, height: 1.4, color: PColors.textFaint)),
              ],
            ),
          ),
        ],
      ),
    );
  }
}

/// A row that highlights on hover and does something on tap.
class _HoverRow extends StatefulWidget {
  const _HoverRow({required this.child, required this.onTap, required this.tooltip});

  final Widget child;
  final VoidCallback onTap;
  final String tooltip;

  @override
  State<_HoverRow> createState() => _HoverRowState();
}

class _HoverRowState extends State<_HoverRow> {
  bool _hover = false;

  @override
  Widget build(BuildContext context) {
    return Tooltip(
      message: widget.tooltip,
      waitDuration: const Duration(milliseconds: 600),
      child: MouseRegion(
        cursor: SystemMouseCursors.click,
        onEnter: (_) => setState(() => _hover = true),
        onExit: (_) => setState(() => _hover = false),
        child: GestureDetector(
          onTap: widget.onTap,
          child: AnimatedContainer(
            duration: const Duration(milliseconds: 100),
            padding: const EdgeInsets.all(4),
            decoration: BoxDecoration(color: _hover ? PColors.hover : Colors.transparent, borderRadius: BorderRadius.circular(12)),
            child: widget.child,
          ),
        ),
      ),
    );
  }
}

// ---------------------------------------------------------------- nothing open

class _Start extends StatelessWidget {
  const _Start({super.key, required this.state});

  final AppState state;

  @override
  Widget build(BuildContext context) {
    return Panel(
      child: Center(
        child: ConstrainedBox(
          constraints: const BoxConstraints(maxWidth: 580),
          child: Padding(
            padding: const EdgeInsets.all(28),
            child: Column(
              mainAxisSize: MainAxisSize.min,
              children: [
                Container(
                  decoration: BoxDecoration(
                    borderRadius: BorderRadius.circular(18),
                    boxShadow: [BoxShadow(color: PColors.accent.withValues(alpha: 0.28), blurRadius: 36, offset: const Offset(0, 8))],
                  ),
                  child: ClipRRect(
                    borderRadius: BorderRadius.circular(18),
                    child: Image.asset('assets/brand/logo.png', width: 72, height: 72, filterQuality: FilterQuality.medium),
                  ),
                ),
                const SizedBox(height: 18),
                Text('Hello, ${state.myName}', style: PType.title.copyWith(fontSize: 24), textAlign: TextAlign.center),
                const SizedBox(height: 8),
                // Narrower than the buttons, for an easy line length.
                ConstrainedBox(
                  constraints: const BoxConstraints(maxWidth: 400),
                  child: const Text(
                    'Everything you send is end-to-end encrypted, with keys that resist quantum computers. Only the people in a conversation can read it.',
                    style: TextStyle(fontSize: 14, height: 1.55, color: PColors.textMuted),
                    textAlign: TextAlign.center,
                  ),
                ),
                const SizedBox(height: 24),
                Wrap(
                  alignment: WrapAlignment.center,
                  spacing: 10,
                  runSpacing: 10,
                  children: [
                    FilledButton.icon(
                      onPressed: () => startDm(context, state),
                      icon: const Icon(Icons.edit_outlined, size: 18),
                      label: const Text('New message'),
                    ),
                    OutlinedButton.icon(
                      onPressed: () => newGroup(context, state),
                      icon: const Icon(Icons.group_add_outlined, size: 18),
                      label: const Text('New group'),
                    ),
                    OutlinedButton.icon(
                      onPressed: () => linkDevice(context, state),
                      icon: const Icon(Icons.devices_outlined, size: 18),
                      label: const Text('Link a device'),
                    ),
                  ],
                ),
              ],
            ),
          ),
        ),
      ),
    );
  }
}
