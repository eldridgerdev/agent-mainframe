import 'package:flutter/material.dart';

import 'api_client.dart';
import 'credential_store.dart';
import 'models.dart';

/// Manual pairing entry: the server address and one-time code, both shown
/// on the desktop's pairing dialog (`Ctrl+Space Q`). QR-code scanning is a
/// later increment on top of this — the digits are always the fallback the
/// desktop dialog itself offers, so this alone is a complete v1 path.
class PairingScreen extends StatefulWidget {
  final void Function(DeviceCredential) onPaired;

  const PairingScreen({super.key, required this.onPaired});

  @override
  State<PairingScreen> createState() => _PairingScreenState();
}

class _PairingScreenState extends State<PairingScreen> {
  final _formKey = GlobalKey<FormState>();
  final _serverAddressController = TextEditingController();
  final _codeController = TextEditingController();
  final _deviceNameController = TextEditingController();

  bool _submitting = false;
  String? _error;

  @override
  void dispose() {
    _serverAddressController.dispose();
    _codeController.dispose();
    _deviceNameController.dispose();
    super.dispose();
  }

  Future<void> _submit() async {
    if (!_formKey.currentState!.validate()) return;

    setState(() {
      _submitting = true;
      _error = null;
    });

    final client = AmfApiClient(
      serverAddress: _serverAddressController.text.trim(),
    );
    try {
      final credential = await client.exchangePairingCode(
        code: _codeController.text.trim(),
        deviceName: _deviceNameController.text.trim(),
      );
      final store = CredentialStore();
      await store.save(credential);
      widget.onPaired(credential);
    } on PairingException catch (e) {
      setState(() => _error = e.message);
    } catch (_) {
      setState(() => _error = 'Could not reach the server');
    } finally {
      if (mounted) setState(() => _submitting = false);
    }
  }

  @override
  Widget build(BuildContext context) {
    return Scaffold(
      appBar: AppBar(title: const Text('Pair with AMF')),
      body: Padding(
        padding: const EdgeInsets.all(24),
        child: Form(
          key: _formKey,
          child: Column(
            crossAxisAlignment: CrossAxisAlignment.stretch,
            children: [
              const Text(
                'On the desktop, open the pairing dialog with '
                'Ctrl+Space Q and enter the address and code shown there.',
              ),
              const SizedBox(height: 24),
              TextFormField(
                controller: _serverAddressController,
                decoration: const InputDecoration(
                  labelText: 'Server address',
                  hintText: '192.168.1.23:54321',
                ),
                validator: (v) =>
                    (v == null || v.trim().isEmpty) ? 'Required' : null,
              ),
              const SizedBox(height: 16),
              TextFormField(
                controller: _codeController,
                decoration: const InputDecoration(
                  labelText: 'Pairing code',
                  hintText: '123456',
                ),
                keyboardType: TextInputType.number,
                maxLength: 6,
                validator: (v) => (v == null || v.trim().length != 6)
                    ? 'Enter the 6-digit code'
                    : null,
              ),
              const SizedBox(height: 16),
              TextFormField(
                controller: _deviceNameController,
                decoration: const InputDecoration(
                  labelText: 'This device\'s name',
                  hintText: 'My phone',
                ),
              ),
              const SizedBox(height: 24),
              if (_error != null)
                Padding(
                  padding: const EdgeInsets.only(bottom: 16),
                  child: Text(
                    _error!,
                    style: TextStyle(color: Theme.of(context).colorScheme.error),
                  ),
                ),
              FilledButton(
                onPressed: _submitting ? null : _submit,
                child: _submitting
                    ? const SizedBox(
                        width: 20,
                        height: 20,
                        child: CircularProgressIndicator(strokeWidth: 2),
                      )
                    : const Text('Pair'),
              ),
            ],
          ),
        ),
      ),
    );
  }
}
