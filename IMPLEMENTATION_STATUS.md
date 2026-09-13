# Coolscan Studio — Implementation & Verification Status

**Baseline Commit:** `72514ea` (Phase 14 baseline)  
**Branch:** `stabilisation`  
**Tracking Plan:** [COOLSCAN_STUDIO_STABILISATION_AND_CI_PLAN.md](COOLSCAN_STUDIO_STABILISATION_AND_CI_PLAN.md)

---

## 1. Feature Status Matrix

| Subsystem / Feature | Implemented | Mock Validated | Hardware Tested | DT Parity Tested | Release Ready | Notes |
|---|---|---|---|---|---|---|
| **Discovery & Framing** | ✅ Yes | ✅ Yes | ⚠️ Partial (Phase 1-8) | N/A | ⏳ Pending | Frame bounds detection works; offset control pending Phase 4 |
| **Exposure & Transport Retry** | ✅ Yes | ✅ Yes | ⚠️ Needs reset scope | N/A | ⏳ Pending | PR 1 scopes retry exposure state per attempt |
| **Real Hardware Cancellation** | ❌ No | ✅ Mock only | ❌ No | N/A | ❌ Blocked | Phase 4 target |
| **Stop After Current Frame** | ❌ No | ✅ Mock only | ❌ No | N/A | ❌ Blocked | Phase 4 target |
| **Per-Frame Streaming Delivery** | ❌ No | ✅ Mock only | ❌ No | N/A | ❌ Blocked | Phase 5 target; hardware currently batches whole strip |
| **Sparse Frame Numbering** | ⚠️ In progress | ⚠️ Broken in review | N/A | N/A | ⏳ Pending | PR 1 preserves `p.source.frame_number` |
| **Multi-Sampling Limits (1–16)** | ⚠️ In progress | ✅ Yes | ⚠️ Hardware unverified >16 | N/A | ⏳ Pending | PR 1 aligns core 1–64 down to 1–16 |
| **Roll vs Stock Calibration** | ⚠️ In progress | ✅ Yes | ⚠️ Measured ProImage only | ⚠️ In progress | ⏳ Pending | Phase 2 separates `FilmStock` from `RollCalibration` |
| **Negadoctor Math & Inversion** | ✅ Yes | ✅ Yes | N/A | ⚠️ Synthetic golden | ⏳ Pending | Phase 6 adds real Darktable 5.6 reference fixtures |
| **Processing Dependency Graph** | ⚠️ Partial | ✅ Yes | N/A | N/A | ⏳ Pending | Phase 3 introduces deterministic Auto/Manual tracking |
| **Darktable XMP Export** | ✅ Yes | ✅ Yes | N/A | ⚠️ Partial | ⏳ Pending | Phase 7 adds XML writer & ICC validation |
| **TIFF Master / Working Output** | ✅ Yes | ✅ Yes | N/A | N/A | ⏳ Pending | Phase 7 formalizes master vs working TIFF policy |
| **GUI Review Studio** | ✅ Yes | ✅ Yes | ⚠️ Live worker wired | N/A | ⏳ Pending | PR 1 removes auto-mock fallback; disables no-op offset |
| **GitHub Actions CI** | ❌ Not active | N/A | N/A | N/A | ❌ Blocked | Phase 8 adds `ci.yml` & `windows-build.yml` |

---

## 2. Remediation Phase Tracker

- [x] **Phase 0: Freeze & Baseline** (Branch `stabilisation`, `IMPLEMENTATION_STATUS.md` created)
- [x] **Phase 1: Correctness Hotfixes (PR 1)**
  - [x] 1.1 Preserve source frame numbers across review & export
  - [x] 1.2 Scope retry exposure state strictly per attempt in `pipeline.rs`
  - [x] 1.3 Return `ScanError::AllFramesFailed` when zero requested frames succeed
  - [x] 1.4 Align sample limits to 1–16 across `types.rs`, CLI, GUI, and docs
  - [x] 1.5 Remove automatic mock fallback on missing scanner
  - [x] 1.6 Disable/hide no-op scanner offset control in GUI
  - [x] 1.7 Correct mock crop end-exclusive bounds
- [ ] **Phase 2: Roll & Calibration Model (PR 2)**
- [ ] **Phase 3: Processing Dependency Model (PR 3)**
- [ ] **Phase 4: Real Hardware Control (PR 4)**
- [ ] **Phase 5: Per-Frame Streaming & Memory Architecture (PR 5)**
- [ ] **Phase 6: Darktable Parity Fixtures (PR 6)**
- [ ] **Phase 7: Export Hardening (PR 7)**
- [x] **Phase 8: GitHub CI Foundation (Workflows created: `ci.yml`, `windows-build.yml`)**
- [ ] **Phase 9: Hardware Acceptance Gate (PR 9)**
- [ ] **Phase 10: Release CI & Packaging (PR 10)**
