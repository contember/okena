//! TLS material for the remote control server.
//!
//! The server uses a **persisted self-signed certificate**. There is no CA in
//! the picture — clients establish trust by pinning the certificate's SHA-256
//! fingerprint on first connect (TOFU) and verifying it out-of-band against the
//! fingerprint shown here on the host. See `okena-core::client::tls` for the
//! client-side pinned verifier.
//!
//! The cert is generated once and reused across restarts so the pinned
//! fingerprint stays stable; regenerating it would force every paired client to
//! re-verify.

use anyhow::{Context, Result};
use rustls_pki_types::pem::PemObject;
use rustls_pki_types::{CertificateDer, PrivateKeyDer};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// Paths + fingerprint of the server's persisted self-signed certificate.
pub struct TlsMaterial {
    pub cert_path: PathBuf,
    pub key_path: PathBuf,
    /// Lowercase-hex SHA-256 of the DER certificate — the value clients pin and
    /// the user eyeball-verifies against the client during pairing.
    pub fingerprint: String,
}

fn cert_path(dir: &Path) -> PathBuf {
    dir.join("remote_cert.pem")
}

fn key_path(dir: &Path) -> PathBuf {
    dir.join("remote_key.pem")
}

/// Read the fingerprint of the already-persisted cert *without* generating one.
///
/// Used by the `okena pair` CLI to show the fingerprint for out-of-band
/// verification. Returns `None` if no cert exists yet (TLS never enabled) or it
/// can't be parsed — callers treat that as "no fingerprint to show".
pub fn read_fingerprint(config_dir: &Path) -> Option<String> {
    let der = CertificateDer::from_pem_file(cert_path(config_dir)).ok()?;
    Some(fingerprint_hex(der.as_ref()))
}

/// Load the persisted self-signed cert, generating + persisting one on first use.
pub fn load_or_generate(config_dir: &Path) -> Result<TlsMaterial> {
    std::fs::create_dir_all(config_dir)
        .with_context(|| format!("creating config dir {config_dir:?}"))?;
    let cert_path = cert_path(config_dir);
    let key_path = key_path(config_dir);

    if !(cert_path.exists() && key_path.exists()) {
        generate_and_write(&cert_path, &key_path)?;
    }

    let der = CertificateDer::from_pem_file(&cert_path)
        .with_context(|| format!("reading certificate {cert_path:?}"))?;
    let fingerprint = fingerprint_hex(der.as_ref());

    Ok(TlsMaterial {
        cert_path,
        key_path,
        fingerprint,
    })
}

/// Generate a fresh self-signed cert and write it atomically (key as 0600).
///
/// Hostname verification is disabled on the client (it pins the exact cert), so
/// the SAN here is cosmetic — `localhost` keeps the cert valid for the common
/// loopback case without enumerating every LAN IP.
fn generate_and_write(cert_path: &Path, key_path: &Path) -> Result<()> {
    let certified = rcgen::generate_simple_self_signed(vec!["localhost".to_string()])
        .context("generating self-signed certificate")?;
    let cert_pem = certified.cert.pem();
    let key_pem = certified.key_pair.serialize_pem();

    atomic_write(cert_path, cert_pem.as_bytes(), 0o644)
        .with_context(|| format!("writing certificate {cert_path:?}"))?;
    atomic_write(key_path, key_pem.as_bytes(), 0o600)
        .with_context(|| format!("writing private key {key_path:?}"))?;
    log::info!("Generated self-signed remote-server certificate at {cert_path:?}");
    Ok(())
}

/// Atomic write (tmp + fsync + rename) with a Unix mode applied before rename.
fn atomic_write(path: &Path, bytes: &[u8], _mode: u32) -> std::io::Result<()> {
    use std::io::Write;
    let tmp_path = path.with_extension("pem.tmp");
    {
        let mut f = std::fs::File::create(&tmp_path)?;
        f.write_all(bytes)?;
        f.sync_all()?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            f.set_permissions(std::fs::Permissions::from_mode(_mode))?;
        }
    }
    std::fs::rename(&tmp_path, path)
}

/// Build a rustls `ServerConfig` from the persisted self-signed cert + key,
/// using the ring provider (matches the client side).
pub fn server_config(material: &TlsMaterial) -> Result<Arc<rustls::ServerConfig>> {
    let certs: Vec<CertificateDer<'static>> =
        CertificateDer::pem_file_iter(&material.cert_path)
            .with_context(|| format!("reading certificate {:?}", material.cert_path))?
            .collect::<std::result::Result<_, _>>()
            .context("parsing certificate chain")?;
    let key = PrivateKeyDer::from_pem_file(&material.key_path)
        .with_context(|| format!("reading private key {:?}", material.key_path))?;

    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let config = rustls::ServerConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .context("rustls default protocol versions")?
        .with_no_client_auth()
        .with_single_cert(certs, key)
        .context("installing server certificate")?;
    Ok(Arc::new(config))
}

/// Lowercase-hex SHA-256 of a DER-encoded certificate.
pub fn fingerprint_hex(der: &[u8]) -> String {
    use std::fmt::Write;
    let digest = Sha256::digest(der);
    let mut s = String::with_capacity(digest.len() * 2);
    for b in digest {
        let _ = write!(s, "{b:02x}");
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fingerprint_is_64_hex_chars() {
        let fp = fingerprint_hex(&[0u8; 4]);
        assert_eq!(fp.len(), 64);
        assert!(fp.chars().all(|c| c.is_ascii_hexdigit() && !c.is_uppercase()));
    }

    #[test]
    fn fingerprint_is_stable_and_distinct() {
        assert_eq!(fingerprint_hex(b"abc"), fingerprint_hex(b"abc"));
        assert_ne!(fingerprint_hex(b"abc"), fingerprint_hex(b"abd"));
        // Known SHA-256("abc")
        assert_eq!(
            fingerprint_hex(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn generate_then_load_is_idempotent_and_stable() {
        let dir = tempfile::tempdir().unwrap();
        let a = load_or_generate(dir.path()).unwrap();
        // Second call must reuse the same cert → same fingerprint (clients stay pinned).
        let b = load_or_generate(dir.path()).unwrap();
        assert_eq!(a.fingerprint, b.fingerprint);
        assert_eq!(a.fingerprint.len(), 64);
        assert!(a.cert_path.exists() && a.key_path.exists());
    }
}

/// End-to-end TLS handshake tests for the desktop→desktop path: a real rustls
/// server (using our generated cert) against the real okena-core pinned client.
#[cfg(test)]
mod handshake_tests {
    use super::*;
    use axum::Router;
    use axum::routing::get;

    /// Start the real dual-stack server (both http + TLS on one port).
    async fn spawn_dual_stack(material: &TlsMaterial) -> std::net::SocketAddr {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let tls = super::server_config(material).unwrap();
        let app = Router::new().route("/health", get(|| async { "ok" }));
        tokio::spawn(async move {
            let _ = crate::serve::serve_dual_stack(
                listener,
                app,
                tls,
                std::future::pending::<()>(),
            )
            .await;
        });
        addr
    }

    /// GET with a short readiness retry while the server task spins up.
    async fn get_with_retry(client: &reqwest::Client, url: &str) -> Result<reqwest::Response, reqwest::Error> {
        let mut last_err = None;
        for _ in 0..20 {
            match client.get(url).send().await {
                Ok(resp) => return Ok(resp),
                Err(e) => {
                    last_err = Some(e);
                    tokio::time::sleep(std::time::Duration::from_millis(25)).await;
                }
            }
        }
        Err(last_err.unwrap())
    }

    #[tokio::test]
    async fn dual_stack_serves_http_and_pinned_https_on_one_port() {
        // Server providers may consult the process-default CryptoProvider.
        let _ = rustls::crypto::ring::default_provider().install_default();

        let dir = tempfile::tempdir().unwrap();
        let material = load_or_generate(dir.path()).unwrap();
        let addr = spawn_dual_stack(&material).await;
        let port = addr.port();
        let url = format!("https://127.0.0.1:{}/health", port);

        // 0) Plain HTTP on the SAME port still works (back-compat for existing
        //    plain-http clients after the server enables TLS).
        let http_url = format!("http://127.0.0.1:{}/health", port);
        let http_client = reqwest::Client::new();
        assert!(
            get_with_retry(&http_client, &http_url)
                .await
                .map(|r| r.status().is_success())
                .unwrap_or(false),
            "plain http must work on the dual-stack port"
        );

        // 1) TOFU (no pin): handshake succeeds and the client captures exactly
        //    the server's cert fingerprint.
        let observed = okena_transport::client::tls::new_observed();
        let client = okena_transport::client::tls::build_reqwest_client(true, None, observed.clone());
        let resp = get_with_retry(&client, &url).await.expect("TOFU connect should succeed");
        assert!(resp.status().is_success());
        assert_eq!(
            observed.lock().unwrap().as_deref(),
            Some(material.fingerprint.as_str()),
            "TOFU must capture the server's real fingerprint"
        );

        // 2) Correct pin: handshake succeeds.
        let client_ok = okena_transport::client::tls::build_reqwest_client(
            true,
            Some(material.fingerprint.clone()),
            okena_transport::client::tls::new_observed(),
        );
        assert!(
            get_with_retry(&client_ok, &url).await.is_ok(),
            "matching pin must connect"
        );

        // 3) Wrong pin: handshake is rejected (MITM / cert swap defense).
        let client_bad = okena_transport::client::tls::build_reqwest_client(
            true,
            Some("00".repeat(32)),
            okena_transport::client::tls::new_observed(),
        );
        assert!(
            client_bad.get(&url).send().await.is_err(),
            "mismatched pin must be rejected"
        );
    }
}
