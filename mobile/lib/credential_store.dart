import 'package:shared_preferences/shared_preferences.dart';

import 'models.dart';

/// Persists the one device credential this app holds, across restarts.
/// Deliberately singular — the app pairs with one AMF server at a time; a
/// fresh pairing (or "forget device") simply overwrites/clears it.
class CredentialStore {
  static const _serverAddressKey = 'server_address';
  static const _deviceIdKey = 'device_id';
  static const _tokenKey = 'token';

  Future<DeviceCredential?> load() async {
    final prefs = await SharedPreferences.getInstance();
    final serverAddress = prefs.getString(_serverAddressKey);
    final deviceId = prefs.getString(_deviceIdKey);
    final token = prefs.getString(_tokenKey);
    if (serverAddress == null || deviceId == null || token == null) {
      return null;
    }
    return DeviceCredential(
      serverAddress: serverAddress,
      deviceId: deviceId,
      token: token,
    );
  }

  Future<void> save(DeviceCredential credential) async {
    final prefs = await SharedPreferences.getInstance();
    await prefs.setString(_serverAddressKey, credential.serverAddress);
    await prefs.setString(_deviceIdKey, credential.deviceId);
    await prefs.setString(_tokenKey, credential.token);
  }

  Future<void> clear() async {
    final prefs = await SharedPreferences.getInstance();
    await prefs.remove(_serverAddressKey);
    await prefs.remove(_deviceIdKey);
    await prefs.remove(_tokenKey);
  }
}
