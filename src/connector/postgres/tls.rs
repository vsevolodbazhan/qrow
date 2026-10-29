//! Encryption-only TLS for the explicit Postgres Require mode.
use rustls::{
    ClientConfig, DigitallySignedStruct, Error, SignatureScheme,
    client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier},
    crypto::{CryptoProvider, verify_tls12_signature, verify_tls13_signature},
    pki_types::{CertificateDer, ServerName, UnixTime},
};
use std::sync::Arc;

pub(super) fn encryption_only() -> anyhow::Result<ClientConfig> {
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    Ok(ClientConfig::builder_with_provider(provider.clone())
        .with_safe_default_protocol_versions()?
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(EncryptionOnlyVerifier { provider }))
        .with_no_client_auth())
}

#[derive(Debug)]
struct EncryptionOnlyVerifier {
    provider: Arc<CryptoProvider>,
}

impl ServerCertVerifier for EncryptionOnlyVerifier {
    fn verify_server_cert(
        &self,
        _end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, Error> {
        // Require encrypts without authenticating the server. The handshake
        // signature checks below still prove possession of the certificate key.
        Ok(ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, Error> {
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
    ) -> Result<HandshakeSignatureValid, Error> {
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

#[cfg(test)]
mod tests {
    use crate::{
        connector::{Connector, Secret, postgres::PostgresConnector},
        model::{DatabaseType, PostgresSslMode, Profile},
    };
    use std::{
        io::{Read, Write},
        net::TcpListener,
        thread,
    };

    #[test]
    fn require_tls_refuses_a_server_that_declines_encryption() {
        for mode in [PostgresSslMode::Require, PostgresSslMode::VerifyFull] {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let profile = Profile {
                database_type: DatabaseType::Postgres,
                postgres_ssl_mode: Some(mode),
                host: "127.0.0.1".into(),
                port: listener.local_addr().unwrap().port(),
                username: "synthetic-user".into(),
                database: "synthetic-db".into(),
                ..Profile::default()
            };
            let server = thread::spawn(move || {
                let (mut socket, _) = listener.accept().unwrap();
                socket
                    .set_read_timeout(Some(std::time::Duration::from_secs(5)))
                    .unwrap();
                let mut request = [0; 8];
                socket.read_exact(&mut request).unwrap();
                assert_eq!(request, [0, 0, 0, 8, 4, 210, 22, 47]);
                socket.write_all(b"N").unwrap();
                let mut next = [0; 1];
                assert_eq!(
                    socket.read(&mut next).unwrap(),
                    0,
                    "must not send a plaintext startup packet"
                );
            });
            assert!(
                PostgresConnector::default()
                    .connect(&profile, Secret::password("synthetic-password"))
                    .is_err()
            );
            server.join().unwrap();
        }
    }
}
