//! Integrity of locally retained server-issued offline policy, not a Microsoft
//! signature or a substitute for the lease/ownership checks. The existing
//! device-registration secret remains in the OS keyring, never in the cache.
use base64_23::{Engine, prelude::BASE64_STANDARD};
use hmac::{Hmac, KeyInit, Mac};
use sha2_11::Sha256;

const DOMAIN: &[u8] = b"nexus-offline-license-cache-v4\0";

pub fn seal(secret: &[u8], data: &[u8]) -> Option<String> {
    if secret.is_empty() {
        return None;
    }
    let mut mac = Hmac::<Sha256>::new_from_slice(secret).ok()?;
    mac.update(DOMAIN);
    mac.update(data);
    Some(BASE64_STANDARD.encode(mac.finalize().into_bytes()))
}

pub fn verify(secret: &[u8], data: &[u8], tag: &str) -> bool {
    if secret.is_empty() || tag.len() != 44 {
        return false;
    }
    let Ok(tag) = BASE64_STANDARD.decode(tag) else {
        return false;
    };
    let Ok(mut mac) = Hmac::<Sha256>::new_from_slice(secret) else {
        return false;
    };
    mac.update(DOMAIN);
    mac.update(data);
    mac.verify_slice(&tag).is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn offline_integrity_requires_the_keyring_secret_and_exact_payload() {
        let tag = seal(b"synthetic private secret", b"server policy").unwrap();
        assert!(verify(b"synthetic private secret", b"server policy", &tag));
        assert!(!verify(b"other registration", b"server policy", &tag));
        assert!(!verify(
            b"synthetic private secret",
            b"changed policy",
            &tag
        ));
        assert!(!verify(b"", b"server policy", &tag));
        assert!(seal(b"", b"server policy").is_none());
        assert!(!verify(
            b"synthetic private secret",
            b"server policy",
            "invalid"
        ));
    }
}
