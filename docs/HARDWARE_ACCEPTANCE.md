# Coolscan Studio — Physical Hardware Acceptance Gate

This document defines the mandatory physical-hardware verification gate required before tagging any release candidate that modifies scanner communication, transport recovery, frame geometry, or streaming acquisition (`src/scanner/**`, `src/frame_position.rs`, `src/crop.rs`).

GitHub-hosted CI runners verify compilation, formatting, Clippy lints, unit/integration suites, Windows x86_64 release builds, and Darktable 5.6.0 parity fixtures, but **cannot** validate physical Nikon Coolscan USB communication or mechanical film transport.

---

## 1. Release Candidate Hardware Record Template

Copy and complete this record for each release candidate tested against physical hardware:

```text
Commit:            <git rev-parse HEAD>
Date:              <YYYY-MM-DD>
Scanner:           Nikon COOLSCAN IV ED (LS-40 ED)
Firmware:          <e.g. 1.20>
Adapter:           SA-21 Strip Film Adapter
Windows version:   <e.g. Windows 11 Pro 24H2>
WinUSB driver:     <e.g. WinUSB v6.1.7600.16385 via Zadig>
Darktable version: <e.g. 5.6.0>
Film strip:        <e.g. Kodak Pro Image 100 6-frame strip>
Operator:          <Name>
```

---

## 2. Automated Hardware Acceptance Suite (`tests/hardware_acceptance.rs`)

With the Nikon LS-40 powered on, connected via USB (WinUSB driver active), and a 35mm film strip loaded in the SA-21 adapter, run:

```powershell
# 1. Verify USB detection, WinUSB status, Darktable CLI, and ICC profile integrity
cargo run --release -- --diagnose

# 2. Execute the ignored hardware-in-the-loop acceptance tests sequentially
cargo test --test hardware_acceptance -- --ignored --test-threads=1 --nocapture
```

> **Important:** Always pass `--test-threads=1` when running `--ignored` hardware tests so multiple test threads do not contend for the same USB device handle.

---

## 3. Mandatory Verification Checklist

### 3.1 Scanner Basics
- [ ] **Scanner detected:** `coolscan-studio --diagnose` and GUI startup detect `Nikon LS-40 ED` over USB without falling back to mock mode.
- [ ] **Film load detected:** Inserting a 35mm strip into the SA-21 adapter transitions the status to ready.
- [ ] **Boundary discovery:** Frame discovery returns the expected frame count (e.g. 6 frames on a full strip) and valid `BoundaryTable` coordinates.
- [ ] **Travel offset (`offset_mm`):** Applying a small positive/negative offset (e.g. `+0.5 mm`, `-0.5 mm`) shifts the scan window along the strip travel axis without corrupting frame width or out-of-bounds errors.
- [ ] **Eject works:** Clicking **Eject Film** (or running `--eject`) cleanly feeds the strip out of the SA-21 adapter.

### 3.2 Scan Modes
- [ ] **725 DPI single frame:** Preview-resolution single-frame scan completes and streams immediately into the Review Studio.
- [ ] **2900 DPI single frame:** Full optical resolution (2900 DPI) single-frame scan completes with accurate crop bounds.
- [ ] **Multi-frame strip streaming:** Selecting multiple frames (e.g. `[1, 3, 5]` or `All`) streams Frame 1 into the Review Studio for inspection and adjustment while Frame 2 is actively scanning.
- [ ] **Infrared dust/scratch cleaning (OpenICE):** Scanning with `--clean` / GUI IR Cleaning enabled acquires the 4th infrared channel, removes dust defects, and releases the IR plane buffer immediately after cleaning.
- [ ] **Multi-sampling (1–16):** Scanning with `samples = 2` or `samples = 4` completes cleanly within the validated `1..=16` limit.

### 3.3 Cancellation & Transport Recovery
- [ ] **Cancel current frame (`cancel_current`):** Clicking **Cancel Frame** mid-pass aborts the active `nkscan` pass via `ControlFlow::Break(())` without entering the 3-attempt USB transport retry loop.
- [ ] **Next scan works after cancellation:** Starting a new scan immediately after cancelling a frame succeeds without power-cycling the scanner.
- [ ] **Stop after current frame (`stop_after_current`):** Clicking **Stop After Frame** during Frame 1 finishes Frame 1, preserves it in the Review Studio, and halts before advancing to Frame 2.

### 3.4 Processing & Export Verification
- [ ] **Two-stage Negadoctor analysis:** Auto `D-max`, `scan_bias`, regional `wb_high` (from user rectangle), `paper_black`, and `print_exposure` compute deterministically and respect `Auto`/`Manual` locks.
- [ ] **Orientation:** Rotating (`0°`, `90°`, `180°`, `270°`) updates the preview instantaneously and encodes the matching Darktable `flip` module parameter.
- [ ] **Export bundle:** Exporting produces:
  - `frame-<N>-master.tif` (uncropped 16-bit RGB with embedded `NKLS4000LS40_N.icc` in Tag 34675)
  - `frame-<N>.tif` (cropped 16-bit RGB working image preserving source frame number `<N>`)
  - `frame-<N>.tif.xmp` (XML-safe Darktable 5.6.0 sidecar referencing `frame-<N>.tif` and verified `NKLS4000LS40_N.icc`)
- [ ] **Darktable import:** Opening `frame-<N>.tif` in Darktable 5.6.0 applies the companion `.tif.xmp` history stack (`colorin` -> `flip` -> `negadoctor`) with matching visual rendering.

---

## 4. Recorded Hardware Acceptance Runs

### Run 1 — Stabilisation Release Candidate (`6d2dfe7`)

```text
Commit:            6d2dfe7ab277ea9d2770bbf71d17f896646842ff
Date:              2026-10-25
Scanner:           Nikon LS-40 ED (usb:PCIROOT(0)#PCI(0103)#PCI(0000)#USBROOT(0)-12)
Adapter:           SA-21 Strip Film Adapter (Perforation registration, 6 frames @ 2900x2900 optical DPI)
Windows toolchain: 1.98.1-x86_64-pc-windows-msvc
WinUSB driver:     Active and verified via `coolscan-studio.exe --diagnose`
Darktable version: darktable 5.6.0 (`C:\Program Files\darktable\bin\darktable-cli.exe`)
ICC profile state: Verified (`C:\Users\AJEHG\AppData\Local\darktable\color\in\NKLS4000LS40_N.icc`, 223276 bytes, FNV-1a `0893507334834018`)
Film strip:        6-frame 35mm negative strip (`starts=[5, 148, 292, 436, 581, 709]`, `length=137`)
Operator:          Austin + Automated Hardware Acceptance Suite
```

#### Automated Hardware-in-the-Loop Results (`tests/hardware_acceptance.rs`)
- [x] `ls40_device_discovery_and_status` — **PASS** (`15.47s`): Connected via USB, discovered 6 frames via perforation registration (`DPI: (2900, 2900)`).
- [x] `ls40_single_frame_725_dpi_streaming_and_export` — **PASS** (`81.52s`): Acquired Frame 1 (`1082x717 @ 725 DPI`), streamed immediately via `on_frame`, prepared `PreparedFrame`, and exported `frame-1-master.tif` (with embedded ICC Tag 34675), `frame-1.tif`, and `frame-1.tif.xmp`.
- [x] `ls40_cancel_current_frame_and_recover` — **PASS** (`111.77s`): Aborted Frame 1 mid-pass via `ScanControl::request_cancel_current()`, returned `Err(ScanError::Cancelled)` without entering USB retry loops, and immediately completed a follow-up recovery scan on the same scanner without power-cycling.
- [x] `ls40_stop_after_current_frame_multi_frame` — **PASS** (`72.64s`): Requested `stop_after_current` inside the `on_frame` streaming callback upon Frame 1 completion during a `[1, 2]` scan; preserved Frame 1 (`frames_acquired == 1`) and halted before Frame 2.

---

## 5. Evidence Retention

For each hardware acceptance run, archive outside the public source tree (or attach to the GitHub Release / PR notes):
1. Output of `coolscan-studio --diagnose`
2. Console log of `cargo test --test hardware_acceptance -- --ignored --test-threads=1 --nocapture`
3. Generated `.tif.xmp` sidecar and parameter summary
4. Exact tested commit SHA
