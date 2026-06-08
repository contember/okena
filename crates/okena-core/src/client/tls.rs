//! Client-side TLS with certificate pinning (Trust On First Use).
//!
//! The remote server uses a self-signed cert (no CA). The client therefore
//! cannot validate it against a trust store; instead it **pins the cert's
//! SHA-256 fingerprint**:
//!
//! - On the first TLS handshake the pin is `None` → we accept the cert and
//!   record its fingerprint (TOFU). The caller persists it and the user
//!   verifies it out-of-band against the fingerprint shown on the host.
//! - On every later handshake the pin is `Some` → we require the presented
//!   cert's fingerprint to match exactly, defeating an active MITM.
//!
//! Crucially, we still verify the handshake *signature* against the presented
//! cert's key (via the crypto provider), so an attacker who merely replays the
//! pinned cert without its private key cannot complete the handshake.
//!
//! This trust model is correct for a LAN self-signed server identified by a
//! pinned fingerprint. It is NOT appropriate for the public web (no hostname or
//! chain validation).

use std::sync::{Arc, Mutex};

use rustls::DigitallySignedStruct;
use rustls::SignatureScheme;
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::crypto::{CryptoProvider, verify_tls12_signature, verify_tls13_signature};
use rustls_pki_types::{CertificateDer, ServerName, UnixTime};
use sha2::{Digest, Sha256};

/// Lowercase-hex SHA-256 of a DER-encoded certificate.
pub fn cert_fingerprint(der: &[u8]) -> String {
    use std::fmt::Write;
    let digest = Sha256::digest(der);
    let mut s = String::with_capacity(digest.len() * 2);
    for b in digest {
        let _ = write!(s, "{b:02x}");
    }
    s
}

/// Format a hex fingerprint as colon-separated byte pairs for readable
/// out-of-band comparison (e.g. `ab:cd:ef:01 23:45:67:89 …`). Pairs are grouped
/// four-to-a-block with spaces between blocks so the long hex string can
/// soft-wrap inside narrow containers instead of overflowing horizontally.
pub fn format_fingerprint(fp: &str) -> String {
    fp.as_bytes()
        .chunks(2)
        .map(|pair| std::str::from_utf8(pair).unwrap_or("??"))
        .collect::<Vec<_>>()
        .chunks(4)
        .map(|group| group.join(":"))
        .collect::<Vec<_>>()
        .join(" ")
}

/// Shared slot the verifier writes the most recently observed fingerprint into,
/// so the connection task can read it back after a successful handshake and
/// persist a freshly-pinned cert.
pub type ObservedFingerprint = Arc<Mutex<Option<String>>>;

/// Fresh empty observed-fingerprint slot for one connection attempt.
pub fn new_observed() -> ObservedFingerprint {
    Arc::new(Mutex::new(None))
}

/// rustls verifier that trusts a server cert by its pinned SHA-256 fingerprint.
#[derive(Debug)]
pub struct PinnedCertVerifier {
    /// Expected fingerprint (lowercase hex). `None` → TOFU: accept and record.
    pinned: Option<String>,
    /// Fingerprint observed on the last handshake (for TOFU capture).
    observed: ObservedFingerprint,
    provider: Arc<CryptoProvider>,
}

impl PinnedCertVerifier {
    pub fn new(
        pinned: Option<String>,
        observed: ObservedFingerprint,
        provider: Arc<CryptoProvider>,
    ) -> Self {
        Self {
            pinned,
            observed,
            provider,
        }
    }

    /// Pure pin-check: record the presented fingerprint, then enforce the pin.
    /// Factored out so the decision is unit-testable without a live handshake.
    fn check(&self, end_entity_der: &[u8]) -> Result<(), rustls::Error> {
        let fp = cert_fingerprint(end_entity_der);
        if let Ok(mut slot) = self.observed.lock() {
            *slot = Some(fp.clone());
        }
        match &self.pinned {
            // Trust on first use: no pin yet, accept and let the caller persist.
            None => Ok(()),
            Some(expected) if expected.eq_ignore_ascii_case(&fp) => Ok(()),
            Some(expected) => Err(rustls::Error::General(format!(
                "remote certificate fingerprint mismatch: pinned {expected}, server presented {fp}"
            ))),
        }
    }
}

impl ServerCertVerifier for PinnedCertVerifier {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        self.check(end_entity.as_ref())?;
        Ok(ServerCertVerified::assertion())
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

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.provider
            .signature_verification_algorithms
            .supported_schemes()
    }
}

fn provider() -> Arc<CryptoProvider> {
    Arc::new(rustls::crypto::aws_lc_rs::default_provider())
}

/// Build a rustls `ClientConfig` that pins the server cert via [`PinnedCertVerifier`].
fn pinned_client_config(pinned: Option<String>, observed: ObservedFingerprint) -> rustls::ClientConfig {
    let provider = provider();
    let verifier = Arc::new(PinnedCertVerifier::new(pinned, observed, provider.clone()));
    #[allow(
        clippy::expect_used,
        reason = "aws_lc_rs default provider always supports the default protocol versions"
    )]
    let builder = rustls::ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .expect("aws_lc_rs default provider supports default protocol versions");
    builder
        .dangerous()
        .with_custom_certificate_verifier(verifier)
        .with_no_client_auth()
}

/// Build a reqwest client for a connection. When `tls` is false this is a plain
/// `reqwest::Client`; when true it uses the pinned rustls config (TOFU/enforce).
pub fn build_reqwest_client(
    tls: bool,
    pinned: Option<String>,
    observed: ObservedFingerprint,
) -> reqwest::Client {
    if !tls {
        return reqwest::Client::new();
    }
    let config = pinned_client_config(pinned, observed);
    reqwest::Client::builder()
        .use_preconfigured_tls(config)
        .build()
        .unwrap_or_else(|e| {
            log::error!("Failed to build pinned TLS client, falling back to default: {e}");
            reqwest::Client::new()
        })
}

/// Build a tokio-tungstenite connector for the WebSocket. Returns `None` when
/// `tls` is false (the caller uses the plain `connect_async` path).
pub fn ws_connector(
    tls: bool,
    pinned: Option<String>,
    observed: ObservedFingerprint,
) -> Option<tokio_tungstenite::Connector> {
    if !tls {
        return None;
    }
    let config = pinned_client_config(pinned, observed);
    Some(tokio_tungstenite::Connector::Rustls(Arc::new(config)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn verifier(pinned: Option<&str>) -> (PinnedCertVerifier, ObservedFingerprint) {
        let observed: ObservedFingerprint = Arc::new(Mutex::new(None));
        let v = PinnedCertVerifier::new(pinned.map(str::to_string), observed.clone(), provider());
        (v, observed)
    }

    #[test]
    fn tofu_accepts_and_records_when_no_pin() {
        let (v, observed) = verifier(None);
        assert!(v.check(b"fake-der-bytes").is_ok());
        assert_eq!(
            observed.lock().unwrap().as_deref(),
            Some(cert_fingerprint(b"fake-der-bytes").as_str())
        );
    }

    #[test]
    fn matching_pin_is_accepted() {
        let fp = cert_fingerprint(b"server-cert");
        let (v, _o) = verifier(Some(&fp));
        assert!(v.check(b"server-cert").is_ok());
    }

    #[test]
    fn mismatched_pin_is_rejected() {
        let wrong = cert_fingerprint(b"other-cert");
        let (v, observed) = verifier(Some(&wrong));
        assert!(v.check(b"server-cert").is_err());
        // Still records what the server actually presented (for diagnostics).
        assert_eq!(
            observed.lock().unwrap().as_deref(),
            Some(cert_fingerprint(b"server-cert").as_str())
        );
    }

    #[test]
    fn pin_comparison_is_case_insensitive() {
        let fp = cert_fingerprint(b"server-cert").to_uppercase();
        let (v, _o) = verifier(Some(&fp));
        assert!(v.check(b"server-cert").is_ok());
    }

    #[test]
    fn format_fingerprint_groups_pairs_with_colons_and_spaces() {
        // 12 hex chars = 6 byte pairs → colons within 4-pair blocks, space between.
        assert_eq!(format_fingerprint("aabbccddeeff"), "aa:bb:cc:dd ee:ff");
        // A full 64-char fingerprint round-trips to the same hex when separators stripped.
        let fp = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";
        let formatted = format_fingerprint(fp);
        assert_eq!(formatted.replace([':', ' '], ""), fp);
        // Spaces every 4 pairs (8 hex chars) → 32 pairs / 4 = 8 blocks → 7 spaces.
        assert_eq!(formatted.matches(' ').count(), 7);
    }
}
