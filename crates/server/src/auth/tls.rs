//! The login service's TLS identity: a self-signed certificate created on first start.

use rustls::pki_types::pem::PemObject;
use rustls::pki_types::{CertificateDer, PrivateKeyDer};
use rustls::ServerConfig;
use std::path::Path;
use std::sync::Arc;

/// Loads `cert.pem`/`key.pem` from `dir`, creating them if missing. Returns the TLS config and
/// the certificate fingerprint (clients pin this on first connect).
pub fn load_or_create(dir: &Path) -> Result<(Arc<ServerConfig>, String), String> {
    let cert_path = dir.join("cert.pem");
    let key_path = dir.join("key.pem");
    if !cert_path.exists() || !key_path.exists() {
        std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        let generated = rcgen::generate_simple_self_signed(vec!["millennium".to_string(), "localhost".to_string()])
            .map_err(|e| e.to_string())?;
        std::fs::write(&cert_path, generated.cert.pem()).map_err(|e| e.to_string())?;
        write_private(&key_path, &generated.signing_key.serialize_pem())?;
        tracing::info!("created a new login-service certificate in {}", dir.display());
    }
    let cert = CertificateDer::from_pem_file(&cert_path).map_err(|e| format!("{}: {e}", cert_path.display()))?;
    let key = PrivateKeyDer::from_pem_file(&key_path).map_err(|e| format!("{}: {e}", key_path.display()))?;
    let fingerprint = shared::auth::client::fingerprint(&cert);
    let config = ServerConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
        .with_safe_default_protocol_versions()
        .map_err(|e| e.to_string())?
        .with_no_client_auth()
        .with_single_cert(vec![cert], key)
        .map_err(|e| e.to_string())?;
    Ok((Arc::new(config), fingerprint))
}

fn write_private(path: &Path, contents: &str) -> Result<(), String> {
    std::fs::write(path, contents).map_err(|e| e.to_string())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)).map_err(|e| e.to_string())?;
    }
    Ok(())
}
