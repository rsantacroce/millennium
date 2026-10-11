//! Tiny helper for TOML config files with defaults.

use serde::de::DeserializeOwned;
use std::path::Path;

/// Loads a TOML config file. A missing file yields `T::default()` (with a log line), so the
/// binaries run out of the box; a malformed file is a hard error so typos don't go unnoticed.
pub fn load_toml<T: DeserializeOwned + Default>(path: impl AsRef<Path>) -> Result<T, String> {
    let path = path.as_ref();
    match std::fs::read_to_string(path) {
        Ok(text) => toml::from_str(&text).map_err(|e| format!("{}: {e}", path.display())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            tracing::warn!("config file {} not found, using defaults", path.display());
            Ok(T::default())
        }
        Err(e) => Err(format!("{}: {e}", path.display())),
    }
}
