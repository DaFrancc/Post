// The server's announcements: one channel everyone on the server is in,
// where only its hosts post. Every card is shown as from the server; what
// the server posts itself says so at the end, like an email would.

import 'dart:async';

import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:post/src/rust/api/client.dart';

import '../state/app_state.dart';
import '../theme.dart';
import '../ui/controls.dart';
import '../ui/format.dart';
import '../ui/markup.dart' show plainText;
import '../ui/markup_text.dart';
import 'home.dart' show Panel;

/// How many announcements to show at first, and how many more each time.
const _page = 200;

/// The line the server's own announcements end with.
const _automatic = '(Sent automatically by the server.)';

/// An announcement as the reader sees it, times in their own time zone.
String announcementText(AnnouncementInfo a) {
  final when = longWhen(a.startsAtMs);
  final long = minutesText(a.minutes);
  final why = a.text.isEmpty ? '' : ': ${a.text}';
  const waits = 'Anything you send meanwhile waits in your app and goes out once the server is back.';
  return switch (a.kind) {
    AnnouncementKind.text => a.text,
    AnnouncementKind.maintenancePlanned =>
      'Maintenance is planned for $when, for about $long$why. $waits You’ll be reminded before it starts.\n\n$_automatic',
    AnnouncementKind.maintenanceReminder =>
      'Reminder: maintenance starts in about ${_untilText(a.startsAtMs - a.atMs)}, $when, for about $long$why. $waits\n\n$_automatic',
    AnnouncementKind.maintenanceStarted =>
      'Maintenance has started$why. The server should be back in about $long. $waits\n\n$_automatic',
    AnnouncementKind.maintenanceOver => 'Maintenance is over, and the server is back.\n\n$_automatic',
    AnnouncementKind.maintenanceCancelled => 'The maintenance planned for $when is cancelled.\n\n$_automatic',
    AnnouncementKind.unsupported => 'An announcement this version of Post cannot show. Update Post to read it.',
  };
}

/// How long until a reminder's maintenance, rounded the way the reminders
/// are spaced: a day, an hour, 30 minutes, 5 minutes.
String _untilText(int ms) {
  final minutes = (ms / 60000).round();
  if (minutes >= 23 * 60) return minutes >= 36 * 60 ? '${(minutes / 1440).round()} days' : 'a day';
  if (minutes >= 90) return '${(minutes / 60).round()} hours';
  if (minutes >= 55) return 'an hour';
  return minutesText(minutes);
}

/// The channel's preview line in the conversation list.
String announcementPreview(AnnouncementSummary s) {
  final a = s.latest;
  if (a == null) return 'Nothing announced yet';
  return plainText(announcementText(a).replaceAll('\n\n$_automatic', '')).replaceAll('\n', ' ');
}

class AnnouncementsView extends StatefulWidget {
  const AnnouncementsView({super.key, required this.state});

  final AppState state;

  @override
  State<AnnouncementsView> createState() => _AnnouncementsViewState();
}

class _AnnouncementsViewState extends State<AnnouncementsView> {
  final _composer = MarkupEditingController();
  final _composerFocus = FocusNode();
  late final StreamSubscription<AppEvent> _events;
  List<AnnouncementInfo> _list = const [];
  int _limit = _page;
  bool _loaded = false;
  bool _posting = false;
  String? _error;

  AppState get _state => widget.state;

  @override
  void initState() {
    super.initState();
    _events = _state.events.where((e) => e.kind == EventKind.announcement || e.kind == EventKind.announcements).listen((_) => _load());
    unawaited(_load());
  }

  @override
  void dispose() {
    _events.cancel();
    _composer.dispose();
    _composerFocus.dispose();
    super.dispose();
  }

  Future<void> _load() async {
    try {
      final list = await _state.client.announcements(limit: _limit);
      if (mounted) {
        setState(() {
          _list = list;
          _loaded = true;
        });
      }
    } catch (_) {
      if (mounted) setState(() => _loaded = true);
    }
  }

  Future<void> _post() async {
    final text = _composer.text.trim();
    if (text.isEmpty || _posting) return;
    if (text == '/maintenance') {
      _composer.clear();
      await planMaintenance(context, _state);
      return;
    }
    setState(() {
      _posting = true;
      _error = null;
    });
    try {
      await _state.client.announce(text: text);
      _composer.clear();
    } catch (e) {
      _error = errorText(e);
    }
    if (mounted) setState(() => _posting = false);
  }

  KeyEventResult _key(FocusNode node, KeyEvent e) {
    if (e is! KeyDownEvent || e.logicalKey != LogicalKeyboardKey.enter) return KeyEventResult.ignored;
    // An announcement cannot be taken back: Enter is a new line, and
    // Ctrl+Enter (or the button) posts. A command runs on Enter.
    if (HardwareKeyboard.instance.isControlPressed || _composer.text.trim() == '/maintenance') {
      unawaited(_post());
      return KeyEventResult.handled;
    }
    return KeyEventResult.ignored;
  }

  @override
  Widget build(BuildContext context) {
    final s = _state.announcements;
    final server = s?.server ?? '';
    return Panel(
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          _Header(server: server),
          if (s?.maintenance case final m?) _MaintenanceBar(state: _state, maintenance: m, host: s!.host),
          Expanded(
            child: !_loaded
                ? const Center(child: PSpinner())
                : _list.isEmpty
                ? const Center(
                    child: Padding(
                      padding: EdgeInsets.all(32),
                      child: Text(
                        'Nothing has been announced yet. News from the people running this server, such as planned maintenance, shows up here.',
                        textAlign: TextAlign.center,
                        style: TextStyle(fontSize: 13.5, height: 1.5, color: PColors.textFaint),
                      ),
                    ),
                  )
                : SelectionArea(
                    child: ListView.builder(
                      reverse: true,
                      padding: const EdgeInsets.fromLTRB(24, 18, 24, 12),
                      itemCount: _list.length + 1,
                      itemBuilder: (context, i) {
                        if (i == _list.length) {
                          return _list.length < _limit
                              ? const SizedBox.shrink()
                              : Center(
                                  child: TextButton(
                                    onPressed: () {
                                      _limit += _page;
                                      unawaited(_load());
                                    },
                                    child: const Text('Show older announcements'),
                                  ),
                                );
                        }
                        final a = _list[_list.length - 1 - i];
                        return Padding(
                          key: ValueKey(a.id),
                          padding: const EdgeInsets.only(bottom: 14),
                          child: _Card(server: server, a: a),
                        );
                      },
                    ),
                  ),
          ),
          if (s?.host ?? false)
            _Composer(
              server: server,
              controller: _composer,
              focus: _composerFocus,
              posting: _posting,
              error: _error,
              onKey: _key,
              onPost: _post,
              onPlan: () => planMaintenance(context, _state),
            )
          else
            const _MemberNote(),
        ],
      ),
    );
  }
}

class _Header extends StatelessWidget {
  const _Header({required this.server});

  final String server;

  @override
  Widget build(BuildContext context) {
    return Container(
      constraints: const BoxConstraints(minHeight: PShape.header),
      padding: const EdgeInsets.fromLTRB(22, 8, 16, 8),
      decoration: const BoxDecoration(border: Border(bottom: BorderSide(color: PColors.border))),
      child: Row(
        children: [
          Flexible(
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.start,
              mainAxisSize: MainAxisSize.min,
              children: [
                const Text('Announcements', style: PType.heading),
                const SizedBox(height: 1),
                Text('from $server', maxLines: 1, overflow: TextOverflow.ellipsis, style: PType.small),
              ],
            ),
          ),
          const SizedBox(width: 12),
          Tooltip(
            message: 'Only accounts on this server can fetch these, over an encrypted connection. The server stores them encrypted.',
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
                    'Only people on this server see these',
                    style: TextStyle(fontSize: 12, fontWeight: FontWeight.w500, color: PColors.accentText),
                  ),
                ],
              ),
            ),
          ),
        ],
      ),
    );
  }
}

/// The server's mark: its gradient, as on every card.
class ServerMark extends StatelessWidget {
  const ServerMark({super.key, this.size = 32});

  final double size;

  @override
  Widget build(BuildContext context) {
    return Container(
      width: size,
      height: size,
      decoration: BoxDecoration(gradient: PColors.brand, borderRadius: BorderRadius.circular(size * 0.31)),
      child: Icon(Icons.campaign_outlined, size: size * 0.56, color: PColors.onAccent),
    );
  }
}

class _Card extends StatelessWidget {
  const _Card({required this.server, required this.a});

  final String server;
  final AnnouncementInfo a;

  @override
  Widget build(BuildContext context) {
    return Container(
      padding: const EdgeInsets.fromLTRB(18, 16, 18, 16),
      decoration: BoxDecoration(
        color: PColors.panelRaised,
        border: Border.all(color: PColors.borderStrong),
        borderRadius: BorderRadius.circular(16),
      ),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Row(
            children: [
              const ServerMark(),
              const SizedBox(width: 10),
              Flexible(child: Text(server, maxLines: 1, overflow: TextOverflow.ellipsis, style: PType.name)),
              const SizedBox(width: 10),
              Text(messageTime(a.atMs), style: PType.small),
            ],
          ),
          const SizedBox(height: 10),
          MarkupText(announcementText(a), style: const TextStyle(fontSize: 14.5, height: 1.55, color: PColors.textBody)),
        ],
      ),
    );
  }
}

/// Maintenance planned or underway, across the top; hosts can end it here.
class _MaintenanceBar extends StatefulWidget {
  const _MaintenanceBar({required this.state, required this.maintenance, required this.host});

  final AppState state;
  final MaintenanceInfo maintenance;
  final bool host;

  @override
  State<_MaintenanceBar> createState() => _MaintenanceBarState();
}

class _MaintenanceBarState extends State<_MaintenanceBar> {
  bool _busy = false;

  Future<void> _end() async {
    final m = widget.maintenance;
    final ok = await confirm(
      context,
      title: m.underway ? 'Maintenance over?' : 'Cancel maintenance?',
      body: m.underway
          ? 'Everyone on the server is told the maintenance is over and the server is back.'
          : 'Everyone on the server is told the maintenance planned for ${longWhen(m.startsAtMs)} is cancelled, and the reminders stop.',
      action: m.underway ? 'It’s over' : 'Cancel it',
      danger: !m.underway,
    );
    if (!ok || !mounted) return;
    setState(() => _busy = true);
    try {
      await widget.state.client.endMaintenance();
    } catch (e) {
      if (mounted) showToast(context, errorText(e), error: true);
    }
    if (mounted) setState(() => _busy = false);
  }

  @override
  Widget build(BuildContext context) {
    final m = widget.maintenance;
    final text = m.underway
        ? 'Maintenance underway since ${clockTime(m.startsAtMs)}, for about ${minutesText(m.minutes)}'
        : 'Maintenance planned for ${longWhen(m.startsAtMs)}, for about ${minutesText(m.minutes)}';
    return Container(
      padding: const EdgeInsets.fromLTRB(22, 8, 14, 8),
      decoration: const BoxDecoration(
        color: PColors.accentSoft,
        border: Border(bottom: BorderSide(color: PColors.border)),
      ),
      child: Row(
        children: [
          Icon(m.underway ? Icons.construction_rounded : Icons.event_outlined, size: 16, color: PColors.accentText),
          const SizedBox(width: 10),
          Expanded(child: Text(text, style: const TextStyle(fontSize: 13, color: PColors.accentText))),
          if (widget.host)
            OutlinedButton(
              onPressed: _busy ? null : _end,
              style: OutlinedButton.styleFrom(minimumSize: const Size(0, 32), padding: const EdgeInsets.symmetric(horizontal: 12)),
              child: _busy ? const PSpinner() : Text(m.underway ? 'It’s over' : 'Cancel'),
            ),
        ],
      ),
    );
  }
}

class _MemberNote extends StatelessWidget {
  const _MemberNote();

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
      child: const Text(
        'Only the server’s hosts can post here. Announcements are kept, and can’t be deleted.',
        style: TextStyle(fontSize: 13, height: 1.5, color: PColors.textMuted),
      ),
    );
  }
}

class _Composer extends StatelessWidget {
  const _Composer({
    required this.server,
    required this.controller,
    required this.focus,
    required this.posting,
    required this.error,
    required this.onKey,
    required this.onPost,
    required this.onPlan,
  });

  final String server;
  final TextEditingController controller;
  final FocusNode focus;
  final bool posting;
  final String? error;
  final FocusOnKeyEventCallback onKey;
  final VoidCallback onPost;
  final VoidCallback onPlan;

  @override
  Widget build(BuildContext context) {
    return Padding(
      padding: const EdgeInsets.fromLTRB(18, 8, 18, 16),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          if (error != null) ...[PErrorBox(error!), const SizedBox(height: 8)],
          Container(
            padding: const EdgeInsets.fromLTRB(14, 4, 8, 4),
            decoration: BoxDecoration(
              color: PColors.sunken,
              border: Border.all(color: PColors.focusEdge),
              borderRadius: BorderRadius.circular(14),
            ),
            child: Row(
              crossAxisAlignment: CrossAxisAlignment.end,
              children: [
                Expanded(
                  child: Focus(
                    onKeyEvent: onKey,
                    child: TextField(
                      controller: controller,
                      focusNode: focus,
                      enabled: !posting,
                      minLines: 1,
                      maxLines: 8,
                      keyboardType: TextInputType.multiline,
                      style: const TextStyle(fontSize: 14.5, height: 1.5, color: PColors.text),
                      decoration: const InputDecoration(
                        hintText: 'Write an announcement, or /maintenance to plan some',
                        border: InputBorder.none,
                        enabledBorder: InputBorder.none,
                        focusedBorder: InputBorder.none,
                        filled: false,
                        isDense: true,
                        contentPadding: EdgeInsets.symmetric(vertical: 12),
                      ),
                    ),
                  ),
                ),
                Padding(
                  padding: const EdgeInsets.only(bottom: 4),
                  child: PIconButton(icon: Icons.event_outlined, tooltip: 'Plan maintenance', onPressed: posting ? null : onPlan),
                ),
                const SizedBox(width: 4),
                Padding(
                  padding: const EdgeInsets.only(bottom: 4),
                  child: FilledButton(onPressed: posting ? null : onPost, child: posting ? const PSpinner() : const Text('Announce')),
                ),
              ],
            ),
          ),
          const SizedBox(height: 8),
          Text(
            'Posted as $server to everyone on the server, with Ctrl+Enter or Announce. It can’t be deleted afterwards.',
            style: PType.small,
          ),
        ],
      ),
    );
  }
}

// ---------------------------------------------------------------- planning

/// Ask when maintenance is, and plan it: everyone is told in their own time
/// zone, and reminded.
Future<void> planMaintenance(BuildContext context, AppState state) {
  return showDialog(context: context, builder: (_) => _PlanDialog(state: state));
}

class _PlanDialog extends StatefulWidget {
  const _PlanDialog({required this.state});

  final AppState state;

  @override
  State<_PlanDialog> createState() => _PlanDialogState();
}

/// Lengths offered for maintenance, in minutes.
const _lengths = [5, 10, 15, 30, 45, 60, 90, 120, 180, 240, 480];

class _PlanDialogState extends State<_PlanDialog> {
  final _reason = TextEditingController();
  bool _now = false;
  late DateTime _starts = _nextHour();
  int _minutes = 15;
  bool _announce = true;
  bool _busy = false;
  String? _error;

  static DateTime _nextHour() {
    final t = DateTime.now().add(const Duration(hours: 1));
    return DateTime(t.year, t.month, t.day, t.hour);
  }

  @override
  void dispose() {
    _reason.dispose();
    super.dispose();
  }

  Future<void> _pickDay() async {
    final now = DateTime.now();
    final d = await showDatePicker(context: context, initialDate: _starts, firstDate: DateTime(now.year, now.month, now.day), lastDate: now.add(const Duration(days: 365)));
    if (d != null) setState(() => _starts = DateTime(d.year, d.month, d.day, _starts.hour, _starts.minute));
  }

  Future<void> _pickTime() async {
    final t = await showTimePicker(
      context: context,
      initialTime: TimeOfDay.fromDateTime(_starts),
      builder: (context, child) => MediaQuery(data: MediaQuery.of(context).copyWith(alwaysUse24HourFormat: true), child: child!),
    );
    if (t != null) setState(() => _starts = DateTime(_starts.year, _starts.month, _starts.day, t.hour, t.minute));
  }

  Future<void> _go() async {
    if (_busy) return;
    if (!_now && !_starts.isAfter(DateTime.now())) {
      setState(() => _error = 'That time has passed. Choose a later one, or start now.');
      return;
    }
    setState(() {
      _busy = true;
      _error = null;
    });
    try {
      await widget.state.client.planMaintenance(
        startsAtMs: _now ? 0 : _starts.millisecondsSinceEpoch,
        minutes: _minutes,
        reason: _reason.text.trim(),
        quiet: !_announce,
      );
      if (mounted) Navigator.of(context).pop();
    } catch (e) {
      if (mounted) {
        setState(() {
          _busy = false;
          _error = errorText(e);
        });
      }
    }
  }

  @override
  Widget build(BuildContext context) {
    final server = widget.state.announcements?.server ?? 'the server';
    final ms = _starts.millisecondsSinceEpoch;
    return PDialog(
      title: 'Plan maintenance',
      width: 480,
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          Text('Everyone on $server is told, in their own time zone, and reminded before it starts.', style: PType.text.copyWith(color: PColors.textMuted)),
          const SizedBox(height: 16),
          SegmentedButton<bool>(
            segments: const [
              ButtonSegment(value: false, label: Text('At a time')),
              ButtonSegment(value: true, label: Text('Starting now')),
            ],
            selected: {_now},
            showSelectedIcon: false,
            onSelectionChanged: _busy ? null : (v) => setState(() => _now = v.first),
          ),
          const SizedBox(height: 14),
          Row(
            crossAxisAlignment: CrossAxisAlignment.start,
            children: [
              if (!_now) ...[
                Expanded(
                  child: _Choice(
                    label: 'Starts',
                    value: '${dayLabel(ms) == 'Today' ? 'Today' : '${_weekday(_starts)} ${_starts.day} ${_month(_starts)}'}, ${clockTime(ms)}',
                    onDay: _busy ? null : _pickDay,
                    onTime: _busy ? null : _pickTime,
                  ),
                ),
                const SizedBox(width: 12),
              ],
              Expanded(
                child: Column(
                  crossAxisAlignment: CrossAxisAlignment.start,
                  children: [
                    const Text('LASTS ABOUT', style: PType.label),
                    const SizedBox(height: 8),
                    DropdownButtonFormField<int>(
                      mouseCursor: SystemMouseCursors.click,
                      initialValue: _minutes,
                      items: [for (final m in _lengths) DropdownMenuItem(value: m, child: Text(minutesText(m)))],
                      onChanged: _busy ? null : (v) => setState(() => _minutes = v ?? _minutes),
                      dropdownColor: PColors.dialog,
                      style: const TextStyle(fontSize: 14, color: PColors.text),
                    ),
                  ],
                ),
              ),
            ],
          ),
          if (!_now) ...[
            const SizedBox(height: 10),
            Row(
              children: [
                const Icon(Icons.schedule_rounded, size: 14, color: PColors.textMuted),
                const SizedBox(width: 8),
                Expanded(
                  child: Text.rich(
                    TextSpan(
                      children: [
                        const TextSpan(text: 'In your time zone: '),
                        TextSpan(
                          text: timeZoneText(_starts),
                          style: const TextStyle(color: PColors.text, fontWeight: FontWeight.w600),
                        ),
                        const TextSpan(text: '. Everyone else sees it in theirs.'),
                      ],
                    ),
                    style: const TextStyle(fontSize: 12.5, color: PColors.textMuted),
                  ),
                ),
              ],
            ),
          ],
          const SizedBox(height: 14),
          PField(label: 'Why (optional)', controller: _reason, hint: 'e.g. moving to a new machine', enabled: !_busy, maxLength: 200),
          if (!_now) ...[
            const SizedBox(height: 14),
            Container(
              padding: const EdgeInsets.fromLTRB(10, 10, 14, 12),
              decoration: BoxDecoration(color: PColors.sunken, borderRadius: BorderRadius.circular(12)),
              child: Column(
                crossAxisAlignment: CrossAxisAlignment.start,
                children: [
                  const Padding(padding: EdgeInsets.only(left: 4), child: Text('ANNOUNCED', style: PType.label)),
                  const SizedBox(height: 4),
                  MouseRegion(
                    cursor: SystemMouseCursors.click,
                    child: GestureDetector(
                      onTap: _busy ? null : () => setState(() => _announce = !_announce),
                      child: Row(
                        children: [
                          Checkbox(value: _announce, onChanged: _busy ? null : (v) => setState(() => _announce = v ?? true)),
                          const Text('Now, when it’s planned', style: TextStyle(fontSize: 13.5, color: PColors.textBody)),
                        ],
                      ),
                    ),
                  ),
                  const Padding(
                    padding: EdgeInsets.only(left: 4),
                    child: Text(
                      'Then 1 day, 1 hour, 30 minutes and 5 minutes before. Reminders already past are skipped.',
                      style: TextStyle(fontSize: 13, height: 1.5, color: PColors.textMuted),
                    ),
                  ),
                ],
              ),
            ),
          ],
          if (_error != null) ...[const SizedBox(height: 16), PErrorBox(_error!)],
          PDialogActions(
            children: [
              OutlinedButton(onPressed: _busy ? null : () => Navigator.of(context).pop(), child: const Text('Cancel')),
              FilledButton(onPressed: _busy ? null : _go, child: _busy ? const PSpinner() : Text(_now ? 'Start it' : 'Plan it')),
            ],
          ),
        ],
      ),
    );
  }

  static String _weekday(DateTime t) => const ['Mon', 'Tue', 'Wed', 'Thu', 'Fri', 'Sat', 'Sun'][t.weekday - 1];
  static String _month(DateTime t) => const ['Jan', 'Feb', 'Mar', 'Apr', 'May', 'Jun', 'Jul', 'Aug', 'Sep', 'Oct', 'Nov', 'Dec'][t.month - 1];
}

/// When maintenance starts: the day and the time, each its own button.
class _Choice extends StatelessWidget {
  const _Choice({required this.label, required this.value, required this.onDay, required this.onTime});

  final String label;
  final String value;
  final VoidCallback? onDay;
  final VoidCallback? onTime;

  @override
  Widget build(BuildContext context) {
    return Column(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        Text(label.toUpperCase(), style: PType.label),
        const SizedBox(height: 8),
        Container(
          height: 48,
          padding: const EdgeInsets.symmetric(horizontal: 4),
          decoration: BoxDecoration(
            color: PColors.sunken,
            border: Border.all(color: PColors.borderStrong),
            borderRadius: BorderRadius.circular(PShape.controlRadius),
          ),
          child: Row(
            children: [
              Expanded(
                child: Padding(padding: const EdgeInsets.only(left: 8), child: Text(value, style: const TextStyle(fontSize: 14, color: PColors.text))),
              ),
              PIconButton(icon: Icons.calendar_today_outlined, tooltip: 'Choose the day', size: 34, iconSize: 16, onPressed: onDay),
              PIconButton(icon: Icons.schedule_rounded, tooltip: 'Choose the time', size: 34, iconSize: 16, onPressed: onTime),
            ],
          ),
        ),
      ],
    );
  }
}
