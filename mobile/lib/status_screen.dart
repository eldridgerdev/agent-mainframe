import 'dart:async';

import 'package:flutter/material.dart';

import 'api_client.dart';
import 'credential_store.dart';
import 'models.dart';

/// Phase 1: a read-only, polling view of every feature's status — the same
/// information the desktop's attention (`i`) view surfaces. Polling rather
/// than a WebSocket, matching the server's own `/status` (Epic 5), which is
/// plain GET, not a push transport.
class StatusScreen extends StatefulWidget {
  final DeviceCredential credential;
  final VoidCallback onForgetDevice;

  const StatusScreen({
    super.key,
    required this.credential,
    required this.onForgetDevice,
  });

  @override
  State<StatusScreen> createState() => _StatusScreenState();
}

class _StatusScreenState extends State<StatusScreen> {
  static const _pollInterval = Duration(seconds: 5);

  late final AmfApiClient _client;
  Timer? _timer;
  RemoteStatusSnapshot? _snapshot;
  String? _error;
  bool _loading = true;

  @override
  void initState() {
    super.initState();
    _client = AmfApiClient(
      serverAddress: widget.credential.serverAddress,
      token: widget.credential.token,
    );
    _refresh();
    _timer = Timer.periodic(_pollInterval, (_) => _refresh());
  }

  @override
  void dispose() {
    _timer?.cancel();
    super.dispose();
  }

  Future<void> _refresh() async {
    try {
      final snapshot = await _client.fetchStatus();
      if (!mounted) return;
      setState(() {
        _snapshot = snapshot;
        _error = null;
        _loading = false;
      });
    } on UnauthorizedException {
      if (!mounted) return;
      // The token no longer works — revoked on the desktop, most likely.
      // Nothing to retry: forget the credential and send the user back to
      // pairing rather than polling a 401 forever.
      await CredentialStore().clear();
      widget.onForgetDevice();
    } catch (e) {
      if (!mounted) return;
      setState(() {
        _error = 'Could not reach the server';
        _loading = false;
      });
    }
  }

  Future<void> _forgetDevice() async {
    await CredentialStore().clear();
    widget.onForgetDevice();
  }

  @override
  Widget build(BuildContext context) {
    return Scaffold(
      appBar: AppBar(
        title: const Text('Feature status'),
        actions: [
          IconButton(
            icon: const Icon(Icons.link_off),
            tooltip: 'Forget device',
            onPressed: _forgetDevice,
          ),
        ],
      ),
      body: RefreshIndicator(
        onRefresh: _refresh,
        child: _buildBody(),
      ),
    );
  }

  Widget _buildBody() {
    if (_loading) {
      return const Center(child: CircularProgressIndicator());
    }
    if (_error != null && _snapshot == null) {
      return Center(child: Text(_error!));
    }
    final features = _snapshot?.features ?? [];
    if (features.isEmpty) {
      return ListView(
        children: const [
          Padding(
            padding: EdgeInsets.all(32),
            child: Center(child: Text('No features yet')),
          ),
        ],
      );
    }
    return ListView.separated(
      itemCount: features.length,
      separatorBuilder: (_, _) => const Divider(height: 1),
      itemBuilder: (context, i) {
        final f = features[i];
        return ListTile(
          leading: Icon(
            f.needsAttention ? Icons.notifications_active : Icons.circle,
            color: f.needsAttention
                ? Colors.orange
                : Theme.of(context).colorScheme.outline,
          ),
          title: Text(f.featureName),
          subtitle: Text('${f.projectName} · ${f.status}'),
          trailing: f.needsAttention && f.attentionReason != null
              ? Chip(label: Text(f.attentionReason!))
              : null,
        );
      },
    );
  }
}
