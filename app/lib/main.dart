// Postern, the app.

import 'dart:io' show Platform;

import 'package:flutter/material.dart';
import 'package:path_provider/path_provider.dart';
import 'package:postern/src/rust/api/client.dart';
import 'package:postern/src/rust/frb_generated.dart';

import 'screens/home.dart';
import 'screens/welcome.dart';
import 'state/app_state.dart';
import 'theme.dart';
import 'ui/window_frame.dart';

/// Like the CLI, `POSTERN_PROFILE` picks another profile directory and
/// `POSTERN_DB_KEY_FILE` takes its key from a file instead of the keyring:
/// for running a second account, or a test profile, beside your own.
Future<void> main() async {
  WidgetsFlutterBinding.ensureInitialized();
  await RustLib.init();
  final env = Platform.environment;
  runApp(PosternApp(profileDir: env['POSTERN_PROFILE'], keyFile: env['POSTERN_DB_KEY_FILE']));
}

class PosternApp extends StatelessWidget {
  const PosternApp({super.key, this.profileDir, this.keyFile});

  /// The profile to open; null for this user's own.
  final String? profileDir;

  /// A key file for it; null to use the platform keyring.
  final String? keyFile;

  @override
  Widget build(BuildContext context) {
    return MaterialApp(
      title: 'Postern',
      debugShowCheckedModeBanner: false,
      theme: posternTheme(),
      // The frame goes around the navigator, so dialogs, menus and toasts
      // open inside the window's rounded frame rather than over its
      // transparent margin. Transparent, so the corners show the desktop.
      builder: (context, navigator) => _OverlayHost(
        child: Scaffold(
          backgroundColor: Colors.transparent,
          body: WindowFrame(child: navigator ?? const SizedBox()),
        ),
      ),
      home: Root(profileDir: profileDir, keyFile: keyFile),
    );
  }
}

/// An overlay for what sits outside the navigator (the window frame's title
/// bar, whose buttons have tooltips). Keeps one entry and rebuilds it when
/// the child changes.
class _OverlayHost extends StatefulWidget {
  const _OverlayHost({required this.child});

  final Widget child;

  @override
  State<_OverlayHost> createState() => _OverlayHostState();
}

class _OverlayHostState extends State<_OverlayHost> {
  late final OverlayEntry _entry = OverlayEntry(builder: (_) => widget.child);

  @override
  void didUpdateWidget(_OverlayHost old) {
    super.didUpdateWidget(old);
    _entry.markNeedsBuild();
  }

  @override
  Widget build(BuildContext context) => Overlay(initialEntries: [_entry]);
}

/// Opens this device's profile, then shows the welcome screens until there
/// is an account, and the app once there is.
class Root extends StatefulWidget {
  const Root({super.key, this.profileDir, this.keyFile});

  final String? profileDir;
  final String? keyFile;

  @override
  State<Root> createState() => _RootState();
}

class _RootState extends State<Root> {
  Client? _client;
  AppState? _app;
  String? _error;
  bool _opening = true;

  @override
  void initState() {
    super.initState();
    _open();
  }

  Future<void> _open() async {
    try {
      final dir = widget.profileDir ?? '${(await getApplicationSupportDirectory()).path}/profile';
      final client = await Client.open(profileDir: dir, keyFile: widget.keyFile);
      final me = await client.identity();
      setState(() {
        _client = client;
        _opening = false;
      });
      if (me != null) _signedIn(me);
    } catch (e) {
      setState(() {
        _error = errorText(e);
        _opening = false;
      });
    }
  }

  /// From here the app stays connected for the rest of the session.
  void _signedIn(Identity me) {
    final client = _client;
    if (client == null) return;
    setState(() => _app = AppState(client, me));
  }

  @override
  void dispose() {
    _app?.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    final client = _client;
    final app = _app;
    if (_opening) return const Center(child: CircularProgressIndicator(strokeWidth: 2));
    if (client == null) return _Problem(text: 'Could not open your profile. $_error');
    if (app == null) return WelcomeScreen(client: client, onSignedIn: _signedIn);
    return HomeScreen(state: app);
  }
}

class _Problem extends StatelessWidget {
  const _Problem({required this.text});

  final String text;

  @override
  Widget build(BuildContext context) {
    return Center(
      child: Padding(
        padding: const EdgeInsets.all(32),
        child: Text(text, textAlign: TextAlign.center, style: PType.text),
      ),
    );
  }
}
