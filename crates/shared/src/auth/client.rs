//! Client side of the login service: TLS with "trust on first use" certificate pinning.
//!
//! Game servers (and Host Game worlds) use self-signed certificates, so there's no certificate
//! authority to check against. Instead, like SSH, the first time we connect to a server we
//! remember a SHA-256 fingerprint of its certificate in a small text file. On every later
//! connection the certificate must match. A mismatch means the server was reinstalled, or
//! someone is impersonating it, and we refuse to send the password.

use super::{read_frame, write_frame, AuthRequest, AuthResponse, LoginTicket};
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::crypto::{verify_tls12_signature, verify_tls13_signature, CryptoProvider};
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use rustls::{ClientConfig, ClientConnection, DigitallySignedStruct, SignatureScheme, StreamOwned};
use sha2::{Digest, Sha256};
use std::net::{SocketAddr, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// Hex SHA-256 of a DER certificate. Shown to server admins so players can compare it.
pub fn fingerprint(der: &[u8]) -> String {
    Sha256::digest(der).iter().map(|b| format!("{b:02x}")).collect()
}

/// A `host:port fingerprint` list on disk.
#[derive(Debug)]
pub struct KnownServers {
    path: PathBuf,
}

impl KnownServers {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    pub fn get(&self, server: &str) -> Option<String> {
        let text = std::fs::read_to_string(&self.path).ok()?;
        text.lines().find_map(|line| {
            let (host, fp) = line.trim().split_once(' ')?;
            (host == server).then(|| fp.trim().to_string())
        })
    }

    fn remember(&self, server: &str, fp: &str) -> std::io::Result<()> {
        if let Some(dir) = self.path.parent().filter(|d| !d.as_os_str().is_empty()) {
            std::fs::create_dir_all(dir)?;
        }
        let mut text = std::fs::read_to_string(&self.path).unwrap_or_default();
        text.push_str(&format!("{server} {fp}\n"));
        std::fs::write(&self.path, text)
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

/// How to decide whether to trust the login service's certificate.
#[derive(Debug)]
pub enum Trust {
    /// Trust on first use, then require the same certificate (normal joins).
    Pinned(KnownServers),
    /// Require exactly this fingerprint (our own hosted world: we already know its certificate).
    Exact(String),
}

#[derive(Debug)]
struct PinnedVerifier {
    server: String,
    trust: Trust,
    provider: Arc<CryptoProvider>,
    /// Set when we saw a mismatch, for a readable error message.
    mismatch: Mutex<bool>,
}

impl ServerCertVerifier for PinnedVerifier {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        let fp = fingerprint(end_entity);
        let known = match &self.trust {
            Trust::Exact(expected) => Some(expected.clone()),
            Trust::Pinned(known) => known.get(&self.server),
        };
        match known {
            Some(known) if known == fp => Ok(ServerCertVerified::assertion()),
            Some(_) => {
                *self.mismatch.lock().unwrap() = true;
                Err(rustls::Error::General("server certificate changed".into()))
            }
            None => {
                tracing::info!("first connection to {}: remembering certificate {fp}", self.server);
                if let Trust::Pinned(known) = &self.trust {
                    known
                        .remember(&self.server, &fp)
                        .map_err(|e| rustls::Error::General(format!("can't save known server: {e}")))?;
                }
                Ok(ServerCertVerified::assertion())
            }
        }
    }

    // The handshake signatures must still be checked: they prove the server owns the private
    // key of the certificate we just pinned.
    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        verify_tls12_signature(message, cert, dss, &self.provider.signature_verification_algorithms)
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        verify_tls13_signature(message, cert, dss, &self.provider.signature_verification_algorithms)
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.provider.signature_verification_algorithms.supported_schemes()
    }
}

/// Talks to the login service at `server` and returns a ticket for the game server.
/// Blocking: call it from a background thread.
pub fn request_ticket(server: SocketAddr, request: &AuthRequest, trust: Trust) -> Result<LoginTicket, String> {
    match exchange(server, request, trust)? {
        AuthResponse::Ok(ticket) => Ok(ticket),
        AuthResponse::Done { message, .. } => Err(message),
        AuthResponse::Error(e) => Err(e),
    }
}

/// A password change, reset or account deletion: (what happened, a new recovery code if any).
pub fn account_request(server: SocketAddr, request: &AuthRequest, trust: Trust) -> Result<(String, Option<String>), String> {
    match exchange(server, request, trust)? {
        AuthResponse::Done { message, recovery_code } => Ok((message, recovery_code)),
        AuthResponse::Ok(_) => Ok(("Done.".into(), None)),
        AuthResponse::Error(e) => Err(e),
    }
}

/// One request and its answer over TLS, checking the server's certificate.
fn exchange(server: SocketAddr, request: &AuthRequest, trust: Trust) -> Result<AuthResponse, String> {
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let verifier = Arc::new(PinnedVerifier {
        server: server.to_string(),
        trust,
        provider: provider.clone(),
        mismatch: Mutex::new(false),
    });
    let config = ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .map_err(|e| e.to_string())?
        .dangerous()
        .with_custom_certificate_verifier(verifier.clone())
        .with_no_client_auth();

    let tcp = TcpStream::connect_timeout(&server, Duration::from_secs(5))
        .map_err(|e| format!("can't reach the login service at {server}: {e}"))?;
    tcp.set_read_timeout(Some(Duration::from_secs(15))).ok();
    tcp.set_write_timeout(Some(Duration::from_secs(15))).ok();
    let name = ServerName::IpAddress(server.ip().into());
    let conn = ClientConnection::new(Arc::new(config), name).map_err(|e| e.to_string())?;
    let mut tls = StreamOwned::new(conn, tcp);

    let result = write_frame(&mut tls, request).and_then(|_| read_frame::<AuthResponse>(&mut tls));
    match result {
        Ok(response) => Ok(response),
        Err(_) if *verifier.mismatch.lock().unwrap() => Err(match &verifier.trust {
            Trust::Pinned(known) => format!(
                "The server's identity has changed since you last connected. If the server was \
                 reinstalled, remove its line from {}; otherwise someone may be impersonating it.",
                known.path().display()
            ),
            Trust::Exact(_) => "Something else is answering on the hosting port.".to_string(),
        }),
        Err(e) => Err(format!("login failed: {e}")),
    }
}
