// Settings: your profile, your devices, security and the account details.

import 'dart:async';
import 'dart:math' as math;

import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:post/src/rust/api/client.dart';

import '../state/app_state.dart';
import '../state/spelling.dart';
import '../theme.dart';
import '../ui/arrival.dart';
import '../ui/controls.dart';
import '../ui/format.dart';
import '../ui/markup_text.dart';
import '../ui/profile_theme.dart';
import '../ui/zoom.dart';
import 'dialogs.dart';
import 'picture_editor.dart';
import 'profile_card.dart' show ProfileCardView, ProfileLook;
import 'sound_settings.dart';
import 'update.dart' show UpdateSettings;

enum SettingsSection { profile, appearance, sounds, spelling, devices, security, invites, updates, about }

Future<void> showSettings(BuildContext context, AppState state, {SettingsSection section = SettingsSection.profile}) => showDialog(
  context: context,
  builder: (_) => _Settings(state: state, initial: section),
);

class _Settings extends StatefulWidget {
  const _Settings({required this.state, required this.initial});

  final AppState state;
  final SettingsSection initial;

  @override
  State<_Settings> createState() => _SettingsState();
}

class _SettingsState extends State<_Settings> {
  late SettingsSection _section = widget.initial;

  @override
  Widget build(BuildContext context) {
    final size = MediaQuery.sizeOf(context);
    return PDialogEntrance(child: _dialog(size));
  }

  Widget _dialog(Size size) {
    return Dialog(
      backgroundColor: PColors.dialog,
      insetPadding: const EdgeInsets.all(32),
      shape: RoundedRectangleBorder(
        borderRadius: BorderRadius.circular(22),
        side: const BorderSide(color: PColors.borderStrong),
      ),
      clipBehavior: Clip.antiAlias,
      // Wider for the profile, with room for its preview beside the fields
      // when the window allows.
      child: AnimatedContainer(
        duration: PMotion.fast,
        curve: Curves.easeOutCubic,
        width: _section == SettingsSection.profile ? math.max(820.0, math.min(1180.0, size.width - 64)) : 820,
        height: (size.height - 64).clamp(420, 640),
        child: Row(
          crossAxisAlignment: CrossAxisAlignment.stretch,
          children: [
            Container(
              width: 200,
              color: PColors.panel,
              padding: const EdgeInsets.fromLTRB(12, 22, 12, 12),
              child: Column(
                crossAxisAlignment: CrossAxisAlignment.stretch,
                children: [
                  const Padding(
                    padding: EdgeInsets.fromLTRB(10, 0, 10, 14),
                    child: Text('Settings', style: PType.title),
                  ),
                  for (final s in SettingsSection.values)
                    // Invites are for the server's hosts.
                    if (s != SettingsSection.invites || (widget.state.announcements?.host ?? false))
                      _NavItem(section: s, selected: s == _section, onTap: () => setState(() => _section = s)),
                ],
              ),
            ),
            Expanded(
              child: Column(
                crossAxisAlignment: CrossAxisAlignment.stretch,
                children: [
                  Padding(
                    padding: const EdgeInsets.fromLTRB(28, 18, 16, 0),
                    child: Row(
                      children: [
                        Expanded(child: Text(_title(_section), style: PType.heading)),
                        PIconButton(icon: Icons.close_rounded, tooltip: 'Close', onPressed: () => Navigator.of(context).pop()),
                      ],
                    ),
                  ),
                  Expanded(
                    child: switch (_section) {
                      SettingsSection.profile => _ProfileSettings(state: widget.state),
                      SettingsSection.devices => _DeviceSettings(state: widget.state),
                      SettingsSection.appearance => const _AppearanceSettings(),
                      SettingsSection.spelling => _SpellingSettings(spelling: widget.state.spelling),
                      SettingsSection.security => _SecuritySettings(state: widget.state),
                      SettingsSection.invites => _InviteSettings(state: widget.state),
                      SettingsSection.sounds => SoundSettings(sounds: widget.state.sounds),
                      SettingsSection.updates => UpdateSettings(
                        state: widget.state,
                        checkRow: ({required value, required label, help, required onChanged}) =>
                            _CheckRow(value: value, label: label, help: help, onChanged: onChanged),
                      ),
                      SettingsSection.about => _About(state: widget.state),
                    },
                  ),
                ],
              ),
            ),
          ],
        ),
      ),
    );
  }

  static String _title(SettingsSection s) => switch (s) {
    SettingsSection.profile => 'Profile',
    SettingsSection.devices => 'Devices',
    SettingsSection.appearance => 'Appearance',
    SettingsSection.spelling => 'Spelling',
    SettingsSection.security => 'Privacy & security',
    SettingsSection.invites => 'Invite people',
    SettingsSection.sounds => 'Sounds',
    SettingsSection.updates => 'Updates',
    SettingsSection.about => 'Account',
  };
}

class _NavItem extends StatefulWidget {
  const _NavItem({required this.section, required this.selected, required this.onTap});

  final SettingsSection section;
  final bool selected;
  final VoidCallback onTap;

  @override
  State<_NavItem> createState() => _NavItemState();
}

class _NavItemState extends State<_NavItem> {
  bool _hover = false;

  @override
  Widget build(BuildContext context) {
    final (icon, label) = switch (widget.section) {
      SettingsSection.profile => (Icons.person_outline_rounded, 'Profile'),
      SettingsSection.devices => (Icons.devices_outlined, 'Devices'),
      SettingsSection.appearance => (Icons.palette_outlined, 'Appearance'),
      SettingsSection.spelling => (Icons.spellcheck_rounded, 'Spelling'),
      SettingsSection.security => (Icons.shield_outlined, 'Privacy'),
      SettingsSection.invites => (Icons.person_add_alt_outlined, 'Invites'),
      SettingsSection.sounds => (Icons.volume_up_outlined, 'Sounds'),
      SettingsSection.updates => (Icons.system_update_alt_rounded, 'Updates'),
      SettingsSection.about => (Icons.info_outline_rounded, 'Account'),
    };
    return MouseRegion(
      cursor: SystemMouseCursors.click,
      onEnter: (_) => setState(() => _hover = true),
      onExit: (_) => setState(() => _hover = false),
      child: GestureDetector(
        onTap: widget.onTap,
        child: Container(
          margin: const EdgeInsets.only(bottom: 2),
          padding: const EdgeInsets.symmetric(horizontal: 12, vertical: 10),
          decoration: BoxDecoration(
            color: widget.selected ? PColors.active : (_hover ? PColors.hover : Colors.transparent),
            borderRadius: BorderRadius.circular(12),
          ),
          child: Row(
            children: [
              Icon(icon, size: 18, color: widget.selected ? PColors.text : PColors.textMuted),
              const SizedBox(width: 10),
              Text(
                label,
                style: TextStyle(fontSize: 14, fontWeight: FontWeight.w500, color: widget.selected ? PColors.textStrong : PColors.textMuted),
              ),
            ],
          ),
        ),
      ),
    );
  }
}

/// A settings page body: scrolls, with the page's padding.
class _Page extends StatelessWidget {
  const _Page({required this.children});

  final List<Widget> children;

  @override
  Widget build(BuildContext context) {
    return ListView(padding: const EdgeInsets.fromLTRB(28, 16, 28, 28), children: children);
  }
}

// ---------------------------------------------------------------- profile

class _ProfileSettings extends StatefulWidget {
  const _ProfileSettings({required this.state});

  final AppState state;

  @override
  State<_ProfileSettings> createState() => _ProfileSettingsState();
}

class _ProfileSettingsState extends State<_ProfileSettings> {
  final _name = TextEditingController();
  final _typing = TextEditingController();
  final _note = TextEditingController();
  // Formatting shown as you type, as in the composer.
  final _bio = MarkupEditingController();
  final _bioLimit = Client.bioLimit();
  // Colours being edited and not yet applied, for the preview. Notifiers,
  // so a slider dragged redraws the preview alone, not the page.
  final _bannerDraft = ValueNotifier<_Draft?>(null);
  final _themeDraft = ValueNotifier<_Draft?>(null);
  bool _keepNote = false;
  bool _busy = false;
  bool _loaded = false;
  String? _error;
  PersonInfo? _was;

  @override
  void initState() {
    super.initState();
    unawaited(_load());
  }

  Future<void> _load() async {
    try {
      final me = await widget.state.client.person(username: widget.state.me.username);
      if (!mounted) return;
      setState(() {
        _was = me;
        _name.text = me.displayName;
        _typing.text = me.typingTemplate;
        _note.text = me.note ?? '';
        _bio.text = me.bio;
        _loaded = true;
      });
    } catch (e) {
      if (mounted) setState(() => _error = errorText(e));
    }
  }

  @override
  void dispose() {
    _name.dispose();
    _typing.dispose();
    _note.dispose();
    _bio.dispose();
    _bannerDraft.dispose();
    _themeDraft.dispose();
    super.dispose();
  }

  Future<void> _save() async {
    final was = _was;
    String? changed(String now, String before) => now.trim() == before ? null : now.trim();
    final change = ProfileChange(
      displayName: changed(_name.text, was?.displayName ?? ''),
      typingTemplate: changed(_typing.text, was?.typingTemplate ?? ''),
      note: changed(_note.text, was?.note ?? ''),
      noteKeep: _keepNote,
      bio: changed(_bio.text, was?.bio ?? ''),
    );
    if (change.displayName == null && change.typingTemplate == null && change.note == null && change.bio == null) {
      showToast(context, 'Nothing has changed.');
      return;
    }
    setState(() {
      _busy = true;
      _error = null;
    });
    try {
      await widget.state.client.setProfile(change: change);
      await _load();
      if (mounted) showToast(context, 'Profile saved and shared with your conversations.');
    } catch (e) {
      if (mounted) setState(() => _error = errorText(e));
    } finally {
      if (mounted) setState(() => _busy = false);
    }
  }

  @override
  Widget build(BuildContext context) {
    if (!_loaded && _error == null) return const Center(child: PSpinner());
    final shownName = _name.text.trim().isEmpty ? widget.state.me.username : _name.text.trim();
    final template = _typing.text.trim().isEmpty ? r'$$NAME$$ is typing' : _typing.text.trim();
    final preview = _ProfilePreview(state: widget.state, fields: [_name, _bio, _note], look: () => _previewLook(), drafts: [_bannerDraft, _themeDraft]);
    final fields = [
      _LookEditor(state: widget.state, bannerDraft: _bannerDraft, themeDraft: _themeDraft),
      const SizedBox(height: 26),
      PField(
        label: 'Display name',
        controller: _name,
        hint: widget.state.me.username,
        help: 'What people see, unless they gave you a nickname. Empty shows your username.',
        maxLength: 64,
        enabled: !_busy,
        onChanged: (_) => setState(() {}),
      ),
      const SizedBox(height: 16),
      PField(
        label: 'About me',
        controller: _bio,
        hint: 'A few words about you',
        help: 'On your profile card. Formatting and mentions work as in messages.',
        lines: 3,
        inputFormatters: [BioFormatter(_bioLimit)],
        counter: '${_bioLimit - _bio.text.runes.length}',
        enabled: !_busy,
        onChanged: (_) => setState(() {}),
      ),
      const SizedBox(height: 16),
      PField(
        label: 'Typing message',
        controller: _typing,
        hint: r'$$NAME$$ is typing',
        help: 'What others see while you type. \$\$NAME\$\$ becomes the name they know you by. Preview: ${template.replaceAll(r'$$NAME$$', shownName)}',
        maxLength: 120,
        enabled: !_busy,
        onChanged: (_) => setState(() {}),
      ),
      const SizedBox(height: 16),
      PField(
        label: 'Note',
        controller: _note,
        hint: 'e.g. at the crag, back sunday',
        help: _keepNote ? 'Shown until you change it.' : 'Shown for 24 hours.',
        maxLength: 140,
        enabled: !_busy,
      ),
      const SizedBox(height: 6),
      // Aligned with the fields' edge: no tap-target padding on the box.
      MouseRegion(
        cursor: _busy ? SystemMouseCursors.basic : SystemMouseCursors.click,
        child: GestureDetector(
          onTap: _busy ? null : () => setState(() => _keepNote = !_keepNote),
          child: Row(
            children: [
              SizedBox(
                width: 20,
                height: 20,
                child: Checkbox(
                  value: _keepNote,
                  materialTapTargetSize: MaterialTapTargetSize.shrinkWrap,
                  visualDensity: VisualDensity.compact,
                  onChanged: _busy ? null : (v) => setState(() => _keepNote = v ?? false),
                ),
              ),
              const SizedBox(width: 10),
              const Text('Keep the note until I change it', style: TextStyle(fontSize: 13.5, color: PColors.textMuted)),
            ],
          ),
        ),
      ),
      if (_error != null) ...[const SizedBox(height: 12), PErrorBox(_error!)],
      const SizedBox(height: 16),
      Align(
        alignment: Alignment.centerRight,
        child: FilledButton(onPressed: _busy ? null : _save, child: _busy ? const PSpinner() : const Text('Save profile')),
      ),
      const SizedBox(height: 28),
      _ArrivalPicker(state: widget.state, chosen: Arrival.parse(_was?.arrival), onChosen: (_) => unawaited(_load())),
    ];
    return LayoutBuilder(
      builder: (context, box) {
        // Room for both: the fields, and beside them the card as others
        // see it, staying in view while the fields scroll. Else the card
        // above the fields.
        if (box.maxWidth < 300 + 2 * 28 + 420) {
          return _Page(
            children: [
              Center(child: preview),
              const SizedBox(height: 26),
              ...fields,
            ],
          );
        }
        return Row(
          crossAxisAlignment: CrossAxisAlignment.stretch,
          children: [
            Expanded(child: _Page(children: fields)),
            Container(
              width: 300 + 2 * 24,
              decoration: const BoxDecoration(
                border: Border(left: BorderSide(color: PColors.border)),
              ),
              child: SingleChildScrollView(padding: const EdgeInsets.fromLTRB(24, 16, 24, 24), child: preview),
            ),
          ],
        );
      },
    );
  }

  /// Your card as others would see it, with what is typed and chosen here
  /// though not yet saved.
  ProfileLook _previewLook() {
    final state = widget.state;
    final me = state.me.username;
    final saved = ProfileLook.of(state, me);
    final name = _name.text.trim();
    final note = _note.text.trim();
    final banner = _bannerDraft.value;
    final theme = _themeDraft.value;
    return ProfileLook(
      username: me,
      name: name.isEmpty ? me : name,
      presence: saved.presence,
      note: note.isEmpty ? null : note,
      bio: _bio.text.trim(),
      avatar: saved.avatar,
      banner: saved.banner,
      bannerFill: banner == null ? saved.bannerFill : AppState.defaultBannerFill(me, banner.fill?.carried),
      theme: theme == null ? saved.theme : (theme.fill == null ? null : ProfileTheme(theme.fill!)),
      sinceMs: saved.sinceMs,
      arrival: saved.arrival,
    );
  }
}

/// Your profile card as others would see it while you edit it: redrawn as
/// the fields and colours change, and nothing else.
class _ProfilePreview extends StatelessWidget {
  const _ProfilePreview({required this.state, required this.fields, required this.look, required this.drafts});

  final AppState state;
  final List<Listenable> fields;
  final List<Listenable> drafts;
  final ProfileLook Function() look;

  static void _none() {}

  @override
  Widget build(BuildContext context) {
    return Column(
      crossAxisAlignment: CrossAxisAlignment.start,
      mainAxisSize: MainAxisSize.min,
      children: [
        const Text('PREVIEW', style: PType.label),
        const SizedBox(height: 10),
        SizedBox(
          width: 300,
          child: ListenableBuilder(
            listenable: Listenable.merge([state, ...fields, ...drafts]),
            // Its own layer: redrawn without the page, and the page without it.
            builder: (context, _) => RepaintBoundary(
              child: ProfileCardView(
                state: state,
                look: look(),
                // Drawn as others see them, but not to be pressed here. The
                // bio still opens.
                actions: IgnorePointer(
                  child: ProfileCardView.peerActions(onMessage: _none, onNickname: _none, onCopy: _none),
                ),
              ),
            ),
          ),
        ),
        const SizedBox(height: 8),
        const Text('As others see your card. Changes show here before you save or apply them.', style: PType.small),
      ],
    );
  }
}

/// Keeps a bio to what a bio may hold: line breaks as `\n`, tabs as
/// spaces, no other control characters or direction overrides, and at
/// most [limit] characters (runes, as the core counts them). Typing past
/// the limit does nothing; a paste is cut to fit.
class BioFormatter extends TextInputFormatter {
  BioFormatter(this.limit);

  final int limit;

  /// Kept: a line break, or anything but a control character or one that
  /// reorders the text around it (as the core's `check_bio`).
  static bool _kept(int r) =>
      r == 0x0A || !(r < 0x20 || (r >= 0x7F && r <= 0x9F) || r == 0x2028 || r == 0x2029 || (r >= 0x202A && r <= 0x202E) || (r >= 0x2066 && r <= 0x2069));

  static String clean(String s) => String.fromCharCodes(s.replaceAll('\r\n', '\n').replaceAll('\r', '\n').replaceAll('\t', ' ').runes.where(_kept));

  @override
  TextEditingValue formatEditUpdate(TextEditingValue oldValue, TextEditingValue newValue) {
    var text = clean(newValue.text);
    // Where the cursor was, in the cleaned text.
    int at(int offset) => offset < 0 ? offset : clean(newValue.text.substring(0, offset.clamp(0, newValue.text.length))).length;
    var selection = newValue.selection.copyWith(baseOffset: at(newValue.selection.baseOffset), extentOffset: at(newValue.selection.extentOffset));
    final runes = text.runes;
    if (runes.length > limit) {
      if (oldValue.text.runes.length >= limit) return oldValue;
      text = String.fromCharCodes(runes.take(limit));
      int fit(int o) => o < 0 ? o : o.clamp(0, text.length);
      selection = selection.copyWith(baseOffset: fit(selection.baseOffset), extentOffset: fit(selection.extentOffset));
    }
    if (text == newValue.text && selection == newValue.selection) return newValue;
    // A composing region no longer matches the changed text.
    return TextEditingValue(text: text, selection: selection);
  }
}

/// Your picture, banner and colours: pictures changed with a click and
/// shared at once; colours shown in the preview as they are edited
/// ([bannerDraft], [themeDraft]) and shared on Apply.
class _LookEditor extends StatefulWidget {
  const _LookEditor({required this.state, required this.bannerDraft, required this.themeDraft});

  final AppState state;
  final ValueNotifier<_Draft?> bannerDraft;
  final ValueNotifier<_Draft?> themeDraft;

  @override
  State<_LookEditor> createState() => _LookEditorState();
}

class _LookEditorState extends State<_LookEditor> {
  /// What is being changed, while it saves.
  String? _busy;

  Future<void> _run(String what, Future<void> Function() change) async {
    setState(() => _busy = what);
    try {
      await change();
    } catch (e) {
      if (mounted) showToast(context, errorText(e), error: true);
    } finally {
      if (mounted) setState(() => _busy = null);
    }
  }

  Future<void> _applyBanner(Fill? f) async {
    await widget.state.client.setBannerFill(fill: f?.carried);
    await widget.state.reloadPerson(widget.state.me.username);
  }

  Future<void> _applyTheme(Fill? f) async {
    await widget.state.client.setTheme(fill: f?.carried);
    await widget.state.reloadPerson(widget.state.me.username);
  }

  /// A change (tonal) or a removal (danger): both tactile, like every
  /// button that does something.
  Widget _button(String what, String label, {required Future<void> Function() onPressed, bool remove = false}) {
    final busy = _busy == what;
    final action = _busy == null ? () => unawaited(_run(what, onPressed)) : null;
    final child = busy ? const PSpinner() : Text(label);
    return FilledButton(style: remove ? PButtons.danger : PButtons.tonal, onPressed: action, child: child);
  }

  @override
  Widget build(BuildContext context) {
    final state = widget.state;
    return ListenableBuilder(
      listenable: state,
      builder: (context, _) {
        final me = state.me.username;
        final p = state.personNow(me);
        final hasAvatar = p?.avatar != null;
        final hasBanner = p?.banner != null;
        final savedBanner = p?.bannerFill == null ? null : Fill.of(p!.bannerFill!);
        final savedTheme = p?.theme == null ? null : Fill.of(p!.theme!);
        return Column(
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            const Text('Picture and banner', style: PType.name),
            const SizedBox(height: 10),
            Wrap(
              spacing: 8,
              runSpacing: 8,
              children: [
                _button('avatar', hasAvatar ? 'Change picture' : 'Add a picture', onPressed: () => choosePicture(context, state, PictureKind.avatar)),
                if (hasAvatar) _button('avatar-remove', 'Remove picture', remove: true, onPressed: () => removePicture(context, state, PictureKind.avatar)),
                _button('banner', hasBanner ? 'Change banner' : 'Add a banner', onPressed: () => choosePicture(context, state, PictureKind.banner)),
                if (hasBanner) _button('banner-remove', 'Remove banner', remove: true, onPressed: () => removePicture(context, state, PictureKind.banner)),
              ],
            ),
            const SizedBox(height: 8),
            Text(
              'A picture is square, shrunk to ${pictureLimitsText(PictureKind.avatar)}. '
              'A banner is three times as wide as tall, shrunk to ${pictureLimitsText(PictureKind.banner)}. '
              'Both are encrypted: your server cannot see them.',
              style: const TextStyle(fontSize: 12.5, height: 1.45, color: PColors.textFaint),
            ),
            if (!hasBanner) ...[
              const SizedBox(height: 22),
              _FillEditor(
                key: const ValueKey('banner-fill'),
                title: 'Banner colour',
                help: 'Across the top of your card while you have no banner image.',
                noneLabel: 'Your colour',
                saved: savedBanner,
                start: PColors.avatarFor(me),
                onDraft: (d) => widget.bannerDraft.value = d,
                onApply: _applyBanner,
              ),
            ],
            const SizedBox(height: 22),
            _FillEditor(
              key: const ValueKey('theme-fill'),
              title: 'Profile colours',
              help: 'Your whole card in a colour or a gradient, for everyone who opens it.',
              noneLabel: "The app's",
              saved: savedTheme,
              start: PColors.avatarFor(me),
              onDraft: (d) => widget.themeDraft.value = d,
              onApply: _applyTheme,
            ),
          ],
        );
      },
    );
  }
}

/// A fill being edited and not yet applied: [fill], or null for none.
class _Draft {
  const _Draft(this.fill);

  final Fill? fill;
}

enum _FillKind { none, solid, gradient }

/// A colour fill to choose: none (the app's choice), one colour, or a
/// gradient between two at an angle; each colour typed as RGB hex beside a
/// square showing it. Tells [onDraft] what it holds as it changes, for a
/// preview (null when that is what is saved), and shares it on Apply.
class _FillEditor extends StatefulWidget {
  const _FillEditor({
    super.key,
    required this.title,
    required this.help,
    required this.noneLabel,
    required this.saved,
    required this.start,
    required this.onDraft,
    required this.onApply,
  });

  final String title;
  final String help;

  /// What none is called (whose look shows instead).
  final String noneLabel;

  /// What the profile has now.
  final Fill? saved;

  /// The first colour offered, coming from none.
  final Color start;
  final ValueChanged<_Draft?> onDraft;
  final Future<void> Function(Fill?) onApply;

  @override
  State<_FillEditor> createState() => _FillEditorState();
}

// The colours and the angle are listenables, and only what shows each one
// listens: a slider step or a keystroke redraws its own row, the Apply
// button and the preview, not the whole editor.
class _FillEditorState extends State<_FillEditor> {
  final _from = TextEditingController();
  final _to = TextEditingController();
  final _angleNow = ValueNotifier(180);
  var _kind = _FillKind.none;
  var _busy = false;

  int get _angle => _angleNow.value;

  @override
  void initState() {
    super.initState();
    _load(widget.saved);
    for (final l in [_from, _to, _angleNow]) {
      l.addListener(_tellDraft);
    }
  }

  @override
  void didUpdateWidget(_FillEditor old) {
    super.didUpdateWidget(old);
    // Applied (here or on another device): show what is saved now.
    if (old.saved != widget.saved && _draft == widget.saved) _load(widget.saved);
  }

  @override
  void dispose() {
    _from.dispose();
    _to.dispose();
    _angleNow.dispose();
    super.dispose();
  }

  static String _hex(Color c) => (c.toARGB32() & 0xFFFFFF).toRadixString(16).padLeft(6, '0');

  static Color? _parse(String s) => s.length == 6 ? Color(0xFF000000 | int.parse(s, radix: 16)) : null;

  /// Filling the fields from what is saved: nothing to tell the preview
  /// (and it may be mid-build).
  var _loading = false;

  void _load(Fill? f) {
    _loading = true;
    try {
      _fill(f);
    } finally {
      _loading = false;
    }
  }

  void _fill(Fill? f) {
    _kind = f == null ? _FillKind.none : (f.solid ? _FillKind.solid : _FillKind.gradient);
    final first = f?.colors.first ?? widget.start;
    // A second colour to start from: the first, a little round the wheel.
    final hsv = HSVColor.fromColor(first);
    final second = f != null && !f.solid ? f.colors[1] : hsv.withHue((hsv.hue + 50) % 360).toColor();
    _from.text = _hex(first);
    _to.text = _hex(second);
    _angleNow.value = f != null && !f.solid ? f.angle : 180;
  }

  /// What the fields hold; null for none. Unchanged if a colour is not
  /// six hex digits yet ([_complete] says).
  Fill? get _draft {
    final a = _parse(_from.text), b = _parse(_to.text);
    return switch (_kind) {
      _FillKind.none => null,
      _FillKind.solid => a == null ? widget.saved : Fill.solid(a),
      _FillKind.gradient => a == null || b == null ? widget.saved : Fill([a, b], _angle),
    };
  }

  bool get _complete => switch (_kind) {
    _FillKind.none => true,
    _FillKind.solid => _parse(_from.text) != null,
    _FillKind.gradient => _parse(_from.text) != null && _parse(_to.text) != null,
  };

  /// Tell the preview what is being edited now.
  void _tellDraft() {
    if (_loading) return;
    final d = _draft;
    widget.onDraft(d == widget.saved ? null : _Draft(d));
  }

  void _choose(_FillKind k) {
    setState(() => _kind = k);
    _tellDraft();
  }

  Future<void> _apply() async {
    setState(() => _busy = true);
    try {
      await widget.onApply(_draft);
      widget.onDraft(null);
    } catch (e) {
      if (mounted) showToast(context, errorText(e), error: true);
    } finally {
      if (mounted) setState(() => _busy = false);
    }
  }

  @override
  Widget build(BuildContext context) {
    return Column(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        Text(widget.title, style: PType.name),
        const SizedBox(height: 4),
        Text(widget.help, style: PType.small),
        const SizedBox(height: 10),
        SegmentedButton<_FillKind>(
          showSelectedIcon: false,
          segments: [
            ButtonSegment(value: _FillKind.none, label: Text(widget.noneLabel)),
            const ButtonSegment(value: _FillKind.solid, label: Text('Colour')),
            const ButtonSegment(value: _FillKind.gradient, label: Text('Gradient')),
          ],
          selected: {_kind},
          onSelectionChanged: _busy ? null : (s) => _choose(s.single),
        ),
        if (_kind != _FillKind.none) ...[
          const SizedBox(height: 12),
          _HexField(label: _kind == _FillKind.gradient ? 'From' : 'Colour', controller: _from, enabled: !_busy),
        ],
        if (_kind == _FillKind.gradient) ...[
          const SizedBox(height: 8),
          _HexField(label: 'To', controller: _to, enabled: !_busy),
          const SizedBox(height: 4),
          ValueListenableBuilder(
            valueListenable: _angleNow,
            builder: (context, angle, _) => Row(
              children: [
                const SizedBox(
                  width: 56,
                  child: Text('Angle', style: TextStyle(fontSize: 13.5, color: PColors.textMuted)),
                ),
                SizedBox(
                  width: 240,
                  // Smooth, without divisions: 359 of them would each be
                  // drawn as a tick mark on every frame of a drag.
                  child: Slider(
                    value: angle.toDouble(),
                    max: 359,
                    // Whole degrees: only a new one redraws anything.
                    onChanged: _busy ? null : (v) => _angleNow.value = v.round(),
                  ),
                ),
                // Which way it runs, and how far round.
                Transform.rotate(
                  angle: angle * math.pi / 180,
                  child: const Icon(Icons.arrow_upward_rounded, size: 18, color: PColors.textMuted),
                ),
                const SizedBox(width: 6),
                SizedBox(
                  width: 40,
                  child: Text('$angle°', style: const TextStyle(fontSize: 13.5, color: PColors.textMuted)),
                ),
              ],
            ),
          ),
        ],
        const SizedBox(height: 10),
        ListenableBuilder(
          listenable: Listenable.merge([_from, _to, _angleNow]),
          builder: (context, _) {
            final changed = _complete && _draft != widget.saved;
            return FilledButton(
              style: PButtons.tonal,
              onPressed: changed && !_busy ? () => unawaited(_apply()) : null,
              child: _busy ? const PSpinner() : const Text('Apply'),
            );
          },
        ),
      ],
    );
  }
}

/// A colour typed as six RGB hex digits, beside a rounded square showing
/// it (a placeholder until all six are there). The square follows the
/// text by itself.
class _HexField extends StatelessWidget {
  const _HexField({required this.label, required this.controller, required this.enabled});

  final String label;
  final TextEditingController controller;
  final bool enabled;

  @override
  Widget build(BuildContext context) {
    return Row(
      children: [
        SizedBox(
          width: 56,
          child: Text(label, style: const TextStyle(fontSize: 13.5, color: PColors.textMuted)),
        ),
        ValueListenableBuilder(
          valueListenable: controller,
          builder: (context, value, _) {
            final text = value.text;
            final colour = text.length == 6 ? Color(0xFF000000 | int.parse(text, radix: 16)) : null;
            return Container(
              width: 40,
              height: 40,
              decoration: BoxDecoration(
                color: colour ?? PColors.sunken,
                borderRadius: BorderRadius.circular(10),
                border: Border.all(color: PColors.borderStrong),
              ),
              child: colour == null ? const Icon(Icons.more_horiz_rounded, size: 16, color: PColors.textFaint) : null,
            );
          },
        ),
        const SizedBox(width: 10),
        SizedBox(
          width: 130,
          child: TextField(
            controller: controller,
            enabled: enabled,
            autocorrect: false,
            inputFormatters: [_HexFormatter()],
            style: const TextStyle(fontSize: 14.5, color: PColors.text, fontFamily: PType.mono),
            decoration: const InputDecoration(prefixText: '#', hintText: 'rrggbb', counterText: ''),
          ),
        ),
      ],
    );
  }
}

/// Keeps a field to six hex digits, lowercase; a pasted `#RRGGBB` loses
/// its `#`.
class _HexFormatter extends TextInputFormatter {
  static final _notHex = RegExp('[^0-9a-f]');

  @override
  TextEditingValue formatEditUpdate(TextEditingValue oldValue, TextEditingValue newValue) {
    final text = newValue.text.toLowerCase().replaceAll(_notHex, '');
    if (text == newValue.text) return newValue.text.length > 6 ? oldValue : newValue;
    final cut = text.length > 6 ? text.substring(0, 6) : text;
    return TextEditingValue(
      text: cut,
      selection: TextSelection.collapsed(offset: cut.length),
    );
  }
}

/// How your messages arrive on everyone's screen: every style playing on a
/// sample line, chosen with a click and shared at once. And whether other
/// people's styles play on yours.
class _ArrivalPicker extends StatefulWidget {
  const _ArrivalPicker({required this.state, required this.chosen, required this.onChosen});

  final AppState state;
  final Arrival chosen;
  final ValueChanged<Arrival> onChosen;

  @override
  State<_ArrivalPicker> createState() => _ArrivalPickerState();
}

class _ArrivalPickerState extends State<_ArrivalPicker> {
  /// Shown chosen at once, while it saves.
  Arrival? _choosing;

  Future<void> _choose(Arrival a) async {
    if (a == (_choosing ?? widget.chosen)) return;
    setState(() => _choosing = a);
    try {
      await widget.state.client.setProfile(change: ProfileChange(arrival: a == Arrival.standard ? '' : a.name, noteKeep: false));
      await widget.state.reloadPerson(widget.state.me.username);
      widget.onChosen(a);
    } catch (e) {
      if (mounted) showToast(context, errorText(e), error: true);
      if (mounted) setState(() => _choosing = null);
    }
  }

  @override
  void didUpdateWidget(_ArrivalPicker old) {
    super.didUpdateWidget(old);
    if (widget.chosen == _choosing) _choosing = null;
  }

  @override
  Widget build(BuildContext context) {
    final chosen = _choosing ?? widget.chosen;
    return Column(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        const Text('How your messages arrive', style: PType.name),
        const SizedBox(height: 4),
        const Text('Everyone sees your messages arrive this way.', style: TextStyle(fontSize: 12.5, color: PColors.textFaint)),
        const SizedBox(height: 12),
        LayoutBuilder(
          builder: (context, box) {
            // Two columns when there is room, one when narrow.
            final columns = box.maxWidth >= 460 ? 2 : 1;
            final width = (box.maxWidth - (columns - 1) * 10) / columns;
            return Wrap(
              spacing: 10,
              runSpacing: 10,
              children: [
                for (final a in Arrival.values)
                  SizedBox(
                    width: width,
                    child: _ArrivalOption(arrival: a, chosen: a == chosen, onTap: () => unawaited(_choose(a))),
                  ),
              ],
            );
          },
        ),
        const SizedBox(height: 14),
        ListenableBuilder(
          listenable: widget.state,
          builder: (context, _) => _CheckRow(
            value: widget.state.showOthersArrivals,
            label: "Show other people's styles",
            help: 'Off, their messages simply rise in.',
            onChanged: (v) => unawaited(widget.state.setShowOthersArrivals(v)),
          ),
        ),
      ],
    );
  }
}

class _ArrivalOption extends StatefulWidget {
  const _ArrivalOption({required this.arrival, required this.chosen, required this.onTap});

  final Arrival arrival;
  final bool chosen;
  final VoidCallback onTap;

  @override
  State<_ArrivalOption> createState() => _ArrivalOptionState();
}

class _ArrivalOptionState extends State<_ArrivalOption> {
  bool _hover = false;

  @override
  Widget build(BuildContext context) {
    final a = widget.arrival;
    return FocusableActionDetector(
      mouseCursor: SystemMouseCursors.click,
      onShowHoverHighlight: (v) => setState(() => _hover = v),
      actions: {ActivateIntent: CallbackAction<ActivateIntent>(onInvoke: (_) => widget.onTap())},
      child: GestureDetector(
        onTap: widget.onTap,
        child: AnimatedContainer(
          duration: PMotion.fast,
          padding: const EdgeInsets.fromLTRB(14, 10, 14, 8),
          decoration: BoxDecoration(
            color: widget.chosen ? PColors.active : (_hover ? PColors.hover : PColors.panelRaised),
            border: Border.all(color: widget.chosen ? PColors.activeEdge : PColors.border),
            borderRadius: BorderRadius.circular(12),
          ),
          child: Column(
            crossAxisAlignment: CrossAxisAlignment.start,
            children: [
              Row(
                children: [
                  Text(a.label, style: PType.name),
                  if (a == Arrival.standard) const Text('  default', style: TextStyle(fontSize: 12, color: PColors.textFaint)),
                  const Spacer(),
                  AnimatedOpacity(
                    opacity: widget.chosen ? 1 : 0,
                    duration: PMotion.fast,
                    child: const Icon(Icons.check_rounded, size: 18, color: PColors.accentText),
                  ),
                ],
              ),
              Text(a.description, style: const TextStyle(fontSize: 12, color: PColors.textFaint)),
              const SizedBox(height: 4),
              ArrivalPreview(arrival: a),
            ],
          ),
        ),
      ),
    );
  }
}

/// A checkbox with its label, lined up with the fields' edge.
class _CheckRow extends StatelessWidget {
  const _CheckRow({required this.value, required this.label, required this.onChanged, this.help});

  final bool value;
  final String label;
  final String? help;
  final ValueChanged<bool> onChanged;

  @override
  Widget build(BuildContext context) {
    return MouseRegion(
      cursor: SystemMouseCursors.click,
      child: GestureDetector(
        onTap: () => onChanged(!value),
        child: Row(
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            SizedBox(
              width: 20,
              height: 20,
              child: Checkbox(
                value: value,
                materialTapTargetSize: MaterialTapTargetSize.shrinkWrap,
                visualDensity: VisualDensity.compact,
                onChanged: (v) => onChanged(v ?? false),
              ),
            ),
            const SizedBox(width: 10),
            Expanded(
              child: Column(
                crossAxisAlignment: CrossAxisAlignment.start,
                children: [
                  Text(label, style: const TextStyle(fontSize: 13.5, color: PColors.textMuted)),
                  if (help != null) Text(help!, style: const TextStyle(fontSize: 12, color: PColors.textFaint)),
                ],
              ),
            ),
          ],
        ),
      ),
    );
  }
}

// ---------------------------------------------------------------- appearance

class _AppearanceSettings extends StatelessWidget {
  const _AppearanceSettings();

  @override
  Widget build(BuildContext context) {
    return _Page(
      children: [
        const Text('Zoom', style: PType.name),
        const SizedBox(height: 4),
        const Text(
          'Makes everything larger or smaller. Also Ctrl+= and Ctrl+−, and Ctrl+0 for 100%.',
          style: TextStyle(fontSize: 12.5, color: PColors.textFaint),
        ),
        const SizedBox(height: 12),
        ValueListenableBuilder<double>(
          valueListenable: appZoom,
          builder: (context, z, _) => Row(
            children: [
              PIconButton(icon: Icons.remove_rounded, tooltip: 'Smaller', onPressed: z > zoomLevels.first ? () => appZoom.value = zoomedOut(z) : null),
              SizedBox(
                width: 64,
                child: Text(zoomLabel(z), textAlign: TextAlign.center, style: PType.name),
              ),
              PIconButton(icon: Icons.add_rounded, tooltip: 'Larger', onPressed: z < zoomLevels.last ? () => appZoom.value = zoomedIn(z) : null),
              const SizedBox(width: 12),
              TextButton(onPressed: z == 1 ? null : () => appZoom.value = 1, child: const Text('Reset to 100%')),
            ],
          ),
        ),
      ],
    );
  }
}

// ---------------------------------------------------------------- spelling

class _SpellingSettings extends StatefulWidget {
  const _SpellingSettings({required this.spelling});

  final Spelling spelling;

  @override
  State<_SpellingSettings> createState() => _SpellingSettingsState();
}

class _SpellingSettingsState extends State<_SpellingSettings> {
  final _search = TextEditingController();

  @override
  void dispose() {
    _search.dispose();
    super.dispose();
  }

  static String _size(BigInt bytes) {
    final mb = bytes.toDouble() / (1024 * 1024);
    return mb >= 1 ? '${mb.toStringAsFixed(1)} MB' : '${(bytes.toDouble() / 1024).round()} KB';
  }

  @override
  Widget build(BuildContext context) {
    final s = widget.spelling;
    return ListenableBuilder(
      listenable: s,
      builder: (context, _) {
        final q = _search.text.trim().toLowerCase();
        // Chosen first, then the rest, each alphabetical.
        final shown = [...s.catalog.where((l) => q.isEmpty || l.name.toLowerCase().contains(q) || l.code.toLowerCase().contains(q))]
          ..sort((a, b) {
            final ca = s.languages.contains(a.code), cb = s.languages.contains(b.code);
            if (ca != cb) return ca ? -1 : 1;
            return a.name.compareTo(b.name);
          });
        return _Page(
          children: [
            _CheckRow(
              value: s.enabled,
              label: 'Check spelling as I type',
              help: 'Words no chosen dictionary knows get a red underline. Right-click one for suggestions.',
              onChanged: (v) => unawaited(s.setEnabled(v)),
            ),
            if (s.error != null) ...[const SizedBox(height: 12), PErrorBox(s.error!)],
            const SizedBox(height: 24),
            const Text('Languages', style: PType.name),
            const SizedBox(height: 4),
            const Text(
              'Check in several at once. A dictionary downloads the first time you choose it: Google learns only that it was downloaded.',
              style: TextStyle(fontSize: 12.5, color: PColors.textFaint),
            ),
            const SizedBox(height: 10),
            PField(label: '', controller: _search, hint: 'Search languages', onChanged: (_) => setState(() {})),
            const SizedBox(height: 6),
            for (final l in shown)
              _LanguageRow(
                name: l.name,
                detail: s.downloading.contains(l.code) ? 'Downloading…' : (l.installed ? 'Downloaded' : _size(l.bytes)),
                chosen: s.languages.contains(l.code),
                busy: s.downloading.contains(l.code),
                enabled: s.enabled,
                onChanged: (v) => unawaited(s.setLanguage(l.code, v)),
                onRemove: l.installed && !s.languages.contains(l.code) ? () => unawaited(s.remove(l.code)) : null,
              ),
            const SizedBox(height: 24),
            const Text('Your dictionary', style: PType.name),
            const SizedBox(height: 4),
            Text(
              s.words.isEmpty ? 'Words you add from the right-click menu show here.' : 'Never marked as misspelled. Click one to remove it.',
              style: const TextStyle(fontSize: 12.5, color: PColors.textFaint),
            ),
            const SizedBox(height: 10),
            Wrap(
              spacing: 6,
              runSpacing: 6,
              children: [
                for (final w in (s.words.toList()..sort()))
                  InputChip(
                    label: Text(w, style: const TextStyle(fontSize: 13)),
                    onDeleted: () => unawaited(s.removeWord(w)),
                    deleteButtonTooltipMessage: 'Remove',
                    backgroundColor: PColors.panelRaised,
                    side: const BorderSide(color: PColors.borderStrong),
                    shape: RoundedRectangleBorder(borderRadius: BorderRadius.circular(8)),
                  ),
              ],
            ),
          ],
        );
      },
    );
  }
}

class _LanguageRow extends StatelessWidget {
  const _LanguageRow({
    required this.name,
    required this.detail,
    required this.chosen,
    required this.busy,
    required this.enabled,
    required this.onChanged,
    this.onRemove,
  });

  final String name;
  final String detail;
  final bool chosen;
  final bool busy;
  final bool enabled;
  final ValueChanged<bool> onChanged;

  /// Delete it from this computer (only offered when not in use).
  final VoidCallback? onRemove;

  @override
  Widget build(BuildContext context) {
    return Padding(
      padding: const EdgeInsets.symmetric(vertical: 4),
      child: Opacity(
        opacity: enabled ? 1 : 0.5,
        child: Row(
          children: [
            Expanded(
              child: _CheckRow(value: chosen, label: name, onChanged: enabled && !busy ? onChanged : (_) {}),
            ),
            if (busy)
              const Padding(
                padding: EdgeInsets.only(right: 8),
                child: SizedBox(width: 14, height: 14, child: PSpinner()),
              ),
            Text(detail, style: const TextStyle(fontSize: 12, color: PColors.textFaint)),
            if (onRemove != null)
              PIconButton(icon: Icons.delete_outline_rounded, tooltip: 'Delete from this computer', size: 28, iconSize: 15, onPressed: onRemove!)
            else
              const SizedBox(width: 28),
          ],
        ),
      ),
    );
  }
}

// ---------------------------------------------------------------- devices

class _DeviceSettings extends StatefulWidget {
  const _DeviceSettings({required this.state});

  final AppState state;

  @override
  State<_DeviceSettings> createState() => _DeviceSettingsState();
}

class _DeviceSettingsState extends State<_DeviceSettings> {
  List<DeviceInfo>? _devices;
  String? _error;
  late final StreamSubscription<AppEvent> _events;

  @override
  void initState() {
    super.initState();
    _events = widget.state.events.where((e) => e.kind == EventKind.devices && e.username == widget.state.me.username).listen((_) => _load());
    unawaited(_load());
  }

  @override
  void dispose() {
    _events.cancel();
    super.dispose();
  }

  Future<void> _load() async {
    try {
      final d = await widget.state.client.devices();
      if (mounted) setState(() => _devices = d);
    } catch (e) {
      if (mounted) setState(() => _error = errorText(e));
    }
  }

  Future<void> _rename(DeviceInfo d) async {
    final controller = TextEditingController(text: d.name);
    final name = await showDialog<String>(
      context: context,
      builder: (context) => PDialog(
        title: 'Name this device',
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.stretch,
          children: [
            PField(
              label: 'Device name',
              controller: controller,
              autofocus: true,
              maxLength: 64,
              help: 'Shown only on your own devices.',
              onSubmitted: (v) => Navigator.of(context).pop(v),
            ),
            PDialogActions(
              children: [
                OutlinedButton(onPressed: () => Navigator.of(context).pop(), child: const Text('Cancel')),
                FilledButton(onPressed: () => Navigator.of(context).pop(controller.text), child: const Text('Save')),
              ],
            ),
          ],
        ),
      ),
    );
    controller.dispose();
    if (name == null) return;
    try {
      await widget.state.client.renameDevice(device: d.id, name: name.trim());
      await _load();
    } catch (e) {
      if (mounted) showToast(context, errorText(e), error: true);
    }
  }

  Future<void> _remove(DeviceInfo d) async {
    final label = d.name.isEmpty ? 'this device' : d.name;
    final ok = await confirm(
      context,
      title: 'Remove $label?',
      body: 'It is signed out and removed from every conversation, and can no longer read new messages. Your contacts see that you removed a device. To use it again, link it again.',
      action: 'Remove device',
      danger: true,
    );
    if (!ok) return;
    try {
      await widget.state.client.removeDevice(device: d.id);
      await _load();
    } catch (e) {
      if (mounted) showToast(context, errorText(e), error: true);
    }
  }

  @override
  Widget build(BuildContext context) {
    final devices = _devices;
    return _Page(
      children: [
        const Text(
          'Each device has its own keys. Messages are encrypted to every device in a conversation, so remove any device you no longer use or do not recognise.',
          style: TextStyle(fontSize: 13.5, height: 1.55, color: PColors.textMuted),
        ),
        const SizedBox(height: 16),
        if (_error != null) PErrorBox(_error!),
        if (devices == null && _error == null) const Center(child: PSpinner()),
        if (devices != null)
          for (final d in devices) _DeviceRow(device: d, onRename: () => _rename(d), onRemove: d.thisDevice ? null : () => _remove(d)),
        const SizedBox(height: 16),
        Align(
          alignment: Alignment.centerLeft,
          child: FilledButton.icon(
            onPressed: () async {
              await linkDevice(context, widget.state);
              await _load();
            },
            icon: const Icon(Icons.add_rounded, size: 18),
            label: const Text('Link a device'),
          ),
        ),
      ],
    );
  }
}

class _DeviceRow extends StatelessWidget {
  const _DeviceRow({required this.device, required this.onRename, required this.onRemove});

  final DeviceInfo device;
  final VoidCallback onRename;
  final VoidCallback? onRemove;

  @override
  Widget build(BuildContext context) {
    return Container(
      margin: const EdgeInsets.only(bottom: 8),
      padding: const EdgeInsets.fromLTRB(14, 12, 8, 12),
      decoration: BoxDecoration(
        color: PColors.sunken,
        border: Border.all(color: PColors.selected),
        borderRadius: BorderRadius.circular(16),
      ),
      child: Row(
        children: [
          Container(
            width: 40,
            height: 40,
            decoration: BoxDecoration(color: const Color(0xFF1A1D30), borderRadius: BorderRadius.circular(12)),
            child: const Icon(Icons.computer_outlined, size: 19, color: PColors.adminText),
          ),
          const SizedBox(width: 12),
          Expanded(
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
                Row(
                  children: [
                    Flexible(
                      child: Text(device.name.isEmpty ? 'Unnamed device' : device.name, style: PType.name, overflow: TextOverflow.ellipsis),
                    ),
                    if (device.thisDevice) ...[
                      const SizedBox(width: 8),
                      Container(
                        padding: const EdgeInsets.symmetric(horizontal: 7, vertical: 1),
                        decoration: BoxDecoration(color: PColors.accentSoft, borderRadius: BorderRadius.circular(6)),
                        child: const Text(
                          'this device',
                          style: TextStyle(fontSize: 11, fontWeight: FontWeight.w600, color: PColors.accentText),
                        ),
                      ),
                    ],
                  ],
                ),
                const SizedBox(height: 2),
                Text(
                  shortId(device.id),
                  style: const TextStyle(fontFamily: PType.mono, fontSize: 12, color: PColors.textFaint),
                ),
              ],
            ),
          ),
          PIconButton(icon: Icons.edit_outlined, tooltip: 'Rename', onPressed: onRename),
          if (onRemove != null)
            PIconButton(icon: Icons.delete_outline_rounded, tooltip: 'Remove from your account', color: PColors.danger, onPressed: onRemove),
        ],
      ),
    );
  }
}

// ---------------------------------------------------------------- security

class _SecuritySettings extends StatefulWidget {
  const _SecuritySettings({required this.state});

  final AppState state;

  @override
  State<_SecuritySettings> createState() => _SecuritySettingsState();
}

class _SecuritySettingsState extends State<_SecuritySettings> {
  List<NoticeInfo>? _notices;
  bool _refreshing = false;
  bool? _readReceipts;

  @override
  void initState() {
    super.initState();
    unawaited(_load());
  }

  Future<void> _load() async {
    try {
      final on = await widget.state.client.readReceipts();
      if (mounted) setState(() => _readReceipts = on);
    } catch (_) {
      // The setting stays hidden.
    }
    try {
      final n = await widget.state.client.notices(limit: 100);
      if (mounted) setState(() => _notices = n.reversed.toList());
    } catch (_) {
      if (mounted) setState(() => _notices = const []);
    }
  }

  Future<void> _setReadReceipts(bool on) async {
    final before = _readReceipts;
    setState(() => _readReceipts = on);
    try {
      await widget.state.client.setReadReceipts(enabled: on);
    } catch (e) {
      if (mounted) {
        setState(() => _readReceipts = before);
        showToast(context, errorText(e), error: true);
      }
    }
  }

  Future<void> _refresh() async {
    setState(() => _refreshing = true);
    try {
      await widget.state.client.refreshKeys();
      if (mounted) showToast(context, 'This device has fresh keys in every conversation.');
    } catch (e) {
      if (mounted) showToast(context, errorText(e), error: true);
    } finally {
      if (mounted) setState(() => _refreshing = false);
    }
  }

  @override
  Widget build(BuildContext context) {
    final notices = _notices;
    final readReceipts = _readReceipts;
    return _Page(
      children: [
        if (readReceipts != null) ...[
          const Text('RECEIPTS', style: PType.label),
          const SizedBox(height: 10),
          _CheckRow(
            value: readReceipts,
            label: 'Send read receipts',
            help: 'Off, people don’t see when you’ve read their messages, and you don’t see when they’ve read yours. On all your devices.',
            onChanged: _setReadReceipts,
          ),
          const SizedBox(height: 10),
          const Text(
            'One tick: the server has your message. Two: everyone in the conversation has it. Two blue: everyone has read it. Delivery always shows. Groups of more than 32 people have no receipts.',
            style: TextStyle(fontSize: 12.5, height: 1.5, color: PColors.textFaint),
          ),
          const SizedBox(height: 28),
        ],
        const Text('FRESH KEYS', style: PType.label),
        const SizedBox(height: 8),
        const Text(
          'This device replaces its keys in every conversation once a week. Doing it now means a copy of its keys taken earlier cannot read anything sent from here on.',
          style: TextStyle(fontSize: 13.5, height: 1.55, color: PColors.textMuted),
        ),
        const SizedBox(height: 12),
        Align(
          alignment: Alignment.centerLeft,
          child: OutlinedButton.icon(
            onPressed: _refreshing ? null : _refresh,
            icon: _refreshing ? const PSpinner(size: 16) : const Icon(Icons.autorenew_rounded, size: 18),
            label: const Text('Refresh keys now'),
          ),
        ),
        const SizedBox(height: 28),
        const Text('SECURITY NOTICES', style: PType.label),
        const SizedBox(height: 8),
        if (notices == null)
          const PSpinner()
        else if (notices.isEmpty)
          const Text(
            'Nothing to report. Warnings about your account or the server would show here.',
            style: TextStyle(fontSize: 13.5, color: PColors.textFaint),
          )
        else
          for (final n in notices)
            Padding(
              padding: const EdgeInsets.only(bottom: 10),
              child: Row(
                crossAxisAlignment: CrossAxisAlignment.start,
                children: [
                  const Padding(
                    padding: EdgeInsets.only(top: 2),
                    child: Icon(Icons.shield_outlined, size: 15, color: PColors.unverified),
                  ),
                  const SizedBox(width: 10),
                  Expanded(
                    child: Text(n.text, style: const TextStyle(fontSize: 13.5, height: 1.45, color: PColors.textBody)),
                  ),
                  const SizedBox(width: 10),
                  Text(messageTime(n.atMs), style: PType.small),
                ],
              ),
            ),
      ],
    );
  }
}

// ---------------------------------------------------------------- invites

/// For the server's hosts: invite codes, made here instead of on the server.
class _InviteSettings extends StatefulWidget {
  const _InviteSettings({required this.state});

  final AppState state;

  @override
  State<_InviteSettings> createState() => _InviteSettingsState();
}

class _InviteSettingsState extends State<_InviteSettings> {
  static const _lengths = [1, 3, 7, 14, 30];
  int _days = 7;
  bool _bot = false;
  bool _busy = false;
  String? _error;

  /// Codes made while this page was open, newest first. Shown only here:
  /// the server keeps nothing it could show again.
  final _made = <InviteInfo>[];

  Future<void> _make() async {
    setState(() {
      _busy = true;
      _error = null;
    });
    try {
      final invite = await widget.state.client.createInvite(bot: _bot, days: _days);
      if (mounted) setState(() => _made.insert(0, invite));
    } catch (e) {
      if (mounted) setState(() => _error = errorText(e));
    }
    if (mounted) setState(() => _busy = false);
  }

  @override
  Widget build(BuildContext context) {
    final server = widget.state.announcements?.server ?? 'your server';
    return _Page(
      children: [
        Text(
          'Make a code for someone to create an account on $server. Each code works once.',
          style: const TextStyle(fontSize: 13.5, height: 1.55, color: PColors.textMuted),
        ),
        const SizedBox(height: 18),
        Row(
          crossAxisAlignment: CrossAxisAlignment.end,
          children: [
            SizedBox(
              width: 180,
              child: Column(
                crossAxisAlignment: CrossAxisAlignment.start,
                children: [
                  const Text('WORKS FOR', style: PType.label),
                  const SizedBox(height: 8),
                  DropdownButtonFormField<int>(
                    mouseCursor: SystemMouseCursors.click,
                    initialValue: _days,
                    items: [for (final d in _lengths) DropdownMenuItem(value: d, child: Text(d == 1 ? '1 day' : '$d days'))],
                    onChanged: _busy ? null : (v) => setState(() => _days = v ?? _days),
                    dropdownColor: PColors.dialog,
                    style: const TextStyle(fontSize: 14, color: PColors.text),
                  ),
                ],
              ),
            ),
            const SizedBox(width: 14),
            FilledButton.icon(
              onPressed: _busy ? null : _make,
              icon: _busy ? const PSpinner(size: 16) : const Icon(Icons.person_add_alt_rounded, size: 18),
              label: const Text('Make an invite'),
            ),
          ],
        ),
        const SizedBox(height: 14),
        _CheckRow(
          value: _bot,
          label: 'For a bot account',
          help: 'Bots sign up with the command-line app, and show as bots to everyone.',
          onChanged: (v) => setState(() => _bot = v),
        ),
        if (_error != null) ...[const SizedBox(height: 14), PErrorBox(_error!)],
        for (final invite in _made) ...[const SizedBox(height: 16), _InviteCard(invite)],
        if (_made.isNotEmpty) ...[
          const SizedBox(height: 12),
          const Text(
            'A code shows only here, now: the server keeps just a scrambled copy to check it against. Make another any time.',
            style: TextStyle(fontSize: 12, height: 1.5, color: PColors.textFaint),
          ),
        ],
      ],
    );
  }
}

/// One invite: the code, where to use it, until when, and copy buttons.
class _InviteCard extends StatelessWidget {
  const _InviteCard(this.invite);

  final InviteInfo invite;

  @override
  Widget build(BuildContext context) {
    final both = 'Server: ${invite.server}\nInvite code: ${invite.code}';
    return Container(
      padding: const EdgeInsets.fromLTRB(16, 14, 10, 14),
      decoration: BoxDecoration(
        color: PColors.sunken,
        border: Border.all(color: PColors.borderStrong),
        borderRadius: BorderRadius.circular(14),
      ),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Row(
            children: [
              Expanded(
                child: SelectableText(
                  invite.code,
                  style: const TextStyle(fontFamily: PType.mono, fontSize: 15, letterSpacing: 0.5, color: PColors.textStrong),
                ),
              ),
              PIconButton(
                icon: Icons.copy_rounded,
                tooltip: 'Copy the code',
                onPressed: () => copyText(context, invite.code, what: 'Invite code copied'),
              ),
            ],
          ),
          const SizedBox(height: 6),
          Row(
            children: [
              Expanded(
                child: SelectableText(
                  invite.server,
                  style: const TextStyle(fontFamily: PType.mono, fontSize: 12.5, color: PColors.textMuted),
                ),
              ),
              PIconButton(
                icon: Icons.copy_rounded,
                tooltip: 'Copy the server address',
                onPressed: () => copyText(context, invite.server, what: 'Server address copied'),
              ),
            ],
          ),
          const SizedBox(height: 8),
          Row(
            children: [
              Expanded(child: Text('${invite.bot ? 'For a bot. ' : ''}Works once, until ${longWhen(invite.expiresAtMs)}.', style: PType.small)),
              OutlinedButton.icon(
                onPressed: () => copyText(context, both, what: 'Server and code copied'),
                icon: const Icon(Icons.content_copy_rounded, size: 16),
                label: const Text('Copy both'),
                style: OutlinedButton.styleFrom(minimumSize: const Size(0, 32), padding: const EdgeInsets.symmetric(horizontal: 12)),
              ),
            ],
          ),
        ],
      ),
    );
  }
}

// ---------------------------------------------------------------- about

class _About extends StatefulWidget {
  const _About({required this.state});

  final AppState state;

  @override
  State<_About> createState() => _AboutState();
}

class _AboutState extends State<_About> {
  bool _busy = false;

  /// Asks first, saying what is lost: on the account's only device, the
  /// account itself.
  Future<void> _logOut() async {
    final state = widget.state;
    setState(() => _busy = true);
    try {
      final devices = await state.client.devices();
      if (!mounted) return;
      final lastDevice = !devices.any((d) => !d.thisDevice);
      final name = '@${state.me.username}';
      final ok = await confirm(
        context,
        title: lastDevice ? 'Log out and lose $name?' : 'Log out of $name?',
        body: lastDevice
            ? 'This is the only device signed in to $name, and an account can only be reached from one of its own devices. '
                  'Logging out deletes it from this device for good, with its messages. To keep the account, link another device first.'
            : 'This device leaves your account, and its copy of your messages and settings is deleted. '
                  'Your other devices keep everything, and you can link this one again later.',
        action: lastDevice ? 'Log out and lose account' : 'Log out',
        danger: true,
      );
      if (!ok || !mounted) return;
      final navigator = Navigator.of(context);
      await state.logOut(lastDevice: lastDevice);
      navigator.popUntil((r) => r.isFirst);
      state.onSignedOut?.call();
    } catch (e) {
      if (mounted) showToast(context, errorText(e), error: true);
    } finally {
      if (mounted) setState(() => _busy = false);
    }
  }

  @override
  Widget build(BuildContext context) {
    final me = widget.state.me;
    return _Page(
      children: [
        _Detail(label: 'Username', value: '@${me.username}'),
        _Detail(label: 'Server', value: me.server),
        _Detail(label: 'Account ID', value: me.accountId, mono: true, help: 'Your safety numbers are made from this. It never changes.'),
        _Detail(label: 'This device', value: me.deviceId, mono: true),
        const SizedBox(height: 8),
        const Text('LOG OUT', style: PType.label),
        const SizedBox(height: 6),
        const Text(
          'Takes this account off this device. Your other devices are not affected.',
          style: TextStyle(fontSize: 13.5, height: 1.55, color: PColors.textMuted),
        ),
        const SizedBox(height: 12),
        Align(
          alignment: Alignment.centerLeft,
          child: OutlinedButton.icon(
            onPressed: _busy ? null : _logOut,
            style: PButtons.danger,
            icon: _busy ? const PSpinner() : const Icon(Icons.logout_rounded, size: 18),
            label: const Text('Log out'),
          ),
        ),
      ],
    );
  }
}

class _Detail extends StatelessWidget {
  const _Detail({required this.label, required this.value, this.mono = false, this.help});

  final String label;
  final String value;
  final bool mono;
  final String? help;

  @override
  Widget build(BuildContext context) {
    return Padding(
      padding: const EdgeInsets.only(bottom: 18),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Text(label.toUpperCase(), style: PType.label),
          const SizedBox(height: 6),
          Row(
            children: [
              Expanded(
                child: SelectableText(
                  value,
                  style: TextStyle(fontSize: mono ? 13 : 14.5, fontFamily: mono ? PType.mono : PType.body, color: PColors.text),
                ),
              ),
              PIconButton(icon: Icons.copy_rounded, tooltip: 'Copy', size: 32, iconSize: 15, onPressed: () => copyText(context, value)),
            ],
          ),
          if (help != null) ...[const SizedBox(height: 4), Text(help!, style: PType.small)],
        ],
      ),
    );
  }
}
