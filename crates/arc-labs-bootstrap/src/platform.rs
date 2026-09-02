//! Detect what machine this is, and how software gets installed on it.
//!
//! This is the first thing ARC-LABS does, because everything else — which
//! webview to look for, which packages to name, whether a desktop shell is even
//! possible — follows from the answer.

use std::path::Path;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Os {
    Windows,
    Linux,
    Mac,
    Other,
}

impl Os {
    pub fn detect() -> Os {
        match std::env::consts::OS {
            "windows" => Os::Windows,
            "linux" => Os::Linux,
            "macos" => Os::Mac,
            _ => Os::Other,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Os::Windows => "Windows",
            Os::Linux => "Linux",
            Os::Mac => "macOS",
            Os::Other => "unknown",
        }
    }
}

/// How to install a system package here. Detected by looking for the binary,
/// not by trusting the distro id — a Debian derivative nobody has heard of still
/// has `apt`, and that is the fact that matters.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PackageManager {
    Apt,
    Dnf,
    Pacman,
    Zypper,
    Apk,
    Winget,
    Brew,
    None,
}

impl PackageManager {
    pub fn detect() -> PackageManager {
        const CANDIDATES: &[(&str, PackageManager)] = &[
            ("apt-get", PackageManager::Apt),
            ("dnf", PackageManager::Dnf),
            ("pacman", PackageManager::Pacman),
            ("zypper", PackageManager::Zypper),
            ("apk", PackageManager::Apk),
            ("winget", PackageManager::Winget),
            ("brew", PackageManager::Brew),
        ];
        for (bin, pm) in CANDIDATES {
            if which::which(bin).is_ok() {
                return *pm;
            }
        }
        PackageManager::None
    }

    pub fn as_str(self) -> &'static str {
        match self {
            PackageManager::Apt => "apt",
            PackageManager::Dnf => "dnf",
            PackageManager::Pacman => "pacman",
            PackageManager::Zypper => "zypper",
            PackageManager::Apk => "apk",
            PackageManager::Winget => "winget",
            PackageManager::Brew => "brew",
            PackageManager::None => "none",
        }
    }

    /// The command that installs `packages`, as argv. Returned rather than run,
    /// so `doctor` can print exactly what `setup` would do without doing it.
    pub fn install_command(self, packages: &[&str]) -> Option<Vec<String>> {
        let s = |v: &[&str]| -> Vec<String> { v.iter().map(|x| x.to_string()).collect() };
        let mut cmd = match self {
            PackageManager::Apt => s(&["sudo", "apt-get", "install", "-y"]),
            PackageManager::Dnf => s(&["sudo", "dnf", "install", "-y"]),
            PackageManager::Pacman => s(&["sudo", "pacman", "-S", "--needed", "--noconfirm"]),
            PackageManager::Zypper => s(&["sudo", "zypper", "install", "-y"]),
            PackageManager::Apk => s(&["sudo", "apk", "add"]),
            PackageManager::Winget => {
                s(&["winget", "install", "--accept-package-agreements", "--accept-source-agreements", "-e", "--id"])
            }
            PackageManager::Brew => s(&["brew", "install"]),
            PackageManager::None => return None,
        };
        cmd.extend(packages.iter().map(|p| p.to_string()));
        Some(cmd)
    }
}

/// Everything detected about the machine.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Platform {
    pub os: Os,
    pub arch: String,
    /// Distro id from `/etc/os-release`, e.g. `ubuntu`. Linux only.
    pub distro: Option<String>,
    pub distro_name: Option<String>,
    pub package_manager: PackageManager,
    /// True inside a container. Changes the advice: there is no desktop shell to
    /// install for, and `sudo` usually is not needed or available.
    pub in_container: bool,
    /// True under WSL. Also changes advice — a GUI needs WSLg.
    pub in_wsl: bool,
}

impl Platform {
    pub fn detect() -> Platform {
        let os = Os::detect();
        let (distro, distro_name) = if os == Os::Linux { read_os_release() } else { (None, None) };
        Platform {
            os,
            arch: std::env::consts::ARCH.to_string(),
            distro,
            distro_name,
            package_manager: PackageManager::detect(),
            in_container: detect_container(),
            in_wsl: detect_wsl(),
        }
    }

    /// Whether a windowed desktop build can run here at all. A headless server,
    /// a container, or a machine with no display has to use the browser shell —
    /// and should be told that rather than shown a missing-webview error.
    pub fn supports_desktop_shell(&self) -> bool {
        match self.os {
            Os::Windows | Os::Mac => true,
            Os::Linux => {
                !self.in_container
                    && (std::env::var_os("DISPLAY").is_some()
                        || std::env::var_os("WAYLAND_DISPLAY").is_some())
            }
            Os::Other => false,
        }
    }

    pub fn describe(&self) -> String {
        let mut s = match (&self.distro_name, self.os) {
            (Some(name), _) => name.clone(),
            (None, os) => os.as_str().to_string(),
        };
        s.push_str(" (");
        s.push_str(&self.arch);
        if self.in_wsl {
            s.push_str(", WSL");
        }
        if self.in_container {
            s.push_str(", container");
        }
        s.push(')');
        s
    }
}

fn read_os_release() -> (Option<String>, Option<String>) {
    let Ok(text) = std::fs::read_to_string("/etc/os-release") else {
        return (None, None);
    };
    let get = |key: &str| -> Option<String> {
        text.lines()
            .find_map(|l| l.strip_prefix(key))
            .map(|v| v.trim_matches('"').to_string())
    };
    (get("ID="), get("PRETTY_NAME="))
}

fn detect_container() -> bool {
    if std::env::var_os("ARC_LABS_IN_CONTAINER").is_some() {
        return true;
    }
    if Path::new("/.dockerenv").exists() {
        return true;
    }
    // The cgroup line names the container runtime when there is one.
    std::fs::read_to_string("/proc/1/cgroup")
        .map(|s| s.contains("docker") || s.contains("containerd") || s.contains("kubepods"))
        .unwrap_or(false)
}

fn detect_wsl() -> bool {
    std::env::var_os("WSL_DISTRO_NAME").is_some()
        || std::fs::read_to_string("/proc/version")
            .map(|v| v.to_lowercase().contains("microsoft"))
            .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_this_machine_coherently() {
        let p = Platform::detect();
        assert_ne!(p.os, Os::Other, "unsupported host OS");
        assert!(!p.arch.is_empty());
        // Whatever it found, describe() must produce something a human can read.
        assert!(p.describe().contains(&p.arch));

        if p.os == Os::Linux {
            // A Linux box without any known package manager is worth knowing about.
            assert!(p.distro.is_some() || p.in_container, "no /etc/os-release outside a container");
        }
    }

    #[test]
    fn install_commands_are_shaped_per_manager() {
        let apt = PackageManager::Apt.install_command(&["libwebkit2gtk-4.1-dev"]).unwrap();
        assert_eq!(apt[..4], ["sudo", "apt-get", "install", "-y"]);
        assert_eq!(apt.last().unwrap(), "libwebkit2gtk-4.1-dev");

        let pac = PackageManager::Pacman.install_command(&["webkit2gtk"]).unwrap();
        assert!(pac.contains(&"--noconfirm".to_string()));

        // winget takes one id at a time via -e --id; the caller loops.
        let win = PackageManager::Winget.install_command(&["Ollama.Ollama"]).unwrap();
        assert!(win.contains(&"--id".to_string()));

        assert!(PackageManager::None.install_command(&["anything"]).is_none());
    }

    #[test]
    fn a_container_never_claims_to_support_a_desktop_shell() {
        let p = Platform {
            os: Os::Linux,
            arch: "x86_64".into(),
            distro: Some("debian".into()),
            distro_name: Some("Debian".into()),
            package_manager: PackageManager::Apt,
            in_container: true,
            in_wsl: false,
        };
        assert!(!p.supports_desktop_shell());
        assert!(p.describe().contains("container"));
    }
}
