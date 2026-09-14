import 'dart:convert';

import 'package:http/http.dart' as http;

import 'models.dart';

/// The outcomes `POST /pair/exchange` can report — mirrors
/// `PairingExchangeOutcome` in `src/remote_server.rs` one for one, via the
/// HTTP status each variant maps to there.
enum PairingFailureReason { invalidCode, expired, lockedOut, serverError, unreachable }

class PairingException implements Exception {
  final PairingFailureReason reason;
  PairingException(this.reason);

  String get message => switch (reason) {
    PairingFailureReason.invalidCode => 'Invalid code',
    PairingFailureReason.expired => 'Code expired',
    PairingFailureReason.lockedOut => 'Too many attempts — get a new code on the desktop',
    PairingFailureReason.serverError => 'Server could not complete pairing',
    PairingFailureReason.unreachable => 'Could not reach the server',
  };
}

/// Thrown by [AmfApiClient.fetchStatus] when the token is missing, unknown,
/// or revoked — `require_device_auth` in `src/remote_server.rs` never
/// distinguishes the three, so neither does this.
class UnauthorizedException implements Exception {}

/// Talks to one AMF Remote Control server (`src/remote_server.rs`).
/// `serverAddress` is a bare `host:port` (as shown in the desktop pairing
/// dialog / encoded in its QR), never a full URL.
class AmfApiClient {
  final String serverAddress;
  final String? token;

  AmfApiClient({required this.serverAddress, this.token});

  Uri _uri(String path) => Uri.parse('http://$serverAddress$path');

  Future<DeviceCredential> exchangePairingCode({
    required String code,
    required String deviceName,
  }) async {
    final http.Response response;
    try {
      response = await http
          .post(
            _uri('/pair/exchange'),
            headers: {'Content-Type': 'application/json'},
            body: jsonEncode({'code': code, 'device_name': deviceName}),
          )
          .timeout(const Duration(seconds: 10));
    } catch (_) {
      throw PairingException(PairingFailureReason.unreachable);
    }

    switch (response.statusCode) {
      case 200:
        final body = jsonDecode(response.body) as Map<String, dynamic>;
        return DeviceCredential(
          serverAddress: serverAddress,
          deviceId: body['device_id'] as String,
          token: body['token'] as String,
        );
      case 401:
        throw PairingException(PairingFailureReason.invalidCode);
      case 410:
        throw PairingException(PairingFailureReason.expired);
      case 429:
        throw PairingException(PairingFailureReason.lockedOut);
      case 503:
        throw PairingException(PairingFailureReason.unreachable);
      default:
        throw PairingException(PairingFailureReason.serverError);
    }
  }

  Future<RemoteStatusSnapshot> fetchStatus() async {
    final response = await http
        .get(_uri('/status'), headers: {'Authorization': 'Bearer $token'})
        .timeout(const Duration(seconds: 10));

    if (response.statusCode == 401) {
      throw UnauthorizedException();
    }
    if (response.statusCode != 200) {
      throw Exception('Unexpected /status response: ${response.statusCode}');
    }
    return RemoteStatusSnapshot.fromJson(
      jsonDecode(response.body) as Map<String, dynamic>,
    );
  }
}
