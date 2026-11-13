// Beside a conversation: a group's members by presence, with what an admin
// can do to them; or, for a DM, the other person's profile and the safety
// number to compare with them.

import 'dart:async';

import 'package:flutter/material.dart';
import 'package:post/src/rust/api/client.dart';

import '../state/app_state.dart';
import '../theme.dart';
import '../ui/controls.dart';
import '../ui/format.dart';
import 'dialogs.dart';
import 'home.dart' show Panel;
import 'profile_card.dart';

class SidePanel extends StatelessWidget {
  const SidePanel({super.key, required this.state, required this.conversation});

  final AppState state;
  final ConversationInfo conversation;

  @override
  Widget build(BuildContext context) {
    return Panel(
      child: conversation.isGroup ? _Members(state: state, conversation: conversation) : _Profile(state: state, username: conversation.peer ?? ''),
    );
  }
}

// ---------------------------------------------------------------- members

class _Members extends StatefulWidget {
  const _Members({required this.state, required this.conversation});

  final AppState state;
  final ConversationInfo conversation;

  @override
  State<_Members> createState() => _MembersState();
}

class _MembersState extends State<_Members> {
  List<MemberInfo> _members = const [];
  late final StreamSubscription<AppEvent> _events;

  ConversationInfo get _c => widget.conversation;
  AppState get _state => widget.state;
  bool get _amAdmin => _members.any((m) => m.you && m.admin);

  @override
  void initState() {
    super.initState();
    _events = _state.events.listen((e) {
      if (e.kind == EventKind.profile || e.kind == EventKind.devices || (e.kind == EventKind.conversations && e.conversation == _c.id)) {
        unawaited(_load());
      }
    });
    unawaited(_load());
  }

  @override
  void dispose() {
    _events.cancel();
    super.dispose();
  }

  Future<void> _load() async {
    try {
      final m = await _state.client.members(conversation: _c.id);
      if (mounted) setState(() => _members = m);
    } catch (_) {}
  }

  Future<void> _act(Future<void> Function() f) async {
    try {
      await f();
    } catch (e) {
      if (mounted) showToast(context, errorText(e), error: true);
    }
  }

  Future<void> _groupMenu(BuildContext anchor) async {
    // Opens just under the button.
    final box = anchor.findRenderObject()! as RenderBox;
    final choice = await showContextMenu<String>(anchor, box.localToGlobal(Offset(0, box.size.height + 4)), [
      if (_amAdmin) const PMenuItem(value: 'add', label: 'Add people', icon: Icons.person_add_alt_outlined),
      if (_amAdmin) const PMenuItem(value: 'rename', label: 'Rename group'),
      const PMenuItem(value: 'myname', label: 'Your name in this group', caption: 'Shown to everyone here'),
      if (_c.following) ...[const PMenuDivider(), const PMenuItem(value: 'leave', label: 'Leave group', danger: true)],
    ]);
    if (!mounted) return;
    switch (choice) {
      case 'add':
        await addMembers(context, _state, _c);
      case 'rename':
        await renameGroup(context, _state, _c);
      case 'myname':
        final me = _members.where((m) => m.you).firstOrNull;
        await setConversationName(context, _state, _c, me?.shownName == _state.myName ? '' : (me?.shownName ?? ''));
      case 'leave':
        await leaveGroup(context, _state, _c);
    }
  }

  Future<void> _memberMenu(MemberInfo m, Offset at) async {
    // Like other chat apps: talking to them first, then naming, then what an
    // admin can do, then copying.
    final calls = _state.calls;
    final inCallHere = calls.active?.conversation == _c.id;
    final theyreIn = calls.activeInfo?.participants.any((p) => p.username == m.username) ?? false;
    final callHere = calls.callIn(_c.id);
    final choice = await showContextMenu<String>(context, at, [
      if (!m.you) const PMenuItem(value: 'dm', label: 'Message'),
      // Ring just them: into the call you are in here, or into a new one.
      if (!m.you && _c.following && inCallHere && !theyreIn)
        PMenuItem(value: 'ring', label: 'Ring ${m.shownName} into the call', icon: Icons.call_outlined),
      if (!m.you && _c.following && callHere == null)
        PMenuItem(value: 'call', label: 'Call ${m.shownName}', caption: 'Rings only them', icon: Icons.call_outlined),
      if (!m.you) const PMenuItem(value: 'nickname', label: 'Set nickname', caption: 'Only you see it'),
      if (m.you) const PMenuItem(value: 'myname', label: 'Your name in this group', caption: 'Shown to everyone here'),
      if (_amAdmin) ...[
        const PMenuDivider(),
        if (!m.admin) const PMenuItem(value: 'admin', label: 'Make admin'),
        if (m.admin) const PMenuItem(value: 'unadmin', label: 'Remove as admin'),
        if (!m.you) PMenuItem(value: 'remove', label: 'Remove from ${_c.title}', danger: true),
      ],
      const PMenuDivider(),
      const PMenuItem(value: 'copy', label: 'Copy username', icon: Icons.alternate_email_rounded),
    ]);
    if (!mounted) return;
    switch (choice) {
      case 'dm':
        await _act(() async {
          final dm = await _state.client.startDm(username: m.username);
          await _state.refreshConversations();
          _state.select(dm.id);
        });
      case 'ring':
        await calls.ringInto([m.username]);
      case 'call':
        await calls.start(_c.id, ring: [m.username]);
      case 'nickname':
        await setNickname(context, _state, m.username);
      case 'myname':
        await setConversationName(context, _state, _c, '');
      case 'admin':
        await _act(() => _state.client.setAdmin(conversation: _c.id, username: m.username, admin: true));
      case 'unadmin':
        await _act(() => _state.client.setAdmin(conversation: _c.id, username: m.username, admin: false));
      case 'remove':
        final ok = await confirm(
          context,
          title: 'Remove ${m.shownName}?',
          body: 'They stop getting messages in ${_c.title}, on all their devices. An admin can add them back.',
          action: 'Remove',
          danger: true,
        );
        if (ok) await _act(() => _state.client.removeMembers(conversation: _c.id, usernames: [m.username]));
      case 'copy':
        await copyText(context, m.username, what: 'Username copied');
    }
  }

  @override
  Widget build(BuildContext context) {
    return ListenableBuilder(
      listenable: _state,
      builder: (context, _) {
        const order = ['online', 'away', 'dnd', 'offline'];
        final sections = <String, List<MemberInfo>>{for (final s in order) s: []};
        for (final m in _members) {
          (sections[_state.presenceOf(m.username)] ?? sections['offline']!).add(m);
        }
        return Column(
          crossAxisAlignment: CrossAxisAlignment.stretch,
          children: [
            Padding(
              padding: const EdgeInsets.fromLTRB(20, 14, 10, 4),
              child: Row(
                children: [
                  const Expanded(child: Text('Members', style: PType.heading)),
                  Builder(
                    builder: (anchor) => PIconButton(icon: Icons.more_horiz_rounded, tooltip: 'Group options', onPressed: () => _groupMenu(anchor)),
                  ),
                ],
              ),
            ),
            Expanded(
              child: ListView(
                padding: const EdgeInsets.fromLTRB(10, 0, 10, 12),
                children: [
                  for (final s in order)
                    if (sections[s]!.isNotEmpty) ...[
                      Padding(
                        padding: const EdgeInsets.fromLTRB(10, 12, 10, 4),
                        child: Text('${presenceLabel(s).toUpperCase()} · ${sections[s]!.length}', style: PType.label),
                      ),
                      for (final m in sections[s]!) _MemberRow(state: _state, m: m, presence: s, onMenu: (at) => _memberMenu(m, at)),
                    ],
                  if (_amAdmin && _c.following)
                    Padding(
                      padding: const EdgeInsets.fromLTRB(4, 12, 4, 0),
                      child: OutlinedButton.icon(
                        onPressed: () => addMembers(context, _state, _c),
                        icon: const Icon(Icons.person_add_alt_outlined, size: 18),
                        label: const Text('Add people'),
                      ),
                    ),
                ],
              ),
            ),
          ],
        );
      },
    );
  }
}

class _MemberRow extends StatefulWidget {
  const _MemberRow({required this.state, required this.m, required this.presence, required this.onMenu});

  final AppState state;
  final MemberInfo m;
  final String presence;
  final ValueChanged<Offset> onMenu;

  @override
  State<_MemberRow> createState() => _MemberRowState();
}

class _MemberRowState extends State<_MemberRow> {
  bool _hover = false;

  @override
  Widget build(BuildContext context) {
    final m = widget.m;
    final note = widget.state.personNow(m.username)?.note;
    final bg = _hover ? PColors.hover : PColors.panel;
    return PRightClick(
      onMenu: widget.onMenu,
      child: MouseRegion(
        cursor: SystemMouseCursors.click,
        onEnter: (_) => setState(() => _hover = true),
        onExit: (_) => setState(() => _hover = false),
        // A click shows their card beside the row; a right-click, the menu.
        child: GestureDetector(
          onTap: () => unawaited(showProfileCard(context, widget.state, m.username)),
          child: Container(
            padding: const EdgeInsets.symmetric(horizontal: 10, vertical: 7),
            decoration: BoxDecoration(color: bg, borderRadius: BorderRadius.circular(12)),
            child: Opacity(
              opacity: widget.presence == 'offline' ? 0.6 : 1,
              child: Row(
                children: [
                  PAvatar(
                    name: m.shownName,
                    colorKey: m.username,
                    size: 32,
                    presence: widget.presence,
                    ringColor: bg,
                    image: widget.state.avatarOf(m.username),
                  ),
                  const SizedBox(width: 10),
                  Expanded(
                    child: Column(
                      crossAxisAlignment: CrossAxisAlignment.start,
                      children: [
                        Row(
                          children: [
                            Flexible(
                              child: Text(
                                m.you ? '${m.shownName} (you)' : m.shownName,
                                maxLines: 1,
                                overflow: TextOverflow.ellipsis,
                                style: const TextStyle(fontSize: 14, fontWeight: FontWeight.w500, color: PColors.text),
                              ),
                            ),
                            if (m.admin) ...[
                              const SizedBox(width: 6),
                              Container(
                                padding: const EdgeInsets.symmetric(horizontal: 6, vertical: 1),
                                decoration: BoxDecoration(color: PColors.adminSoft, borderRadius: BorderRadius.circular(6)),
                                child: const Text(
                                  'admin',
                                  style: TextStyle(fontSize: 10.5, fontWeight: FontWeight.w600, color: PColors.adminText),
                                ),
                              ),
                            ],
                          ],
                        ),
                        Text(
                          note ?? '@${m.username}',
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
      ),
    );
  }
}

// ---------------------------------------------------------------- profile

class _Profile extends StatefulWidget {
  const _Profile({required this.state, required this.username});

  final AppState state;
  final String username;

  @override
  State<_Profile> createState() => _ProfileState();
}

class _ProfileState extends State<_Profile> {
  String? _safetyNumber;
  bool _busy = false;

  /// The number shows only when asked for, so it is not on screen by
  /// accident. Forgotten with the panel (another conversation hides it).
  bool _shown = false;

  AppState get _state => widget.state;

  @override
  void initState() {
    super.initState();
    unawaited(_loadNumber());
  }

  Future<void> _loadNumber() async {
    try {
      final n = await _state.client.safetyNumber(username: widget.username);
      if (mounted) setState(() => _safetyNumber = n);
    } catch (_) {}
  }

  Future<void> _setVerified(bool v) async {
    setState(() => _busy = true);
    try {
      await _state.client.setVerified(username: widget.username, verified: v);
    } catch (e) {
      if (mounted) showToast(context, errorText(e), error: true);
    } finally {
      if (mounted) setState(() => _busy = false);
    }
  }

  @override
  Widget build(BuildContext context) {
    return ListenableBuilder(
      listenable: _state,
      builder: (context, _) {
        final p = _state.personNow(widget.username);
        final name = p?.shownName ?? widget.username;
        final presence = _state.presenceOf(widget.username);
        final verified = p?.verified ?? false;
        final banner = _state.bannerOf(widget.username);
        return ListView(
          padding: const EdgeInsets.fromLTRB(16, 16, 16, 16),
          children: [
            // Their banner (3 : 1, a rounded shape filled with it), the
            // picture centred on its lower edge.
            LayoutBuilder(
              builder: (context, box) {
                final height = box.maxWidth / 3;
                return SizedBox(
                  height: height + 48,
                  child: Stack(
                    alignment: Alignment.topCenter,
                    children: [
                      SizedBox(
                        height: height,
                        width: double.infinity,
                        child: BannerView(picture: banner, color: _state.bannerColorOf(widget.username), radius: BorderRadius.circular(14)),
                      ),
                      Positioned(
                        top: height - 40,
                        child: Container(
                          padding: const EdgeInsets.all(4),
                          decoration: const BoxDecoration(color: PColors.panel, shape: BoxShape.circle),
                          child: PAvatar(
                            name: name,
                            colorKey: widget.username,
                            size: 80,
                            presence: presence,
                            image: _state.avatarOf(widget.username),
                          ),
                        ),
                      ),
                    ],
                  ),
                );
              },
            ),
            const SizedBox(height: 12),
            Text(name, textAlign: TextAlign.center, style: PType.heading),
            const SizedBox(height: 2),
            Text(
              '@${widget.username} · ${presenceLabel(presence)}',
              textAlign: TextAlign.center,
              style: const TextStyle(fontSize: 13, color: PColors.textFaint),
            ),
            if (p != null && p.nickname.isNotEmpty && p.displayName.isNotEmpty && p.displayName != p.nickname) ...[
              const SizedBox(height: 2),
              Text(
                'calls themselves ${p.displayName}',
                textAlign: TextAlign.center,
                style: const TextStyle(fontSize: 12, color: PColors.textFaint),
              ),
            ],
            if (p?.note != null) ...[
              const SizedBox(height: 12),
              Center(
                child: Container(
                  padding: const EdgeInsets.symmetric(horizontal: 12, vertical: 6),
                  decoration: BoxDecoration(color: PColors.panelRaised, borderRadius: BorderRadius.circular(12)),
                  child: Text(
                    p!.note!,
                    textAlign: TextAlign.center,
                    style: const TextStyle(fontSize: 13, color: PColors.textBody),
                  ),
                ),
              ),
            ],
            const SizedBox(height: 14),
            Center(
              child: TextButton.icon(
                onPressed: () => setNickname(context, _state, widget.username),
                icon: const Icon(Icons.badge_outlined, size: 16),
                label: Text(p != null && p.nickname.isNotEmpty ? 'Change nickname' : 'Set nickname'),
              ),
            ),
            const SizedBox(height: 10),
            _SafetyCard(
              firstName: name.split(' ').first,
              number: _safetyNumber,
              shown: _shown,
              onShow: (v) => setState(() => _shown = v),
              verified: verified,
              busy: _busy,
              onSetVerified: _setVerified,
            ),
          ],
        );
      },
    );
  }
}

class _SafetyCard extends StatelessWidget {
  const _SafetyCard({
    required this.firstName,
    required this.number,
    required this.shown,
    required this.onShow,
    required this.verified,
    required this.busy,
    required this.onSetVerified,
  });

  final String firstName;
  final String? number;

  /// Whether the number itself is showing, or masked.
  final bool shown;
  final ValueChanged<bool> onShow;
  final bool verified;
  final bool busy;
  final ValueChanged<bool> onSetVerified;

  static const _numberStyle = TextStyle(fontFamily: PType.mono, fontSize: 13, height: 1.7, letterSpacing: 0.5, color: Color(0xFFC9CDDC));

  /// 60 digits as three lines of four groups.
  static String _lines(String n) {
    final groups = n.trim().split(RegExp(r'\s+'));
    final lines = <String>[];
    for (var i = 0; i < groups.length; i += 4) {
      lines.add(groups.sublist(i, (i + 4).clamp(0, groups.length)).join(' '));
    }
    return lines.join('\n');
  }

  @override
  Widget build(BuildContext context) {
    final color = verified ? PColors.verified : PColors.unverified;
    return Container(
      padding: const EdgeInsets.all(14),
      decoration: BoxDecoration(
        color: PColors.sunken,
        border: Border.all(color: PColors.selected),
        borderRadius: BorderRadius.circular(16),
      ),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Row(
            children: [
              Icon(verified ? Icons.verified_user_outlined : Icons.shield_outlined, size: 16, color: color),
              const SizedBox(width: 8),
              Text(
                verified ? 'Safety number verified' : 'Not verified yet',
                style: TextStyle(fontWeight: FontWeight.w600, fontSize: 14, color: color),
              ),
            ],
          ),
          const SizedBox(height: 8),
          Text(
            'Compare this number with $firstName in person or on a call. If it matches, no one is in between. It stays the same when either of you adds a device.',
            style: const TextStyle(fontSize: 12.5, height: 1.5, color: PColors.textFaint),
          ),
          const SizedBox(height: 10),
          if (number == null)
            const PSpinner(size: 14)
          else
            // Masked until asked for: the same three lines in dots, so the
            // card keeps its size either way.
            Stack(
              alignment: Alignment.center,
              children: [
                AnimatedSwitcher(
                  duration: PMotion.normal,
                  child: shown
                      ? SelectableText(key: const ValueKey('number'), _lines(number!), style: _numberStyle)
                      : Text(
                          key: const ValueKey('masked'),
                          _lines(number!.replaceAll(RegExp(r'\d'), '•')),
                          style: _numberStyle.copyWith(color: PColors.textFaint.withValues(alpha: 0.35)),
                        ),
                ),
                if (!shown)
                  FilledButton.tonalIcon(
                    onPressed: () => onShow(true),
                    icon: const Icon(Icons.visibility_outlined, size: 16),
                    label: const Text('Show'),
                    style: PButtons.tactile(face: PColors.panelRaised, onFace: PColors.text),
                  ),
              ],
            ),
          if (shown) ...[
            const SizedBox(height: 4),
            Align(
              alignment: Alignment.centerLeft,
              child: TextButton.icon(onPressed: () => onShow(false), icon: const Icon(Icons.visibility_off_outlined, size: 15), label: const Text('Hide')),
            ),
          ],
          const SizedBox(height: 12),
          Row(
            children: [
              Expanded(
                child: verified
                    ? OutlinedButton(onPressed: busy ? null : () => onSetVerified(false), child: const Text('Unmark'))
                    // Comparing means looking at it: shown first.
                    : FilledButton(onPressed: busy || number == null || !shown ? null : () => onSetVerified(true), child: const Text('Mark as verified')),
              ),
              const SizedBox(width: 6),
              PIconButton(
                icon: Icons.copy_rounded,
                tooltip: 'Copy the number',
                size: 44,
                onPressed: number == null ? null : () => copyText(context, number!, what: 'Safety number copied'),
              ),
            ],
          ),
        ],
      ),
    );
  }
}
