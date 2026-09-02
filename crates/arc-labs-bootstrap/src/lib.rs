//! Environment detection, diagnosis and dependency installation.
//!
//! This is the **only** crate in ARC-LABS permitted an outbound network client.
//! `arc-labs-core` cannot depend on it, which is what turns "fully offline" from
//! a promise into a property of the dependency graph. See [`setup`] for how that
//! coexists with a command whose whole job is to download things.

pub mod doctor;
pub mod platform;
pub mod setup;

pub use doctor::{Level, Report};
pub use platform::{Os, PackageManager, Platform};
