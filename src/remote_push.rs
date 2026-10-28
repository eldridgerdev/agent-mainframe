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
use p256::ecdh::diffie_hellman;
use p256::ecdsa::signature::Signer;
use p256::ecdsa::{Signature, SigningKey};
use p256::elliptic_curve::sec1::ToSec1Point;
use p256::{PublicKey, SecretKey};
use serde::Serialize;

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

/// `N` bytes from the operating system's CSPRNG.
fn random_bytes<const N: usize>() -> [u8; N] {
    use aes_gcm::aead::OsRng;
    use aes_gcm::aead::rand_core::RngCore;
    let mut bytes = [0u8; N];
    OsRng.fill_bytes(&mut bytes);
    bytes
}

/// A fresh P-256 scalar; the loop only repeats in the astronomically
/// unlikely case the bytes aren't a valid one.
fn random_secret_key() -> SecretKey {
    loop {
        if let Ok(secret) = SecretKey::from_slice(&random_bytes::<32>()) {
            return secret;
        }
    }
}

/// Record size announced in the aes128gcm header. The payload always fits
/// one record (`MAX_PLAINTEXT`), so this only has to be at least that large.
const RECORD_SIZE: u32 = 4096;

/// Longest plaintext that fits one record: the record minus the GCM tag
/// (16) and the padding delimiter (1). Push services accept 4096 bytes of
/// body; the header (86) takes some of that, so the cap is conservative.
const MAX_PLAINTEXT: usize = 3993;

/// RFC 8291 `aes128gcm` encryption of `plaintext` to a browser subscription.
/// `ephemeral` and `salt` are inputs so tests can pin the RFC's vectors.
fn encrypt_aes128gcm(
    plaintext: &[u8],
    ua_public: &PublicKey,
    auth_secret: &[u8],
    ephemeral: &SecretKey,
    salt: &[u8; 16],
) -> Result<Vec<u8>> {
    use aes_gcm::aead::{Aead, KeyInit};
    use aes_gcm::{Aes128Gcm, Nonce};
    use hkdf::Hkdf;
    use sha2::Sha256;

    if plaintext.len() > MAX_PLAINTEXT {
        return Err(anyhow!(
            "push payload is {} bytes; limit is {MAX_PLAINTEXT}",
            plaintext.len()
        ));
    }
    let as_public = ephemeral.public_key().to_sec1_point(false);
    let ua_point = ua_public.to_sec1_point(false);
    let shared = diffie_hellman(ephemeral.to_nonzero_scalar(), ua_public.as_affine());

    // RFC 8291 §3.4: mix the auth secret and both public keys into the IKM.
    let mut key_info = b"WebPush: info\0".to_vec();
    key_info.extend_from_slice(ua_point.as_bytes());
    key_info.extend_from_slice(as_public.as_bytes());
    let mut ikm = [0u8; 32];
    Hkdf::<Sha256>::new(Some(auth_secret), shared.raw_secret_bytes().as_slice())
        .expand(&key_info, &mut ikm)
        .map_err(|e| anyhow!("hkdf expand failed: {e}"))?;

    // RFC 8188 §2.2-2.3: content-encryption key and nonce from the salt.
    let prk = Hkdf::<Sha256>::new(Some(salt), &ikm);
    let mut cek = [0u8; 16];
    let mut nonce = [0u8; 12];
    prk.expand(b"Content-Encoding: aes128gcm\0", &mut cek)
        .map_err(|e| anyhow!("hkdf expand failed: {e}"))?;
    prk.expand(b"Content-Encoding: nonce\0", &mut nonce)
        .map_err(|e| anyhow!("hkdf expand failed: {e}"))?;

    // 0x02 marks the final (here the only) record.
    let mut record = plaintext.to_vec();
    record.push(2);
    let ciphertext = Aes128Gcm::new(&cek.into())
        .encrypt(Nonce::from_slice(&nonce), record.as_slice())
        .map_err(|e| anyhow!("push encryption failed: {e}"))?;

    let mut body = Vec::with_capacity(21 + as_public.len() + ciphertext.len());
    body.extend_from_slice(salt);
    body.extend_from_slice(&RECORD_SIZE.to_be_bytes());
    body.push(as_public.len() as u8);
    body.extend_from_slice(as_public.as_bytes());
    body.extend_from_slice(&ciphertext);
    Ok(body)
}

/// How long a VAPID token stays valid. RFC 8292 caps it at 24 hours.
const VAPID_TOKEN_LIFETIME: Duration = Duration::from_secs(12 * 60 * 60);

impl VapidKey {
    /// A fresh P-256 key.
    pub fn generate() -> Self {
        Self {
            signing: SigningKey::from(random_secret_key()),
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
            .to_sec1_point(false)
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

fn decode_keys(p256dh: &str, auth: &str) -> Result<(PublicKey, Vec<u8>)> {
    let public = Base64UrlUnpadded::decode_vec(p256dh)
        .map_err(|e| anyhow!("p256dh is not base64url: {e}"))?;
    let public = PublicKey::from_sec1_bytes(&public).context("p256dh is not a P-256 point")?;
    let auth =
        Base64UrlUnpadded::decode_vec(auth).map_err(|e| anyhow!("auth is not base64url: {e}"))?;
    if auth.len() != 16 {
        return Err(anyhow!("auth secret must be 16 bytes, got {}", auth.len()));
    }
    Ok((public, auth))
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
    let body = encrypt_aes128gcm(
        &body,
        &public,
        &auth,
        &random_secret_key(),
        &random_bytes::<16>(),
    )?;
    ureq::http::Request::post(endpoint)
        .header(ureq::http::header::CONTENT_ENCODING, "aes128gcm")
        .header(ureq::http::header::CONTENT_TYPE, "application/octet-stream")
        .header("TTL", PUSH_TTL.as_secs().to_string())
        .header(ureq::http::header::AUTHORIZATION, authorization)
        .body(body)
        .context("failed to build push request")
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
    use p256::elliptic_curve::sec1::ToSec1Point;

    /// A browser-side key pair, standing in for what `pushManager.subscribe`
    /// would generate.
    fn browser_subscription() -> (SecretKey, [u8; 16], PushSubscription) {
        let secret = SecretKey::from_slice(&VapidKey::generate().signing.to_bytes()).unwrap();
        let public = secret.public_key().to_sec1_point(false);
        let auth = [7u8; 16];
        let subscription = PushSubscription {
            endpoint: "https://push.example.com/send/abc".to_string(),
            device_id: "dev-1".to_string(),
            p256dh: Base64UrlUnpadded::encode_string(public.as_bytes()),
            auth: Base64UrlUnpadded::encode_string(&auth),
        };
        (secret, auth, subscription)
    }

    /// The user-agent side of RFC 8291, to check what `build_request` sends.
    fn decrypt_aes128gcm(body: &[u8], ua_secret: &SecretKey, auth: &[u8]) -> Vec<u8> {
        use aes_gcm::aead::{Aead, KeyInit};
        use aes_gcm::{Aes128Gcm, Nonce};
        use hkdf::Hkdf;
        use sha2::Sha256;

        let salt = &body[..16];
        let id_len = body[20] as usize;
        let as_public = &body[21..21 + id_len];
        let ciphertext = &body[21 + id_len..];
        let shared = diffie_hellman(
            ua_secret.to_nonzero_scalar(),
            PublicKey::from_sec1_bytes(as_public).unwrap().as_affine(),
        );
        let mut key_info = b"WebPush: info\0".to_vec();
        key_info.extend_from_slice(ua_secret.public_key().to_sec1_point(false).as_bytes());
        key_info.extend_from_slice(as_public);
        let mut ikm = [0u8; 32];
        Hkdf::<Sha256>::new(Some(auth), shared.raw_secret_bytes().as_slice())
            .expand(&key_info, &mut ikm)
            .unwrap();
        let prk = Hkdf::<Sha256>::new(Some(salt), &ikm);
        let (mut cek, mut nonce) = ([0u8; 16], [0u8; 12]);
        prk.expand(b"Content-Encoding: aes128gcm\0", &mut cek)
            .unwrap();
        prk.expand(b"Content-Encoding: nonce\0", &mut nonce)
            .unwrap();
        let mut record = Aes128Gcm::new(&cek.into())
            .decrypt(Nonce::from_slice(&nonce), ciphertext)
            .unwrap();
        assert_eq!(record.pop(), Some(2), "final-record delimiter");
        record
    }

    /// The worked example in RFC 8291 Appendix A.
    #[test]
    fn encryption_matches_the_rfc_8291_vector() {
        let b64 = |s: &str| Base64UrlUnpadded::decode_vec(s).unwrap();
        let as_secret =
            SecretKey::from_slice(&b64("yfWPiYE-n46HLnH0KqZOF1fJJU3MYrct3AELtAQ-oRw")).unwrap();
        let ua_public = PublicKey::from_sec1_bytes(&b64(
            "BCVxsr7N_eNgVRqvHtD0zTZsEc6-VV-JvLexhqUzORcxaOzi6-AYWXvTBHm4bjyPjs7Vd8pZGH6SRpkNtoIAiw4",
        ))
        .unwrap();
        let salt: [u8; 16] = b64("DGv6ra1nlYgDCS1FRnbzlw").try_into().unwrap();
        let body = encrypt_aes128gcm(
            b"When I grow up, I want to be a watermelon",
            &ua_public,
            &b64("BTBZMqHH6r4Tts7J_aSIgg"),
            &as_secret,
            &salt,
        )
        .unwrap();
        assert_eq!(
            Base64UrlUnpadded::encode_string(&body),
            "DGv6ra1nlYgDCS1FRnbzlwAAEABBBP4z9KsN6nGRTbVYI_c7VJSPQTBtkgcy27mlmlMoZIIgDll6e3vCYLocInmYWAmS6TlzAC8wEqKK6PBru3jl7A_yl95bQpu6cVPTpK4Mqgkf1CXztLVBSt2Ks3oZwbuwXPXLWyouBWLVWGNWQexSgSxsj_Qulcy4a-fN"
        );
    }

    #[test]
    fn oversized_payload_is_refused() {
        let (secret, _, _) = browser_subscription();
        let result = encrypt_aes128gcm(
            &vec![0u8; MAX_PLAINTEXT + 1],
            &secret.public_key(),
            &[7u8; 16],
            &random_secret_key(),
            &[0u8; 16],
        );
        assert!(result.is_err());
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

        assert_eq!(request.headers()["content-encoding"], "aes128gcm");
        assert_eq!(request.headers()["ttl"], "3600");
        let plaintext = decrypt_aes128gcm(request.body(), &secret, &auth);
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
