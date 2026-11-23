//! Origin-local DTLS identity. Only the fingerprint and expiry cross the JS binding.
use std::io;
use std::time::{SystemTime, UNIX_EPOCH};
use str0m::crypto::dtls::DtlsCert;

pub struct Certificate {
    pub(super) identity: DtlsCert,
    pub expires_ms: u64,
    pub fingerprint: String,
}

impl Certificate {
    pub fn generate(lifetime_ms: u64) -> io::Result<Self> {
        let provider = str0m::crypto::from_feature_flags();
        let (identity, expires_ms) = generate(lifetime_ms)?;
        let digest = provider.sha256_provider.sha256(&identity.certificate);
        let fingerprint = digest.iter().map(|byte| format!("{byte:02x}")).collect::<Vec<_>>().join(":");
        Ok(Self { identity, expires_ms, fingerprint })
    }

    pub fn expired(&self) -> bool {
        SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_millis() >= u128::from(self.expires_ms)
    }
}

#[cfg(not(windows))]
fn generate(lifetime_ms: u64) -> io::Result<(DtlsCert, u64)> {
    use rcgen::{CertificateParams, DistinguishedName, DnType, KeyPair, PKCS_ECDSA_P256_SHA256};
    use time::{Duration, OffsetDateTime};
    let now = OffsetDateTime::now_utc();
    let lifetime = Duration::milliseconds(i64::try_from(lifetime_ms).map_err(io::Error::other)?);
    let expires = now.checked_add(lifetime).ok_or_else(|| io::Error::other("Invalid certificate lifetime"))?;
    let key = KeyPair::generate_for(&PKCS_ECDSA_P256_SHA256).map_err(io::Error::other)?;
    let mut name = DistinguishedName::new();
    name.push(DnType::CommonName, "WebRTC");
    let mut params = CertificateParams::default();
    params.distinguished_name = name;
    params.not_before = now - Duration::days(1);
    params.not_after = expires;
    let cert = params.self_signed(&key).map_err(io::Error::other)?;
    let identity = DtlsCert { certificate: cert.der().to_vec(), private_key: key.serialize_der() };
    Ok((identity, expires.unix_timestamp() as u64 * 1000))
}

#[cfg(windows)]
fn generate(lifetime_ms: u64) -> io::Result<(DtlsCert, u64)> {
    // The Windows provider owns the private key and must issue the matching certificate.
    let identity = str0m::crypto::from_feature_flags().dtls_provider.generate_certificate()
        .ok_or_else(|| io::Error::other("The system DTLS provider could not generate a certificate"))?;
    let (_, parsed) = x509_parser::parse_x509_certificate(&identity.certificate)
        .map_err(|_| io::Error::other("Invalid system DTLS certificate"))?;
    let valid_until = parsed.validity().not_after.timestamp().max(0) as u64 * 1000;
    let now = SystemTime::now().duration_since(UNIX_EPOCH).map_err(io::Error::other)?.as_millis() as u64;
    Ok((identity, valid_until.min(now.saturating_add(lifetime_ms))))
}
