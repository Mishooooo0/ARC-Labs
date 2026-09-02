//! `arc-labs doctor` — diagnose the environment. Reads, never installs.
//!
//! Every failing check carries the exact command that fixes it. A diagnostic
//! that tells you something is wrong without telling you what to type has moved
//! the problem, not solved it.

use std::fmt::Write as _;
use std::net::{TcpStream, ToSocketAddrs};
use std::time::Duration;

use crate::platform::{Os, PackageManager, Platform};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Level {
    Ok,
    /// Works, but something is degraded or a later phase will need it.
    Warn,
    /// This mode cannot run.
    Fail,
}

impl Level {
    pub fn glyph(self) -> &'static str {
        match self {
            Level::Ok => "OK  ",
            Level::Warn => "WARN",
            Level::Fail => "FAIL",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Check {
    pub name: String,
    pub level: Level,
    pub detail: String,
    /// What to type. `None` when there is nothing to fix.
    pub fix: Option<String>,
}

impl Check {
    fn ok(name: &str, detail: impl Into<String>) -> Check {
        Check { name: name.into(), level: Level::Ok, detail: detail.into(), fix: None }
    }
    fn warn(name: &str, detail: impl Into<String>, fix: Option<String>) -> Check {
        Check { name: name.into(), level: Level::Warn, detail: detail.into(), fix }
    }
    fn fail(name: &str, detail: impl Into<String>, fix: Option<String>) -> Check {
        Check { name: name.into(), level: Level::Fail, detail: detail.into(), fix }
    }
}

#[derive(Debug, Clone)]
pub struct Report {
    pub platform: Platform,
    pub checks: Vec<Check>,
}

impl Report {
    pub fn worst(&self) -> Level {
        self.checks.iter().map(|c| c.level).max().unwrap_or(Level::Ok)
    }

    /// Non-zero exit when something is actually broken. A `Warn` is not a
    /// failure — Phase 6 needing an embedding model should not stop Phase 0
    /// from starting, and a CI gate should not go red over it.
    pub fn exit_code(&self) -> i32 {
        if self.worst() == Level::Fail {
            1
        } else {
            0
        }
    }

    pub fn render(&self) -> String {
        let mut out = String::new();
        let _ = writeln!(out, "ARC-LABS doctor");
        let _ = writeln!(out, "  platform        {}", self.platform.describe());
        let _ = writeln!(out, "  package manager {}", self.platform.package_manager.as_str());
        let _ = writeln!(
            out,
            "  desktop shell   {}",
            if self.platform.supports_desktop_shell() { "supported" } else { "not available" }
        );
        let _ = writeln!(out);

        let width = self.checks.iter().map(|c| c.name.len()).max().unwrap_or(0);
        for c in &self.checks {
            let _ = writeln!(out, "  {} {:width$}  {}", c.level.glyph(), c.name, c.detail);
            if let Some(fix) = &c.fix {
                let _ = writeln!(out, "       {:width$}  -> {fix}", "");
            }
        }

        let _ = writeln!(out);
        let _ = match self.worst() {
            Level::Ok => writeln!(out, "  Everything checks out."),
            Level::Warn => writeln!(out, "  Usable now; the warnings above matter for later phases."),
            Level::Fail => writeln!(out, "  Something above must be fixed before ARC-LABS will run."),
        };
        out
    }
}

/// Run every check.
pub fn run(vault: Option<&std::path::Path>, ollama_endpoint: &str) -> Report {
    let platform = Platform::detect();
    let mut checks = Vec::new();

    checks.push(check_webview(&platform));
    checks.extend(check_toolchain());
    checks.push(check_ollama(ollama_endpoint));
    checks.push(check_vault(vault));

    Report { platform, checks }
}

fn check_webview(p: &Platform) -> Check {
    match p.os {
        Os::Windows => {
            // The runtime ships with Windows 11 and current Edge, so this is
            // usually already true; when it is not, the app opens a blank window
            // and gives no clue why. Hence the explicit check.
            let installed = std::path::Path::new(
                r"C:\Program Files (x86)\Microsoft\EdgeWebView\Application",
            )
            .exists();
            if installed {
                Check::ok("webview2", "installed")
            } else {
                Check::fail(
                    "webview2",
                    "not found; the desktop window will render blank",
                    Some("winget install -e --id Microsoft.EdgeWebView2Runtime".into()),
                )
            }
        }
        Os::Linux => {
            if !p.supports_desktop_shell() {
                return Check::ok("webview", "not needed — use `arc-labs serve` and a browser");
            }
            let found = ["/usr/lib/x86_64-linux-gnu", "/usr/lib64", "/usr/lib"].iter().any(|dir| {
                std::fs::read_dir(dir)
                    .map(|mut d| {
                        d.any(|e| {
                            e.map(|e| {
                                e.file_name().to_string_lossy().starts_with("libwebkit2gtk-4.1")
                            })
                            .unwrap_or(false)
                        })
                    })
                    .unwrap_or(false)
            });
            if found {
                Check::ok("webkit2gtk", "found")
            } else {
                Check::fail(
                    "webkit2gtk",
                    "libwebkit2gtk-4.1 not found; the desktop shell cannot start",
                    Some("arc-labs setup".into()),
                )
            }
        }
        _ => Check::ok("webview", "system webview assumed"),
    }
}

fn check_toolchain() -> Vec<Check> {
    // Only needed to build from source. A packaged binary needs none of it, so
    // a miss is a warning rather than a failure.
    ["cargo", "node", "npm"]
        .iter()
        .map(|bin| match which::which(bin) {
            Ok(p) => Check::ok(bin, p.display().to_string()),
            Err(_) => Check::warn(
                bin,
                "not on PATH (only needed to build from source)",
                Some("arc-labs setup".into()),
            ),
        })
        .collect()
}

fn check_ollama(endpoint: &str) -> Check {
    match probe_tcp(endpoint, Duration::from_millis(600)) {
        Ok(()) => Check::ok("ollama", format!("reachable at {endpoint}")),
        Err(reason) => Check::warn(
            "ollama",
            format!("not reachable at {endpoint} ({reason}); needed from Phase 5"),
            Some("start Ollama, or set model.endpoint in config.toml".into()),
        ),
    }
}

/// Reachability by TCP connect rather than an HTTP request.
///
/// Deliberate: it answers the question ("is something listening?") with no HTTP
/// client anywhere in the dependency tree, which keeps this crate's network
/// surface to a single `connect` that the operator asked for by running
/// `doctor`. Enumerating models needs a real client and arrives in Phase 5.
fn probe_tcp(endpoint: &str, timeout: Duration) -> Result<(), String> {
    let hostport = endpoint
        .trim()
        .trim_start_matches("http://")
        .trim_start_matches("https://")
        .trim_end_matches('/');
    let hostport = if hostport.contains(':') {
        hostport.to_string()
    } else {
        format!("{hostport}:11434")
    };

    let addr = hostport
        .to_socket_addrs()
        .map_err(|_| "cannot resolve".to_string())?
        .next()
        .ok_or_else(|| "no address".to_string())?;

    TcpStream::connect_timeout(&addr, timeout).map(|_| ()).map_err(|e| e.kind().to_string())
}

fn check_vault(vault: Option<&std::path::Path>) -> Check {
    let Some(v) = vault else {
        return Check::warn("vault", "none configured", Some("arc-labs --vault <path>".into()));
    };
    match arc_labs_core::Vault::open(v) {
        Ok(vault) => match vault.tree() {
            Ok(t) => Check::ok(
                "vault",
                format!("{} — {} notes, {} canvases", vault.name(), t.note_count, t.canvas_count),
            ),
            Err(e) => Check::fail("vault", format!("cannot scan: {}", e.public()), None),
        },
        Err(e) => Check::fail("vault", e.public(), None),
    }
}

/// The packages `setup` would install here, if any.
pub fn missing_packages(report: &Report) -> Vec<&'static str> {
    let failing = |name: &str| {
        report.checks.iter().any(|c| c.name == name && c.level != Level::Ok)
    };
    let mut pkgs = Vec::new();
    match report.platform.package_manager {
        PackageManager::Apt => {
            if failing("webkit2gtk") {
                pkgs.extend([
                    "libwebkit2gtk-4.1-dev",
                    "libayatana-appindicator3-dev",
                    "librsvg2-dev",
                    "build-essential",
                    "pkg-config",
                    "libssl-dev",
                ]);
            }
            if failing("node") || failing("npm") {
                pkgs.push("nodejs");
                pkgs.push("npm");
            }
        }
        PackageManager::Dnf => {
            if failing("webkit2gtk") {
                pkgs.extend(["webkit2gtk4.1-devel", "libappindicator-gtk3-devel", "librsvg2-devel", "openssl-devel"]);
            }
            if failing("node") || failing("npm") {
                pkgs.push("nodejs");
            }
        }
        PackageManager::Pacman => {
            if failing("webkit2gtk") {
                pkgs.extend(["webkit2gtk-4.1", "libappindicator-gtk3", "librsvg", "openssl"]);
            }
            if failing("node") || failing("npm") {
                pkgs.push("nodejs");
                pkgs.push("npm");
            }
        }
        PackageManager::Winget => {
            if failing("webview2") {
                pkgs.push("Microsoft.EdgeWebView2Runtime");
            }
            if failing("node") || failing("npm") {
                pkgs.push("OpenJS.NodeJS.LTS");
            }
            if failing("cargo") {
                pkgs.push("Rustlang.Rustup");
            }
        }
        _ => {}
    }
    pkgs
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_report_renders_and_names_a_fix_for_every_problem() {
        let report = run(None, "http://127.0.0.1:1");
        let text = report.render();
        assert!(text.contains("ARC-LABS doctor"));
        assert!(text.contains("platform"));

        for c in &report.checks {
            if c.level != Level::Ok {
                assert!(c.fix.is_some(), "check '{}' fails with no fix offered", c.name);
            }
        }
    }

    #[test]
    fn warnings_do_not_fail_the_exit_code_but_failures_do() {
        let mut r = Report { platform: Platform::detect(), checks: vec![Check::ok("a", "fine")] };
        assert_eq!(r.exit_code(), 0);

        r.checks.push(Check::warn("b", "later", Some("x".into())));
        assert_eq!(r.exit_code(), 0, "a warning must not fail CI");

        r.checks.push(Check::fail("c", "broken", Some("y".into())));
        assert_eq!(r.exit_code(), 1);
    }

    #[test]
    fn an_unreachable_endpoint_is_a_warning_not_a_crash() {
        // Port 1 is reliably closed. This must return promptly, not hang.
        let c = check_ollama("http://127.0.0.1:1");
        assert_eq!(c.level, Level::Warn);
        assert!(c.fix.is_some());
    }

    #[test]
    fn endpoint_parsing_tolerates_the_shapes_people_actually_write() {
        // Each should resolve to an address; connecting is expected to fail.
        for e in ["http://127.0.0.1:1", "127.0.0.1:1", "http://127.0.0.1:1/", "https://127.0.0.1:1"] {
            let err = probe_tcp(e, Duration::from_millis(200)).unwrap_err();
            assert!(!err.contains("cannot resolve"), "failed to parse {e}: {err}");
        }
    }

    #[test]
    fn a_real_vault_is_reported_with_its_contents() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("a.md"), b"# a\n").unwrap();
        std::fs::write(tmp.path().join("b.canvas"), b"{}").unwrap();

        let c = check_vault(Some(tmp.path()));
        assert_eq!(c.level, Level::Ok);
        assert!(c.detail.contains("1 notes"), "got {}", c.detail);
        assert!(c.detail.contains("1 canvases"));
    }

    #[test]
    fn a_missing_vault_fails_without_echoing_the_path() {
        let c = check_vault(Some(std::path::Path::new("/no/such/vault/here")));
        assert_eq!(c.level, Level::Fail);
        assert!(!c.detail.contains("no/such"), "leaked the path: {}", c.detail);
    }
}
