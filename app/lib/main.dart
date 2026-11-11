// Postern, the app.

import 'package:flutter/material.dart';
import 'package:path_provider/path_provider.dart';
import 'package:postern/src/rust/api/client.dart';
import 'package:postern/src/rust/frb_generated.dart';

import 'screens/welcome.dart';
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
  Identity? _me;
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
        _me = me;
        _opening = false;
      });
      if (me != null) _signedIn(me);
    } catch (e) {
      setState(() {
        _error = e.toString();
        _opening = false;
      });
    }
  }

  /// Stay connected for the rest of the session.
  void _signedIn(Identity me) {
    setState(() => _me = me);
    _client?.run();
  }

  @override
  Widget build(BuildContext context) {
    final client = _client;
    final me = _me;
    if (_opening) return const Center(child: CircularProgressIndicator(strokeWidth: 2));
    if (client == null) return _Problem(text: 'Could not open your profile: $_error');
    if (me == null) return WelcomeScreen(client: client, onSignedIn: _signedIn);
    return _SignedIn(client: client, me: me);
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

/// A stand-in until the main interface lands: who you are, and your
/// conversations as they arrive.
class _SignedIn extends StatefulWidget {
  const _SignedIn({required this.client, required this.me});

  final Client client;
  final Identity me;

  @override
  State<_SignedIn> createState() => _SignedInState();
}

class _SignedInState extends State<_SignedIn> {
  List<ConversationInfo> _conversations = const [];

  @override
  void initState() {
    super.initState();
    _refresh();
    widget.client.events().listen((e) {
      if (e.kind == EventKind.conversations || e.kind == EventKind.messages) _refresh();
    });
  }

  Future<void> _refresh() async {
    final list = await widget.client.conversations();
    if (mounted) setState(() => _conversations = list);
  }

  @override
  Widget build(BuildContext context) {
    return Padding(
      padding: const EdgeInsets.fromLTRB(PShape.gap, 0, PShape.gap, PShape.gap),
      child: Container(
        width: double.infinity,
        padding: const EdgeInsets.all(28),
        decoration: BoxDecoration(
          color: PColors.panel,
          border: Border.all(color: PColors.border),
          borderRadius: BorderRadius.circular(PShape.panelRadius),
        ),
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            Text('Signed in as ${widget.me.username}', style: PType.title),
            const SizedBox(height: 6),
            Text(widget.me.server, style: PType.small),
            const SizedBox(height: 20),
            if (_conversations.isEmpty) const Text('No conversations yet.', style: PType.text),
            for (final c in _conversations) Text('${c.isGroup ? 'group' : 'DM'}  ${c.title}', style: PType.text),
          ],
        ),
      ),
    );
  }
}
