//! Configuration, loaded from `~/.config/arc-labs/config.toml`.
//!
//! Two decisions, both borrowed from ARC-Cortex because they were right there:
//!
//! **Unknown keys are a hard error.** A typo fails at load instead of silently
//! doing nothing. `themme = "arc-dark"` telling you nothing while your theme
//! quietly stays default is worse than refusing to start.
//!
//! **The same path on every platform.** `~/.config/arc-labs/` on Windows too,
//! not `%APPDATA%`. ARC-LABS runs as a desktop app, a headless server, in WSL
//! and in Docker — often against the same home directory. One path means one
//! answer to "where is my config", and the spec asks for this path by name.
//! `ARC_LABS_CONFIG` overrides it when that is not what someone wants.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};

// `Default` is written out below rather than derived: `templates_folder` has a
// non-empty default, and a derived `Default` would give it "" while serde gave
// it "Templates" — two different notions of default for one field is exactly
// the kind of quiet divergence that shows up as an empty template list.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    /// Last opened vault. Absent on a fresh install, which is what sends the
    /// user to the first-run screen.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vault: Option<PathBuf>,

    /// Who edits are attributed to in the ledger. Falls back to the OS user.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub actor_id: Option<String>,

    #[serde(default)]
    pub ui: UiConfig,

    #[serde(default)]
    pub model: ModelConfig,

    #[serde(default)]
    pub weave: WeaveConfig,

    #[serde(default)]
    pub trash: TrashConfig,

    /// Folder holding note templates, relative to the vault root.
    ///
    /// A setting rather than a constant because plenty of vaults already keep
    /// `_templates/` or `meta/templates/`, and telling someone to move their
    /// files to suit the app is the wrong way round. The default matches
    /// Obsidian's own convention, so templates work in both without moving
    /// anything.
    #[serde(default = "default_templates_folder")]
    pub templates_folder: String,
}

fn default_templates_folder() -> String {
    "Templates".into()
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct UiConfig {
    pub theme: String,
    /// Multiplier on the 120-200 ms motion budget. `0.0` disables animation
    /// outright, which is both an accessibility setting and a way to prove the
    /// app is not relying on motion to explain itself.
    pub motion: f32,
    pub density: Density,
}

impl Default for UiConfig {
    fn default() -> Self {
        UiConfig {
            theme: "arc-dark".into(),
            motion: 1.0,
            density: Density::Comfortable,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Density {
    Compact,
    Comfortable,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ModelConfig {
    pub endpoint: String,
    pub instruct: String,
    pub embed: String,
    pub access: ModelAccess,
}

impl Default for ModelConfig {
    fn default() -> Self {
        ModelConfig {
            endpoint: "http://localhost:11434".into(),
            // This node has no GPU and ~0.24 GB/s effective memory bandwidth.
            // A 0.8B model is the one that stays interactive here; anything
            // larger is a deliberate choice the user makes, not a default.
            instruct: "qwen3.5:0.8b".into(),
            embed: "nomic-embed-text".into(),
            access: ModelAccess::LocalOnly,
        }
    }
}

/// The background link-suggestion daemon.
///
/// Every field here is a ceiling rather than a target. Weave shares a machine
/// with the editor and loses every argument with it, so the knobs are the ones
/// that make it quieter, not faster.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct WeaveConfig {
    /// Off by default.
    ///
    /// It embeds every note in the vault, which on a first run is real work on
    /// someone else's machine. That is a thing to opt into, not to discover.
    pub enabled: bool,
    /// Cosine similarity above which a pair is worth suggesting.
    ///
    /// Deliberately high. An inbox full of weak suggestions trains you to ignore
    /// it, which costs more than showing nothing would.
    pub threshold: f64,
    /// Fraction of one core, averaged over a minute. The spec's ceiling is 0.15
    /// and this cannot exceed it.
    pub cpu_fraction: f64,
    /// Seconds between passes when there is nothing left to do.
    pub interval_secs: u64,
}

/// How long a deleted note's copy survives on disk.
///
/// The ledger and its object store are the primary way a deleted note comes
/// back, and they keep it for ever. The trash copy is a **second** guarantee —
/// against a bad click, a corrupt ledger, or a bug in the restore path — and
/// unlike the ledger it costs a full copy of every note ever deleted, for ever.
/// So this one expires and the ledger does not.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct TrashConfig {
    /// Days a deleted note stays in `.arc/trash/` before it is purged.
    ///
    /// `0` keeps it for ever. Purging is not losing the note: restore replays
    /// content from the object store by hash, never from the trash copy.
    pub retention_days: u32,
}

impl Default for TrashConfig {
    fn default() -> Self {
        // A week. Long enough to notice a mistake, short enough that a vault
        // does not silently carry every note you ever deleted.
        TrashConfig { retention_days: 7 }
    }
}

impl Default for Config {
    fn default() -> Self {
        Config {
            vault: None,
            actor_id: None,
            ui: UiConfig::default(),
            model: ModelConfig::default(),
            weave: WeaveConfig::default(),
            trash: TrashConfig::default(),
            templates_folder: default_templates_folder(),
        }
    }
}

impl Default for WeaveConfig {
    fn default() -> Self {
        WeaveConfig {
            enabled: false,
            threshold: 0.82,
            cpu_fraction: 0.15,
            interval_secs: 60,
        }
    }
}

/// Where vault bytes are allowed to go. Not merely configuration — every choice
/// other than `LocalOnly` produces an `egress` ledger entry per run (Phase 5).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ModelAccess {
    LocalOnly,
    TrustedEndpoint,
    AskEachRun,
}

impl Config {
    /// Where the config lives: `$ARC_LABS_CONFIG`, else
    /// `~/.config/arc-labs/config.toml`.
    pub fn default_path() -> Option<PathBuf> {
        if let Some(explicit) = std::env::var_os("ARC_LABS_CONFIG") {
            return Some(PathBuf::from(explicit));
        }
        home_dir().map(|h| h.join(".config").join("arc-labs").join("config.toml"))
    }

    /// Load from `path`. A missing file is not an error — it is a first run.
    pub fn load(path: &std::path::Path) -> Result<Config> {
        let text = match std::fs::read_to_string(path) {
            Ok(t) => t,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Config::default()),
            Err(e) => return Err(Error::io(path, e)),
        };
        Config::parse(&text).map_err(|reason| Error::Config {
            path: path.to_path_buf(),
            reason,
        })
    }

    pub fn parse(text: &str) -> std::result::Result<Config, String> {
        toml::from_str(text).map_err(|e| e.message().to_string())
    }

    pub fn to_toml(&self) -> String {
        // Serializing our own struct cannot fail.
        toml::to_string_pretty(self).unwrap_or_default()
    }

    /// Write to disk, atomically, refusing to write something unreadable.
    ///
    /// Two guards, both learned the hard way elsewhere in this codebase:
    ///
    /// **Atomic.** A config truncated by a crash mid-write is a config that
    /// fails to parse at next boot, and `deny_unknown_fields` means a
    /// half-written file is a hard error rather than a shrug. Losing your
    /// settings because the machine rebooted at the wrong instant is not a
    /// trade anyone would make.
    ///
    /// **Round-tripped first.** What is serialised is parsed back before it
    /// touches the disk. If this build can produce something it cannot read —
    /// which is exactly what a bad `deny_unknown_fields` interaction looks
    /// like — the write is refused and the old config survives.
    pub fn save(&self, path: &std::path::Path) -> Result<()> {
        let text = self.to_toml();

        Config::parse(&text).map_err(|reason| Error::Config {
            path: path.to_path_buf(),
            reason: format!("refusing to write a config this build cannot read back: {reason}"),
        })?;

        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| Error::io(dir, e))?;
        }
        crate::atomic::replace(path, text.as_bytes())
    }

    /// Vault root from the environment, if set. Checked before the config file
    /// so `ARC_LABS_VAULT=/vault` works in Docker with no config at all.
    pub fn vault_from_env() -> Option<PathBuf> {
        std::env::var_os("ARC_LABS_VAULT")
            .map(PathBuf::from)
            .filter(|p| !p.as_os_str().is_empty())
    }

    /// Ledger actor id: configured value, else the OS user, else `unknown`.
    pub fn resolved_actor_id(&self) -> String {
        self.actor_id
            .clone()
            // Explicit, for a deployment. A container has no `$USER`, so every
            // entry a server writes was landing in the ledger as "unknown" —
            // which empties out the one question the ledger exists to answer.
            .or_else(|| std::env::var("ARC_LABS_ACTOR").ok())
            .or_else(|| std::env::var("USER").ok())
            .or_else(|| std::env::var("USERNAME").ok())
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "unknown".into())
    }
}

fn home_dir() -> Option<PathBuf> {
    // Read explicitly rather than via std::env::home_dir, whose behaviour has
    // changed across releases. HOME first so WSL and Docker agree.
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
        .filter(|p| !p.as_os_str().is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A container has no `$USER`, and every ledger entry a deployed server
    /// wrote was landing as "unknown" — which empties out the one question the
    /// ledger exists to answer. Found by deploying and reading the result back.
    #[test]
    fn a_deployment_can_name_its_actor() {
        let config = Config::default();

        std::env::set_var("ARC_LABS_ACTOR", "arc1-node");
        assert_eq!(config.resolved_actor_id(), "arc1-node");

        // Whitespace is a copy-paste artefact, not part of a name.
        std::env::set_var(
            "ARC_LABS_ACTOR",
            "  arc1-node 
",
        );
        assert_eq!(config.resolved_actor_id(), "arc1-node");

        // The config file is more explicit than the environment, so it wins.
        let explicit = Config {
            actor_id: Some("mishal".into()),
            ..Default::default()
        };
        assert_eq!(explicit.resolved_actor_id(), "mishal");

        std::env::remove_var("ARC_LABS_ACTOR");
    }

    #[test]
    fn empty_config_is_valid_and_all_defaults() {
        let c = Config::parse("").unwrap();
        assert_eq!(c.ui.theme, "arc-dark");
        assert_eq!(c.model.access, ModelAccess::LocalOnly);
        assert_eq!(c.model.endpoint, "http://localhost:11434");
        assert!(c.vault.is_none());
    }

    #[test]
    fn a_typo_is_a_hard_error_not_a_silent_default() {
        // The entire reason for deny_unknown_fields.
        let err = Config::parse("[ui]\nthemme = \"arc-light\"\n").unwrap_err();
        assert!(
            err.contains("themme") || err.contains("unknown"),
            "unhelpful error: {err}"
        );

        assert!(Config::parse("vaultt = \"/notes\"\n").is_err());
        assert!(Config::parse("[modle]\nendpoint = \"x\"\n").is_err());
    }

    #[test]
    fn rejects_a_wrong_value_for_an_enum() {
        assert!(Config::parse("[model]\naccess = \"send-everything\"\n").is_err());
        assert!(Config::parse("[ui]\ndensity = \"roomy\"\n").is_err());
    }

    #[test]
    fn round_trips_through_toml() {
        let c = Config {
            vault: Some(PathBuf::from("/notes")),
            actor_id: Some("mishal".into()),
            model: ModelConfig {
                access: ModelAccess::AskEachRun,
                ..Default::default()
            },
            ..Default::default()
        };

        let back = Config::parse(&c.to_toml()).unwrap();
        assert_eq!(back, c);
    }

    #[test]
    fn partial_sections_keep_the_other_defaults() {
        let c = Config::parse("[model]\ninstruct = \"cogito:3b\"\n").unwrap();
        assert_eq!(c.model.instruct, "cogito:3b");
        assert_eq!(c.model.embed, "nomic-embed-text");
        assert_eq!(c.model.access, ModelAccess::LocalOnly);
    }

    #[test]
    fn a_missing_file_is_a_first_run_not_a_failure() {
        let tmp = tempfile::tempdir().unwrap();
        let c = Config::load(&tmp.path().join("nope.toml")).unwrap();
        assert_eq!(c, Config::default());
    }
}
