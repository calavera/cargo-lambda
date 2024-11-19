use std::path::{Path, PathBuf};

use clap::Args;
use miette::{Diagnostic, Result};
use rustls::ServerConfig;
use rustls_pki_types::{pem::PemObject, CertificateDer, PrivateKeyDer};
use thiserror::Error;

#[derive(Debug, Diagnostic, Error)]
pub enum TlsError {
    #[error("missing TLS certificate")]
    #[diagnostic()]
    MissingTlsCert,

    #[error("missing TLS key")]
    #[diagnostic()]
    MissingTlsKey,

    #[error("invalid TLS file: {0}, {1}")]
    #[diagnostic()]
    InvalidTlsFile(PathBuf, rustls_pki_types::pem::Error),

    #[error("failed to parse TLS key: {0}")]
    #[diagnostic()]
    FailedToParseTlsKey(String),

    #[error("failed to parse server config: {0}")]
    #[diagnostic()]
    FailedToParseServerConfig(#[from] rustls::Error),
}

#[derive(Args, Clone, Debug, Default)]
pub struct TlsOptions {
    /// Path to a TLS certificate file
    #[arg(long)]
    pub tls_cert: Option<PathBuf>,
    /// Path to a TLS key file
    #[arg(long)]
    pub tls_key: Option<PathBuf>,
    /// Path to a TLS CA file
    #[arg(long)]
    pub tls_ca: Option<PathBuf>,
}

impl TlsOptions {
    pub fn is_secure(&self) -> bool {
        self.tls_cert.is_some() && self.tls_key.is_some()
    }

    pub async fn server_config(&self) -> Result<Option<ServerConfig>> {
        if !self.is_secure() {
            return Ok(None);
        }

        let mut cert_chain = match &self.tls_cert {
            Some(path) => parse_certificates(path)?,
            None => return Err(TlsError::MissingTlsCert.into()),
        };

        let key = match &self.tls_key {
            Some(path) => PrivateKeyDer::from_pem_file(path)
                .map_err(|e| TlsError::FailedToParseTlsKey(e.to_string()))?,
            None => return Err(TlsError::MissingTlsKey.into()),
        };

        if let Some(path) = &self.tls_ca {
            let certs = parse_certificates(path)?;
            if !certs.is_empty() {
                cert_chain.extend(certs);
            }
        }

        let mut config = ServerConfig::builder()
            .with_no_client_auth()
            .with_single_cert(cert_chain, key)
            .map_err(TlsError::FailedToParseServerConfig)?;

        config.alpn_protocols = vec![b"h2".to_vec(), b"http/1.1".to_vec()];

        Ok(Some(config))
    }
}

fn parse_certificates<P: AsRef<Path>>(path: P) -> Result<Vec<CertificateDer<'static>>> {
    let path = path.as_ref();
    let parser = CertificateDer::pem_file_iter(path)
        .map_err(|e| TlsError::InvalidTlsFile(path.to_path_buf(), e))?
        .collect::<Vec<_>>();

    let mut certs = Vec::with_capacity(parser.len());
    for cert in parser {
        certs.push(cert.map_err(|e| TlsError::InvalidTlsFile(path.to_path_buf(), e))?);
    }

    Ok(certs)
}
