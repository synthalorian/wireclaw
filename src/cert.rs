//! Certificate authority for HTTPS MITM interception.
//!
//! Generates a root CA on first run, then signs per-host certificates
//! dynamically so the proxy can terminate TLS and inspect traffic.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use anyhow::{Context, Result};
use rcgen::{CertificateParams, DistinguishedName, DnType, Issuer, KeyPair, SanType};
use rustls::pki_types::CertificateDer;
use rustls::ServerConfig;

/// Manages the Ledger CA and per-host certificates.
pub struct CertManager {
    #[allow(dead_code)]
    ca_cert: rcgen::Certificate,
    ca_params: CertificateParams,
    ca_key: KeyPair,
    ca_cert_pem: String,
    cache_dir: PathBuf,
    /// In-memory cache of already-generated host certs
    host_certs: Mutex<HashMap<String, Arc<ServerConfig>>>,
}

impl CertManager {
    /// Load or create a CertManager at the given directory.
    /// If no CA exists, one is generated automatically.
    pub fn load_or_create(dir: &Path) -> Result<Self> {
        let ca_key_path = dir.join("ca.key");
        let ca_cert_path = dir.join("ca.crt");

        if ca_key_path.exists() && ca_cert_path.exists() {
            Self::load_from_files(dir, &ca_key_path, &ca_cert_path)
        } else {
            Self::generate_and_save(dir, &ca_key_path, &ca_cert_path)
        }
    }

    /// Generate a fresh CA and save it to disk.
    fn generate_and_save(dir: &Path, key_path: &Path, cert_path: &Path) -> Result<Self> {
        std::fs::create_dir_all(dir)
            .with_context(|| format!("creating cert directory {}", dir.display()))?;

        let ca_key = KeyPair::generate_for(&rcgen::PKCS_ECDSA_P256_SHA256)
            .context("generating CA key pair")?;

        let mut dn = DistinguishedName::new();
        dn.push(DnType::CommonName, "Ledger Proxy CA");
        dn.push(DnType::OrganizationName, "Ledger");
        dn.push(DnType::CountryName, "US");

        let mut params = CertificateParams::new(vec!["ledger.proxy".to_string()])
            .context("creating CA certificate params")?;
        params.distinguished_name = dn;
        params.is_ca = rcgen::IsCa::Ca(rcgen::BasicConstraints::Unconstrained);
        params.key_usages = vec![
            rcgen::KeyUsagePurpose::KeyCertSign,
            rcgen::KeyUsagePurpose::CrlSign,
        ];

        let ca_cert = params
            .self_signed(&ca_key)
            .context("self-signing CA certificate")?;

        let ca_cert_pem = ca_cert.pem();
        let ca_key_pem = ca_key.serialize_pem();

        std::fs::write(cert_path, &ca_cert_pem)
            .with_context(|| format!("writing CA cert to {}", cert_path.display()))?;
        std::fs::write(key_path, &ca_key_pem)
            .with_context(|| format!("writing CA key to {}", key_path.display()))?;

        // Restrict key file permissions (owner read/write only)
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = std::fs::metadata(key_path)?.permissions();
            perms.set_mode(0o600);
            std::fs::set_permissions(key_path, perms)?;
        }

        Ok(Self {
            ca_cert,
            ca_params: params,
            ca_key,
            ca_cert_pem,
            cache_dir: dir.to_path_buf(),
            host_certs: Mutex::new(HashMap::new()),
        })
    }

    /// Load an existing CA from PEM files.
    fn load_from_files(dir: &Path, key_path: &Path, cert_path: &Path) -> Result<Self> {
        let ca_key_pem = std::fs::read_to_string(key_path)
            .with_context(|| format!("reading CA key from {}", key_path.display()))?;
        let ca_cert_pem = std::fs::read_to_string(cert_path)
            .with_context(|| format!("reading CA cert from {}", cert_path.display()))?;

        let ca_key = KeyPair::from_pem(&ca_key_pem).context("parsing CA key PEM")?;

        // Reconstruct params and cert from the stored PEM
        let mut dn = DistinguishedName::new();
        dn.push(DnType::CommonName, "Ledger Proxy CA");
        dn.push(DnType::OrganizationName, "Ledger");
        dn.push(DnType::CountryName, "US");

        let mut params = CertificateParams::new(vec!["ledger.proxy".to_string()])
            .context("creating CA certificate params")?;
        params.distinguished_name = dn;
        params.is_ca = rcgen::IsCa::Ca(rcgen::BasicConstraints::Unconstrained);
        params.key_usages = vec![
            rcgen::KeyUsagePurpose::KeyCertSign,
            rcgen::KeyUsagePurpose::CrlSign,
        ];

        let ca_cert = params
            .self_signed(&ca_key)
            .context("re-signing loaded CA cert")?;

        Ok(Self {
            ca_cert,
            ca_params: params,
            ca_key,
            ca_cert_pem,
            cache_dir: dir.to_path_buf(),
            host_certs: Mutex::new(HashMap::new()),
        })
    }

    /// Return the CA certificate PEM for installation/trust instructions.
    pub fn ca_cert_pem(&self) -> &str {
        &self.ca_cert_pem
    }

    /// Path to the CA certificate file.
    pub fn ca_cert_path(&self) -> PathBuf {
        self.cache_dir.join("ca.crt")
    }

    /// Get (or generate) a TLS server config for the given host.
    /// The returned config presents a certificate signed by our CA.
    pub fn server_config_for_host(&self, host: &str) -> Result<Arc<ServerConfig>> {
        // Strip port if present
        let host_clean = host.split(':').next().unwrap_or(host);

        // Check cache first
        {
            let cache = self.host_certs.lock().unwrap();
            if let Some(cfg) = cache.get(host_clean) {
                return Ok(Arc::clone(cfg));
            }
        }

        // Generate a new cert for this host
        let cert = self.generate_host_cert(host_clean)?;

        // Parse into rustls types
        let cert_chain: Vec<CertificateDer<'static>> =
            rustls_pemfile::certs(&mut cert.cert_pem.as_bytes())
                .collect::<Result<Vec<_>, _>>()
                .context("parsing host cert PEM")?;

        let key = rustls_pemfile::private_key(&mut cert.key_pem.as_bytes())
            .context("parsing host key PEM")?
            .context("no private key found in PEM")?;

        let config = ServerConfig::builder()
            .with_no_client_auth()
            .with_single_cert(cert_chain, key)
            .context("building rustls server config")?;

        let config = Arc::new(config);

        // Cache it
        {
            let mut cache = self.host_certs.lock().unwrap();
            cache.insert(host_clean.to_string(), Arc::clone(&config));
        }

        Ok(config)
    }

    /// Generate a certificate signed by our CA for a specific host.
    fn generate_host_cert(&self, host: &str) -> Result<HostCert> {
        let host_key = KeyPair::generate_for(&rcgen::PKCS_ECDSA_P256_SHA256)
            .context("generating host key pair")?;

        let mut dn = DistinguishedName::new();
        dn.push(DnType::CommonName, host);

        let mut params = CertificateParams::new(vec![host.to_string()])
            .with_context(|| format!("creating cert params for host {host}"))?;
        params.distinguished_name = dn;
        params.subject_alt_names = vec![SanType::DnsName(
            host.try_into().map_err(|e| anyhow::anyhow!("invalid hostname for SAN: {e:?}"))?
        )];

        let issuer = Issuer::new(self.ca_params.clone(), &self.ca_key);

        let cert = params
            .signed_by(&host_key, &issuer)
            .with_context(|| format!("signing host cert for {host}"))?;

        Ok(HostCert {
            cert_pem: cert.pem(),
            key_pem: host_key.serialize_pem(),
        })
    }
}

struct HostCert {
    cert_pem: String,
    key_pem: String,
}

// ── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn init_crypto() {
        let _ = rustls::crypto::ring::default_provider().install_default();
    }

    #[test]
    fn test_generate_and_load_ca() {
        let tmp = tempfile::tempdir().unwrap();
        let mgr = CertManager::load_or_create(tmp.path()).unwrap();

        assert!(!mgr.ca_cert_pem().is_empty());
        assert!(mgr.ca_cert_path().exists());
        assert!(tmp.path().join("ca.key").exists());

        // Loading again should reuse the same CA
        let mgr2 = CertManager::load_or_create(tmp.path()).unwrap();
        assert_eq!(mgr.ca_cert_pem(), mgr2.ca_cert_pem());
    }

    #[test]
    fn test_host_cert_generation() {
        init_crypto();
        let tmp = tempfile::tempdir().unwrap();
        let mgr = CertManager::load_or_create(tmp.path()).unwrap();

        let config = mgr.server_config_for_host("example.com").unwrap();
        // rustls ServerConfig doesn't expose much, but if we got here it parsed
        assert!(Arc::strong_count(&config) >= 1);

        // Second call should hit cache
        let config2 = mgr.server_config_for_host("example.com").unwrap();
        assert!(Arc::ptr_eq(&config, &config2));
    }

    #[test]
    fn test_host_cert_with_port_stripped() {
        init_crypto();
        let tmp = tempfile::tempdir().unwrap();
        let mgr = CertManager::load_or_create(tmp.path()).unwrap();

        let config = mgr.server_config_for_host("api.example.com:443").unwrap();
        assert!(Arc::strong_count(&config) >= 1);
    }
}
