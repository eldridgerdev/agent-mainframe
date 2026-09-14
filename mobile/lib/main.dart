import 'package:flutter/material.dart';

import 'credential_store.dart';
import 'models.dart';
import 'pairing_screen.dart';
import 'status_screen.dart';

void main() {
  runApp(const AmfCompanionApp());
}

class AmfCompanionApp extends StatelessWidget {
  const AmfCompanionApp({super.key});

  @override
  Widget build(BuildContext context) {
    return MaterialApp(
      title: 'AMF Companion',
      theme: ThemeData(colorScheme: ColorScheme.fromSeed(seedColor: Colors.teal)),
      home: const RootScreen(),
    );
  }
}

/// Decides which screen to show based on whether a device credential is
/// already stored — see `CredentialStore`. Re-checked whenever the child
/// screens call back (paired, or forgot the device), rather than each
/// screen independently owning navigation.
class RootScreen extends StatefulWidget {
  const RootScreen({super.key});

  @override
  State<RootScreen> createState() => _RootScreenState();
}

class _RootScreenState extends State<RootScreen> {
  final _store = CredentialStore();
  DeviceCredential? _credential;
  bool _loading = true;

  @override
  void initState() {
    super.initState();
    _load();
  }

  Future<void> _load() async {
    final credential = await _store.load();
    if (!mounted) return;
    setState(() {
      _credential = credential;
      _loading = false;
    });
  }

  @override
  Widget build(BuildContext context) {
    if (_loading) {
      return const Scaffold(body: Center(child: CircularProgressIndicator()));
    }
    if (_credential == null) {
      return PairingScreen(
        onPaired: (credential) => setState(() => _credential = credential),
      );
    }
    return StatusScreen(
      credential: _credential!,
      onForgetDevice: () => setState(() => _credential = null),
    );
  }
}
