// Calls on screen (design section 10): the button that starts one, the card
// for one ringing, the strip at the top of a conversation with a call in
// it, and the card above your name while you are in one.

import 'dart:async';

import 'package:flutter/material.dart';
import 'package:post/src/rust/api/client.dart';

import '../state/app_state.dart';
import '../state/calls.dart';
import '../theme.dart';
import '../ui/controls.dart';

String _clock(Duration d) {
  final m = d.inMinutes.remainder(60).toString().padLeft(2, '0');
  final s = d.inSeconds.remainder(60).toString().padLeft(2, '0');
  return d.inHours > 0 ? '${d.inHours}:$m:$s' : '$m:$s';
}

/// A ticking clock for a call's length.
class _Elapsed extends StatefulWidget {
  const _Elapsed(this.since, {required this.style});

  final DateTime since;
  final TextStyle style;

  @override
  State<_Elapsed> createState() => _ElapsedState();
}

class _ElapsedState extends State<_Elapsed> {
  late final Timer _tick = Timer.periodic(const Duration(seconds: 1), (_) => setState(() {}));

  @override
  void dispose() {
    _tick.cancel();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) => Text(_clock(DateTime.now().difference(widget.since)), style: widget.style);
}

// ---------------------------------------------------------------- header

/// In a conversation's header: start a call, or show that one is on.
class CallButton extends StatelessWidget {
  const CallButton({super.key, required this.state, required this.conversation});

  final AppState state;
  final ConversationInfo conversation;

  @override
  Widget build(BuildContext context) {
    return ListenableBuilder(
      listenable: state.calls,
      builder: (context, _) {
        final calls = state.calls;
        final here = calls.callIn(conversation.id);
        final inThis = calls.active?.conversation == conversation.id;
        return Builder(
          builder: (anchor) => PIconButton(
            icon: Icons.call_outlined,
            tooltip: inThis
                ? 'In a call here'
                : here != null
                ? 'Join the call'
                : 'Start a call',
            size: 40,
            selected: inThis,
            onPressed: inThis || calls.busy
                ? null
                : here != null
                ? () => unawaited(calls.answer(here))
                : conversation.isGroup
                ? () => unawaited(_chooseWhom(anchor))
                : () => unawaited(calls.start(conversation.id)),
          ),
        );
      },
    );
  }

  /// In a group: ring everyone (a small group), or one person.
  Future<void> _chooseWhom(BuildContext anchor) async {
    final members = (await state.client.members(conversation: conversation.id)).where((m) => !m.you).toList();
    if (!anchor.mounted) return;
    final box = anchor.findRenderObject()! as RenderBox;
    // A larger group is not rung by a plain start: the call shows to join.
    final small = members.length + 1 <= 8;
    final chosen = await showContextMenu<String>(anchor, box.localToGlobal(Offset(0, box.size.height + 4)), [
      PMenuItem(
        value: '',
        label: small ? 'Call everyone' : 'Start a call',
        caption: small ? 'Rings all ${members.length}' : 'No one is rung; members can join',
        icon: Icons.groups_outlined,
      ),
      const PMenuDivider(),
      for (final m in members) PMenuItem(value: m.username, label: 'Call ${m.shownName}', caption: 'Rings only them', icon: Icons.call_outlined),
    ]);
    if (chosen == null) return;
    await state.calls.start(conversation.id, ring: chosen.isEmpty ? const [] : [chosen]);
  }
}

// ---------------------------------------------------------------- ringing

/// A card over everything while a call rings: who is calling, answer or
/// decline.
class IncomingCall extends StatelessWidget {
  const IncomingCall({super.key, required this.state});

  final AppState state;

  @override
  Widget build(BuildContext context) {
    // The app's state too: the caller's name arrives with their profile.
    return ListenableBuilder(
      listenable: Listenable.merge([state.calls, state]),
      builder: (context, _) {
        final ringing = state.calls.ringing;
        return AnimatedSwitcher(
          duration: pMenuAnimation.duration!,
          reverseDuration: pMenuAnimation.reverseDuration!,
          switchInCurve: Curves.easeOutBack,
          switchOutCurve: Curves.easeInCubic,
          transitionBuilder: (child, a) => FadeTransition(
            opacity: a,
            child: ScaleTransition(scale: Tween(begin: 0.92, end: 1.0).animate(a), child: child),
          ),
          child: ringing == null ? const SizedBox.shrink(key: ValueKey('none')) : _card(context, ringing),
        );
      },
    );
  }

  Widget _card(BuildContext context, CallInfo call) {
    final conv = state.conversationById(call.conversation);
    // A caller is in the conversation, so their profile may be loaded (unlike
    // a username read out of message text).
    final caller = state.personNow(call.startedBy)?.shownName ?? call.startedBy;
    final where = conv != null && conv.isGroup ? 'in ${conv.title}' : (call.video ? 'Video call' : 'Voice call');
    return Material(
      key: ValueKey(call.id),
      color: PColors.panelRaised,
      elevation: 16,
      shadowColor: Colors.black,
      shape: RoundedRectangleBorder(
        borderRadius: BorderRadius.circular(20),
        side: const BorderSide(color: PColors.borderStrong),
      ),
      child: Padding(
        padding: const EdgeInsets.all(16),
        child: SizedBox(
          width: 300,
          child: Column(
            mainAxisSize: MainAxisSize.min,
            children: [
              _Pulse(
                child: PAvatar(name: caller, colorKey: call.startedBy, size: 64),
              ),
              const SizedBox(height: 12),
              Text(
                '$caller is calling',
                style: const TextStyle(fontSize: 16, fontWeight: FontWeight.w600, color: PColors.textStrong),
              ),
              const SizedBox(height: 2),
              Text(where, style: PType.small),
              const SizedBox(height: 16),
              Row(
                children: [
                  Expanded(
                    child: OutlinedButton.icon(
                      onPressed: () => unawaited(state.calls.decline(call)),
                      icon: const Icon(Icons.call_end_rounded, size: 18, color: PColors.danger),
                      label: const Text('Decline'),
                    ),
                  ),
                  const SizedBox(width: 10),
                  Expanded(
                    child: FilledButton.icon(
                      style: FilledButton.styleFrom(backgroundColor: PColors.online, foregroundColor: PColors.onAvatar),
                      onPressed: () {
                        state.select(call.conversation);
                        unawaited(state.calls.answer(call));
                      },
                      icon: const Icon(Icons.call_rounded, size: 18),
                      label: const Text('Answer'),
                    ),
                  ),
                ],
              ),
            ],
          ),
        ),
      ),
    );
  }
}

/// A slow ring of light around the caller's avatar.
class _Pulse extends StatefulWidget {
  const _Pulse({required this.child});

  final Widget child;

  @override
  State<_Pulse> createState() => _PulseState();
}

class _PulseState extends State<_Pulse> with SingleTickerProviderStateMixin {
  late final AnimationController _c = AnimationController(vsync: this, duration: const Duration(milliseconds: 1600))..repeat();

  @override
  void dispose() {
    _c.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    return AnimatedBuilder(
      animation: _c,
      builder: (context, child) {
        final t = Curves.easeOut.transform(_c.value);
        return Container(
          padding: const EdgeInsets.all(6),
          decoration: BoxDecoration(
            shape: BoxShape.circle,
            boxShadow: [
              BoxShadow(
                color: PColors.online.withValues(alpha: 0.45 * (1 - t)),
                spreadRadius: 14 * t,
                blurRadius: 4,
              ),
            ],
          ),
          child: child,
        );
      },
      child: widget.child,
    );
  }
}

// ---------------------------------------------------------------- in a conversation

/// At the top of a conversation with a call in it: join it, or (in it)
/// who is there and who is talking.
class CallStrip extends StatelessWidget {
  const CallStrip({super.key, required this.state, required this.conversation});

  final AppState state;
  final String conversation;

  @override
  Widget build(BuildContext context) {
    return ListenableBuilder(
      listenable: Listenable.merge([state.calls, state]),
      builder: (context, _) {
        final calls = state.calls;
        final call = calls.callIn(conversation);
        final active = calls.active?.conversation == conversation ? calls.active : null;
        return AnimatedSize(
          duration: PMotion.normal,
          curve: PMotion.curve,
          alignment: Alignment.topCenter,
          child: call == null && active == null ? const SizedBox(width: double.infinity) : _strip(calls, call, active),
        );
      },
    );
  }

  Widget _strip(CallController calls, CallInfo? call, ActiveCall? active) {
    final people = (call ?? active?.join.call)?.participants ?? const <CallParticipantInfo>[];
    return Container(
      margin: const EdgeInsets.fromLTRB(14, 10, 14, 0),
      padding: const EdgeInsets.fromLTRB(12, 8, 8, 8),
      decoration: BoxDecoration(
        color: active != null ? const Color(0xFF0F1E1A) : PColors.accentSoft,
        border: Border.all(color: active != null ? PColors.online.withValues(alpha: 0.35) : PColors.activeEdge),
        borderRadius: BorderRadius.circular(14),
      ),
      child: Row(
        children: [
          Icon(Icons.graphic_eq_rounded, size: 18, color: active != null ? PColors.online : PColors.accentText),
          const SizedBox(width: 10),
          for (final p in people) ...[
            _Speaker(state: state, person: p, speaking: active?.speaking.contains(p.participantId) ?? false),
            const SizedBox(width: 6),
          ],
          const SizedBox(width: 4),
          Expanded(
            child: Text(
              active != null
                  ? (people.length <= 1 ? 'Waiting for others to join…' : '${people.length} in the call')
                  : 'Call in progress · ${people.length} ${people.length == 1 ? 'person' : 'people'}',
              maxLines: 1,
              overflow: TextOverflow.ellipsis,
              style: const TextStyle(fontSize: 13, color: PColors.text),
            ),
          ),
          if (active != null) ...[
            _Elapsed(
              active.since,
              style: const TextStyle(fontSize: 12.5, color: PColors.textMuted, fontFeatures: [FontFeature.tabularFigures()]),
            ),
            const SizedBox(width: 6),
            if (state.conversationById(conversation)?.isGroup ?? false)
              Builder(
                builder: (anchor) => PIconButton(
                  icon: Icons.person_add_alt_outlined,
                  tooltip: 'Ring someone',
                  size: 32,
                  iconSize: 17,
                  onPressed: () => unawaited(_ringSomeone(anchor, calls, people)),
                ),
              ),
            _MuteButton(calls: calls),
            PIconButton(
              icon: Icons.call_end_rounded,
              tooltip: 'Leave the call',
              size: 32,
              iconSize: 17,
              color: PColors.danger,
              onPressed: () => unawaited(calls.hangUp()),
            ),
          ] else if (call != null)
            FilledButton(
              onPressed: calls.busy ? null : () => unawaited(calls.answer(call)),
              style: FilledButton.styleFrom(minimumSize: const Size(0, 32), padding: const EdgeInsets.symmetric(horizontal: 14)),
              child: const Text('Join'),
            ),
        ],
      ),
    );
  }

  /// Ring a member who is not in the call yet.
  Future<void> _ringSomeone(BuildContext anchor, CallController calls, List<CallParticipantInfo> people) async {
    final inCall = {for (final p in people) p.username};
    final members = (await state.client.members(conversation: conversation)).where((m) => !m.you && !inCall.contains(m.username)).toList();
    if (!anchor.mounted) return;
    final box = anchor.findRenderObject()! as RenderBox;
    final at = box.localToGlobal(Offset(0, box.size.height + 4));
    if (members.isEmpty) {
      await showContextMenu<String>(anchor, at, [const PMenuItem(value: '', label: 'Everyone is in the call', icon: Icons.check_rounded)]);
      return;
    }
    final chosen = await showContextMenu<String>(anchor, at, [
      for (final m in members) PMenuItem(value: m.username, label: 'Ring ${m.shownName}', icon: Icons.call_outlined),
    ]);
    if (chosen != null && chosen.isNotEmpty) await calls.ringInto([chosen]);
  }
}

/// A participant's avatar, lit while they speak.
class _Speaker extends StatelessWidget {
  const _Speaker({required this.state, required this.person, required this.speaking});

  final AppState state;
  final CallParticipantInfo person;
  final bool speaking;

  @override
  Widget build(BuildContext context) {
    final name = state.personNow(person.username)?.shownName ?? person.username;
    return Tooltip(
      message: person.thisDevice ? 'You' : name,
      child: AnimatedContainer(
        duration: PMotion.fast,
        padding: const EdgeInsets.all(2),
        decoration: BoxDecoration(
          shape: BoxShape.circle,
          border: Border.all(color: speaking ? PColors.online : Colors.transparent, width: 2),
        ),
        child: PAvatar(name: name, colorKey: person.username, size: 24),
      ),
    );
  }
}

class _MuteButton extends StatelessWidget {
  const _MuteButton({required this.calls});

  final CallController calls;

  @override
  Widget build(BuildContext context) {
    final muted = calls.active?.muted ?? false;
    return PIconButton(
      icon: muted ? Icons.mic_off_rounded : Icons.mic_none_rounded,
      tooltip: muted ? 'Unmute' : 'Mute',
      size: 32,
      iconSize: 17,
      selected: muted,
      color: muted ? PColors.danger : null,
      onPressed: () => unawaited(calls.setMuted(!muted)),
    );
  }
}

// ---------------------------------------------------------------- above your name

/// While this device is in a call: where, for how long, and the controls,
/// wherever you are in the app.
class CallDock extends StatelessWidget {
  const CallDock({super.key, required this.state});

  final AppState state;

  @override
  Widget build(BuildContext context) {
    return ListenableBuilder(
      listenable: state.calls,
      builder: (context, _) {
        final calls = state.calls;
        final a = calls.active;
        final error = calls.error;
        return AnimatedSize(
          duration: PMotion.normal,
          curve: PMotion.curve,
          alignment: Alignment.bottomCenter,
          child: Column(
            mainAxisSize: MainAxisSize.min,
            children: [if (error != null && a == null) _error(calls, error), if (a != null) _dock(context, calls, a)],
          ),
        );
      },
    );
  }

  Widget _error(CallController calls, String error) {
    return Container(
      margin: const EdgeInsets.fromLTRB(8, 0, 8, 8),
      padding: const EdgeInsets.fromLTRB(12, 8, 6, 8),
      decoration: BoxDecoration(color: PColors.dangerSoft, borderRadius: BorderRadius.circular(12)),
      child: Row(
        children: [
          Expanded(
            child: Text(error, style: const TextStyle(fontSize: 12.5, color: PColors.dangerText)),
          ),
          PIconButton(icon: Icons.close_rounded, tooltip: 'Dismiss', size: 26, iconSize: 14, onPressed: calls.dismissError),
        ],
      ),
    );
  }

  Widget _dock(BuildContext context, CallController calls, ActiveCall a) {
    final conv = state.conversationById(a.conversation);
    return Container(
      margin: const EdgeInsets.fromLTRB(8, 0, 8, 8),
      padding: const EdgeInsets.fromLTRB(12, 8, 6, 8),
      decoration: BoxDecoration(
        color: const Color(0xFF0F1E1A),
        border: Border.all(color: PColors.online.withValues(alpha: 0.35)),
        borderRadius: BorderRadius.circular(14),
      ),
      child: Row(
        children: [
          Expanded(
            child: InkWell(
              borderRadius: BorderRadius.circular(8),
              onTap: () => state.select(a.conversation),
              child: Column(
                crossAxisAlignment: CrossAxisAlignment.start,
                children: [
                  Row(
                    children: [
                      Text(
                        a.connected ? 'In a call' : 'Connecting…',
                        style: const TextStyle(fontSize: 13, fontWeight: FontWeight.w600, color: PColors.online),
                      ),
                      const SizedBox(width: 6),
                      _Elapsed(
                        a.since,
                        style: const TextStyle(fontSize: 12, color: PColors.textMuted, fontFeatures: [FontFeature.tabularFigures()]),
                      ),
                    ],
                  ),
                  Text(
                    conv?.title ?? '',
                    maxLines: 1,
                    overflow: TextOverflow.ellipsis,
                    style: const TextStyle(fontSize: 12, color: PColors.textMuted),
                  ),
                ],
              ),
            ),
          ),
          _MuteButton(calls: calls),
          Builder(
            builder: (anchor) => PIconButton(
              icon: Icons.settings_voice_outlined,
              tooltip: 'Microphone',
              size: 32,
              iconSize: 17,
              onPressed: () => unawaited(_chooseMic(anchor, calls)),
            ),
          ),
          PIconButton(
            icon: Icons.call_end_rounded,
            tooltip: 'Leave the call',
            size: 32,
            iconSize: 17,
            color: PColors.danger,
            onPressed: () => unawaited(calls.hangUp()),
          ),
        ],
      ),
    );
  }

  Future<void> _chooseMic(BuildContext anchor, CallController calls) async {
    await calls.loadMicrophones();
    if (!anchor.mounted || calls.microphones.isEmpty) return;
    final box = anchor.findRenderObject()! as RenderBox;
    final chosen = await showContextMenu<String>(anchor, box.localToGlobal(Offset.zero), [
      for (final m in calls.microphones)
        PMenuItem(
          value: m.deviceId,
          label: m.label.replaceFirst('default: ', ''),
          icon: m.deviceId == calls.microphone ? Icons.check_rounded : Icons.mic_none_rounded,
        ),
    ]);
    if (chosen != null) await calls.chooseMicrophone(chosen);
  }
}
