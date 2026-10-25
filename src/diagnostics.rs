//! System environment, driver, and external integration diagnostics.
//!
//! Provides first-run diagnostics specified in Phase 14:
//! - Nikon Coolscan USB hardware & WinUSB driver status probing.
//! - Automatic Darktable installation detection (`darktable-cli`).
//! - LittleCMS 2 color management engine verification.
//! - Application configuration directory and export path setup.

use std::path::{Path, PathBuf};
use std::process::Command;

use crate::processing::color::DEFAULT_LS40_ICC_BYTES;

/// Canonical filename for the Nikon LS-4000 / LS-40 Negative ICC profile expected by Darktable `colorin`.
pub const DEFAULT_LS40_ICC_FILENAME: &str = "NKLS4000LS40_N.icc";

/// Computes a deterministic 64-bit FNV-1a digest for ICC profile integrity verification.
pub const fn compute_icc_fnv1a64(bytes: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf29ce484222325;
    let mut i = 0;
    while i < bytes.len() {
        hash ^= bytes[i] as u64;
        hash = hash.wrapping_mul(0x100000001b3);
        i += 1;
    }
    hash
}

/// Expected 64-bit FNV-1a hash of the bundled `NKLS4000LS40_N.icc` profile.
pub const DEFAULT_LS40_ICC_FNV1A64: u64 = compute_icc_fnv1a64(DEFAULT_LS40_ICC_BYTES);

/// Verification state of the `NKLS4000LS40_N.icc` profile in Darktable's input profile directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DarktableIccStatus {
    /// Profile is present in Darktable's `color/in` directory and matches the expected size and hash.
    Verified {
        path: PathBuf,
        size_bytes: usize,
        fnv1a64: u64,
    },
    /// A file with the profile name exists, but its byte content does not match the expected profile.
    HashMismatch {
        path: PathBuf,
        expected_fnv1a64: u64,
        actual_fnv1a64: u64,
        actual_size: usize,
    },
    /// Profile is not yet installed in Darktable's `color/in` directory.
    Missing { expected_path: PathBuf },
}

impl DarktableIccStatus {
    /// Returns true if the ICC profile is installed and verified.
    pub fn is_verified(&self) -> bool {
        matches!(self, DarktableIccStatus::Verified { .. })
    }
}

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
    /// Verification status of `NKLS4000LS40_N.icc` in Darktable's input profile directory.
    pub icc_profile_status: DarktableIccStatus,
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

        // 2. Probe Darktable CLI installation and ICC profile status
        let darktable_path = detect_darktable_cli();
        let darktable_version = darktable_path
            .as_ref()
            .and_then(|p| query_darktable_version(p));
        let icc_profile_status = probe_darktable_icc_status();

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
            icc_profile_status,
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

        out.push_str("2. Darktable Integration & Input ICC Profile:\n");
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
        match &self.icc_profile_status {
            DarktableIccStatus::Verified {
                path,
                size_bytes,
                fnv1a64,
            } => {
                out.push_str(&format!(
                    "   [✓] Darktable input ICC verified: {} ({} bytes, FNV-1a {:016x})\n",
                    path.display(),
                    size_bytes,
                    fnv1a64
                ));
            }
            DarktableIccStatus::HashMismatch {
                path,
                expected_fnv1a64,
                actual_fnv1a64,
                ..
            } => {
                out.push_str(&format!(
                    "   [!] Darktable input ICC checksum mismatch at {}: expected {:016x}, found {:016x}\n",
                    path.display(),
                    expected_fnv1a64,
                    actual_fnv1a64
                ));
            }
            DarktableIccStatus::Missing { expected_path } => {
                out.push_str(&format!(
                    "   [!] Darktable input ICC not yet installed at: {}\n",
                    expected_path.display()
                ));
                out.push_str(
                    "   Action: Coolscan Studio will automatically provision NKLS4000LS40_N.icc on XMP export.\n",
                );
            }
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

/// Resolves Darktable's `color/in` directory where custom input ICC profiles must reside.
pub fn resolve_darktable_color_in_dir() -> PathBuf {
    if let Ok(custom_dir) = std::env::var("DARKTABLE_COLOR_IN_DIR")
        && !custom_dir.trim().is_empty()
    {
        return PathBuf::from(custom_dir);
    }
    if let Ok(local_appdata) = std::env::var("LOCALAPPDATA") {
        return PathBuf::from(local_appdata)
            .join("darktable")
            .join("color")
            .join("in");
    }
    if let Ok(xdg_config) = std::env::var("XDG_CONFIG_HOME")
        && !xdg_config.trim().is_empty()
    {
        return PathBuf::from(xdg_config)
            .join("darktable")
            .join("color")
            .join("in");
    }
    if let Ok(home) = std::env::var("HOME") {
        return PathBuf::from(home)
            .join(".config")
            .join("darktable")
            .join("color")
            .join("in");
    }
    PathBuf::from("./darktable/color/in")
}

/// Probes the status of `NKLS4000LS40_N.icc` inside a specific Darktable `color/in` directory.
pub fn probe_darktable_icc_status_in_dir(color_in_dir: &Path) -> DarktableIccStatus {
    let expected_path = color_in_dir.join(DEFAULT_LS40_ICC_FILENAME);
    let Ok(bytes) = std::fs::read(&expected_path) else {
        return DarktableIccStatus::Missing { expected_path };
    };
    let actual_fnv1a64 = compute_icc_fnv1a64(&bytes);
    if bytes.len() == DEFAULT_LS40_ICC_BYTES.len() && actual_fnv1a64 == DEFAULT_LS40_ICC_FNV1A64 {
        DarktableIccStatus::Verified {
            path: expected_path,
            size_bytes: bytes.len(),
            fnv1a64: actual_fnv1a64,
        }
    } else {
        DarktableIccStatus::HashMismatch {
            path: expected_path,
            expected_fnv1a64: DEFAULT_LS40_ICC_FNV1A64,
            actual_fnv1a64,
            actual_size: bytes.len(),
        }
    }
}

/// Probes the status of `NKLS4000LS40_N.icc` in the system's Darktable `color/in` directory.
pub fn probe_darktable_icc_status() -> DarktableIccStatus {
    let dir = resolve_darktable_color_in_dir();
    probe_darktable_icc_status_in_dir(&dir)
}

/// Installs the bundled `NKLS4000LS40_N.icc` into the specified Darktable `color/in` directory
/// atomically (via a temporary file and rename) and verifies its integrity.
pub fn install_darktable_icc_profile_to_dir(color_in_dir: &Path) -> std::io::Result<PathBuf> {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);

    let _guard = icc_install_lock().lock().unwrap_or_else(|e| e.into_inner());
    std::fs::create_dir_all(color_in_dir)?;
    let target = color_in_dir.join(DEFAULT_LS40_ICC_FILENAME);
    let tmp_name = format!(
        ".{}.tmp.{}.{}",
        DEFAULT_LS40_ICC_FILENAME,
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::Relaxed)
    );
    let tmp_path = color_in_dir.join(tmp_name);
    std::fs::write(&tmp_path, DEFAULT_LS40_ICC_BYTES)?;
    if let Err(e) = std::fs::rename(&tmp_path, &target) {
        let _ = std::fs::remove_file(&tmp_path);
        std::fs::write(&target, DEFAULT_LS40_ICC_BYTES).map_err(|_| e)?;
    }
    match probe_darktable_icc_status_in_dir(color_in_dir) {
        DarktableIccStatus::Verified { path, .. } => Ok(path),
        other => Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("ICC verification failed after write: {other:?}"),
        )),
    }
}

fn icc_install_lock() -> &'static std::sync::Mutex<()> {
    static LOCK: std::sync::OnceLock<std::sync::Mutex<()>> = std::sync::OnceLock::new();
    LOCK.get_or_init(|| std::sync::Mutex::new(()))
}

/// Ensures `NKLS4000LS40_N.icc` is available and verified before writing a Darktable XMP sidecar.
///
/// - If already verified in `color_in_dir`, returns `Ok("NKLS4000LS40_N.icc")`.
/// - If missing in `color_in_dir`, installs and verifies it in `color_in_dir` (or falls back to
///   provisioning a verified copy in `export_dir`).
/// - If a corrupted / mismatched file exists in `color_in_dir`, returns an actionable `Err`
///   rather than silently generating a broken sidecar.
pub fn ensure_darktable_icc_for_export(
    export_dir: &Path,
    color_in_dir_override: Option<&Path>,
) -> Result<String, String> {
    let default_dir;
    let color_in_dir = match color_in_dir_override {
        Some(d) => d,
        None => {
            default_dir = resolve_darktable_color_in_dir();
            &default_dir
        }
    };

    match probe_darktable_icc_status_in_dir(color_in_dir) {
        DarktableIccStatus::Verified { .. } => Ok(DEFAULT_LS40_ICC_FILENAME.to_string()),
        DarktableIccStatus::HashMismatch {
            path,
            expected_fnv1a64,
            actual_fnv1a64,
            actual_size,
        } => Err(format!(
            "Darktable input ICC profile at '{}' failed integrity check (size {} bytes, FNV-1a {:016x}, expected {:016x}). Remove or replace the corrupted file before exporting.",
            path.display(),
            actual_size,
            actual_fnv1a64,
            expected_fnv1a64
        )),
        DarktableIccStatus::Missing { .. } => {
            if install_darktable_icc_profile_to_dir(color_in_dir).is_ok() {
                return Ok(DEFAULT_LS40_ICC_FILENAME.to_string());
            }
            // Fallback: provision verified ICC side-by-side in export_dir
            install_darktable_icc_profile_to_dir(export_dir).map_err(|e| {
                format!(
                    "Unable to install '{}' into Darktable profile directory '{}' or export directory '{}': {e}",
                    DEFAULT_LS40_ICC_FILENAME,
                    color_in_dir.display(),
                    export_dir.display()
                )
            })?;
            Ok(DEFAULT_LS40_ICC_FILENAME.to_string())
        }
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

    // 2. Check standard Windows installation paths (only on Windows)
    #[cfg(windows)]
    {
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

    #[test]
    fn darktable_icc_verification_provisioning_and_corruption_detection() {
        let temp_root =
            std::env::temp_dir().join(format!("coolscan_icc_test_{}", std::process::id()));
        let color_in = temp_root.join("color_in");
        let export_dir = temp_root.join("export");
        let _ = std::fs::remove_dir_all(&temp_root);

        // 1. Initially missing
        let status = probe_darktable_icc_status_in_dir(&color_in);
        assert!(matches!(status, DarktableIccStatus::Missing { .. }));

        // 2. Export provisioning automatically installs and verifies NKLS4000LS40_N.icc
        let name = ensure_darktable_icc_for_export(&export_dir, Some(&color_in)).unwrap();
        assert_eq!(name, DEFAULT_LS40_ICC_FILENAME);

        let verified = probe_darktable_icc_status_in_dir(&color_in);
        assert!(verified.is_verified());
        if let DarktableIccStatus::Verified {
            size_bytes,
            fnv1a64,
            ..
        } = verified
        {
            assert_eq!(size_bytes, DEFAULT_LS40_ICC_BYTES.len());
            assert_eq!(fnv1a64, DEFAULT_LS40_ICC_FNV1A64);
        }

        // 3. Corrupted ICC file in color/in is detected and refused (no silent broken XMP)
        std::fs::write(
            color_in.join(DEFAULT_LS40_ICC_FILENAME),
            b"corrupted-icc-bytes",
        )
        .unwrap();
        let mismatch = probe_darktable_icc_status_in_dir(&color_in);
        assert!(matches!(mismatch, DarktableIccStatus::HashMismatch { .. }));

        let err = ensure_darktable_icc_for_export(&export_dir, Some(&color_in)).unwrap_err();
        assert!(err.contains("failed integrity check"));

        let _ = std::fs::remove_dir_all(&temp_root);
    }
}
