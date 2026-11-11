// Postern, the app. For now a check that the pieces connect: the Rust core
// loads, opens this device's profile with its key from the keyring, and
// reports the account and conversations. The real interface replaces this.

import 'package:flutter/material.dart';
import 'package:path_provider/path_provider.dart';
import 'package:postern/src/rust/api/client.dart';
import 'package:postern/src/rust/frb_generated.dart';

Future<void> main() async {
  await RustLib.init();
  runApp(const PosternApp());
}

class PosternApp extends StatelessWidget {
  const PosternApp({super.key});

  @override
  Widget build(BuildContext context) {
    return MaterialApp(
      title: 'Postern',
      theme: ThemeData(
        brightness: Brightness.dark,
        colorScheme: ColorScheme.fromSeed(seedColor: const Color(0xFF6E8BFF), brightness: Brightness.dark),
        scaffoldBackgroundColor: const Color(0xFF06070C),
      ),
      home: const BridgeCheck(),
    );
  }
}

class BridgeCheck extends StatefulWidget {
  const BridgeCheck({super.key});

  @override
  State<BridgeCheck> createState() => _BridgeCheckState();
}

class _BridgeCheckState extends State<BridgeCheck> {
  String _status = 'Opening your profile…';
  List<ConversationInfo> _conversations = const [];

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
      final conversations = await client.conversations();
      setState(() {
        _status = me == null
            ? 'Profile open at ${dir.path}/profile. No account on this device yet.'
            : 'Signed in as ${me.username} on ${me.server}.';
        _conversations = conversations;
      });
    } catch (e) {
      setState(() => _status = 'Could not open the profile: $e');
    }
  }

  @override
  Widget build(BuildContext context) {
    return Scaffold(
      body: Padding(
        padding: const EdgeInsets.all(32),
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            Text('Postern', style: Theme.of(context).textTheme.headlineMedium),
            const SizedBox(height: 16),
            Text(_status),
            const SizedBox(height: 16),
            for (final c in _conversations) Text('${c.isGroup ? 'group' : 'DM'}  ${c.title}'),
          ],
        ),
      ),
    );
  }
}
