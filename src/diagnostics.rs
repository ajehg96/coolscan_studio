//! System environment, driver, and external integration diagnostics.
//!
//! Provides first-run diagnostics specified in Phase 14:
//! - Nikon Coolscan USB hardware & WinUSB driver status probing.
//! - Automatic Darktable installation detection (`darktable-cli`).
//! - LittleCMS 2 color management engine verification.
//! - Application configuration directory and export path setup.

use std::path::{Path, PathBuf};
use std::process::Command;

/// System diagnostic report on hardware, drivers, Darktable, and color management.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SystemDiagnostics {
    /// Whether a Nikon Coolscan scanner was detected via USB.
    pub scanner_detected: bool,
    /// Model name / description of the detected scanner.
    pub scanner_model: Option<String>,
    /// Status message regarding scanner and WinUSB driver.
    pub driver_status: String,
    /// Absolute path to the detected Darktable CLI executable, if present.
    pub darktable_path: Option<PathBuf>,
    /// Version string of the detected Darktable CLI executable, if available.
    pub darktable_version: Option<String>,
    /// Status description of the LittleCMS 2 color engine.
    pub color_engine_info: String,
    /// Application configuration and preferences directory.
    pub config_directory: PathBuf,
    /// Default destination directory for scanned images and XMP sidecars.
    pub default_output_directory: PathBuf,
}

impl SystemDiagnostics {
    /// Runs a full diagnostic scan of the current system environment.
    pub fn probe() -> Self {
        // 1. Probe scanner hardware via nkscan / libusb
        let scanners = nkscan::device::list();
        let (scanner_detected, scanner_model, driver_status) =
            if let Some(scanner) = scanners.first() {
                let desc = scanner.to_string();
                (
                    true,
                    Some(desc.clone()),
                    format!("Connected and accessible via USB: {desc}"),
                )
            } else {
                (
                    false,
                    None,
                    "No scanner found. If connected, ensure WinUSB driver is assigned via Zadig."
                        .into(),
                )
            };

        // 2. Probe Darktable CLI installation
        let darktable_path = detect_darktable_cli();
        let darktable_version = darktable_path
            .as_ref()
            .and_then(|p| query_darktable_version(p));

        // 3. Probe LittleCMS 2 color management
        let color_engine_info =
            match crate::processing::color::ScannerColorPipeline::default_ls40() {
                Ok(_) => "LittleCMS 2.16 (statically linked, Rec.2020 working space, sRGB display)"
                    .into(),
                Err(e) => format!("Color pipeline error: {e}"),
            };

        // 4. Resolve standard config and output paths
        let config_directory = resolve_app_config_dir();
        let default_output_directory = resolve_default_output_dir();

        Self {
            scanner_detected,
            scanner_model,
            driver_status,
            darktable_path,
            darktable_version,
            color_engine_info,
            config_directory,
            default_output_directory,
        }
    }

    /// Formats the diagnostic report as human-readable text.
    pub fn format_summary(&self) -> String {
        let mut out = String::new();
        out.push_str("====================================================\n");
        out.push_str("          Coolscan Studio — System Diagnostics      \n");
        out.push_str("====================================================\n\n");

        out.push_str("1. Scanner Hardware & USB Driver:\n");
        if self.scanner_detected {
            out.push_str(&format!(
                "   [✓] Scanner detected: {}\n",
                self.scanner_model.as_deref().unwrap_or("Unknown model")
            ));
            out.push_str(&format!("   Status: {}\n", self.driver_status));
        } else {
            out.push_str("   [!] No Nikon Coolscan scanner detected.\n");
            out.push_str("   Notice: Connect scanner via USB and ensure the WinUSB driver\n");
            out.push_str("           is installed using Zadig (https://zadig.akeo.ie).\n");
        }
        out.push('\n');

        out.push_str("2. Darktable Integration:\n");
        if let Some(path) = &self.darktable_path {
            out.push_str(&format!(
                "   [✓] darktable-cli detected: {}\n",
                path.display()
            ));
            if let Some(ver) = &self.darktable_version {
                out.push_str(&format!("   Version: {}\n", ver));
            }
            out.push_str("   Integration: Full Darktable Negadoctor XMP parity enabled.\n");
        } else {
            out.push_str("   [!] darktable-cli not found in standard paths.\n");
            out.push_str("   Notice: Install Darktable (https://www.darktable.org) to\n");
            out.push_str(
                "           enable automatic render testing and direct raw development.\n",
            );
        }
        out.push('\n');

        out.push_str("3. Color Management Engine:\n");
        out.push_str(&format!("   [✓] {}\n\n", self.color_engine_info));

        out.push_str("4. File Storage Paths:\n");
        out.push_str(&format!(
            "   Preferences dir: {}\n",
            self.config_directory.display()
        ));
        out.push_str(&format!(
            "   Default export:  {}\n",
            self.default_output_directory.display()
        ));
        out.push_str("====================================================\n");

        out
    }

    /// Prints the formatted diagnostic report to stdout.
    pub fn print_report(&self) {
        println!("{}", self.format_summary());
    }
}

/// Detects the Darktable CLI executable location across environment variables, standard paths, and PATH.
pub fn detect_darktable_cli() -> Option<PathBuf> {
    // 1. Check DARKTABLE_PATH or DARKTABLE_CLI environment variable
    if let Ok(env_path) =
        std::env::var("DARKTABLE_PATH").or_else(|_| std::env::var("DARKTABLE_CLI"))
    {
        let p = PathBuf::from(env_path);
        if p.is_file() {
            return Some(p);
        }
    }

    // 2. Check standard Windows installation paths
    let windows_candidates = [
        "C:\\Program Files\\darktable\\bin\\darktable-cli.exe",
        "C:\\Program Files (x86)\\darktable\\bin\\darktable-cli.exe",
        "D:\\Program Files\\darktable\\bin\\darktable-cli.exe",
    ];
    for candidate in &windows_candidates {
        let p = Path::new(candidate);
        if p.is_file() {
            return Some(p.to_path_buf());
        }
    }

    // 3. Check Unix / Linux / macOS standard paths
    let unix_candidates = [
        "/usr/bin/darktable-cli",
        "/usr/local/bin/darktable-cli",
        "/opt/homebrew/bin/darktable-cli",
    ];
    for candidate in &unix_candidates {
        let p = Path::new(candidate);
        if p.is_file() {
            return Some(p.to_path_buf());
        }
    }

    // 4. Try resolving darktable-cli in PATH
    if let Ok(output) = Command::new("darktable-cli").arg("--version").output()
        && output.status.success()
    {
        return Some(PathBuf::from("darktable-cli"));
    }

    None
}

/// Queries the version string of the given Darktable CLI binary.
pub fn query_darktable_version(path: &Path) -> Option<String> {
    let output = Command::new(path).arg("--version").output().ok()?;
    if output.status.success() {
        let stdout = String::from_utf8_lossy(&output.stdout);
        // Typical output starts with "this is darktable 4.8.1" or "darktable 4.8.1"
        for line in stdout.lines() {
            let trimmed = line.trim();
            if trimmed.contains("darktable") {
                return Some(trimmed.to_string());
            }
        }
    }
    None
}

/// Resolves standard application configuration and preferences directory.
pub fn resolve_app_config_dir() -> PathBuf {
    if let Ok(appdata) = std::env::var("APPDATA") {
        PathBuf::from(appdata).join("CoolscanStudio")
    } else if let Ok(home) = std::env::var("HOME") {
        PathBuf::from(home).join(".config").join("coolscan-studio")
    } else {
        PathBuf::from("./config")
    }
}

/// Resolves standard default destination directory for scanned outputs.
pub fn resolve_default_output_dir() -> PathBuf {
    if let Ok(userprofile) = std::env::var("USERPROFILE") {
        let pictures = PathBuf::from(userprofile)
            .join("Pictures")
            .join("CoolscanStudio");
        if pictures.parent().map(|p| p.exists()).unwrap_or(false) {
            return pictures;
        }
    }
    PathBuf::from("./scans")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn system_diagnostics_probes_without_panic() {
        let diag = SystemDiagnostics::probe();
        assert!(!diag.color_engine_info.is_empty());
        assert!(!diag.driver_status.is_empty());
        assert!(!diag.config_directory.as_os_str().is_empty());

        let summary = diag.format_summary();
        assert!(summary.contains("Coolscan Studio — System Diagnostics"));
        assert!(summary.contains("LittleCMS"));
    }

    #[test]
    fn app_paths_are_non_empty() {
        let cfg = resolve_app_config_dir();
        assert!(!cfg.as_os_str().is_empty());

        let out = resolve_default_output_dir();
        assert!(!out.as_os_str().is_empty());
    }
}
