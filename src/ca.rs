//! Local certificate authority. Generated once and reused. Because hudsucker's
//! `RcgenAuthority` signs every per-host leaf certificate with the CA's own key
//! pair, all intercepted sites present a leaf whose public key is the CA's —
//! so pinning a single SPKI hash in the browser (`--ignore-certificate-errors-
//! spki-list`) trusts every intercepted host without installing anything.

use anyhow::{Context, Result};
use base64::{engine::general_purpose::STANDARD, Engine};
use hudsucker::{
    certificate_authority::RcgenAuthority,
    rcgen::{
        BasicConstraints, CertificateParams, DnType, IsCa, Issuer, KeyPair, KeyUsagePurpose,
    },
    rustls::crypto::aws_lc_rs,
};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

pub struct CaBundle {
    pub authority: RcgenAuthority,
    /// base64(SHA-256(SubjectPublicKeyInfo)) — the value to pin in the browser.
    pub spki_sha256_b64: String,
    pub cert_path: PathBuf,
}

pub fn load_or_create(dir: &Path) -> Result<CaBundle> {
    let cert_path = dir.join("reheader-ca.pem");
    let key_path = dir.join("reheader-ca.key");

    let (cert_pem, key) = if cert_path.exists() && key_path.exists() {
        let cert_pem = std::fs::read_to_string(&cert_path).context("read CA cert")?;
        let key_pem = std::fs::read_to_string(&key_path).context("read CA key")?;
        let key = KeyPair::from_pem(&key_pem).context("parse CA key")?;
        (cert_pem, key)
    } else {
        let (cert_pem, key_pem, key) = generate()?;
        std::fs::write(&cert_path, &cert_pem).context("write CA cert")?;
        std::fs::write(&key_path, &key_pem).context("write CA key")?;
        // Key material — best-effort tighten to owner-only on Unix.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&key_path, std::fs::Permissions::from_mode(0o600));
        }
        (cert_pem, key)
    };

    let spki_sha256_b64 = spki_pin(&key);
    let issuer = Issuer::from_ca_cert_pem(&cert_pem, key).context("build CA issuer")?;
    let authority = RcgenAuthority::new(issuer, 1_000, aws_lc_rs::default_provider());

    Ok(CaBundle {
        authority,
        spki_sha256_b64,
        cert_path,
    })
}

/// Compute `base64(SHA-256(SubjectPublicKeyInfo))` — the exact value Chrome's
/// `--ignore-certificate-errors-spki-list` expects. `public_key_pem()` emits
/// the SPKI as a PEM "PUBLIC KEY" block; we strip it back to DER and hash.
fn spki_pin(key: &KeyPair) -> String {
    let pem = key.public_key_pem();
    let b64: String = pem.lines().filter(|l| !l.starts_with("-----")).collect();
    let der = STANDARD.decode(b64.trim()).unwrap_or_default();
    STANDARD.encode(Sha256::digest(&der))
}

fn generate() -> Result<(String, String, KeyPair)> {
    let mut params = CertificateParams::new(Vec::new()).context("CA params")?;
    params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    params.key_usages = vec![
        KeyUsagePurpose::KeyCertSign,
        KeyUsagePurpose::CrlSign,
        KeyUsagePurpose::DigitalSignature,
    ];
    params
        .distinguished_name
        .push(DnType::CommonName, "ReHeader Desktop CA");
    params
        .distinguished_name
        .push(DnType::OrganizationName, "ReHeader Desktop");

    let key = KeyPair::generate().context("generate CA key")?;
    let cert = params.self_signed(&key).context("self-sign CA")?;
    Ok((cert.pem(), key.serialize_pem(), key))
}
