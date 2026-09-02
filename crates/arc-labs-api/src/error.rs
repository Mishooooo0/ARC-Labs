//! API errors, shaped for crossing a transport.
//!
//! Every error that leaves this crate carries a stable machine-readable `code`
//! and a message already run through [`arc_labs_core::Error::public`], so no
//! host path reaches a caller that might be a browser on another machine. The
//! detailed form is logged locally instead.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApiError {
    /// Stable identifier the UI switches on. Never localised, never reworded —
    /// the human-readable half is `message`.
    pub code: ErrorCode,
    pub message: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    /// No vault is open. The UI shows the first-run surface.
    NoVault,
    VaultNotFound,
    NotADirectory,
    NoteNotFound,
    InvalidPath,
    NotUtf8,
    Config,
    /// The operation exists but is switched off in this deployment — e.g.
    /// filesystem browsing on a server bound beyond loopback.
    NotPermitted,
    Io,
}

impl ApiError {
    pub fn new(code: ErrorCode, message: impl Into<String>) -> Self {
        ApiError { code, message: message.into() }
    }

    pub fn no_vault() -> Self {
        ApiError::new(ErrorCode::NoVault, "no vault is open")
    }

    pub fn not_permitted(what: &str) -> Self {
        ApiError::new(ErrorCode::NotPermitted, format!("{what} is not available in this mode"))
    }
}

impl std::fmt::Display for ApiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for ApiError {}

impl From<arc_labs_core::Error> for ApiError {
    fn from(e: arc_labs_core::Error) -> Self {
        use arc_labs_core::Error as E;
        let code = match &e {
            E::VaultRootMissing(_) => ErrorCode::VaultNotFound,
            E::VaultRootNotDirectory(_) => ErrorCode::NotADirectory,
            E::NoteNotFound(_) => ErrorCode::NoteNotFound,
            E::PathEscapesVault(_) | E::InvalidVaultPath { .. } => ErrorCode::InvalidPath,
            E::NotUtf8 { .. } => ErrorCode::NotUtf8,
            E::Config { .. } => ErrorCode::Config,
            E::Io { .. } => ErrorCode::Io,
        };
        // The full Display form, which may name host paths, goes to the log.
        tracing::debug!(error = %e, "api error");
        ApiError::new(code, e.public())
    }
}

pub type ApiResult<T> = std::result::Result<T, ApiError>;

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn conversion_never_carries_a_host_path_across_the_boundary() {
        let secret = PathBuf::from(r"C:\Users\misho\.ssh\id_ed25519");
        let api: ApiError = arc_labs_core::Error::PathEscapesVault(secret).into();
        assert_eq!(api.code, ErrorCode::InvalidPath);
        assert!(!api.message.contains("misho"), "leaked: {}", api.message);
    }

    #[test]
    fn codes_are_stable_snake_case_for_the_ui_to_switch_on() {
        let json = serde_json::to_string(&ApiError::no_vault()).unwrap();
        assert!(json.contains("\"no_vault\""), "got {json}");
    }
}
