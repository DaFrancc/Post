// First run: create an account with an invite, or link this device to an
// account you already have on another device.

import 'dart:io' show Platform;

import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:post/src/rust/api/client.dart';
import 'package:post/src/rust/api/error.dart';

import '../theme.dart';
import '../ui/controls.dart';

/// Checks for the common mistakes, before anything is sent. The core checks
/// again; these just say what is wrong, next to the field it is about.
abstract final class _Check {
  static String? server(String s) {
    if (s.isEmpty) return 'Enter your server’s address.';
    if (!s.startsWith('wss://') && !s.startsWith('ws://')) {
      return 'The address starts with wss://, like wss://chat.example.org/v1/ws.';
    }
    if (Uri.tryParse(s)?.host.isEmpty ?? true) return 'That is not a complete address.';
    return null;
  }

  static String? invite(String s) {
    if (s.isEmpty) return 'Enter the invite code.';
    if (!RegExp(r'^[0-9a-fA-F]{32}$').hasMatch(s)) return 'Invite codes are 32 characters of 0–9 and a–f.';
    return null;
  }

  static String? username(String s) {
    if (s.isEmpty) return 'Choose a username.';
    if (s.length < 3) return 'Use at least 3 characters.';
    if (s.length > 32) return 'Use at most 32 characters.';
    if (s.contains(RegExp(r'[A-Z]'))) return 'Use lowercase letters only.';
    if (!RegExp(r'^[a-z]').hasMatch(s)) return 'Start with a letter.';
    if (!RegExp(r'^[a-z0-9_]+$').hasMatch(s)) return 'Use only a–z, 0–9 and _.';
    return null;
  }

  static String? deviceName(String s) => s.isEmpty ? 'Give this device a name.' : null;
}

class WelcomeScreen extends StatefulWidget {
  const WelcomeScreen({super.key, required this.client, required this.onSignedIn});

  final Client client;
  final ValueChanged<Identity> onSignedIn;

  @override
  State<WelcomeScreen> createState() => _WelcomeScreenState();
}

enum _Step { choose, register, link, linkWaiting }

class _WelcomeScreenState extends State<WelcomeScreen> {
  _Step _step = _Step.choose;
  bool _busy = false;
  String? _error;
  String? _code;
  Map<String, String> _fieldErrors = const {};

  final _server = TextEditingController();
  final _invite = TextEditingController();
  final _username = TextEditingController();
  final _deviceName = TextEditingController(text: Platform.localHostname);

  @override
  void dispose() {
    for (final c in [_server, _invite, _username, _deviceName]) {
      c.dispose();
    }
    super.dispose();
  }

  void _go(_Step step) => setState(() {
    _step = step;
    _error = null;
    _fieldErrors = const {};
  });

  /// Show field problems if there are any; true if all is well.
  bool _validate(Map<String, String?> checks) {
    final problems = {
      for (final e in checks.entries)
        if (e.value != null) e.key: e.value!,
    };
    setState(() {
      _fieldErrors = problems;
      _error = null;
    });
    return problems.isEmpty;
  }

  /// Put an error from the core next to the field it concerns, or under the
  /// form when it concerns none.
  void _showError(Object e) {
    if (e is! PostError) {
      setState(() => _error = 'Something went wrong. Try again.');
      return;
    }
    final field = switch (e.kind) {
      ErrorKind.usernameTaken => 'username',
      ErrorKind.inviteInvalid => 'invite',
      ErrorKind.network || ErrorKind.server => 'server',
      _ => null,
    };
    setState(() {
      if (field != null) {
        _fieldErrors = {field: e.message};
      } else {
        _error = e.message;
      }
    });
  }

  Future<void> _register() async {
    final server = _server.text.trim(), invite = _invite.text.trim(), username = _username.text.trim();
    if (!_validate({'server': _Check.server(server), 'invite': _Check.invite(invite), 'username': _Check.username(username)})) return;
    setState(() => _busy = true);
    try {
      final me = await widget.client.register(server: server, invite: invite, username: username);
      // Name the device after the computer, as linking does, so it is
      // recognisable in the device list. Only a convenience: carry on if
      // it fails.
      try {
        await widget.client.renameDevice(device: me.deviceId, name: Platform.localHostname);
      } catch (_) {}
      widget.onSignedIn(me);
    } catch (e) {
      _showError(e);
    } finally {
      if (mounted) setState(() => _busy = false);
    }
  }

  Future<void> _link() async {
    final server = _server.text.trim(), name = _deviceName.text.trim();
    if (!_validate({'server': _Check.server(server), 'device': _Check.deviceName(name)})) return;
    setState(() => _busy = true);
    try {
      final code = await widget.client.beginLink(server: server, deviceName: name);
      setState(() {
        _code = code;
        _step = _Step.linkWaiting;
      });
      final me = await widget.client.finishLink();
      widget.onSignedIn(me);
    } catch (e) {
      setState(() => _step = _Step.link);
      _showError(e);
    } finally {
      if (mounted) setState(() => _busy = false);
    }
  }

  @override
  Widget build(BuildContext context) {
    return Padding(
      padding: const EdgeInsets.fromLTRB(PShape.gap, 0, PShape.gap, PShape.gap),
      child: Container(
        decoration: BoxDecoration(
          color: PColors.panel,
          border: Border.all(color: PColors.border),
          borderRadius: BorderRadius.circular(PShape.panelRadius),
        ),
        child: Center(
          child: SingleChildScrollView(
            padding: const EdgeInsets.all(32),
            child: ConstrainedBox(
              constraints: const BoxConstraints(maxWidth: 440),
              child: AnimatedSwitcher(
                duration: const Duration(milliseconds: 180),
                child: KeyedSubtree(key: ValueKey(_step), child: _body()),
              ),
            ),
          ),
        ),
      ),
    );
  }

  Widget _body() => switch (_step) {
    _Step.choose => _choose(),
    _Step.register => _registerForm(),
    _Step.link => _linkForm(),
    _Step.linkWaiting => _linkWaiting(),
  };

  Widget _choose() {
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        Center(
          child: ClipRRect(
            borderRadius: BorderRadius.circular(18),
            child: Image.asset('assets/brand/logo.png', width: 72, height: 72, filterQuality: FilterQuality.medium),
          ),
        ),
        const SizedBox(height: 20),
        const Text(
          'Welcome to Post',
          textAlign: TextAlign.center,
          style: TextStyle(fontFamily: PType.display, fontWeight: FontWeight.w600, fontSize: 26),
        ),
        const SizedBox(height: 8),
        const Text(
          'Private messaging for your community. Everything is end-to-end encrypted, and the server never learns who talks to whom.',
          textAlign: TextAlign.center,
          style: TextStyle(fontSize: 14, height: 1.5, color: PColors.textMuted),
        ),
        const SizedBox(height: 28),
        _Choice(
          icon: Icons.person_add_alt_1_outlined,
          title: 'Create an account',
          detail: 'You have an invite code from your server’s admin.',
          onTap: () => _go(_Step.register),
        ),
        const SizedBox(height: 12),
        _Choice(icon: Icons.devices_outlined, title: 'Link to my account', detail: 'You already use Post on another device.', onTap: () => _go(_Step.link)),
      ],
    );
  }

  Widget _registerForm() {
    return _Form(
      title: 'Create an account',
      onBack: _busy ? null : () => _go(_Step.choose),
      error: _error,
      action: FilledButton(onPressed: _busy ? null : _register, child: _busy ? const PSpinner() : const Text('Create account')),
      children: [
        PField(label: 'Server', controller: _server, hint: 'wss://chat.example.org/v1/ws', enabled: !_busy, error: _fieldErrors['server']),
        PField(label: 'Invite code', controller: _invite, hint: 'From your server’s admin', enabled: !_busy, mono: true, error: _fieldErrors['invite']),
        PField(
          label: 'Username',
          controller: _username,
          hint: 'e.g. sam',
          help: '3 to 32 lowercase letters, digits and _, starting with a letter. It cannot be changed later.',
          enabled: !_busy,
          error: _fieldErrors['username'],
          onSubmitted: (_) => _register(),
        ),
      ],
    );
  }

  Widget _linkForm() {
    return _Form(
      title: 'Link to my account',
      onBack: _busy ? null : () => _go(_Step.choose),
      error: _error,
      action: FilledButton(onPressed: _busy ? null : _link, child: _busy ? const PSpinner() : const Text('Get a link code')),
      children: [
        PField(label: 'Server', controller: _server, hint: 'wss://chat.example.org/v1/ws', enabled: !_busy, error: _fieldErrors['server']),
        PField(
          label: 'Device name',
          controller: _deviceName,
          help: 'Shown only on your own devices.',
          enabled: !_busy,
          error: _fieldErrors['device'],
          onSubmitted: (_) => _link(),
        ),
      ],
    );
  }

  Widget _linkWaiting() {
    final code = _code ?? '';
    return _Form(
      title: 'Enter this code on your other device',
      error: _error,
      children: [
        const Text(
          'On a device that already uses your account, choose Link a device, or run post link add with this code.',
          style: TextStyle(fontSize: 14, height: 1.5, color: PColors.textMuted),
        ),
        Container(
          padding: const EdgeInsets.symmetric(vertical: 18, horizontal: 16),
          decoration: BoxDecoration(
            color: PColors.sunken,
            border: Border.all(color: PColors.borderStrong),
            borderRadius: BorderRadius.circular(16),
          ),
          child: Row(
            children: [
              Expanded(
                child: SelectableText(
                  code,
                  textAlign: TextAlign.center,
                  style: const TextStyle(fontFamily: PType.mono, fontSize: 19, letterSpacing: 1.5, color: PColors.textStrong),
                ),
              ),
              IconButton(
                tooltip: 'Copy',
                onPressed: () => Clipboard.setData(ClipboardData(text: code)),
                icon: const Icon(Icons.copy_rounded, size: 18, color: PColors.textMuted),
              ),
            ],
          ),
        ),
        const Row(
          children: [
            PSpinner(),
            SizedBox(width: 12),
            Expanded(child: Text('Waiting for your other device. The code works for ten minutes.', style: PType.small)),
          ],
        ),
      ],
    );
  }
}

class _Choice extends StatelessWidget {
  const _Choice({required this.icon, required this.title, required this.detail, required this.onTap});

  final IconData icon;
  final String title;
  final String detail;
  final VoidCallback onTap;

  @override
  Widget build(BuildContext context) {
    return Material(
      color: PColors.panelRaised,
      shape: RoundedRectangleBorder(
        borderRadius: BorderRadius.circular(16),
        side: const BorderSide(color: PColors.borderStrong),
      ),
      clipBehavior: Clip.antiAlias,
      child: InkWell(
        onTap: onTap,
        mouseCursor: SystemMouseCursors.click,
        child: Padding(
          padding: const EdgeInsets.all(16),
          child: Row(
            children: [
              Container(
                width: 44,
                height: 44,
                decoration: BoxDecoration(color: PColors.accentSoft, borderRadius: BorderRadius.circular(14)),
                child: Icon(icon, color: PColors.accentText, size: 22),
              ),
              const SizedBox(width: 14),
              Expanded(
                child: Column(
                  crossAxisAlignment: CrossAxisAlignment.start,
                  children: [
                    Text(title, style: PType.name),
                    const SizedBox(height: 2),
                    Text(detail, style: PType.small),
                  ],
                ),
              ),
              const Icon(Icons.chevron_right_rounded, color: PColors.textFaint),
            ],
          ),
        ),
      ),
    );
  }
}

class _Form extends StatelessWidget {
  const _Form({required this.title, required this.children, this.action, this.onBack, this.error});

  final String title;
  final List<Widget> children;
  final Widget? action;
  final VoidCallback? onBack;
  final String? error;

  @override
  Widget build(BuildContext context) {
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        Row(
          children: [
            if (onBack != null) ...[
              IconButton(
                tooltip: 'Back',
                onPressed: onBack,
                icon: const Icon(Icons.arrow_back_rounded, color: PColors.textMuted),
              ),
              const SizedBox(width: 4),
            ],
            Expanded(child: Text(title, style: PType.title)),
          ],
        ),
        const SizedBox(height: 20),
        for (final c in children) ...[c, const SizedBox(height: 16)],
        if (error != null) ...[PErrorBox(error!), const SizedBox(height: 16)],
        if (action != null) Align(alignment: Alignment.centerRight, child: action!),
      ],
    );
  }
}
