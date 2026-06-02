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
use rustls_pki_types::CertificateDer;
use rustls_pki_types::pem::PemObject;
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

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
