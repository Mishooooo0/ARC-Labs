//! ARC-LABS core: the vault model.
//!
//! No Tauri, no HTTP, no MCP, no globals, and — deliberately — no dependency
//! anywhere in this crate's tree that can open a socket. Everything here is
//! testable with `cargo test`: no window, no server, no model. That is what lets
//! one implementation back four shells without four sets of bugs, and it is what
//! makes "fully offline" a property of the dependency graph rather than a promise.

pub mod atomic;
pub mod config;
pub mod error;
pub mod fidelity;
pub mod markdown;
pub mod path;
pub mod seam;
pub mod tree;
pub mod vault;

pub use config::{Config, Density, ModelAccess, UiConfig, WeaveConfig};
pub use error::{Error, Result};
pub use fidelity::{FileFidelity, LineEnding, NoteText};
pub use markdown::{render, Heading, RenderedNote, WikiLink};
pub use path::{VaultPath, VaultRoot};
pub use tree::{Tree, TreeEntry};
pub use vault::{Saved, Vault};
