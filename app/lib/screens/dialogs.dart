// Dialogs for starting conversations, managing groups, naming people and
// linking devices. Each says what went wrong in its own words and stays
// open until it succeeds or is closed.

import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:postern/src/rust/api/client.dart';

import '../state/app_state.dart';
import '../theme.dart';
import '../ui/controls.dart';

/// Usernames typed as a list: separated by commas or spaces, with or
/// without a leading @.
List<String> parseUsernames(String text) {
  final seen = <String>{};
  return text.split(RegExp(r'[\s,;]+')).map((u) => u.trim().replaceFirst(RegExp(r'^@'), '').toLowerCase()).where((u) => u.isNotEmpty && seen.add(u)).toList();
}

/// A field in a [_FormDialog].
class _Input {
  _Input(this.label, {this.hint, this.help, String initial = '', this.maxLength}) : controller = TextEditingController(text: initial);

  final String label;
  final String? hint;
  final String? help;
  final int? maxLength;
  final TextEditingController controller;

  String get text => controller.text.trim();
}

/// A dialog of fields and one action. `submit` throws to show an error;
/// the dialog closes when it returns.
class _FormDialog extends StatefulWidget {
  const _FormDialog({required this.title, required this.inputs, required this.action, required this.submit, this.intro});

  final String title;
  final String? intro;
  final List<_Input> inputs;
  final String action;
  final Future<void> Function() submit;

  @override
  State<_FormDialog> createState() => _FormDialogState();
}

class _FormDialogState extends State<_FormDialog> {
  bool _busy = false;
  String? _error;

  @override
  void dispose() {
    for (final i in widget.inputs) {
      i.controller.dispose();
    }
    super.dispose();
  }

  Future<void> _go() async {
    if (_busy) return;
    setState(() {
      _busy = true;
      _error = null;
    });
    try {
      await widget.submit();
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
    return PDialog(
      title: widget.title,
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          if (widget.intro != null) ...[Text(widget.intro!, style: PType.text.copyWith(color: PColors.textMuted)), const SizedBox(height: 16)],
          for (final (i, input) in widget.inputs.indexed) ...[
            if (i > 0) const SizedBox(height: 14),
            PField(
              label: input.label,
              controller: input.controller,
              hint: input.hint,
              help: input.help,
              maxLength: input.maxLength,
              enabled: !_busy,
              autofocus: i == 0,
              onSubmitted: (_) => _go(),
            ),
          ],
          if (_error != null) ...[const SizedBox(height: 16), PErrorBox(_error!)],
          PDialogActions(
            children: [
              OutlinedButton(onPressed: _busy ? null : () => Navigator.of(context).pop(), child: const Text('Cancel')),
              FilledButton(onPressed: _busy ? null : _go, child: _busy ? const PSpinner() : Text(widget.action)),
            ],
          ),
        ],
      ),
    );
  }
}

Future<void> startDm(BuildContext context, AppState state) {
  final who = _Input('Username', hint: 'e.g. alice');
  return showDialog(
    context: context,
    builder: (_) => _FormDialog(
      title: 'New message',
      intro: 'Who do you want to talk to? Enter their username on this server.',
      inputs: [who],
      action: 'Start',
      submit: () async {
        final names = parseUsernames(who.text);
        if (names.isEmpty) throw ArgumentError('Enter a username.');
        if (names.length > 1) throw ArgumentError('Enter one username. For several people, make a group.');
        if (names.first == state.me.username) throw ArgumentError('That is you. Enter someone else’s username.');
        final c = await state.client.startDm(username: names.first);
        await state.refreshConversations();
        state.select(c.id);
      },
    ),
  );
}

Future<void> newGroup(BuildContext context, AppState state) {
  final name = _Input('Group name', hint: 'e.g. climbing', maxLength: 64);
  final people = _Input('People', hint: 'alice, bob', help: 'Usernames, separated by commas. You can add more later.');
  return showDialog(
    context: context,
    builder: (_) => _FormDialog(
      title: 'New group',
      inputs: [name, people],
      action: 'Create group',
      submit: () async {
        if (name.text.isEmpty) throw ArgumentError('Give the group a name.');
        final names = parseUsernames(people.text).where((u) => u != state.me.username).toList();
        final c = await state.client.createGroup(name: name.text, usernames: names);
        await state.refreshConversations();
        state.select(c.id);
      },
    ),
  );
}

Future<void> addMembers(BuildContext context, AppState state, ConversationInfo c) {
  final people = _Input('People', hint: 'carol, devi', help: 'Usernames, separated by commas. They see messages from now on.');
  return showDialog(
    context: context,
    builder: (_) => _FormDialog(
      title: 'Add people to ${c.title}',
      inputs: [people],
      action: 'Add',
      submit: () async {
        final names = parseUsernames(people.text);
        if (names.isEmpty) throw ArgumentError('Enter at least one username.');
        await state.client.addMembers(conversation: c.id, usernames: names);
      },
    ),
  );
}

Future<void> renameGroup(BuildContext context, AppState state, ConversationInfo c) {
  final name = _Input('Group name', initial: c.title, maxLength: 64);
  return showDialog(
    context: context,
    builder: (_) => _FormDialog(
      title: 'Rename group',
      inputs: [name],
      action: 'Rename',
      submit: () async {
        if (name.text.isEmpty) throw ArgumentError('Give the group a name.');
        await state.client.renameGroup(conversation: c.id, name: name.text);
      },
    ),
  );
}

Future<void> setNickname(BuildContext context, AppState state, String username) async {
  final current = (await state.person(username))?.nickname ?? '';
  if (!context.mounted) return;
  final name = _Input('Nickname', initial: current, hint: 'Leave empty to use their own name', maxLength: 64);
  return showDialog(
    context: context,
    builder: (_) => _FormDialog(
      title: 'Nickname for @$username',
      intro: 'Only you see it, on all your devices. It replaces the name they chose everywhere.',
      inputs: [name],
      action: 'Save',
      submit: () => state.client.setNickname(username: username, nickname: name.text),
    ),
  );
}

Future<void> setConversationName(BuildContext context, AppState state, ConversationInfo c, String current) {
  final name = _Input('Your name here', initial: current, hint: 'Leave empty to use your display name', maxLength: 64);
  return showDialog(
    context: context,
    builder: (_) => _FormDialog(
      title: 'Your name in ${c.title}',
      intro: 'Everyone in this group sees you by this name, unless they gave you a nickname.',
      inputs: [name],
      action: 'Save',
      submit: () => state.client.setConversationName(conversation: c.id, name: name.text),
    ),
  );
}

Future<void> leaveGroup(BuildContext context, AppState state, ConversationInfo c) async {
  final ok = await confirm(
    context,
    title: 'Leave ${c.title}?',
    body: 'You stop getting its messages on all your devices. What you already have stays readable. Someone in the group can add you back.',
    action: 'Leave group',
    danger: true,
  );
  if (!ok) return;
  try {
    await state.client.leaveGroup(conversation: c.id);
  } catch (e) {
    if (context.mounted) showToast(context, errorText(e), error: true);
  }
}

// ---------------------------------------------------------------- linking

Future<void> linkDevice(BuildContext context, AppState state) => showDialog(
  context: context,
  builder: (_) => _LinkDialog(state: state),
);

enum _LinkStep { enter, confirm, done }

class _LinkDialog extends StatefulWidget {
  const _LinkDialog({required this.state});

  final AppState state;

  @override
  State<_LinkDialog> createState() => _LinkDialogState();
}

class _LinkDialogState extends State<_LinkDialog> {
  final _code = TextEditingController();
  _LinkStep _step = _LinkStep.enter;
  LinkRequestInfo? _request;
  bool _busy = false;
  String? _error;

  @override
  void dispose() {
    _code.dispose();
    super.dispose();
  }

  Future<void> _run(Future<void> Function() f) async {
    if (_busy) return;
    setState(() {
      _busy = true;
      _error = null;
    });
    try {
      await f();
    } catch (e) {
      if (mounted) setState(() => _error = errorText(e));
    } finally {
      if (mounted) setState(() => _busy = false);
    }
  }

  Future<void> _inspect() => _run(() async {
    final code = _code.text.trim();
    if (code.isEmpty) throw ArgumentError('Enter the code your new device shows.');
    final r = await widget.state.client.linkInspect(code: code);
    setState(() {
      _request = r;
      _step = _LinkStep.confirm;
    });
  });

  Future<void> _approve() => _run(() async {
    await widget.state.client.linkApprove(code: _code.text.trim());
    setState(() => _step = _LinkStep.done);
  });

  @override
  Widget build(BuildContext context) {
    final r = _request;
    final name = (r == null || r.name.isEmpty) ? 'New device' : r.name;
    return PDialog(
      title: _step == _LinkStep.done ? 'Device linked' : 'Link a device',
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          ...switch (_step) {
            _LinkStep.enter => [
              Text.rich(
                TextSpan(
                  children: [
                    const TextSpan(text: 'On your new device, choose '),
                    TextSpan(
                      text: 'Link to my account',
                      style: PType.text.copyWith(color: PColors.text, fontWeight: FontWeight.w600),
                    ),
                    const TextSpan(text: ', then enter the code it shows.'),
                  ],
                ),
                style: PType.text.copyWith(color: PColors.textMuted),
              ),
              const SizedBox(height: 16),
              PField(
                label: 'Link code',
                controller: _code,
                hint: 'XXXXX-XXXXX-XXXXX-XXXXX-XXXXX',
                mono: true,
                autofocus: true,
                enabled: !_busy,
                onSubmitted: (_) => _inspect(),
              ),
            ],
            _LinkStep.confirm => [
              Container(
                padding: const EdgeInsets.all(14),
                decoration: BoxDecoration(
                  color: PColors.sunken,
                  border: Border.all(color: PColors.selected),
                  borderRadius: BorderRadius.circular(16),
                ),
                child: Row(
                  children: [
                    Container(
                      width: 44,
                      height: 44,
                      decoration: BoxDecoration(color: const Color(0xFF1A1D30), borderRadius: BorderRadius.circular(14)),
                      child: const Icon(Icons.devices_outlined, size: 20, color: PColors.adminText),
                    ),
                    const SizedBox(width: 14),
                    Expanded(
                      child: Column(
                        crossAxisAlignment: CrossAxisAlignment.start,
                        children: [
                          Text(name, style: PType.name),
                          const SizedBox(height: 2),
                          Text(
                            shortId(r?.deviceId ?? ''),
                            style: const TextStyle(fontFamily: 'monospace', fontSize: 12.5, color: PColors.textFaint),
                          ),
                        ],
                      ),
                    ),
                  ],
                ),
              ),
              const SizedBox(height: 16),
              Text(
                'Add this device to your account? It will get new messages in every conversation you are in. Your contacts will see that you added a device.',
                style: PType.text.copyWith(color: PColors.textMuted),
              ),
            ],
            _LinkStep.done => [
              Row(
                children: [
                  const Icon(Icons.check_circle_outline_rounded, color: PColors.verified, size: 22),
                  const SizedBox(width: 10),
                  Expanded(
                    child: Text('$name is linked', style: PType.name.copyWith(color: PColors.verified)),
                  ),
                ],
              ),
              const SizedBox(height: 12),
              Text('It is joining your conversations now. It sees messages sent from here on.', style: PType.text.copyWith(color: PColors.textMuted)),
            ],
          },
          if (_error != null) ...[const SizedBox(height: 16), PErrorBox(_error!)],
          PDialogActions(
            children: switch (_step) {
              _LinkStep.enter => [
                OutlinedButton(onPressed: _busy ? null : () => Navigator.of(context).pop(), child: const Text('Cancel')),
                FilledButton(onPressed: _busy ? null : _inspect, child: _busy ? const PSpinner() : const Text('Continue')),
              ],
              _LinkStep.confirm => [
                OutlinedButton(onPressed: _busy ? null : () => Navigator.of(context).pop(), child: const Text('Not my device')),
                FilledButton(onPressed: _busy ? null : _approve, child: _busy ? const PSpinner() : const Text('Add device')),
              ],
              _LinkStep.done => [FilledButton(autofocus: true, onPressed: () => Navigator.of(context).pop(), child: const Text('Done'))],
            },
          ),
        ],
      ),
    );
  }
}

/// A device ID's first 16 hex characters, in two groups: enough to tell
/// devices apart at a glance.
String shortId(String hex) {
  if (hex.length < 16) return hex;
  return '${hex.substring(0, 8)} ${hex.substring(8, 16)}';
}

/// Copy text, and say so.
Future<void> copyText(BuildContext context, String text, {String what = 'Copied'}) async {
  await Clipboard.setData(ClipboardData(text: text));
  if (context.mounted) showToast(context, what);
}
