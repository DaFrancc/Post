// Postern, the app.

import 'package:flutter/material.dart';
import 'package:path_provider/path_provider.dart';
import 'package:postern/src/rust/api/client.dart';
import 'package:postern/src/rust/frb_generated.dart';

import 'screens/home.dart';
import 'screens/welcome.dart';
import 'state/app_state.dart';
import 'theme.dart';
import 'ui/window_frame.dart';

Future<void> main() async {
  WidgetsFlutterBinding.ensureInitialized();
  await RustLib.init();
  runApp(const PosternApp());
}

class PosternApp extends StatelessWidget {
  const PosternApp({super.key});

  @override
  Widget build(BuildContext context) {
    return MaterialApp(
      title: 'Postern',
      debugShowCheckedModeBanner: false,
      theme: posternTheme(),
      // Transparent, so the window frame's rounded corners show the desktop.
      home: const Scaffold(backgroundColor: Colors.transparent, body: WindowFrame(child: Root())),
    );
  }
}

/// Opens this device's profile, then shows the welcome screens until there
/// is an account, and the app once there is.
class Root extends StatefulWidget {
  const Root({super.key});

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
      final dir = await getApplicationSupportDirectory();
      final client = await Client.open(profileDir: '${dir.path}/profile');
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
      child: Padding(padding: const EdgeInsets.all(32), child: Text(text, textAlign: TextAlign.center, style: PType.text)),
    );
  }
}

