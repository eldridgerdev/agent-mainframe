//! Web Push delivery for the Remote Control PWA (RFC 8030 + VAPID, RFC 8292).
//!
//! AMF sends straight to each browser vendor's push service (FCM for
//! Chrome, Mozilla's autopush, Apple's for Safari) — the subscription's
//! endpoint says which. No AMF-run relay and no Firebase project: VAPID
//! identifies AMF to the push service with a key pair of its own, and each
//! payload is end-to-end encrypted to the subscribing browser's keys, so the
//! push service carries it without being able to read it.
//!
//! Nothing here touches `App` or the database. Building a request is pure;
//! [`send_all`] does the network I/O on a thread of its own and reports each
//! outcome back over a channel, so the main loop never blocks on a push
//! service and stays the only writer when a report says a subscription is
//! gone.

use std::sync::mpsc::Sender;
use std::time::Duration;

use anyhow::{Context, Result, anyhow};
use base64ct::{Base64UrlUnpadded, Encoding};
use p256::PublicKey;
use p256::ecdsa::signature::Signer;
use p256::ecdsa::{Signature, SigningKey};
use serde::Serialize;
use web_push_native::{Auth, WebPushBuilder};

use crate::db::remote_push::PushSubscription;

/// The VAPID `sub` claim — who a push service should contact about abuse.
/// Apple's push service rejects a push without one.
const VAPID_CONTACT: &str = "https://agentmainframe.dev";

/// How long a push service should hold an undelivered notification. An
/// attention alert an hour old has usually been dealt with at the desk.
const PUSH_TTL: Duration = Duration::from_secs(60 * 60);

/// AMF's VAPID signing key. One per database (`remote_push_vapid`): every
/// subscription is bound to its public half, so replacing it would orphan
/// them all.
pub struct VapidKey {
    signing: SigningKey,
}

/// How long a VAPID token stays valid. RFC 8292 caps it at 24 hours.
const VAPID_TOKEN_LIFETIME: Duration = Duration::from_secs(12 * 60 * 60);

impl VapidKey {
    /// A fresh P-256 key. The scalar comes from two v4 UUIDs — the same
    /// `getrandom`-backed source `remote_server::generate_device_token`
    /// uses — rather than a new RNG dependency; the loop only repeats in
    /// the astronomically unlikely case the bytes aren't a valid scalar.
    pub fn generate() -> Self {
        loop {
            let mut bytes = [0u8; 32];
            bytes[..16].copy_from_slice(uuid::Uuid::new_v4().as_bytes());
            bytes[16..].copy_from_slice(uuid::Uuid::new_v4().as_bytes());
            if let Ok(signing) = SigningKey::from_slice(&bytes) {
                return Self { signing };
            }
        }
    }

    /// Restore a key stored by [`Self::private_key_b64`].
    pub fn from_private_key_b64(encoded: &str) -> Result<Self> {
        let raw = Base64UrlUnpadded::decode_vec(encoded)
            .map_err(|e| anyhow!("VAPID key is not base64url: {e}"))?;
        let signing =
            SigningKey::from_slice(&raw).map_err(|e| anyhow!("invalid VAPID key: {e}"))?;
        Ok(Self { signing })
    }

    pub fn private_key_b64(&self) -> String {
        Base64UrlUnpadded::encode_string(&self.signing.to_bytes())
    }

    fn public_key_bytes(&self) -> Vec<u8> {
        self.signing
            .verifying_key()
            .to_encoded_point(false)
            .as_bytes()
            .to_vec()
    }

    /// The uncompressed public point, base64url — exactly what the browser's
    /// `pushManager.subscribe({ applicationServerKey })` expects.
    pub fn public_key_b64(&self) -> String {
        Base64UrlUnpadded::encode_string(&self.public_key_bytes())
    }

    /// The `Authorization` header for a push to `endpoint` (RFC 8292 §3):
    /// an ES256 JWT naming the push service's origin, plus the public key.
    fn authorization(&self, endpoint: &ureq::http::Uri) -> Result<String> {
        let scheme = endpoint
            .scheme_str()
            .context("push endpoint has no scheme")?;
        let authority = endpoint.authority().context("push endpoint has no host")?;
        let expires = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .saturating_add(VAPID_TOKEN_LIFETIME)
            .as_secs();
        let encode = |value: serde_json::Value| {
            Base64UrlUnpadded::encode_string(value.to_string().as_bytes())
        };
        let signed = format!(
            "{}.{}",
            encode(serde_json::json!({"typ": "JWT", "alg": "ES256"})),
            encode(serde_json::json!({
                "aud": format!("{scheme}://{authority}"),
                "exp": expires,
                "sub": VAPID_CONTACT,
            })),
        );
        let signature: Signature = self.signing.sign(signed.as_bytes());
        Ok(format!(
            "vapid t={signed}.{}, k={}",
            Base64UrlUnpadded::encode_string(&signature.to_bytes()),
            self.public_key_b64()
        ))
    }
}

/// The notification payload the PWA's service worker renders (`sw.js`).
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct PushMessage {
    pub title: String,
    pub body: String,
    /// Notifications sharing a tag replace each other, so one feature
    /// raising attention twice shows one notification, not a stack.
    pub tag: String,
    /// Where tapping the notification opens the PWA (a same-origin path).
    pub url: String,
}

/// Check a subscription's keys before storing it, so a malformed one is
/// refused at subscribe time rather than failing every later push.
pub fn validate_subscription_keys(p256dh: &str, auth: &str) -> Result<()> {
    decode_keys(p256dh, auth).map(|_| ())
}

fn decode_keys(p256dh: &str, auth: &str) -> Result<(PublicKey, Auth)> {
    let public = Base64UrlUnpadded::decode_vec(p256dh)
        .map_err(|e| anyhow!("p256dh is not base64url: {e}"))?;
    let public = PublicKey::from_sec1_bytes(&public).context("p256dh is not a P-256 point")?;
    let auth =
        Base64UrlUnpadded::decode_vec(auth).map_err(|e| anyhow!("auth is not base64url: {e}"))?;
    if auth.len() != 16 {
        return Err(anyhow!("auth secret must be 16 bytes, got {}", auth.len()));
    }
    Ok((public, Auth::clone_from_slice(&auth)))
}

/// Encrypt `message` for one subscription and sign it with `key`.
pub fn build_request(
    key: &VapidKey,
    subscription: &PushSubscription,
    message: &PushMessage,
) -> Result<ureq::http::Request<Vec<u8>>> {
    let (public, auth) = decode_keys(&subscription.p256dh, &subscription.auth)?;
    let endpoint: ureq::http::Uri = subscription
        .endpoint
        .parse()
        .context("push endpoint is not a URI")?;
    let body = serde_json::to_vec(message)?;
    let authorization = key.authorization(&endpoint)?;
    let mut request = WebPushBuilder::new(endpoint, public, auth)
        .with_valid_duration(PUSH_TTL)
        .build(body)
        .map_err(|e| anyhow!("failed to build push request: {e}"))?;
    request.headers_mut().insert(
        ureq::http::header::AUTHORIZATION,
        ureq::http::HeaderValue::from_str(&authorization)?,
    );
    Ok(request)
}

/// What happened to one push.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeliveryOutcome {
    Delivered,
    /// The push service says this subscription no longer exists (404/410) —
    /// the browser unsubscribed, or the site's data was cleared. Its row
    /// should be deleted.
    Gone,
    Failed(String),
}

#[derive(Debug, Clone)]
pub struct DeliveryReport {
    pub endpoint: String,
    pub device_id: String,
    pub outcome: DeliveryOutcome,
}

/// Send every request on a background thread, reporting each outcome on
/// `reports`. Returns immediately.
pub fn send_all(
    requests: Vec<(PushSubscription, ureq::http::Request<Vec<u8>>)>,
    reports: Sender<DeliveryReport>,
) {
    if requests.is_empty() {
        return;
    }
    // A failed spawn (thread limit) drops this batch; the next attention
    // change sends again, which is all a notification is worth.
    let _ = std::thread::Builder::new()
        .name("amf-remote-push".into())
        .spawn(move || {
            let agent = crate::http_client::https_agent();
            for (subscription, request) in requests {
                let outcome = match agent.run(request) {
                    Ok(_) => DeliveryOutcome::Delivered,
                    Err(ureq::Error::StatusCode(404 | 410)) => DeliveryOutcome::Gone,
                    Err(e) => DeliveryOutcome::Failed(e.to_string()),
                };
                let _ = reports.send(DeliveryReport {
                    endpoint: subscription.endpoint,
                    device_id: subscription.device_id,
                    outcome,
                });
            }
        });
}

#[cfg(test)]
mod tests {
    use super::*;
    use p256::SecretKey;
    use p256::ecdsa::VerifyingKey;
    use p256::ecdsa::signature::Verifier;
    use p256::elliptic_curve::sec1::ToEncodedPoint;

    /// A browser-side key pair, standing in for what `pushManager.subscribe`
    /// would generate.
    fn browser_subscription() -> (SecretKey, [u8; 16], PushSubscription) {
        let secret = SecretKey::from_slice(&VapidKey::generate().signing.to_bytes()).unwrap();
        let public = secret.public_key().to_encoded_point(false);
        let auth = [7u8; 16];
        let subscription = PushSubscription {
            endpoint: "https://push.example.com/send/abc".to_string(),
            device_id: "dev-1".to_string(),
            p256dh: Base64UrlUnpadded::encode_string(public.as_bytes()),
            auth: Base64UrlUnpadded::encode_string(&auth),
        };
        (secret, auth, subscription)
    }

    #[test]
    fn vapid_key_round_trips_through_its_stored_form() {
        let key = VapidKey::generate();
        let restored = VapidKey::from_private_key_b64(&key.private_key_b64()).unwrap();
        assert_eq!(restored.public_key_b64(), key.public_key_b64());
        // Uncompressed P-256 point: 65 bytes → 87 base64url chars.
        assert_eq!(key.public_key_b64().len(), 87);
    }

    #[test]
    fn built_request_is_signed_and_decrypts_to_the_message() {
        let key = VapidKey::generate();
        let (secret, auth, subscription) = browser_subscription();
        let message = PushMessage {
            title: "my-feature needs attention".to_string(),
            body: "my-project · Question".to_string(),
            tag: "amf-my-feature".to_string(),
            url: "/#/f/feature-1".to_string(),
        };

        let request = build_request(&key, &subscription, &message).unwrap();

        assert_eq!(request.uri(), "https://push.example.com/send/abc");
        let authorization = request.headers()["authorization"].to_str().unwrap();
        let (token, public) = authorization
            .strip_prefix("vapid t=")
            .and_then(|rest| rest.split_once(", k="))
            .unwrap();
        assert_eq!(public, key.public_key_b64());

        // The JWT names the push service's origin and verifies against the
        // advertised key — what the push service itself checks.
        let (signed, signature) = token.rsplit_once('.').unwrap();
        let claims = signed.split('.').nth(1).unwrap();
        let claims: serde_json::Value =
            serde_json::from_slice(&Base64UrlUnpadded::decode_vec(claims).unwrap()).unwrap();
        assert_eq!(claims["aud"], "https://push.example.com");
        assert_eq!(claims["sub"], VAPID_CONTACT);
        let verifying =
            VerifyingKey::from_sec1_bytes(&Base64UrlUnpadded::decode_vec(public).unwrap()).unwrap();
        let signature =
            Signature::from_slice(&Base64UrlUnpadded::decode_vec(signature).unwrap()).unwrap();
        verifying.verify(signed.as_bytes(), &signature).unwrap();

        let plaintext = web_push_native::decrypt(
            request.body().clone(),
            &secret,
            &Auth::clone_from_slice(&auth),
        )
        .unwrap();
        let decoded: serde_json::Value = serde_json::from_slice(&plaintext).unwrap();
        assert_eq!(decoded["title"], "my-feature needs attention");
        assert_eq!(decoded["tag"], "amf-my-feature");
    }

    #[test]
    fn malformed_subscription_keys_are_rejected() {
        let (_, _, subscription) = browser_subscription();
        assert!(validate_subscription_keys(&subscription.p256dh, &subscription.auth).is_ok());
        assert!(validate_subscription_keys("not-a-key", &subscription.auth).is_err());
        assert!(validate_subscription_keys(&subscription.p256dh, "c2hvcnQ").is_err());
    }
}
