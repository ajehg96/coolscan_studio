# Coolscan Studio — Implementation & Verification Status

**Baseline Commit:** `72514ea` (Phase 14 baseline)  
**Branch:** `stabilisation`  
**Tracking Plan:** [COOLSCAN_STUDIO_STABILISATION_AND_CI_PLAN.md](COOLSCAN_STUDIO_STABILISATION_AND_CI_PLAN.md)

---

## 1. Feature Status Matrix (Current `stabilisation` Branch)

| Subsystem / Feature | Implemented | Mock Validated | Hardware Tested | DT Parity Tested | Release Ready | Notes |
|---|---|---|---|---|---|---|
| **Discovery & Framing** | ✅ Yes | ✅ Yes | ⚠️ Partial (Phase 1-8) | N/A | ⏳ Pending | Frame bounds detection works; offset control pending Phase 4 |
| **Exposure & Transport Retry** | ✅ Yes | ✅ Yes | ⚠️ Pending HW run | N/A | ⏳ Pending | Scoped per attempt via `AttemptExposureState`; resets cleanly across retries |
| **Real Hardware Cancellation** | ❌ No | ✅ Mock only | ❌ No | N/A | ❌ Blocked | Phase 4 target; hardware backend currently defers cancellation |
| **Stop After Current Frame** | ❌ No | ✅ Mock only | ❌ No | N/A | ❌ Blocked | Phase 4 target |
| **Per-Frame Streaming Delivery** | ❌ No | ✅ Mock only | ❌ No | N/A | ❌ Blocked | Phase 5 target; hardware currently batches whole strip |
| **Sparse Frame Numbering** | ✅ Yes | ✅ Yes | ⚠️ Pending HW run | N/A | ⏳ Pending | Preserves `p.source.frame_number` end-to-end; covered by regression tests |
| **Multi-Sampling Limits (1–16)** | ✅ Yes | ✅ Yes | ⚠️ Hardware unverified >16 | N/A | ⏳ Pending | Core, CLI, and GUI aligned to 1–16 |
| **Frame Selection & Failure Handling** | ✅ Yes | ✅ Yes | ⚠️ Pending HW run | N/A | ⏳ Pending | Empty manual selection triggers validation; zero frames yields `AllFramesFailed` |
| **Roll vs Stock Calibration** | ⚠️ In progress | ✅ Yes | ⚠️ Measured ProImage only | ⚠️ In progress | ⏳ Pending | Phase 2 separates `FilmStock` from `RollCalibration` |
| **Negadoctor Math & Inversion** | ✅ Yes | ✅ Yes | N/A | ⚠️ Synthetic golden | ⏳ Pending | Phase 6 adds real Darktable 5.6 reference fixtures |
| **Processing Dependency Graph** | ⚠️ Partial | ✅ Yes | N/A | N/A | ⏳ Pending | Phase 3 introduces deterministic Auto/Manual tracking |
| **Darktable XMP Export** | ✅ Yes | ✅ Yes | N/A | ⚠️ Partial | ⏳ Pending | Phase 7 adds XML writer & ICC validation |
| **TIFF Master / Working Output** | ✅ Yes | ✅ Yes | N/A | N/A | ⏳ Pending | Phase 7 formalizes master vs working TIFF policy |
| **GUI Review Studio** | ✅ Yes | ✅ Yes | ⚠️ Live worker wired | N/A | ⏳ Pending | Disconnected mode on missing HW; unsupported offset slider disabled |
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
- [ ] **Phase 2: Roll & Calibration Model (PR 2)**
- [ ] **Phase 3: Processing Dependency Model (PR 3)**
- [ ] **Phase 4: Real Hardware Control (PR 4)**
- [ ] **Phase 5: Per-Frame Streaming & Memory Architecture (PR 5)**
- [ ] **Phase 6: Darktable Parity Fixtures (PR 6)**
- [ ] **Phase 7: Export Hardening (PR 7)**
- [x] **Phase 8: GitHub CI Foundation (Workflows created: `ci.yml`, `windows-build.yml`, pinned Rust 1.98.1 toolchain)**
- [ ] **Phase 9: Hardware Acceptance Gate (PR 9)**
- [ ] **Phase 10: Release CI & Packaging (PR 10)**
