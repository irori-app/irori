//! Sunshine on another computer (`iroriSunshine.address`): where its web UI
//! is, and how its certificate is trusted.
//!
//! Sunshine makes its own self-signed certificate, which no authority vouches
//! for. On this computer nothing is checked: whatever answers on the loopback
//! address is already here. Across the network Irori trusts the certificate
//! on first use: the first connection that works records its SHA-256
//! fingerprint, and a different certificate later is refused inside the TLS
//! handshake, before any request (and so before the password) is sent. The
//! user then accepts the new fingerprint or finds out why it changed.

use std::sync::Arc;

use anyhow::{Result, bail};
use reqwest::Url;
use rustls::DigitallySignedStruct;
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::crypto::{CryptoProvider, verify_tls12_signature, verify_tls13_signature};
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use sha2::{Digest as _, Sha256};

use super::detect;

/// Sunshine's web UI on some computer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Address {
    /// A name or address, IPv6 without brackets.
    pub host: String,
    pub port: u16,
}

impl Address {
    /// The web UI's address.
    pub fn base(&self) -> String {
        if self.host.contains(':') {
            format!("https://[{}]:{}", self.host, self.port)
        } else {
            format!("https://{}:{}", self.host, self.port)
        }
    }

    /// `host:port`, which certificates are pinned by.
    pub fn key(&self) -> String {
        self.base().trim_start_matches("https://").to_string()
    }

    /// This computer, where no certificate is checked.
    pub fn is_loopback(&self) -> bool {
        let host = self.host.to_ascii_lowercase();
        host == "localhost"
            || host.ends_with(".localhost")
            || host
                .parse::<std::net::IpAddr>()
                .is_ok_and(|ip| ip.is_loopback())
    }
}

/// Read the address the user typed: `192.168.1.20`, `htpc.local:47990`,
/// `https://[fe80::1]:47990/`. Empty means this computer (`None`). The port
/// defaults to Sunshine's web UI port; a plain `http://` address is refused,
/// since the password would cross the network readable.
pub fn parse_address(text: &str) -> Result<Option<Address>> {
    let text = text.trim();
    if text.is_empty() {
        return Ok(None);
    }
    let with_scheme = if text.contains("://") {
        text.to_string()
    } else {
        format!("https://{text}")
    };
    let Ok(url) = Url::parse(&with_scheme) else {
        bail!("“{text}” isn't an address Irori can read");
    };
    if url.scheme() != "https" {
        bail!("Sunshine's web UI uses HTTPS: write the address without “http://”");
    }
    let Some(host) = url.host_str().filter(|h| !h.is_empty()) else {
        bail!("“{text}” names no computer");
    };
    if url.username() != "" || url.password().is_some() {
        bail!("Leave the user name out of the address: Irori asks for it below");
    }
    Ok(Some(Address {
        host: host
            .trim_start_matches('[')
            .trim_end_matches(']')
            .to_string(),
        // `Url` drops the scheme's own port (443), so it counts as unset.
        port: url.port().unwrap_or(detect::web_port("")),
    }))
}

/// A certificate's SHA-256 fingerprint as people compare it:
/// `AB:CD:…`, 32 pairs.
pub fn fingerprint(der: &[u8]) -> String {
    Sha256::digest(der)
        .iter()
        .map(|b| format!("{b:02X}"))
        .collect::<Vec<_>>()
        .join(":")
}

/// What the certificate check saw on the last handshake.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Seen {
    pub fingerprint: Option<String>,
    /// The certificate differed from the pinned one.
    pub changed: bool,
}

/// Accept the server's certificate when it is the pinned one, or any on
/// first use (no pin yet); the handshake's signatures are still checked
/// against that certificate, so only its identity is the pin.
#[derive(Debug)]
pub struct PinVerifier {
    pin: Option<String>,
    seen: parking_lot::Mutex<Seen>,
    provider: Arc<CryptoProvider>,
}

impl PinVerifier {
    pub fn new(pin: Option<String>) -> Arc<Self> {
        Arc::new(Self {
            pin,
            seen: parking_lot::Mutex::new(Seen::default()),
            provider: Arc::new(rustls::crypto::ring::default_provider()),
        })
    }

    /// The last certificate the handshake met.
    pub fn seen(&self) -> Seen {
        self.seen.lock().clone()
    }

    /// The pin decision alone, apart from rustls' types.
    fn check(&self, der: &[u8]) -> bool {
        let fingerprint = fingerprint(der);
        let changed = self.pin.as_ref().is_some_and(|pin| *pin != fingerprint);
        *self.seen.lock() = Seen {
            fingerprint: Some(fingerprint),
            changed,
        };
        !changed
    }

    /// A TLS configuration that checks certificates with this pin.
    pub fn tls_config(self: &Arc<Self>) -> Result<rustls::ClientConfig> {
        Ok(
            rustls::ClientConfig::builder_with_provider(self.provider.clone())
                .with_safe_default_protocol_versions()?
                .dangerous()
                .with_custom_certificate_verifier(self.clone())
                .with_no_client_auth(),
        )
    }
}

impl ServerCertVerifier for PinVerifier {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        if self.check(end_entity.as_ref()) {
            Ok(ServerCertVerified::assertion())
        } else {
            Err(rustls::Error::General(
                "the certificate isn't the one Irori trusted".into(),
            ))
        }
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        verify_tls12_signature(
            message,
            cert,
            dss,
            &self.provider.signature_verification_algorithms,
        )
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        verify_tls13_signature(
            message,
            cert,
            dss,
            &self.provider.signature_verification_algorithms,
        )
    }

    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        self.provider
            .signature_verification_algorithms
            .supported_schemes()
    }
}

/// Sunshine's certificate isn't the one Irori trusted on first use.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CertificateChanged {
    /// The new certificate's fingerprint.
    pub fingerprint: String,
}

impl std::fmt::Display for CertificateChanged {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Sunshine's certificate changed, so Irori sent nothing to it. If Sunshine was \
             reinstalled or made a new certificate, accept the new one (fingerprint {})",
            self.fingerprint
        )
    }
}

impl std::error::Error for CertificateChanged {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_addresses() {
        let read = |s: &str| parse_address(s).unwrap();
        assert_eq!(read(""), None);
        assert_eq!(read("  "), None);
        let a = read("192.168.1.20").unwrap();
        assert_eq!((a.host.as_str(), a.port), ("192.168.1.20", 47990));
        assert_eq!(a.base(), "https://192.168.1.20:47990");
        assert!(!a.is_loopback());
        let a = read("https://HTPC.local:48000/").unwrap();
        assert_eq!((a.host.as_str(), a.port), ("htpc.local", 48000));
        assert_eq!(a.key(), "htpc.local:48000");
        let a = read("[fe80::1]:47990").unwrap();
        assert_eq!(a.host, "fe80::1");
        assert_eq!(a.base(), "https://[fe80::1]:47990");
        assert!(read("localhost").unwrap().is_loopback());
        assert!(read("127.0.0.2:1").unwrap().is_loopback());
        assert!(read("[::1]").unwrap().is_loopback());

        assert!(parse_address("http://htpc:47990").is_err());
        assert!(parse_address("https://ana:pw@htpc").is_err());
        assert!(parse_address("htpc:notaport").is_err());
    }

    #[test]
    fn fingerprints_read_like_browsers_show_them() {
        let f = fingerprint(b"certificate");
        assert_eq!(f.len(), 32 * 3 - 1);
        assert!(f.split(':').all(|p| p.len() == 2));
        assert_eq!(f, f.to_uppercase());
        assert_ne!(f, fingerprint(b"another"));
    }

    #[test]
    fn the_first_certificate_is_trusted_and_a_new_one_refused() {
        let first = PinVerifier::new(None);
        assert!(first.check(b"cert-a"));
        let seen = first.seen();
        assert_eq!(seen.fingerprint, Some(fingerprint(b"cert-a")));
        assert!(!seen.changed);

        let pinned = PinVerifier::new(seen.fingerprint.clone());
        assert!(pinned.check(b"cert-a"));
        assert!(!pinned.check(b"cert-b"));
        assert_eq!(
            pinned.seen(),
            Seen {
                fingerprint: Some(fingerprint(b"cert-b")),
                changed: true
            }
        );
        assert!(pinned.tls_config().is_ok());
    }
}
