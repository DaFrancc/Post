// Someone's profile in a card beside where you clicked their name or
// avatar: who they are, what they are up to, and the usual things to do
// with them.

import 'dart:async';

import 'package:flutter/material.dart';
import 'package:flutter/rendering.dart';

import '../state/app_state.dart';
import '../theme.dart';
import '../ui/arrival.dart';
import '../ui/controls.dart';
import '../ui/format.dart';
import '../ui/markup_text.dart';
import '../ui/profile_theme.dart';
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
        final theme = state.themeOf(username);
        // What the picture and the boxes sit on.
        final face = theme?.top ?? PColors.panelRaised;
        final well = theme == null ? PColors.sunken : Colors.black.withValues(alpha: 0.28);
        return ProfileFrame(
          theme: theme,
          header: (inner) => SizedBox(
            height: 144,
            child: Stack(
              children: [
                // Its top corners rounded with the card's, as a shape
                // rather than by the card's clip: smooth on every
                // renderer.
                SizedBox(
                  height: 100,
                  width: double.infinity,
                  child: BannerView(
                    picture: state.bannerOf(username),
                    color: state.bannerColorOf(username),
                    radius: BorderRadius.vertical(top: Radius.circular(inner)),
                  ),
                ),
                Positioned(
                  left: 12,
                  top: 56,
                  child: Container(
                    padding: const EdgeInsets.all(4),
                    decoration: BoxDecoration(color: face, shape: BoxShape.circle),
                    child: PAvatar(name: name, colorKey: username, size: 80, presence: presence, ringColor: face, image: state.avatarOf(username)),
                  ),
                ),
                if (p?.verified ?? false)
                  Positioned(
                    right: 12,
                    top: 110,
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
          body: Padding(
            padding: const EdgeInsets.fromLTRB(16, 4, 16, 14),
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
                Text(name, maxLines: 2, overflow: TextOverflow.ellipsis, style: PType.heading),
                const SizedBox(height: 2),
                Text(
                  ['@$username', if (p != null && p.nickname.isNotEmpty && p.displayName.isNotEmpty && p.displayName != p.nickname) p.displayName].join(' · '),
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
                    decoration: BoxDecoration(color: well, borderRadius: BorderRadius.circular(10)),
                    child: Text(note, style: const TextStyle(fontSize: 13, height: 1.4, color: PColors.textBody)),
                  ),
                ],
                if (p != null && p.bio.isNotEmpty) ...[
                  const SizedBox(height: 10),
                  ProfileBio(
                    state: state,
                    bio: p.bio,
                    background: well,
                    maxHeight: 132,
                    // Another person's card in place of this one.
                    onMention: (u, at) {
                      final nav = Navigator.of(context);
                      nav.pop();
                      unawaited(showProfileCard(nav.context, state, u, at: at));
                    },
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
        );
      },
    );
  }
}

/// Someone's bio (about me), formatted like a message, mentions showing by
/// name. Taller than [maxHeight], it is cut off there, fading out, with a
/// way to show the rest.
class ProfileBio extends StatefulWidget {
  const ProfileBio({super.key, required this.state, required this.bio, this.maxHeight, this.onMention, this.background = PColors.sunken});

  final AppState state;
  final String bio;
  final double? maxHeight;

  /// The box it sits in.
  final Color background;

  /// A mention clicked, and where (window coordinates).
  final void Function(String username, Offset at)? onMention;

  @override
  State<ProfileBio> createState() => _ProfileBioState();
}

class _ProfileBioState extends State<ProfileBio> {
  final _scroll = ScrollController();
  bool _cut = false;
  bool _open = false;

  @override
  void dispose() {
    _scroll.dispose();
    super.dispose();
  }

  @override
  void didUpdateWidget(ProfileBio old) {
    super.didUpdateWidget(old);
    if (old.bio != widget.bio) _open = false;
  }

  @override
  Widget build(BuildContext context) {
    final max = widget.maxHeight;
    Widget text = Mentions(
      nameOf: widget.state.knownName,
      you: widget.state.me.username,
      onTap: widget.onMention,
      child: MarkupText(widget.bio, style: const TextStyle(fontSize: 13, height: 1.45, color: PColors.textBody)),
    );
    if (max != null && _open) {
      // All of it, but a bio of line breaks could be thousands of pixels
      // tall: past a few times the cut, it scrolls.
      text = ConstrainedBox(
        constraints: BoxConstraints(maxHeight: max * 2.5),
        child: Scrollbar(
          controller: _scroll,
          child: SingleChildScrollView(controller: _scroll, child: text),
        ),
      );
    } else if (max != null) {
      text = _CutOff(
        maxHeight: max,
        onCut: (cut) {
          if (cut != _cut) setState(() => _cut = cut);
        },
        child: text,
      );
      if (_cut) {
        // The last line fades out, so the text plainly goes on.
        text = ShaderMask(
          blendMode: BlendMode.dstIn,
          shaderCallback: (r) => const LinearGradient(
            begin: Alignment.topCenter,
            end: Alignment.bottomCenter,
            colors: [Colors.white, Colors.white, Colors.transparent],
            stops: [0, 0.78, 1],
          ).createShader(r),
          child: text,
        );
      }
    }
    return Container(
      width: double.infinity,
      padding: const EdgeInsets.fromLTRB(11, 9, 11, 10),
      decoration: BoxDecoration(color: widget.background, borderRadius: BorderRadius.circular(10)),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          const Text('ABOUT ME', style: PType.label),
          const SizedBox(height: 5),
          text,
          if (max != null && (_cut || _open))
            Padding(
              padding: const EdgeInsets.only(top: 4),
              child: TextButton(
                style: TextButton.styleFrom(
                  padding: EdgeInsets.zero,
                  minimumSize: Size.zero,
                  tapTargetSize: MaterialTapTargetSize.shrinkWrap,
                  textStyle: const TextStyle(fontSize: 12.5, fontWeight: FontWeight.w600),
                ),
                onPressed: () => setState(() => _open = !_open),
                child: Text(_open ? 'Show less' : 'Show more'),
              ),
            ),
        ],
      ),
    );
  }
}

/// Its child at most [maxHeight] tall, the rest cut off; [onCut] hears
/// whether anything was, after each layout that changes it.
class _CutOff extends SingleChildRenderObjectWidget {
  const _CutOff({required this.maxHeight, required this.onCut, required super.child});

  final double maxHeight;
  final ValueChanged<bool> onCut;

  @override
  _RenderCutOff createRenderObject(BuildContext context) => _RenderCutOff(maxHeight, onCut);

  @override
  void updateRenderObject(BuildContext context, _RenderCutOff r) {
    r
      ..maxHeight = maxHeight
      ..onCut = onCut;
  }
}

class _RenderCutOff extends RenderProxyBox {
  _RenderCutOff(this._maxHeight, this.onCut);

  double _maxHeight;
  set maxHeight(double v) {
    if (v == _maxHeight) return;
    _maxHeight = v;
    markNeedsLayout();
  }

  ValueChanged<bool> onCut;
  bool? _told;
  bool _cut = false;

  @override
  void performLayout() {
    final c = child!;
    c.layout(constraints.copyWith(minHeight: 0, maxHeight: double.infinity), parentUsesSize: true);
    _cut = c.size.height > _maxHeight + 0.5;
    size = constraints.constrain(Size(c.size.width, _cut ? _maxHeight : c.size.height));
    if (_told != _cut) {
      _told = _cut;
      final cut = _cut;
      // Not during layout: the listener rebuilds.
      WidgetsBinding.instance.addPostFrameCallback((_) => onCut(cut));
    }
  }

  @override
  void paint(PaintingContext context, Offset offset) {
    if (!_cut) {
      layer = null;
      return super.paint(context, offset);
    }
    // Straight edges only, so no smoothing is lost to the clip.
    layer = context.pushClipRect(needsCompositing, offset, Offset.zero & size, super.paint, oldLayer: layer as ClipRectLayer?);
  }

  @override
  bool hitTestChildren(BoxHitTestResult result, {required Offset position}) =>
      (Offset.zero & size).contains(position) && super.hitTestChildren(result, position: position);
}
