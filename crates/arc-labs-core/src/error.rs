//! Error taxonomy for the core.
//!
//! One thing here is not obvious and is deliberate: [`Error::public`] exists
//! because ARC-LABS has a server shell. A browser client — possibly on another
//! machine — receives these errors, and `Display` for several variants embeds an
//! absolute host path. Telling a remote caller that their request resolved to
//! `C:\Users\misho\.ssh\id_ed25519` confirms both the layout of the machine and
//! the existence of the file. So the transports render `public()`, and the full
//! `Display` form goes to the local log only.

use std::path::PathBuf;

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("vault root does not exist: {0}")]
    VaultRootMissing(PathBuf),

    #[error("vault root is not a directory: {0}")]
    VaultRootNotDirectory(PathBuf),

    #[error("path escapes the vault: {0}")]
    PathEscapesVault(PathBuf),

    #[error("path is not valid inside a vault ({reason}): {input}")]
    InvalidVaultPath { input: String, reason: &'static str },

    #[error("no such note: {0}")]
    NoteNotFound(String),

    #[error("{path} is not valid UTF-8; ARC-LABS vaults are UTF-8 only")]
    NotUtf8 { path: String },

    #[error("config error in {path}: {reason}")]
    Config { path: PathBuf, reason: String },

    #[error("io error at {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}

impl Error {
    /// A message safe to hand to a caller that may be remote.
    ///
    /// Rule: never name an absolute host path, and never confirm or deny the
    /// existence of anything outside the vault. Vault-relative paths are fine —
    /// the caller already knows them, they are what it asked about.
    pub fn public(&self) -> String {
        match self {
            Error::VaultRootMissing(_) => "vault root does not exist".into(),
            Error::VaultRootNotDirectory(_) => "vault root is not a directory".into(),
            // Deliberately says nothing about what the path resolved to.
            Error::PathEscapesVault(_) => "path is outside the vault".into(),
            Error::InvalidVaultPath { reason, .. } => format!("invalid path: {reason}"),
            Error::NoteNotFound(rel) => format!("no such note: {rel}"),
            Error::NotUtf8 { path } => format!("{path} is not valid UTF-8"),
            Error::Config { reason, .. } => format!("config error: {reason}"),
            Error::Io { source, .. } => format!("io error: {}", source.kind()),
        }
    }

    pub(crate) fn io(path: impl Into<PathBuf>, source: std::io::Error) -> Self {
        Error::Io {
            path: path.into(),
            source,
        }
    }

    pub(crate) fn invalid(input: impl Into<String>, reason: &'static str) -> Self {
        Error::InvalidVaultPath {
            input: input.into(),
            reason,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn public_form_never_leaks_a_host_path() {
        let secret = PathBuf::from(r"C:\Users\misho\.ssh\id_ed25519");
        let errors = [
            Error::PathEscapesVault(secret.clone()),
            Error::VaultRootMissing(secret.clone()),
            Error::VaultRootNotDirectory(secret.clone()),
            Error::Config {
                path: secret.clone(),
                reason: "bad key".into(),
            },
            Error::io(
                secret.clone(),
                std::io::Error::from(std::io::ErrorKind::PermissionDenied),
            ),
        ];
        for e in errors {
            let public = e.public();
            assert!(
                !public.contains("misho") && !public.contains("id_ed25519"),
                "public() leaked a host path: {public}"
            );
            // The full Display form is allowed to contain it — that goes to the
            // local log, which is how the operator debugs.
            assert!(e.to_string().contains("id_ed25519") || matches!(e, Error::Io { .. }));
        }
    }
}
