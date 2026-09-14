/// Wire types for the AMF Remote Control server (`src/remote_server.rs` in
/// the main repo). Kept as a direct mirror of the Rust structs' JSON shape,
/// not a general-purpose model layer — see that file for the source of
/// truth.
class RemoteFeatureStatus {
  final String projectName;
  final String featureName;
  final String status;
  final bool needsAttention;
  final String? attentionReason;

  RemoteFeatureStatus({
    required this.projectName,
    required this.featureName,
    required this.status,
    required this.needsAttention,
    required this.attentionReason,
  });

  factory RemoteFeatureStatus.fromJson(Map<String, dynamic> json) {
    return RemoteFeatureStatus(
      projectName: json['project_name'] as String,
      featureName: json['feature_name'] as String,
      status: json['status'] as String,
      needsAttention: json['needs_attention'] as bool,
      attentionReason: json['attention_reason'] as String?,
    );
  }
}

class RemoteStatusSnapshot {
  final String generatedAt;
  final List<RemoteFeatureStatus> features;

  RemoteStatusSnapshot({required this.generatedAt, required this.features});

  factory RemoteStatusSnapshot.fromJson(Map<String, dynamic> json) {
    final rawFeatures = json['features'] as List<dynamic>? ?? [];
    return RemoteStatusSnapshot(
      generatedAt: json['generated_at'] as String? ?? '',
      features: rawFeatures
          .map((f) => RemoteFeatureStatus.fromJson(f as Map<String, dynamic>))
          .toList(),
    );
  }
}

/// A device credential minted by `POST /pair/exchange` on a successful
/// pairing exchange — everything needed to authenticate future requests.
class DeviceCredential {
  final String serverAddress;
  final String deviceId;
  final String token;

  DeviceCredential({
    required this.serverAddress,
    required this.deviceId,
    required this.token,
  });
}
