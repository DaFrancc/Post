// Someone's profile in a card beside where you clicked their name or
// avatar: who they are, what they are up to, and the usual things to do
// with them.

import 'dart:async';

import 'package:flutter/material.dart';

import '../state/app_state.dart';
import '../theme.dart';
import '../ui/arrival.dart';
import '../ui/controls.dart';
import '../ui/format.dart';
import 'dialogs.dart';
import 'settings.dart' show showSettings;

/// Show [username]'s card beside [anchor] (the avatar or name clicked), or
/// beside the point [at] in it (a mention clicked in a message).
Future<void> showProfileCard(BuildContext anchor, AppState state, String username, {Offset? at}) => showBeside<void>(
  anchor,
  width: 300,
  at: at,
  child: _ProfileCard(state: state, username: username),
);

class _ProfileCard extends StatelessWidget {
  const _ProfileCard({required this.state, required this.username});

  final AppState state;
  final String username;

  bool get _me => username == state.me.username;

  Future<void> _message(BuildContext context) async {
    // The card's own context goes with it.
    final nav = Navigator.of(context);
    final outer = nav.context;
    nav.pop();
    try {
      final dm = await state.client.startDm(username: username);
      await state.refreshConversations();
      state.select(dm.id);
    } catch (e) {
      if (outer.mounted) showToast(outer, errorText(e), error: true);
    }
  }

  @override
  Widget build(BuildContext context) {
    return ListenableBuilder(
      listenable: state,
      builder: (context, _) {
        final p = state.personNow(username);
        final name = p?.shownName ?? username;
        final presence = state.presenceOf(username);
        final colour = PColors.avatarFor(username);
        return Material(
          color: PColors.panelRaised,
          elevation: 12,
          clipBehavior: Clip.antiAlias,
          shape: RoundedRectangleBorder(
            borderRadius: BorderRadius.circular(16),
            side: const BorderSide(color: PColors.borderStrong),
          ),
          child: Column(
            mainAxisSize: MainAxisSize.min,
            crossAxisAlignment: CrossAxisAlignment.stretch,
            children: [
              // Their colour across the top, the avatar sitting on its edge.
              SizedBox(
                height: 96,
                child: Stack(
                  children: [
                    Container(
                      height: 60,
                      decoration: BoxDecoration(
                        gradient: LinearGradient(
                          begin: Alignment.topLeft,
                          end: Alignment.bottomRight,
                          colors: [colour.withValues(alpha: 0.55), colour.withValues(alpha: 0.18)],
                        ),
                      ),
                    ),
                    Positioned(
                      left: 16,
                      top: 24,
                      child: Container(
                        padding: const EdgeInsets.all(4),
                        decoration: const BoxDecoration(color: PColors.panelRaised, shape: BoxShape.circle),
                        child: PAvatar(name: name, colorKey: username, size: 64, presence: presence, ringColor: PColors.panelRaised),
                      ),
                    ),
                    if (p?.verified ?? false)
                      Positioned(
                        right: 12,
                        top: 68,
                        child: Tooltip(
                          message: 'You compared safety numbers',
                          child: Container(
                            padding: const EdgeInsets.symmetric(horizontal: 8, vertical: 3),
                            decoration: BoxDecoration(color: PColors.accentSoft, borderRadius: BorderRadius.circular(8)),
                            child: const Row(
                              mainAxisSize: MainAxisSize.min,
                              children: [
                                Icon(Icons.verified_user_outlined, size: 13, color: PColors.accentText),
                                SizedBox(width: 4),
                                Text(
                                  'Verified',
                                  style: TextStyle(fontSize: 11.5, fontWeight: FontWeight.w600, color: PColors.accentText),
                                ),
                              ],
                            ),
                          ),
                        ),
                      ),
                  ],
                ),
              ),
              Padding(
                padding: const EdgeInsets.fromLTRB(16, 4, 16, 14),
                child: Column(
                  crossAxisAlignment: CrossAxisAlignment.start,
                  children: [
                    Text(name, maxLines: 2, overflow: TextOverflow.ellipsis, style: PType.heading),
                    const SizedBox(height: 2),
                    Text(
                      [
                        '@$username',
                        if (p != null && p.nickname.isNotEmpty && p.displayName.isNotEmpty && p.displayName != p.nickname) p.displayName,
                      ].join(' · '),
                      style: PType.small,
                    ),
                    const SizedBox(height: 10),
                    Row(
                      children: [
                        PresenceDot(presence),
                        const SizedBox(width: 6),
                        Text(presenceLabel(presence), style: const TextStyle(fontSize: 13, color: PColors.textMuted)),
                      ],
                    ),
                    if (p?.note case final note?) ...[
                      const SizedBox(height: 10),
                      Container(
                        width: double.infinity,
                        padding: const EdgeInsets.fromLTRB(10, 7, 10, 7),
                        decoration: BoxDecoration(color: PColors.sunken, borderRadius: BorderRadius.circular(10)),
                        child: Text(note, style: const TextStyle(fontSize: 13, height: 1.4, color: PColors.textBody)),
                      ),
                    ],
                    const SizedBox(height: 10),
                    Text('Messages arrive: ${Arrival.parse(p?.arrival).label}', style: const TextStyle(fontSize: 12, color: PColors.textFaint)),
                    const SizedBox(height: 14),
                    if (_me)
                      SizedBox(
                        width: double.infinity,
                        child: OutlinedButton(
                          onPressed: () {
                            // The card's own context goes with it.
                            final nav = Navigator.of(context);
                            nav.pop();
                            unawaited(showSettings(nav.context, state));
                          },
                          child: const Text('Edit profile'),
                        ),
                      )
                    else
                      Row(
                        children: [
                          Expanded(
                            child: FilledButton.icon(
                              onPressed: () => unawaited(_message(context)),
                              icon: const Icon(Icons.chat_bubble_outline_rounded, size: 16),
                              label: const Text('Message'),
                            ),
                          ),
                          const SizedBox(width: 6),
                          PIconButton(
                            icon: Icons.edit_outlined,
                            tooltip: 'Set nickname',
                            onPressed: () {
                              final nav = Navigator.of(context);
                              nav.pop();
                              unawaited(setNickname(nav.context, state, username));
                            },
                          ),
                          PIconButton(
                            icon: Icons.alternate_email_rounded,
                            tooltip: 'Copy username',
                            onPressed: () => unawaited(copyText(context, username, what: 'Username copied')),
                          ),
                        ],
                      ),
                  ],
                ),
              ),
            ],
          ),
        );
      },
    );
  }
}
