// Settings: your profile, your devices, security and the account details.

import 'dart:async';

import 'package:flutter/material.dart';
import 'package:post/src/rust/api/client.dart';

import '../state/app_state.dart';
import '../theme.dart';
import '../ui/arrival.dart';
import '../ui/controls.dart';
import '../ui/format.dart';
import 'dialogs.dart';

enum SettingsSection { profile, devices, security, about }

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
      child: SizedBox(
        width: 820,
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
                  for (final s in SettingsSection.values) _NavItem(section: s, selected: s == _section, onTap: () => setState(() => _section = s)),
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
                      SettingsSection.security => _SecuritySettings(state: widget.state),
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
    SettingsSection.security => 'Security',
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
      SettingsSection.security => (Icons.shield_outlined, 'Security'),
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
    );
    if (change.displayName == null && change.typingTemplate == null && change.note == null) {
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
    return _Page(
      children: [
        Row(
          children: [
            PAvatar(name: shownName, colorKey: widget.state.me.username, size: 56),
            const SizedBox(width: 14),
            Expanded(
              child: Column(
                crossAxisAlignment: CrossAxisAlignment.start,
                children: [
                  Text(shownName, style: PType.heading),
                  Text('@${widget.state.me.username}', style: PType.small),
                ],
              ),
            ),
          ],
        ),
        const SizedBox(height: 22),
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
      ],
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

  @override
  void initState() {
    super.initState();
    unawaited(_load());
  }

  Future<void> _load() async {
    try {
      final n = await widget.state.client.notices(limit: 100);
      if (mounted) setState(() => _notices = n.reversed.toList());
    } catch (_) {
      if (mounted) setState(() => _notices = const []);
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
    return _Page(
      children: [
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

// ---------------------------------------------------------------- about

class _About extends StatelessWidget {
  const _About({required this.state});

  final AppState state;

  @override
  Widget build(BuildContext context) {
    final me = state.me;
    return _Page(
      children: [
        _Detail(label: 'Username', value: '@${me.username}'),
        _Detail(label: 'Server', value: me.server),
        _Detail(label: 'Account ID', value: me.accountId, mono: true, help: 'Your safety numbers are made from this. It never changes.'),
        _Detail(label: 'This device', value: me.deviceId, mono: true),
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
