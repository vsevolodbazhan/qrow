//! TLS for Kyuubi connections and sign-in providers.
//!
//! The application trusts the certificate authorities of the macOS trust
//! store. Tests trust a synthetic authority only.
use anyhow::{Context, Result};
use rustls::{ClientConfig, ClientConnection, RootCertStore, StreamOwned};
use rustls_pki_types::{CertificateDer, ServerName, pem::PemObject};
use std::{
    net::TcpStream,
    sync::{Arc, OnceLock},
};

pub type TlsStream = StreamOwned<ClientConnection, TcpStream>;

/// The certificate authorities that verify servers.
#[derive(Clone)]
pub struct Trust {
    authorities: Authorities,
    config: Arc<OnceLock<Result<Arc<ClientConfig>, String>>>,
}

#[derive(Clone)]
enum Authorities {
    System,
    Only(Arc<Vec<CertificateDer<'static>>>),
}

impl Default for Trust {
    fn default() -> Self {
        Self::system()
    }
}

impl std::fmt::Debug for Trust {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self.authorities {
            Authorities::System => f.write_str("Trust::System"),
            Authorities::Only(certificates) => write!(f, "Trust::Only({})", certificates.len()),
        }
    }
}

impl Trust {
    /// The authorities of the operating system trust store.
    pub fn system() -> Self {
        Self {
            authorities: Authorities::System,
            config: Arc::default(),
        }
    }

    /// Only the authorities in a PEM document, for example a test authority.
    pub fn from_pem(pem: &[u8]) -> Result<Self> {
        let certificates = CertificateDer::pem_slice_iter(pem)
            .collect::<Result<Vec<_>, _>>()
            .context("Could not read the PEM certificates")?;
        anyhow::ensure!(
            !certificates.is_empty(),
            "The PEM document contains no certificate"
        );
        Ok(Self {
            authorities: Authorities::Only(Arc::new(certificates)),
            config: Arc::default(),
        })
    }

    fn client_config(&self) -> Result<Arc<ClientConfig>> {
        self.config
            .get_or_init(|| self.build().map_err(|error| format!("{error:#}")))
            .clone()
            .map_err(anyhow::Error::msg)
    }

    fn build(&self) -> Result<Arc<ClientConfig>> {
        let mut roots = RootCertStore::empty();
        match &self.authorities {
            Authorities::System => {
                let found = rustls_native_certs::load_native_certs();
                let (added, _) = roots.add_parsable_certificates(found.certs);
                anyhow::ensure!(
                    added > 0,
                    "Could not read certificate authorities from the system trust store"
                );
            }
            Authorities::Only(certificates) => {
                for certificate in certificates.iter() {
                    roots
                        .add(certificate.clone())
                        .context("Could not use the certificate authority")?;
                }
            }
        }
        let provider = Arc::new(rustls::crypto::ring::default_provider());
        let config = ClientConfig::builder_with_provider(provider)
            .with_safe_default_protocol_versions()?
            .with_root_certificates(roots)
            .with_no_client_auth();
        Ok(Arc::new(config))
    }

    /// Completes a TLS handshake on `stream` and verifies that the server
    /// certificate is valid for `host`.
    pub fn connect(&self, host: &str, mut stream: TcpStream) -> Result<TlsStream> {
        let name = ServerName::try_from(host.to_owned())
            .with_context(|| format!("{host} is not a valid TLS server name"))?;
        let mut connection = ClientConnection::new(self.client_config()?, name)?;
        while connection.is_handshaking() {
            connection
                .complete_io(&mut stream)
                .with_context(|| format!("TLS handshake with {host} failed"))?;
        }
        Ok(StreamOwned::new(connection, stream))
    }
}
