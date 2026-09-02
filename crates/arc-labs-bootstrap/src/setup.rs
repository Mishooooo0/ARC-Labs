//! `arc-labs setup` — the one place in ARC-LABS that installs anything.
//!
//! # How this coexists with "fully offline"
//!
//! Constraint 3 says no network call the user did not initiate. `setup` makes
//! network calls, and it does not violate the constraint, for four reasons that
//! together are the whole design:
//!
//! 1. **It is the only outbound path in the product.** All installer code lives
//!    in this crate, and `arc-labs-core` cannot depend on it.
//! 2. **It never runs on its own.** Not at startup, not on first run, not as a
//!    side effect of opening a vault. Only when typed.
//! 3. **It says what it will do first**, and waits for a yes. `--yes` exists for
//!    Docker builds and CI; it is never the default.
//! 4. **It touches no vault content.** It installs system packages. Nothing the
//!    user has written goes anywhere.
//!
//! The README states this plainly, because a notebook that promises to work with
//! the cable pulled has to be honest about the one command that needs the cable.

use std::process::Command;

use crate::doctor::Report;
use crate::platform::PackageManager;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Plan {
    pub manager: PackageManager,
    pub packages: Vec<String>,
    /// The exact commands that would run, in order.
    pub commands: Vec<Vec<String>>,
}

impl Plan {
    pub fn is_empty(&self) -> bool {
        self.commands.is_empty()
    }

    /// What the user reads before deciding. Shows the literal commands rather
    /// than a summary: "installs some dependencies" is not consent.
    pub fn render(&self) -> String {
        if self.is_empty() {
            return "Nothing to install — this machine already has what ARC-LABS needs.\n".into();
        }
        let mut out = String::from("ARC-LABS setup will run the following, and nothing else:\n\n");
        for cmd in &self.commands {
            out.push_str("  ");
            out.push_str(&cmd.join(" "));
            out.push('\n');
        }
        out.push_str("\nThis installs system packages only. No vault content is read or sent.\n");
        out
    }
}

/// Build the plan from a doctor report. Pure — it runs nothing.
pub fn plan(report: &Report) -> Plan {
    let manager = report.platform.package_manager;
    let packages = crate::doctor::missing_packages(report);

    let commands = if packages.is_empty() {
        Vec::new()
    } else if manager == PackageManager::Winget {
        // winget installs one id per invocation, so the plan is one command each
        // rather than a single line that would silently only do the first.
        packages
            .iter()
            .filter_map(|p| manager.install_command(&[p]))
            .collect()
    } else {
        manager.install_command(&packages).into_iter().collect()
    };

    Plan {
        manager,
        packages: packages.iter().map(|p| p.to_string()).collect(),
        commands,
    }
}

#[derive(Debug)]
pub enum Outcome {
    NothingToDo,
    Declined,
    NoPackageManager,
    Ran { failed: Vec<String> },
}

/// Execute a plan. `confirm` is called once with the rendered plan and decides.
///
/// Passing a closure rather than reading stdin here keeps this testable and lets
/// the caller decide what consent means — a terminal prompt, `--yes`, or (later)
/// a dialog in the app.
pub fn execute(plan: &Plan, confirm: impl FnOnce(&str) -> bool) -> Outcome {
    if plan.is_empty() {
        return Outcome::NothingToDo;
    }
    if plan.manager == PackageManager::None {
        return Outcome::NoPackageManager;
    }
    if !confirm(&plan.render()) {
        return Outcome::Declined;
    }

    let mut failed = Vec::new();
    for cmd in &plan.commands {
        let (bin, args) = cmd.split_first().expect("plan commands are never empty");
        tracing::info!(command = %cmd.join(" "), "running installer");
        match Command::new(bin).args(args).status() {
            Ok(s) if s.success() => {}
            // Carry on rather than stopping: a machine that got three of four
            // packages is closer to working, and `doctor` will name what is left.
            Ok(s) => failed.push(format!(
                "{} (exit {})",
                cmd.join(" "),
                s.code().unwrap_or(-1)
            )),
            Err(e) => failed.push(format!("{} ({e})", cmd.join(" "))),
        }
    }
    Outcome::Ran { failed }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::doctor::{Check, Level};
    use crate::platform::{Os, Platform};

    fn report_with(manager: PackageManager, failing: &[&str]) -> Report {
        let checks = failing
            .iter()
            .map(|n| Check {
                name: (*n).into(),
                level: Level::Fail,
                detail: "missing".into(),
                fix: Some("arc-labs setup".into()),
            })
            .collect();
        Report {
            platform: Platform {
                os: Os::Linux,
                arch: "x86_64".into(),
                distro: Some("ubuntu".into()),
                distro_name: Some("Ubuntu 24.04".into()),
                package_manager: manager,
                in_container: false,
                in_wsl: false,
            },
            checks,
        }
    }

    #[test]
    fn a_healthy_machine_produces_an_empty_plan() {
        let p = plan(&report_with(PackageManager::Apt, &[]));
        assert!(p.is_empty());
        assert!(p.render().contains("Nothing to install"));
    }

    #[test]
    fn apt_gets_one_command_with_every_package() {
        let p = plan(&report_with(PackageManager::Apt, &["webkit2gtk"]));
        assert_eq!(p.commands.len(), 1);
        assert!(p.packages.contains(&"libwebkit2gtk-4.1-dev".to_string()));
        assert!(p.render().contains("apt-get install -y"));
    }

    #[test]
    fn winget_gets_one_command_per_package() {
        // winget silently installs only the first id if several are passed, so
        // the plan must split them or the user gets a partial install and no error.
        let mut r = report_with(PackageManager::Winget, &["webview2", "node"]);
        r.platform.os = Os::Windows;
        let p = plan(&r);
        assert_eq!(p.commands.len(), p.packages.len());
        assert!(p.commands.iter().all(|c| c.contains(&"--id".to_string())));
    }

    #[test]
    fn the_plan_shows_literal_commands_because_a_summary_is_not_consent() {
        let p = plan(&report_with(PackageManager::Apt, &["webkit2gtk"]));
        let text = p.render();
        assert!(
            text.contains("sudo apt-get install -y libwebkit2gtk-4.1-dev"),
            "got:\n{text}"
        );
        assert!(text.contains("No vault content is read or sent"));
    }

    #[test]
    fn declining_runs_nothing() {
        let p = plan(&report_with(PackageManager::Apt, &["webkit2gtk"]));
        // If this ran anything, the test suite would try to install packages.
        assert!(matches!(execute(&p, |_| false), Outcome::Declined));
    }

    #[test]
    fn an_unknown_package_manager_is_reported_not_guessed_at() {
        let p = plan(&report_with(PackageManager::None, &["webkit2gtk"]));
        assert!(p.is_empty(), "no manager means no plan to offer");
        assert!(matches!(execute(&p, |_| true), Outcome::NothingToDo));
    }

    #[test]
    fn consent_is_asked_before_anything_happens() {
        let p = plan(&report_with(PackageManager::Apt, &["webkit2gtk"]));
        let mut shown = String::new();
        let _ = execute(&p, |text| {
            shown = text.to_string();
            false
        });
        assert!(
            shown.contains("apt-get"),
            "the user must see the command before deciding"
        );
    }
}
