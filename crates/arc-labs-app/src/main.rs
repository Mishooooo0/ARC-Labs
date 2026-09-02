//! The Tauri desktop shell for Windows and Linux.
//!
//! Every command below is a wrapper over [`arc_labs_api::Api`] and contains no
//! logic of its own. If one ever grows a branch, that branch would exist for
//! desktop users and not for browser users — which is the exact class of bug the
//! one-core/one-API architecture exists to make impossible. Keep them boring.

// Windows: no console window behind the app in a release build. Kept in debug so
// `tracing` output stays visible while developing.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::path::PathBuf;
use std::sync::Arc;

use arc_labs_api::{Api, ApiError, Capabilities, DirListing, NoteView, Status, TreeView, VaultInfo};
use arc_labs_core::{Config, VaultPath};
use tauri::Manager;
use tauri_plugin_dialog::DialogExt;

type CmdResult<T> = Result<T, ApiError>;

#[tauri::command]
fn status(api: tauri::State<'_, Arc<Api>>) -> Status {
    api.status()
}

#[tauri::command]
fn tree(api: tauri::State<'_, Arc<Api>>) -> CmdResult<TreeView> {
    api.tree()
}

#[tauri::command]
fn note(api: tauri::State<'_, Arc<Api>>, path: VaultPath) -> CmdResult<NoteView> {
    // `path` deserialises through VaultPath, so containment is enforced before
    // this body runs — the same guarantee the HTTP shell gets, from one place.
    api.read_note(&path)
}

#[tauri::command]
fn browse(api: tauri::State<'_, Arc<Api>>, path: Option<String>) -> CmdResult<DirListing> {
    api.browse(path.as_deref().map(std::path::Path::new))
}

#[tauri::command]
fn open_vault(api: tauri::State<'_, Arc<Api>>, path: String) -> CmdResult<VaultInfo> {
    api.open_vault(std::path::Path::new(&path))
}

/// The native folder dialog — the one capability the browser shell cannot have,
/// and the reason `Transport` has a `pickFolder` that returns `null` there.
#[tauri::command]
async fn pick_folder(app: tauri::AppHandle) -> Option<String> {
    let (tx, rx) = tokio::sync::oneshot::channel();
    app.dialog().file().set_title("Open a vault").pick_folder(move |chosen| {
        let _ = tx.send(chosen.map(|p| p.to_string()));
    });
    rx.await.ok().flatten()
}

fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_env("ARC_LABS_LOG")
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("arc_labs=info")),
        )
        .with_target(false)
        .init();

    let config_path = Config::default_path();
    let config = config_path
        .as_deref()
        .map(|p| Config::load(p).unwrap_or_default())
        .unwrap_or_default();

    // The desktop user is sitting at the machine, so the full capability set.
    let api = Arc::new(Api::new(config.clone(), config_path, Capabilities::desktop()));

    // `--vault` for parity with the CLI, then ARC_LABS_VAULT, then last used.
    let explicit = std::env::args()
        .skip_while(|a| a != "--vault")
        .nth(1)
        .map(PathBuf::from);
    if let Some(path) = api.resolve_startup_vault(explicit) {
        match api.open_vault(&path) {
            Ok(info) => tracing::info!(vault = %info.name, notes = info.note_count, "vault open"),
            // Not fatal — the first-run screen exists for exactly this.
            Err(e) => tracing::warn!(error = %e, "could not open the configured vault"),
        }
    }

    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .manage(api)
        .invoke_handler(tauri::generate_handler![
            status,
            tree,
            note,
            browse,
            open_vault,
            pick_folder
        ])
        .setup(|app| {
            if let Some(window) = app.get_webview_window("main") {
                let _ = window.set_title("ARC-LABS");
            }
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("failed to start the ARC-LABS window");
}
