# Coolscan Studio — Implementation & Verification Status

**Baseline Commit:** `72514ea` (Phase 14 baseline)  
**Branch:** `stabilisation`  
**Tracking Plan:** [COOLSCAN_STUDIO_STABILISATION_AND_CI_PLAN.md](COOLSCAN_STUDIO_STABILISATION_AND_CI_PLAN.md)

---

## 1. Feature Status Matrix (Current `stabilisation` Branch)

| Subsystem / Feature | Implemented | Mock Validated | Hardware Tested | DT Parity Tested | Release Ready | Notes |
|---|---|---|---|---|---|---|
| **Discovery & Framing** | ✅ Yes | ✅ Yes | ⚠️ Partial (Phase 1-8) | N/A | ⏳ Pending | Frame bounds detection and validated travel offset (`offset_mm`) active |
| **Exposure & Transport Retry** | ✅ Yes | ✅ Yes | ⚠️ Pending HW run | N/A | ⏳ Pending | Scoped per attempt via `AttemptExposureState`; resets cleanly across retries |
| **Real Hardware Cancellation** | ✅ Yes | ✅ Yes | ⚠️ Pending HW run | N/A | ⏳ Pending | Phase 4 wires `ScanControl` across all 5 checkpoints & `nkscan` callback |
| **Stop After Current Frame** | ✅ Yes | ✅ Yes | ⚠️ Pending HW run | N/A | ⏳ Pending | Phase 4 stops cleanly after active frame without inter-frame USB reset |
| **Per-Frame Streaming Delivery** | ✅ Yes | ✅ Yes | ⚠️ Pending HW run | N/A | ⏳ Pending | Phase 5 streams each `FrameArtifact` via `scan_strip_streaming` and reuses `WorkingImage` |
| **Sparse Frame Numbering** | ✅ Yes | ✅ Yes | ⚠️ Pending HW run | N/A | ⏳ Pending | Preserves `p.source.frame_number` end-to-end; covered by regression tests |
| **Multi-Sampling Limits (1–16)** | ✅ Yes | ✅ Yes | ⚠️ Hardware unverified >16 | N/A | ⏳ Pending | Core, CLI, and GUI aligned to 1–16 |
| **Frame Selection & Failure Handling** | ✅ Yes | ✅ Yes | ⚠️ Pending HW run | N/A | ⏳ Pending | Empty manual selection triggers validation; zero frames yields `AllFramesFailed` |
| **Roll vs Stock Calibration** | ✅ Yes | ✅ Yes | ⚠️ Measured ProImage only | ✅ Guarded uncalibrated | ⏳ Pending | Phase 2 separates `FilmStock` from `RollCalibration` (Schema v2); unverified D-min defaults removed |
| **Negadoctor Math & Inversion** | ✅ Yes | ✅ Yes | N/A | ✅ Darktable 5.6 fixture | ⏳ Pending | Phase 6 verifies Stages A–D against real LS-40 Kodak Pro Image 100 + Darktable 5.6.0 |
| **Processing Dependency Graph** | ✅ Yes | ✅ Yes | N/A | N/A | ⏳ Pending | Phase 3 enforces deterministic Auto/Manual tracking (`NegadoctorModes`) |
| **Darktable XMP Export** | ✅ Yes | ✅ Yes | N/A | ✅ Darktable 5.6 fixture | ⏳ Pending | Phase 7 uses `quick-xml` writer escaping & verifies/provisions `NKLS4000LS40_N.icc` |
| **TIFF Master / Working Output** | ✅ Yes | ✅ Yes | N/A | N/A | ⏳ Pending | Phase 7 distinguishes uncropped ICC-tagged `frame-<N>-master.tif` vs cropped `frame-<N>.tif` |
| **GUI Review Studio** | ✅ Yes | ✅ Yes | ⚠️ Live worker wired | N/A | ⏳ Pending | Disconnected mode on missing HW; travel offset slider enabled & validated |
| **GitHub Actions CI** | ✅ Yes | N/A | N/A | N/A | ✅ Active | Ubuntu & Windows workflows active on pinned Rust 1.98.1 toolchain |

### Baseline Context (at Commit `72514ea`)
At the freeze baseline `72514ea`, sparse frame numbering was broken in review/export, retry exposure leaked across attempts, empty manual selection defaulted to All, zero-frame discoveries completed silently, sample limits conflicted (1–64 vs 1–16), missing hardware auto-launched mock frames, and GitHub Actions CI was absent.

---

## 2. Remediation Phase Tracker

- [x] **Phase 0: Freeze & Baseline** (Branch `stabilisation`, `IMPLEMENTATION_STATUS.md` created)
- [x] **Phase 1: Correctness Hotfixes (PR 1)**
  - [x] 1.1 Preserve source frame numbers across review & export
  - [x] 1.2 Scope retry exposure state strictly per attempt in `pipeline.rs` (`AttemptExposureState`)
  - [x] 1.3 Return `ScanError::AllFramesFailed` when zero requested/discovered frames succeed
  - [x] 1.4 Align sample limits to 1–16 across `types.rs`, CLI, GUI, and docs
  - [x] 1.5 Remove automatic mock fallback on missing scanner
  - [x] 1.6 Disable/hide no-op scanner offset control in GUI
  - [x] 1.7 Correct mock crop end-exclusive bounds
  - [x] 1.8 Empty manual frame selection validation & regression coverage
- [x] **Phase 2: Roll & Calibration Model (PR 2)**
  - [x] 2.1 Separate `FilmStock` catalog metadata from empirical `RollCalibration` (Schema v2)
  - [x] 2.2 Retain measured D-min values exclusively for calibrated Kodak Pro Image 100 baseline
  - [x] 2.3 Remove unverified Portra 400 and Gold 200 D-min defaults
  - [x] 2.4 Guard Negadoctor auto-processing, `PreparedFrame::from_artifact`, and XMP generation on roll calibration
  - [x] 2.5 Support backward-compatible migration for v0 and v1 roll profile JSON
  - [x] 2.6 Surface calibrated vs uncalibrated status in UI and CLI
- [x] **Phase 3: Processing Dependency Model (PR 3)**
  - [x] 3.1 Introduce `NegadoctorModes` and `WhiteBalanceMode` (`Neutral`, `SampledAuto`, `Manual`)
  - [x] 3.2 Implement `ReviewFrameState::recalculate_downstream` (`D-min` → `D-max` → `scan bias` → `highlight WB` → `paper black` → `print exposure`)
  - [x] 3.3 Preserve full-frame `image_stats` in `ReviewFrameState` across live worker and session initialization
  - [x] 3.4 Add Reset-to-Auto (`⟲`) controls and downstream recalculation triggers in `ReviewApp`
  - [x] 3.5 Add regression tests covering dependency chains, manual overrides, and live mock worker stats
- [x] **Phase 4: Real Hardware Control (PR 4)**
  - [x] 4.1 Introduce `ScanControl` (`cancel_current`, `stop_after_current`) and `scan_strip_with_control`
  - [x] 4.2 Enforce cancellation checkpoints before frame, before pass, inside `nkscan` callback (`ControlFlow::Break`), and after pass without triggering USB retry loops
  - [x] 4.3 Enforce `stop_after_current` after frame completion before inter-frame USB refresh
  - [x] 4.4 Wire `HardwareScannerBackend::scan_strip` to `scan_strip_with_control` and emit `WorkerMessage::ScanCancelled`
  - [x] 4.5 Implement and validate `offset_mm` travel offset geometry (`resolve_frame_rect`, `adjust_boundary_table`) and re-enable GUI slider
- [x] **Phase 5: Per-Frame Streaming & Memory Architecture (PR 5)**
  - [x] 5.1 Introduce `scan_strip_streaming` and `execute_streaming_frames` for immediate per-frame delivery
  - [x] 5.2 Free infrared channel memory (`final_samples.ir = None`) immediately after OpenICE cleaning
  - [x] 5.3 Retain `WorkingImage` in `PreparedFrame::from_artifact` and reuse in `ReviewSession` to eliminate duplicate color conversion
  - [x] 5.4 Stream `WorkerMessage::FrameReady` per frame from `HardwareScannerBackend::scan_strip`
  - [x] 5.5 Add regression tests for streaming delivery order, cancellation preservation, and single-conversion color pipeline
- [x] **Phase 6: Darktable Parity Fixtures (PR 6)**
  - [x] 6.1 Commit real Nikon LS-40 Kodak Pro Image 100 16-bit scanner sample fixture (`pro_image_100_frame4_crop.tif`), Darktable 5.6.0 XMP sidecars, and parity manifest (`pro_image_100_parity_fixture.json`)
  - [x] 6.2 Stage A: Verify ICC input transform parity (`NKLS4000LS40_N.icc` → Linear Rec.2020)
  - [x] 6.3 Stage B: Verify regional & full-frame picker statistics parity (`min`, `max`, `mean`, `count`)
  - [x] 6.4 Stage C: Verify Negadoctor auto parameter parity (`D-max`, `scan_bias`, `wb_high`, `paper_black`, `print_exposure`) and XMP binary struct/history layout
  - [x] 6.5 Stage D: Verify rendered pixel parity in linear print space and 8-bit display sRGB
- [x] **Phase 7: Export Hardening (PR 7)**
  - [x] 7.1 Replace manual XMP string interpolation with `quick-xml` writer (`BytesStart` / `BytesDecl`) and attribute unescaping, with tests for `&`, `"`, `<`, `>`, and Unicode filenames
  - [x] 7.2 Implement Darktable `color/in` ICC profile verification (size + 64-bit FNV-1a digest), automatic provisioning on clean machines, and corruption rejection
  - [x] 7.3 Formalize Scanner Master TIFF (`frame-<N>-master.tif`, uncropped 16-bit with embedded ICC Tag 34675) vs Working TIFF (`frame-<N>.tif`, cropped 16-bit paired with `.tif.xmp`) and `read_tiff` parser
  - [x] 7.4 Reconcile Nikon ICC profile license carve-out and Darktable mathematical specification provenance in `NOTICES.md` and `profiles/README.md`
  - [x] 7.5 Add `.github/workflows/darktable-parity.yml` CI workflow
- [x] **Phase 8: GitHub CI Foundation (Workflows created: `ci.yml`, `windows-build.yml`, pinned Rust 1.98.1 toolchain)**
- [ ] **Phase 9: Hardware Acceptance Gate (PR 9)**
- [ ] **Phase 10: Release CI & Packaging (PR 10)**
