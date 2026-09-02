//! `arc-labs` — the composition root.
//!
//! The one place that knows about every other crate. Everything below it is a
//! library with no opinion about how it is invoked, which is what lets the same
//! code back this binary, the Tauri desktop shell and (from Phase 6) an MCP
//! server without any of them importing each other.

use std::net::IpAddr;
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{Context, Result};
use arc_labs_api::Api;
use arc_labs_bootstrap::{doctor, setup};
use arc_labs_core::Config;
use arc_labs_server::{generate_token, ServerConfig};
use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(
    name = "arc-labs",
    version,
    about = "A notebook your agents can hold — plain markdown, Obsidian-compatible, with provenance."
)]
struct Cli {
    /// Vault to open. Overrides ARC_LABS_VAULT and the last vault used.
    #[arg(long, global = true, value_name = "PATH")]
    vault: Option<PathBuf>,

    /// Config file. Defaults to ~/.config/arc-labs/config.toml on every platform.
    #[arg(long, global = true, value_name = "PATH")]
    config: Option<PathBuf>,

    #[arg(long, global = true, default_value = "info", value_name = "LEVEL")]
    log: String,

    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// Serve the notebook over HTTP for a browser. The headless and Docker mode.
    Serve {
        /// Address to bind. Anything other than a loopback address requires a
        /// token, which is generated and printed at startup.
        #[arg(long, default_value = "127.0.0.1")]
        host: IpAddr,
        #[arg(long, default_value_t = 7777)]
        port: u16,
        /// Directory holding the built UI.
        #[arg(long, value_name = "PATH")]
        ui: Option<PathBuf>,
    },
    /// Report on this machine and what ARC-LABS needs from it. Never installs.
    Doctor,
    /// Rebuild the derived index from the vault.
    ///
    /// Safe at any time: the index holds nothing the vault does not, so the
    /// worst case of running this is that it takes a few seconds.
    Reindex {
        /// Re-render every note instead of skipping unchanged ones.
        #[arg(long)]
        force: bool,
        /// Delete the database first. The recovery path, exercised on purpose.
        #[arg(long)]
        clean: bool,
    },
    /// Install the system packages this platform is missing. Asks first.
    Setup {
        /// Skip the prompt. For Docker builds and CI — never the interactive default.
        #[arg(long)]
        yes: bool,
    },
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    init_logging(&cli.log);

    let config_path = cli.config.clone().or_else(Config::default_path);
    let config = match &config_path {
        Some(p) => Config::load(p).with_context(|| format!("loading {}", p.display()))?,
        None => Config::default(),
    };

    match cli.command {
        Some(Command::Doctor) => cmd_doctor(&cli, &config),
        Some(Command::Reindex { force, clean }) => cmd_reindex(&cli, config, config_path, force, clean),
        Some(Command::Setup { yes }) => cmd_setup(&cli, &config, yes),
        Some(Command::Serve { host, port, ui }) => {
            cmd_serve(cli.vault, config, config_path, host, port, ui)
        }
        // No subcommand is the common case: serve locally. The desktop window is
        // a separate binary, so the CLI's default is the mode the CLI can do.
        None => cmd_serve(cli.vault, config, config_path, "127.0.0.1".parse()?, 7777, None),
    }
}

fn init_logging(level: &str) {
    use tracing_subscriber::EnvFilter;
    let filter = EnvFilter::try_from_env("ARC_LABS_LOG")
        .unwrap_or_else(|_| EnvFilter::new(format!("arc_labs={level},tower_http=warn")));
    tracing_subscriber::fmt().with_env_filter(filter).with_target(false).init();
}

/// Vault to use: `--vault`, then `ARC_LABS_VAULT`, then the last one opened.
fn resolve_vault(explicit: Option<PathBuf>, config: &Config) -> Option<PathBuf> {
    explicit.or_else(Config::vault_from_env).or_else(|| config.vault.clone())
}

fn cmd_doctor(cli: &Cli, config: &Config) -> Result<()> {
    let vault = resolve_vault(cli.vault.clone(), config);
    let report = doctor::run(vault.as_deref(), &config.model.endpoint);
    print!("{}", report.render());

    let plan = setup::plan(&report);
    if !plan.is_empty() {
        println!("\nRun `arc-labs setup` to install what is missing.");
    }
    std::process::exit(report.exit_code());
}

fn cmd_setup(cli: &Cli, config: &Config, yes: bool) -> Result<()> {
    let vault = resolve_vault(cli.vault.clone(), config);
    let report = doctor::run(vault.as_deref(), &config.model.endpoint);
    let plan = setup::plan(&report);

    let outcome = setup::execute(&plan, |text| {
        print!("{text}");
        if yes {
            println!("\nProceeding (--yes).");
            return true;
        }
        print!("\nProceed? [y/N] ");
        use std::io::Write;
        let _ = std::io::stdout().flush();
        let mut answer = String::new();
        // A closed stdin reads zero bytes, which must mean "no" rather than
        // "yes" — a piped invocation must never install by accident.
        if std::io::stdin().read_line(&mut answer).unwrap_or(0) == 0 {
            return false;
        }
        matches!(answer.trim().to_lowercase().as_str(), "y" | "yes")
    });

    match outcome {
        setup::Outcome::NothingToDo => println!("Nothing to install."),
        setup::Outcome::Declined => println!("Nothing was installed."),
        setup::Outcome::NoPackageManager => {
            println!("No supported package manager found. Install the packages above by hand.");
        }
        setup::Outcome::Ran { failed } if failed.is_empty() => {
            println!("\nDone. Run `arc-labs doctor` to confirm.");
        }
        setup::Outcome::Ran { failed } => {
            println!("\nSome steps failed:");
            for f in &failed {
                println!("  {f}");
            }
            println!("\nRun `arc-labs doctor` to see what is still missing.");
            std::process::exit(1);
        }
    }
    Ok(())
}

fn cmd_reindex(
    cli: &Cli,
    config: Config,
    config_path: Option<PathBuf>,
    force: bool,
    clean: bool,
) -> Result<()> {
    let Some(vault_path) = resolve_vault(cli.vault.clone(), &config) else {
        anyhow::bail!("no vault; pass --vault or set ARC_LABS_VAULT");
    };

    if clean {
        let db = vault_path.join(".arc").join("index.db");
        arc_labs_index::remove_database(&db);
        println!("removed {}", db.display());
    }

    let api = Api::new(config, config_path, arc_labs_api::Capabilities::desktop());
    let info = api.open_vault(&vault_path)?;
    println!("indexing {} ({} notes)…", info.name, info.note_count);

    let stats = api.open_index(force)?;
    println!(
        "  {} notes, {} canvases, {} links, {} tags, {} headings in {} ms",
        stats.notes, stats.canvases, stats.links, stats.tags, stats.headings, stats.elapsed_ms
    );
    if stats.skipped_unchanged > 0 {
        println!("  {} unchanged notes skipped", stats.skipped_unchanged);
    }
    if !stats.failed.is_empty() {
        println!("  {} file(s) could not be indexed:", stats.failed.len());
        for (p, why) in stats.failed.iter().take(10) {
            println!("    {p}: {why}");
        }
    }

    let s = api.index_stats()?;
    println!(
        "  {} of {} links resolve; {} unresolved; {} distinct tags; {} orphan notes",
        s.resolved_links, s.links, s.unresolved_links, s.distinct_tags, s.orphans
    );
    Ok(())
}

fn cmd_serve(
    vault: Option<PathBuf>,
    config: Config,
    config_path: Option<PathBuf>,
    host: IpAddr,
    port: u16,
    ui: Option<PathBuf>,
) -> Result<()> {
    let cfg = ServerConfig {
        host,
        port,
        ui_dir: ui.unwrap_or_else(default_ui_dir),
        // Pairing enforced here, in one place: off-loopback always gets a token.
        token: (!host.is_loopback()).then(generate_token),
    };

    let api = Arc::new(Api::new(config.clone(), config_path, cfg.capabilities()));

    if let Some(path) = resolve_vault(vault, &config) {
        match api.open_vault(&path) {
            Ok(info) => {
                tracing::info!(vault = %info.name, notes = info.note_count, "vault open");
                // Index on a background thread so the server starts answering
                // immediately. The status endpoint reports INDEXING meanwhile,
                // and index-backed routes say "not ready yet" rather than
                // blocking — a 5,000-note vault takes a few seconds, and making
                // the whole app wait for it would be the wrong trade.
                let api = Arc::clone(&api);
                std::thread::spawn(move || match api.open_index(false) {
                    Ok(s) => tracing::info!(
                        notes = s.notes, links = s.links, ms = s.elapsed_ms, "index ready"
                    ),
                    Err(e) => tracing::warn!(error = %e, "could not build the index"),
                });
            }
            // Not fatal: the first-run screen exists exactly for this.
            Err(e) => tracing::warn!(error = %e, "could not open the configured vault"),
        }
    }

    if !cfg.ui_dir.join("index.html").exists() {
        tracing::warn!(
            dir = %cfg.ui_dir.display(),
            "no UI bundle found; API will serve but the page will not. Run `npm run build` in ui/"
        );
    }

    println!("ARC-LABS  http://{}:{}", display_host(host), port);
    if let Some(token) = &cfg.token {
        // Printed once, to the operator's terminal only.
        println!("\n  Bound beyond loopback, so a token is required:\n");
        println!("    http://{}:{}/?token={}\n", display_host(host), port, token);
        println!("  Anyone who can reach this port and has this token can read the vault.");
    }

    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?
        .block_on(arc_labs_server::serve(api, cfg))
}

fn display_host(host: IpAddr) -> String {
    // 0.0.0.0 is not somewhere a browser can go; say something clickable.
    if host.is_unspecified() {
        "localhost".into()
    } else {
        host.to_string()
    }
}

/// Where the UI bundle lives: next to the binary once installed, or in the repo
/// during development.
fn default_ui_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os("ARC_LABS_UI_DIR") {
        return PathBuf::from(dir);
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            let beside = dir.join("ui");
            if beside.join("index.html").exists() {
                return beside;
            }
        }
    }
    PathBuf::from("ui/dist")
}
