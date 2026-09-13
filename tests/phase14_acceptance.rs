use coolscan_studio::{cli, diagnostics::SystemDiagnostics};
use std::path::Path;

#[test]
fn phase14_system_diagnostics_and_darktable_detection() {
    let diag = SystemDiagnostics::probe();

    // Verify diagnostic probing fields
    assert!(!diag.driver_status.is_empty());
    assert!(!diag.color_engine_info.is_empty());
    assert!(diag.color_engine_info.contains("LittleCMS"));
    assert!(!diag.config_directory.as_os_str().is_empty());
    assert!(!diag.default_output_directory.as_os_str().is_empty());

    // Summary formatting
    let summary = diag.format_summary();
    assert!(summary.contains("Coolscan Studio — System Diagnostics"));
    assert!(summary.contains("Scanner Hardware & USB Driver"));
    assert!(summary.contains("Darktable Integration"));
    assert!(summary.contains("Color Management Engine"));
    assert!(summary.contains("File Storage Paths"));

    // If Darktable is installed on this host (e.g. C:\Program Files\darktable), verify detection
    let dt_installed = Path::new("C:\\Program Files\\darktable\\bin\\darktable-cli.exe").exists();
    if dt_installed {
        assert!(
            diag.darktable_path.is_some(),
            "Darktable CLI should be detected when installed in Program Files"
        );
        let dt_path = diag.darktable_path.as_ref().unwrap();
        assert!(dt_path.is_file());
    }
}

#[test]
fn phase14_packaging_notices_and_zadig_driver_guide() {
    let notices_path = Path::new("NOTICES.md");
    assert!(
        notices_path.is_file(),
        "NOTICES.md must exist in root repository"
    );

    let content = std::fs::read_to_string(notices_path).expect("Failed to read NOTICES.md");
    assert!(content.contains("WinUSB Driver Configuration"));
    assert!(content.contains("Zadig"));
    assert!(content.contains("LittleCMS 2"));
    assert!(content.contains("Darktable Compatibility Notice"));
    assert!(content.contains("Negadoctor"));
    assert!(content.contains("--diagnose"));
}

#[test]
fn phase14_cli_version_and_diagnose_flags() {
    let opt_diag = cli::parse(["--diagnose".to_string()]).unwrap().unwrap();
    assert!(opt_diag.diagnose);

    let opt_ver1 = cli::parse(["--version".to_string()]).unwrap().unwrap();
    assert!(opt_ver1.version);

    let opt_ver2 = cli::parse(["-V".to_string()]).unwrap().unwrap();
    assert!(opt_ver2.version);
}
