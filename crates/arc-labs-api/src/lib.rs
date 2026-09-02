//! Every ARC-LABS operation, defined exactly once.
//!
//! This crate is the reason four shells cost one implementation. A Tauri
//! command, an HTTP handler and (from Phase 6) an MCP tool are each a ten-line
//! wrapper around a method here. An operation added once appears in all of them;
//! an operation that forgets to go through the ledger cannot exist in one shell
//! and not another, because there is only one place it could have been written.
//!
//! Nothing here knows about Tauri, HTTP, WebSockets or MCP. That is what keeps
//! the whole API surface testable with `cargo test` and no socket.
//!
//! # Phase 0 is read-only
//!
//! There is no write operation on [`Api`], not even a private one. The Phase 0
//! gate is that a real vault is byte-identical after an hour of browsing, and
//! the way to guarantee that is for the capability not to exist yet.

pub mod error;
pub mod types;

use std::path::{Path, PathBuf};
use std::sync::RwLock;

use arc_labs_core::{Config, LineEnding, Vault, VaultPath};

pub use error::{ApiError, ApiResult, ErrorCode};
pub use types::*;

/// What this deployment is allowed to do beyond reading an open vault.
///
/// Filesystem browsing is the interesting one. The browser shell needs it —
/// there is no native folder picker in a browser, so picking a vault means
/// listing directories. But on a server bound past loopback, that same operation
/// is a remote directory-listing primitive for anyone who can reach the port. So
/// it is a capability the shell grants, not a method that always exists.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Capabilities {
    pub shell: Shell,
    pub browse_filesystem: bool,
    pub native_folder_picker: bool,
    /// Whether absolute paths may be shown to the caller. False when serving a
    /// client that has no business learning the server's layout.
    pub expose_paths: bool,
}

impl Capabilities {
    /// Desktop: the user is at the machine, so everything is theirs to see.
    pub fn desktop() -> Self {
        Capabilities {
            shell: Shell::Desktop,
            browse_filesystem: true,
            native_folder_picker: true,
            expose_paths: true,
        }
    }

    /// A server on loopback: same machine, same person, but no native dialogs.
    pub fn local_server() -> Self {
        Capabilities {
            shell: Shell::Server,
            browse_filesystem: true,
            native_folder_picker: false,
            expose_paths: true,
        }
    }

    /// A server reachable from elsewhere. The vault must be chosen at launch
    /// (`--vault` or `ARC_LABS_VAULT`), and the filesystem stays invisible.
    pub fn remote_server() -> Self {
        Capabilities {
            shell: Shell::Server,
            browse_filesystem: false,
            native_folder_picker: false,
            expose_paths: false,
        }
    }
}

struct State {
    config: Config,
    config_path: Option<PathBuf>,
    vault: Option<Vault>,
    status: VaultStatus,
}

pub struct Api {
    caps: Capabilities,
    state: RwLock<State>,
}

impl Api {
    pub fn new(config: Config, config_path: Option<PathBuf>, caps: Capabilities) -> Api {
        Api {
            caps,
            state: RwLock::new(State {
                config,
                config_path,
                vault: None,
                status: VaultStatus::Offline,
            }),
        }
    }

    pub fn capabilities(&self) -> Capabilities {
        self.caps
    }

    /// Resolve the vault to open at startup: explicit flag, then environment,
    /// then the last one used. The same order on every platform, so a Docker
    /// user setting `ARC_LABS_VAULT` and a desktop user who opened one last week
    /// both get what they expect.
    pub fn resolve_startup_vault(&self, explicit: Option<PathBuf>) -> Option<PathBuf> {
        explicit
            .or_else(Config::vault_from_env)
            .or_else(|| self.state.read().ok()?.config.vault.clone())
    }

    pub fn status(&self) -> Status {
        let state = self.state.read().expect("state lock poisoned");
        Status {
            status: state.status,
            shell: self.caps.shell,
            version: env!("CARGO_PKG_VERSION").to_string(),
            vault: state.vault.as_ref().map(|v| self.vault_info(v)),
            can_browse: self.caps.browse_filesystem,
            can_pick_folder: self.caps.native_folder_picker,
        }
    }

    fn vault_info(&self, vault: &Vault) -> VaultInfo {
        // Counts come from a tree walk, which is cheap enough at Phase 0 scale
        // and becomes an index lookup in Phase 2.
        let (notes, canvases) = vault.tree().map(|t| (t.note_count, t.canvas_count)).unwrap_or((0, 0));
        VaultInfo {
            name: vault.name(),
            path: self.caps.expose_paths.then(|| vault.root().path().display().to_string()),
            note_count: notes,
            canvas_count: canvases,
        }
    }

    /// Open a vault and remember it. Persisting is best-effort: a read-only
    /// config directory (common in Docker) should not stop the vault opening.
    pub fn open_vault(&self, path: &Path) -> ApiResult<VaultInfo> {
        let vault = Vault::open(path)?;
        let info = self.vault_info(&vault);

        let mut state = self.state.write().expect("state lock poisoned");
        state.config.vault = Some(vault.root().path().to_path_buf());
        state.vault = Some(vault);
        state.status = VaultStatus::Online;

        if let (Some(p), cfg) = (state.config_path.clone(), state.config.clone()) {
            if let Some(dir) = p.parent() {
                let _ = std::fs::create_dir_all(dir);
            }
            if let Err(e) = std::fs::write(&p, cfg.to_toml()) {
                tracing::warn!(path = %p.display(), error = %e, "could not persist config");
            }
        }
        Ok(info)
    }

    pub fn close_vault(&self) {
        let mut state = self.state.write().expect("state lock poisoned");
        state.vault = None;
        state.status = VaultStatus::Offline;
    }

    fn with_vault<T>(&self, f: impl FnOnce(&Vault) -> ApiResult<T>) -> ApiResult<T> {
        let state = self.state.read().expect("state lock poisoned");
        f(state.vault.as_ref().ok_or_else(ApiError::no_vault)?)
    }

    pub fn tree(&self) -> ApiResult<TreeView> {
        self.with_vault(|v| Ok(TreeView { tree: v.tree()? }))
    }

    pub fn read_note(&self, path: &VaultPath) -> ApiResult<NoteView> {
        self.read_note_inner(path, false)
    }

    /// Read a note including its raw markdown, for the editor.
    ///
    /// Separate from [`Self::read_note`] so a plain render never ships the
    /// source over the wire twice.
    pub fn read_note_for_edit(&self, path: &VaultPath) -> ApiResult<NoteView> {
        self.read_note_inner(path, true)
    }

    fn read_note_inner(&self, path: &VaultPath, with_text: bool) -> ApiResult<NoteView> {
        self.with_vault(|v| {
            let note = v.read_note(path)?;
            let rendered = arc_labs_core::render(note.text());
            Ok(NoteView {
                name: path.file_name().to_string(),
                path: path.clone(),
                html: rendered.html,
                frontmatter: rendered.frontmatter,
                links: rendered.links.iter().map(Into::into).collect(),
                embeds: rendered.embeds.iter().map(Into::into).collect(),
                tags: rendered.tags,
                size: note.text().len(),
                line_ending: match note.fidelity().line_ending() {
                    LineEnding::Lf => "LF",
                    LineEnding::Crlf => "CRLF",
                }
                .into(),
                line_endings_mixed: note.fidelity().is_mixed(),
                text: with_text.then(|| note.text().to_string()),
                hash: note.content_hash(),
            })
        })
    }

    /// Save a note.
    ///
    /// `base_hash` is the hash the editor started from. When it does not match
    /// what is on disk, the save is refused rather than applied: a vault is
    /// frequently open in Obsidian, syncing over Syncthing, or being changed by
    /// git at the same time, and silently discarding someone else's write is the
    /// one failure mode a notebook must not have.
    ///
    /// Fidelity comes from the file as it is *now*, so line endings and BOM stay
    /// the file's own even across an external change.
    pub fn write_note(
        &self,
        path: &VaultPath,
        text: &str,
        base_hash: Option<&str>,
    ) -> ApiResult<SaveResult> {
        self.with_vault(|v| {
            let current = v.read_note(path)?;
            if let Some(base) = base_hash {
                if current.content_hash() != base {
                    return Err(ApiError::conflict());
                }
            }

            let saved = v.write_note(path, &current, text)?;
            // Hash the text we just committed to, so the editor's next save has
            // the right base whether or not bytes were actually written.
            let hash = arc_labs_core::NoteText::decode(text.as_bytes())
                .map(|n| n.content_hash())
                .unwrap_or_else(|| current.content_hash());

            Ok(match saved {
                arc_labs_core::Saved::Written { bytes } => {
                    SaveResult { written: true, bytes, hash }
                }
                arc_labs_core::Saved::Unchanged => {
                    SaveResult { written: false, bytes: 0, hash }
                }
            })
        })
    }

    pub fn config(&self) -> Config {
        self.state.read().expect("state lock poisoned").config.clone()
    }

    /// List directories under `path` so a browser client can choose a vault.
    ///
    /// Directories only, and gated on [`Capabilities::browse_filesystem`].
    pub fn browse(&self, path: Option<&Path>) -> ApiResult<DirListing> {
        if !self.caps.browse_filesystem {
            return Err(ApiError::not_permitted("browsing the filesystem"));
        }

        let dir = match path {
            Some(p) => p.to_path_buf(),
            None => home_dir().unwrap_or_else(|| PathBuf::from("/")),
        };
        let dir = dunce_canonicalize(&dir)?;

        let mut entries = Vec::new();
        let read = std::fs::read_dir(&dir)
            .map_err(|e| ApiError::new(ErrorCode::Io, format!("cannot list directory: {}", e.kind())))?;
        for entry in read.flatten() {
            if !entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                continue;
            }
            let name = entry.file_name().to_string_lossy().into_owned();
            // Hidden directories are noise when picking a notes folder.
            if name.starts_with('.') {
                continue;
            }
            let p = entry.path();
            entries.push(DirEntry {
                is_vault: p.join(".obsidian").is_dir() || p.join(".arc").is_dir(),
                name,
                path: p.display().to_string(),
            });
        }
        entries.sort_by_key(|e| e.name.to_lowercase());

        Ok(DirListing {
            parent: dir.parent().map(|p| p.display().to_string()),
            path: dir.display().to_string(),
            entries,
        })
    }
}

fn dunce_canonicalize(p: &Path) -> ApiResult<PathBuf> {
    dunce::canonicalize(p).map_err(|e| match e.kind() {
        std::io::ErrorKind::NotFound => ApiError::new(ErrorCode::VaultNotFound, "no such directory"),
        _ => ApiError::new(ErrorCode::Io, format!("cannot open directory: {}", e.kind())),
    })
}

fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
        .filter(|p| !p.as_os_str().is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn api_with_vault(caps: Capabilities) -> (tempfile::TempDir, Api) {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp.path().join("Daily")).unwrap();
        std::fs::write(tmp.path().join("a.md"), b"# A\n\nlink to [[B]] #tag\n").unwrap();
        std::fs::write(tmp.path().join("Daily/b.md"), b"# B\n").unwrap();
        std::fs::write(tmp.path().join("board.canvas"), b"{}").unwrap();

        let api = Api::new(Config::default(), None, caps);
        api.open_vault(tmp.path()).unwrap();
        (tmp, api)
    }

    #[test]
    fn status_reflects_whether_a_vault_is_open() {
        let api = Api::new(Config::default(), None, Capabilities::desktop());
        assert_eq!(api.status().status, VaultStatus::Offline);
        assert!(api.status().vault.is_none());

        let (_t, api) = api_with_vault(Capabilities::desktop());
        let s = api.status();
        assert_eq!(s.status, VaultStatus::Online);
        assert_eq!(s.vault.as_ref().unwrap().note_count, 2);
        assert_eq!(s.vault.as_ref().unwrap().canvas_count, 1);
    }

    #[test]
    fn operations_without_a_vault_say_so_rather_than_panicking() {
        let api = Api::new(Config::default(), None, Capabilities::desktop());
        assert_eq!(api.tree().unwrap_err().code, ErrorCode::NoVault);
        let p = VaultPath::new("a.md").unwrap();
        assert_eq!(api.read_note(&p).unwrap_err().code, ErrorCode::NoVault);
    }

    #[test]
    fn reads_and_renders_a_note() {
        let (_t, api) = api_with_vault(Capabilities::desktop());
        let n = api.read_note(&VaultPath::new("a.md").unwrap()).unwrap();
        assert_eq!(n.name, "a.md");
        assert!(n.html.contains("<h1>A</h1>"));
        assert_eq!(n.links[0].target, "B");
        assert_eq!(n.tags, ["tag"]);
        assert_eq!(n.line_ending, "LF");
        // Phase 0 has no index, so resolution is unknown — not guessed.
        assert_eq!(n.links[0].resolved, None);
    }

    #[test]
    fn editing_returns_the_source_and_a_base_hash() {
        let (_t, api) = api_with_vault(Capabilities::desktop());
        let p = VaultPath::new("a.md").unwrap();

        assert!(api.read_note(&p).unwrap().text.is_none(), "a render should not ship the source");

        let edit = api.read_note_for_edit(&p).unwrap();
        assert_eq!(edit.text.as_deref(), Some("# A\n\nlink to [[B]] #tag\n"));
        assert!(edit.hash.starts_with("blake3:"));
    }

    #[test]
    fn saving_unchanged_text_writes_nothing() {
        let (_t, api) = api_with_vault(Capabilities::desktop());
        let p = VaultPath::new("a.md").unwrap();
        let note = api.read_note_for_edit(&p).unwrap();

        let r = api.write_note(&p, note.text.as_deref().unwrap(), Some(&note.hash)).unwrap();
        assert!(!r.written);
        assert_eq!(r.hash, note.hash);
    }

    #[test]
    fn saving_changed_text_writes_and_returns_a_new_base() {
        let (_t, api) = api_with_vault(Capabilities::desktop());
        let p = VaultPath::new("a.md").unwrap();
        let note = api.read_note_for_edit(&p).unwrap();

        let r = api.write_note(&p, "# A changed\n", Some(&note.hash)).unwrap();
        assert!(r.written && r.bytes > 0);
        assert_ne!(r.hash, note.hash);

        // The returned hash is the right base for the next save.
        assert!(api.write_note(&p, "# A changed again\n", Some(&r.hash)).is_ok());
    }

    #[test]
    fn a_stale_base_hash_is_refused_rather_than_clobbering_the_other_writer() {
        let (tmp, api) = api_with_vault(Capabilities::desktop());
        let p = VaultPath::new("a.md").unwrap();
        let note = api.read_note_for_edit(&p).unwrap();

        // Someone else — Obsidian, Syncthing, git — writes to the same file.
        std::fs::write(tmp.path().join("a.md"), b"# written by someone else\n").unwrap();

        let err = api.write_note(&p, "# my version\n", Some(&note.hash)).unwrap_err();
        assert_eq!(err.code, ErrorCode::Conflict);
        // Their work is still there.
        assert_eq!(
            std::fs::read(tmp.path().join("a.md")).unwrap(),
            b"# written by someone else\n"
        );
    }

    #[test]
    fn a_remote_server_hides_the_filesystem_and_the_vault_path() {
        let (_t, api) = api_with_vault(Capabilities::remote_server());

        // The directory-listing primitive simply is not available.
        assert_eq!(api.browse(None).unwrap_err().code, ErrorCode::NotPermitted);

        // And the client is not told where on the server the vault lives.
        let s = api.status();
        assert!(s.vault.unwrap().path.is_none());
        assert!(!s.can_browse && !s.can_pick_folder);
    }

    #[test]
    fn a_local_shell_can_browse_but_only_sees_directories() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir(tmp.path().join("Notes")).unwrap();
        std::fs::create_dir(tmp.path().join(".hidden")).unwrap();
        std::fs::write(tmp.path().join("file.txt"), b"x").unwrap();

        let api = Api::new(Config::default(), None, Capabilities::local_server());
        let listing = api.browse(Some(tmp.path())).unwrap();

        let names: Vec<&str> = listing.entries.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, ["Notes"], "should list directories only, and no dotfiles");
        assert!(listing.parent.is_some());
    }

    #[test]
    fn browse_marks_folders_that_are_already_vaults() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp.path().join("MyVault/.obsidian")).unwrap();
        std::fs::create_dir(tmp.path().join("Plain")).unwrap();

        let api = Api::new(Config::default(), None, Capabilities::desktop());
        let listing = api.browse(Some(tmp.path())).unwrap();
        let vault = listing.entries.iter().find(|e| e.name == "MyVault").unwrap();
        let plain = listing.entries.iter().find(|e| e.name == "Plain").unwrap();
        assert!(vault.is_vault);
        assert!(!plain.is_vault);
    }

    #[test]
    fn startup_vault_resolution_prefers_the_explicit_flag() {
        let api = Api::new(
            Config { vault: Some(PathBuf::from("/from-config")), ..Default::default() },
            None,
            Capabilities::desktop(),
        );
        assert_eq!(
            api.resolve_startup_vault(Some(PathBuf::from("/from-flag"))),
            Some(PathBuf::from("/from-flag"))
        );
        assert_eq!(api.resolve_startup_vault(None), Some(PathBuf::from("/from-config")));
    }

    #[test]
    fn opening_a_missing_vault_is_a_clean_error() {
        let api = Api::new(Config::default(), None, Capabilities::desktop());
        let err = api.open_vault(Path::new("/definitely/not/here")).unwrap_err();
        assert_eq!(err.code, ErrorCode::VaultNotFound);
        assert!(!err.message.contains("definitely"), "echoed the path back: {}", err.message);
    }
}
