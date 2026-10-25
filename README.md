# Coolscan Studio

**Coolscan Studio** is a post-scan review studio and hardware controller for Nikon Coolscan film scanners (**LS-40 ED / COOLSCAN IV ED**, LS-50 ED / V ED, LS-4000 ED, LS-5000 ED), featuring per-frame streaming acquisition, OpenICE infrared dust and scratch removal, and verified mathematical and XMP sidecar parity with **Darktable 5.6.0** (`negadoctor` + `colorin` + `flip`).

---

## Key Capabilities

- **Direct USB Hardware Control & Live Streaming (`src/scanner/`):**
  - Interfaces directly with Nikon Coolscan scanners over WinUSB via `nkscan` without requiring legacy Nikon Scan software.
  - **Per-Frame Streaming (`scan_strip_streaming`):** Emits each completed, infrared-cleaned, and cropped `FrameArtifact` to the GUI review session immediately so you can inspect and color-correct Frame 1 while Frame 2 is still scanning.
  - **Responsive Hardware Cancellation (`ScanControl`):** Supports immediate mid-pass frame cancellation (`Cancel Frame` via `ControlFlow::Break`) and clean inter-frame stopping (`Stop After Frame`) without triggering spurious USB retry loops.
  - **Validated Travel Offset (`offset_mm`):** Applies signed millimetre framing adjustments along the SA-21 film travel axis with hardware boundary validation.
- **Empirical Roll Calibration Model (`src/processing/roll.rs`):**
  - Strictly separates film stock catalog metadata (`FilmStock`) from measured film base `D-min` calibration (`RollCalibration`, Schema v2).
  - Ships with empirical Nikon LS-40 calibration for **Kodak Pro Image 100** and prevents uncalibrated rolls from silently using invented `D-min` presets.
- **Darktable 5.6.0 Parity & Dependency Engine (`src/processing/`, `src/darktable/`):**
  - Single-conversion LittleCMS 2 color pipeline (`NKLS4000LS40_N.icc` → Linear Rec.2020 working space → sRGB display preview).
  - Deterministic `Auto`/`Manual` parameter dependency graph (`D-min` → `D-max` → `scan bias` → `highlight WB` → `paper black` → `print exposure`).
  - Verified against real 2900 DPI Nikon LS-40 Kodak Pro Image 100 scans and Darktable 5.6.0 sidecars (`tests/darktable_parity.rs`).
- **Hardened Export Bundle (`src/output.rs`, `src/tiff.rs`, `src/diagnostics.rs`):**
  - **Scanner Master TIFF (`frame-<N>-master.tif`):** Uncropped 16-bit RGB sensor samples with embedded `NKLS4000LS40_N.icc` (Baseline TIFF Tag `34675`).
  - **Working TIFF (`frame-<N>.tif`):** Cropped 16-bit RGB working image preserving source frame numbering across sparse selections (e.g. `[2, 4, 6]`).
  - **Darktable XMP Sidecar (`frame-<N>.tif.xmp`):** Generated via `quick-xml` with full XML entity escaping and automatic `NKLS4000LS40_N.icc` integrity verification (64-bit FNV-1a) and provisioning.

---

## Quick Start

### 1. System Diagnostics
Check USB scanner detection, WinUSB driver status, LittleCMS 2 engine, and Darktable ICC profile verification:

```powershell
cargo run --release -- --diagnose
```

### 2. Desktop GUI Studio
Launch the interactive desktop application (connects to a physical Nikon Coolscan scanner if present, or enters disconnected inspection mode):

```powershell
cargo run --release
```

To run in explicit simulation mode with synthetic test frames without hardware:

```powershell
cargo run --release -- --mock
```

### 3. Headless CLI Batch Scanning
Scan specific frames at 2900 DPI with infrared dust removal and signed framing offset:

```powershell
cargo run --release -- --frames 2,4,6 --dpi 2900 --samples 2 --clean --offset-mm +0.35 --output-dir ./scans
```

---

## Verification & CI Suites

| Suite | Command | Environment |
|---|---|---|
| **Formatting** | `cargo fmt --all -- --check` | Linux / Windows CI |
| **Clippy Lints** | `cargo clippy --all-targets --all-features -- -D warnings` | Linux / Windows CI |
| **Unit & Integration Tests** | `cargo test --all-targets --all-features --locked` | Linux / Windows CI |
| **Darktable 5.6.0 Parity Suite** | `cargo test --test darktable_parity --locked -- --nocapture` | Linux / Windows CI |
| **Physical LS-40 Hardware Gate** | `cargo test --test hardware_acceptance -- --ignored --test-threads=1 --nocapture` | Local Windows + LS-40 (`docs/HARDWARE_ACCEPTANCE.md`) |

---

## Licensing & Notices

- **Coolscan Studio Source Code:** Licensed under either `MIT` or `Apache-2.0` at your option.
- **Third-Party & Nikon ICC Profile Notices:** See [NOTICES.md](NOTICES.md) and [profiles/README.md](profiles/README.md) for LittleCMS 2 licensing, Darktable mathematical specification provenance, and the Nikon Scan ICC profile (`profiles/NKLS4000LS40_N.icc`) license carve-out.
