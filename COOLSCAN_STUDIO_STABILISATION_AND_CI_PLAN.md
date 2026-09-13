# Coolscan Studio — Stabilisation, Verification & GitHub CI Plan

**Repository:** `ajehg96/coolscan_studio`  
**Purpose:** Remediate issues found during post-implementation review, strengthen test seams, and add GitHub CI so future reviews can rely on independently executed build/test evidence.  
**Status at review:** Architecture is promising and substantial, but several hardware-path, state-management, calibration, and release-readiness issues remain.

---

## 1. Executive Summary

The current implementation is worth keeping. The scanner refactor, typed scan events, processing modules, orientation model, review state machine, Negadoctor implementation, and Darktable XMP work provide a strong base.

The main concern is that the **mock/test path is currently more complete than the real hardware path**. Several late-stage features are present in the UI and tests but are either incomplete on physical hardware or not yet validated against a real Nikon LS-40 + Darktable reference workflow.

The remediation should therefore be approached as a **stabilisation programme**, not another feature sprint.

The recommended sequence is:

1. Fix definite correctness bugs and misleading UI.
2. Correct the roll/calibration model.
3. Make real-hardware cancellation and frame streaming genuine.
4. Establish trusted Darktable parity fixtures.
5. Harden processing dependency logic.
6. Harden XMP/TIFF export.
7. Introduce GitHub CI and required checks.
8. Perform explicit LS-40 hardware acceptance.
9. Finish licensing/packaging only after the above gates pass.

---

# 2. Key Findings to Remediate

## Critical / High Priority

### 2.1 Real hardware cancellation is not implemented
The mock backend honours cancellation flags. The hardware backend receives the cancellation tokens but currently ignores them and runs the synchronous scan pipeline to completion.

**Impact**
- “Cancel Frame” and “Stop After Frame” appear functional in the GUI but are not trustworthy on the real LS-40.
- Queued cancellation commands cannot interrupt the worker while the worker is blocked inside scanning.

**Target state**
- Cancellation token checked in the real scan loop.
- Cancellation checked inside the `nkscan` progress callback where possible.
- `StopAfterCurrentFrame` checked between frames.
- Clear event emitted for cancelled/stopped scans.
- Scanner remains usable after cancellation.

---

### 2.2 Roll D-min values are incorrectly treated as generic film presets
The Pro Image 100 D-min values were measured from a specific roll/scanner workflow. Portra 400 and Gold 200 values were introduced as hard-coded stock presets without equivalent calibration provenance.

**Impact**
- Creates false precision.
- Blurs the distinction between film stock metadata and measured roll calibration.
- Risks incorrect Negadoctor inversion on future rolls.

**Target state**
Separate:
- `FilmStock` metadata
- `RollCalibration`
- scanner profile
- measured D-min

A new roll should not silently inherit invented D-min values.

---

### 2.3 Frame numbering is lost in the review layer
Prepared frames are renumbered according to insertion order instead of preserving `source.frame_number`.

**Example failure**
Scanning frame 4 only may cause it to be presented and exported as frame 1.

**Target state**
- `frame_number` remains the scanner/source frame number end-to-end.
- UI index is separate from source frame number.
- TIFF and XMP names use the source frame number.
- Tests explicitly cover sparse selections such as `[2, 4, 6]`.

---

### 2.4 Retry exposure state may leak across USB retries
The original scanner loop reset first-pass exposure state for each scan attempt. The refactored code appears to retain it across retry attempts.

**Impact**
A failed attempt followed by a transport reset may reuse stale exposure state from the failed session.

**Target state**
- First-pass scan state scoped to one retry attempt.
- Recovery test verifies that retry starts cleanly.

---

### 2.5 Scanner offset control currently has no effect
The GUI exposes `scanner_offset_mm`, and the request stores it, but the main scanner path does not apply it to frame geometry.

**Target state**
Either:
- implement offset using the existing frame-position adjustment logic, or
- hide/disable the control until implemented.

Do not leave a control that silently does nothing.

---

### 2.6 Real hardware frames are delivered only after the entire strip finishes
The hardware backend receives a full `StripScanResult`, then converts each frame and emits them afterward.

**Impact**
- Cannot review frame 1 while frame 2 is scanning.
- Holds the entire strip in memory.
- Makes the GUI feel less responsive.
- Prevents genuine stop-after-current behaviour from integrating cleanly.

**Target state**
Emit a completed `FrameArtifact` as soon as each frame is acquired, cleaned, cropped, and ready.

---

### 2.7 Darktable parity is asserted more strongly than it is demonstrated
Formula tests are useful, but several are synthetic or compare against values derived from the same implementation assumptions.

**Target state**
Create real fixture-based parity tests using:
- real LS-40 scanner data
- known crop
- known picker rectangles
- exact Darktable 5.6 reference outputs
- explicit numerical tolerances

---

## Medium Priority

### 2.8 Manual advanced edits do not consistently recalculate dependent parameters
D-max, scan bias, WB, paper black, and print exposure have dependencies.

**Target state**
Each parameter should have a clear mode:
- `Auto`
- `Manual`

Changing an upstream auto-controlled parameter should recalculate downstream auto-controlled values.

---

### 2.9 Missing hardware silently falls back to mock mode
Normal GUI startup currently starts mock mode when no scanner is detected.

**Target state**
- `--mock` = explicit simulation mode
- normal GUI with no scanner = “No scanner connected”
- user may still inspect settings / diagnostics
- no fabricated frames unless simulation is explicitly requested

---

### 2.10 All requested frames can fail while the scan call still returns success
`AllFramesFailed` exists but is not consistently surfaced.

**Target state**
If zero requested frames are successfully acquired:
- return a failure
- do not emit “scan complete” as if successful
- GUI must display a clear failure state

---

### 2.11 Sample limits differ between core and CLI
Core allows up to 64 while CLI allows up to 16.

**Target state**
Use one validated maximum. For now, use **16** everywhere unless real LS-40 testing justifies more.

---

### 2.12 Colour conversion work is duplicated
The same full-resolution scanner data can be converted to working space during preparation and then converted again when entering review.

**Target state**
Perform one authoritative scanner-to-working-space conversion per frame and reuse it.

---

### 2.13 ICC availability is assumed
Generated XMP references `NKLS4000LS40_N.icc`, but a clean system may not have that profile installed in Darktable’s input profile path.

**Target state**
- detect profile availability
- offer/install/copy it deliberately where appropriate
- emit actionable diagnostics if unavailable
- never silently generate sidecars that reference a missing profile

---

### 2.14 XMP generation should use XML-safe writing
Hand-built XML strings risk invalid output if filenames contain reserved characters.

**Target state**
Use `quick-xml` writer APIs or equivalent escaping.

---

### 2.15 TIFF colour identity is ambiguous without the XMP
Current TIFFs do not embed ICC metadata.

**Decision required**
Choose one of:
- embed the adapted LS-40 ICC in scanner master TIFFs, or
- deliberately keep master samples untagged but enforce sidecar coupling and documentation.

The recommended direction is to make master files self-describing where practical.

---

### 2.16 Master and working output policy needs to be explicit
A cropped TIFF is useful as the working image, but it is not the same conceptual object as an untouched scanner master.

**Target state**
Output policy should distinguish:
- scanner master
- cropped working TIFF
- Darktable XMP
- optional preview JPEG/BMP

---

## Release / Distribution Issues

### 2.17 Nikon ICC licensing notice requires correction
The pinned nkscan source explicitly states the included Nikon-derived profiles are not covered by the nkscan license and that no license is granted to those profiles.

**Target state**
Before public binary distribution:
- reproduce the required notice accurately
- verify redistribution implications
- avoid implying the profile is covered by the application MIT/Apache license
- consider making profile installation a separate user-supplied step if redistribution is uncertain

---

### 2.18 Darktable provenance wording needs reconciliation
The source says the Negadoctor implementation was ported directly from Darktable mathematics, while the notice describes a clean-room implementation.

**Target state**
Use accurate, consistent wording throughout code and notices.

---

### 2.19 Phase 14 is packaging preparation, not yet packaging
Diagnostics and notices exist, but there is not yet a reproducible Windows release pipeline.

**Target state**
A release phase should produce:
- CI-tested Windows binary
- checksums
- notices/license files
- versioned archive
- optional installer later

---

# 3. Remediation Phases

# Phase 0 — Freeze & Baseline

## Goal
Stop feature expansion and create a known reference point.

## Work
- Tag or record current head commit.
- Create branch:
  - `stabilisation`
- Add an `IMPLEMENTATION_STATUS.md` or issue checklist.
- Document which features are:
  - implemented
  - mock-only
  - hardware-tested
  - parity-tested
  - release-ready

## Testing seam
No code behaviour changes yet.

## Exit criteria
- Stable baseline commit recorded.
- All remediation work happens on focused PRs.
- No new feature work until Phases 1–7 are complete.

---

# Phase 1 — Correctness Hotfixes

## Goal
Fix known bugs that can corrupt behaviour or mislead the user.

## Work

### 1.1 Preserve source frame numbers
Replace insertion-order numbering with `p.source.frame_number`.

Add tests:
- scan only frame 4 -> review frame number remains 4
- scan `[2,4,6]` -> exported files are `frame-2`, `frame-4`, `frame-6`

### 1.2 Reset retry exposure state per attempt
Move the per-attempt exposure state back inside the retry loop.

Add unit seam around retry attempt state if practical.

### 1.3 Fail correctly when no requested frame succeeds
If requested frame list is non-empty and zero artifacts are returned:
- return `ScanError::AllFramesFailed`

### 1.4 Align sample limits
Use 1–16 consistently in:
- `ScanRequest::validate`
- CLI
- GUI
- documentation
- tests

### 1.5 Remove automatic mock fallback
Normal GUI mode:
- no scanner -> empty session + disconnected hardware state

Mock mode only:
- `--mock`

### 1.6 Fix or disable offset
Preferred short-term action:
- disable/hide the offset slider until Phase 4 if implementation is not immediately trivial

### 1.7 Fix mock crop end-exclusive bounds
Ensure mock crop ranges include the intended final pixel and match production semantics.

## Testing seam
All changes can be validated without hardware except retry behaviour confidence.

## Exit criteria
- Unit tests pass.
- Sparse-frame test passes.
- No misleading no-op control.
- No silent mock fallback.
- Empty successful scan impossible.

---

# Phase 2 — Roll & Calibration Model

## Goal
Represent film stock and measured roll calibration correctly.

## Proposed model

```rust
pub struct FilmStock {
    pub id: String,
    pub manufacturer: String,
    pub name: String,
    pub iso: Option<u32>,
}

pub struct RollCalibration {
    pub dmin: [f32; 3],
    pub scanner_profile: ScannerProfile,
    pub measured_at: Option<String>,
    pub notes: Option<String>,
}

pub struct RollProfile {
    pub id: RollId,
    pub film_stock: FilmStock,
    pub calibration: Option<RollCalibration>,
}
```

## Work
- Keep Kodak Pro Image 100 as film-stock metadata.
- Move the measured `0.8965 / 0.9093 / 0.8816` values into a clearly named calibrated-roll profile.
- Remove unverified Portra/Gold D-min defaults.
- Allow stock presets without calibration.
- Require calibration before Negadoctor auto-processing.
- Add JSON schema migration.

## UX
Possible UI states:
- Film: Kodak Pro Image 100
- Calibration: “Measured roll profile loaded”
- or “D-min calibration required”

## Tests
- uncalibrated roll cannot generate calibrated Negadoctor output
- calibrated roll round-trip
- schema migration
- invalid D-min rejection
- stock metadata independent of calibration

## Exit criteria
No generic film stock silently implies an invented D-min.

---

# Phase 3 — Processing Dependency Model

## Goal
Make parameter recalculation deterministic.

## Dependency order

```text
D-min
  ↓
D-max
  ↓
scan bias
  ↓
highlight WB
  ↓
paper black
  ↓
print exposure
```

Paper grade and paper gloss remain independent print-rendering controls.

## Proposed parameter state

```rust
enum ParameterMode<T> {
    Auto(T),
    Manual(T),
}
```

or equivalent internal metadata.

## Rules
- Auto D-max change -> recompute auto scan bias.
- Auto/manual highlight WB change -> recompute auto paper black.
- Paper black change -> recompute auto print exposure.
- Manual downstream values are not overwritten.

## Tests
For every dependency:
- auto downstream updates
- manual downstream remains unchanged
- reset-to-auto recalculates correctly

## Exit criteria
Advanced controls cannot silently produce stale dependent values.

---

# Phase 4 — Real Hardware Control

## Goal
Make the GUI controls real on the LS-40.

## 4.1 Cancellation

### Scanner pipeline API
Pass cancellation state into the scanner loop.

Conceptually:

```rust
pub struct ScanControl {
    pub cancel_current: Arc<AtomicBool>,
    pub stop_after_current: Arc<AtomicBool>,
}
```

### Checkpoints
Check:
- before frame
- before each pass
- inside progress callback
- after each pass
- after current frame before next frame

If `scan_frame_with` supports abort through the callback:

```rust
if cancel_current.load(Ordering::Relaxed) {
    return ControlFlow::Break(());
}
```

Map abort cleanly to a scanner event/error distinct from USB failure.

## 4.2 Stop-after-current
After completing the current artifact:
- emit frame
- stop before opening next frame
- return controlled stopped state

## 4.3 Offset implementation
Reuse existing frame-position geometry.

Required checks:
- adjusted frame remains in scanner bounds
- clear error on impossible offset
- frame 1 and frame 6 boundary cases

## Hardware tests
On LS-40:
1. Start full-resolution frame.
2. Cancel during acquisition.
3. Verify scanner recovers.
4. Start another scan successfully.
5. Start multi-frame strip.
6. Request stop-after-current.
7. Verify exactly current frame completes.
8. Resume a later scan.

## Exit criteria
Mock and hardware behaviour match semantically.

---

# Phase 5 — Per-Frame Streaming & Memory Architecture

## Goal
Review completed frames while later frames continue scanning.

## Current issue
Hardware scanning returns a complete `StripScanResult` before sending any frames to review.

## New seam
Scanner pipeline should emit artifacts incrementally.

Possible API:

```rust
pub fn scan_strip_with_session(
    ...,
    mut on_event: impl FnMut(ScanEvent),
    mut on_frame: impl FnMut(FrameArtifact) -> ControlFlow<()>,
) -> Result<StripSummary, ScanError>
```

or use a channel-based internal API.

## Benefits
- frame 1 can be reviewed while frame 2 scans
- lower peak memory
- stop-after-current becomes natural
- UI receives genuine live workflow

## Memory strategy
Avoid keeping:
- full scanner samples
- duplicate full working image
- duplicate preview
for every frame unnecessarily.

Suggested ownership:
- master scanner samples retained only where required
- one full working image
- downscaled preview cache
- persisted TIFF may permit releasing raw samples later if desired

## Tests
- mock backend emits frame before strip complete
- ordering guaranteed
- cancellation after frame emission
- no duplicated frame delivery
- frame numbers preserved

## Exit criteria
`FrameReady` is genuinely per-frame in both mock and hardware modes.

---

# Phase 6 — Darktable Parity Fixture

## Goal
Replace “formula looks right” with evidence from a real LS-40 frame.

## Reference fixture
Use one of the already-characterised Kodak Pro Image 100 frames.

Store:
- scanner sample fixture or compact deterministic crop
- image dimensions
- crop bounds
- D-min
- Darktable version
- D-max result
- scan bias result
- highlight-WB selection rectangle
- highlight-WB gains
- paper black
- print exposure
- selected rendered RGB reference pixels

## Recommended tolerance classes

### Exact / structural
- orientation code
- XMP struct byte size
- XMP history operation ordering
- frame number
- crop bounds

### Tight numerical
- D-max: e.g. `1e-4` to `1e-3`
- scan bias: `1e-4` to `1e-3`
- WB gains: defined tolerance from Darktable output
- paper black: defined tolerance
- print exposure: defined tolerance

### Rendered pixels
Use a documented tolerance in output colour space because:
- LCMS implementation details
- floating point
- platform differences

## Test stages

### A. ICC input transform parity
Take known scanner RGB pixels and compare:
- Coolscan Studio linear Rec.2020
- Darktable colourin output

### B. Picker parity
For the exact same rectangle:
- min
- max
- mean

### C. Negadoctor parameter parity
Compare auto functions to Darktable.

### D. Render parity
Compare representative output pixels.

## Exit criteria
No documentation or UI says “Darktable parity” until this suite passes.

---

# Phase 7 — Export Hardening

## Goal
Ensure output files remain trustworthy outside the current development machine.

## 7.1 XMP writer
Replace manual XML interpolation with an XML writer.

Required escaping tests:
- filename containing `&`
- filename containing `"`
- Unicode filename

## 7.2 ICC handling
At startup / export:
- locate Darktable profile directory
- verify `NKLS4000LS40_N.icc`
- verify hash if practical
- provide installation action or exact diagnostic

Do not silently generate a broken XMP reference.

## 7.3 TIFF policy
Decide explicitly whether to embed ICC.

Recommended split:

### Scanner master
- original high-bit-depth scanner samples
- no crop
- embedded scanner ICC if legally/distribution-wise appropriate
- acquisition metadata

### Working TIFF
- accepted crop
- original high-bit-depth scanner samples
- intended companion Darktable XMP

## 7.4 Sidecar validation
Generated XMP should be tested with `darktable-cli` on supported Darktable versions.

## Exit criteria
A TIFF/XMP pair produced on a clean test machine can be opened correctly without manual hidden setup.

---

# Phase 8 — GitHub CI Foundation

## Goal
Make every future code review independently verifiable.

CI should answer:

1. Does it compile?
2. Is it formatted?
3. Does Clippy find likely mistakes?
4. Do unit/integration tests pass?
5. Does the Windows target build?
6. Are generated test artifacts valid?
7. Are release binaries produced reproducibly?
8. Which claims still require physical LS-40 testing?

---

## 8.1 Repository layout

Add:

```text
.github/
  workflows/
    ci.yml
    windows-build.yml
    darktable-parity.yml
    release.yml
```

Optional:

```text
.github/
  dependabot.yml
```

---

## 8.2 Core CI workflow

File:

```text
.github/workflows/ci.yml
```

Recommended content:

```yaml
name: CI

on:
  push:
    branches:
      - main
      - stabilisation
  pull_request:
    branches:
      - main

permissions:
  contents: read

env:
  CARGO_TERM_COLOR: always
  RUST_BACKTRACE: 1

jobs:
  fmt:
    name: Rustfmt
    runs-on: ubuntu-latest

    steps:
      - name: Checkout
        uses: actions/checkout@v4

      - name: Install Rust
        uses: dtolnay/rust-toolchain@stable
        with:
          components: rustfmt

      - name: Check formatting
        run: cargo fmt --all -- --check

  clippy:
    name: Clippy
    runs-on: ubuntu-latest

    steps:
      - name: Checkout
        uses: actions/checkout@v4

      - name: Install Rust
        uses: dtolnay/rust-toolchain@stable
        with:
          components: clippy

      - name: Cache Rust build
        uses: Swatinem/rust-cache@v2

      - name: Clippy
        run: cargo clippy --all-targets --all-features -- -D warnings

  test:
    name: Unit & integration tests
    runs-on: ubuntu-latest

    steps:
      - name: Checkout
        uses: actions/checkout@v4

      - name: Install Rust
        uses: dtolnay/rust-toolchain@stable

      - name: Cache Rust build
        uses: Swatinem/rust-cache@v2

      - name: Test
        run: cargo test --all-targets --all-features --locked

  docs:
    name: Documentation build
    runs-on: ubuntu-latest

    steps:
      - name: Checkout
        uses: actions/checkout@v4

      - name: Install Rust
        uses: dtolnay/rust-toolchain@stable

      - name: Build docs
        env:
          RUSTDOCFLAGS: -D warnings
        run: cargo doc --no-deps --all-features
```

### Important note
Action major versions should be kept current. Before merging the workflow, confirm current supported majors in the GitHub Marketplace/repositories.

---

## 8.3 Windows build workflow

The real production environment is Windows, so Linux-only CI is insufficient.

File:

```text
.github/workflows/windows-build.yml
```

Recommended:

```yaml
name: Windows Build

on:
  push:
    branches:
      - main
      - stabilisation
  pull_request:
    branches:
      - main

permissions:
  contents: read

env:
  CARGO_TERM_COLOR: always

jobs:
  windows:
    name: Windows stable
    runs-on: windows-latest

    steps:
      - name: Checkout
        uses: actions/checkout@v4

      - name: Install Rust
        uses: dtolnay/rust-toolchain@stable
        with:
          components: clippy

      - name: Cache Rust build
        uses: Swatinem/rust-cache@v2

      - name: Test
        run: cargo test --all-targets --all-features --locked

      - name: Clippy
        run: cargo clippy --all-targets --all-features -- -D warnings

      - name: Release build
        run: cargo build --release --locked

      - name: Upload executable
        uses: actions/upload-artifact@v4
        with:
          name: coolscan-studio-windows
          path: target/release/coolscan-studio.exe
          if-no-files-found: error
```

This does **not** test USB scanner behaviour, but it proves:
- Windows compilation
- Windows tests
- dependency linking
- production executable creation

---

## 8.4 Darktable parity CI

This should be a separate job because it is more integration-heavy.

File:

```text
.github/workflows/darktable-parity.yml
```

Recommended strategy:

### Option A — Linux Darktable
If the XMP behaviour under test is cross-platform:
- install supported Darktable package/version
- run reference TIFF/XMP fixture
- compare output/result

### Option B — Windows self-hosted runner
For strongest parity with Austin's environment:
- use a dedicated Windows machine
- install exact supported Darktable version
- no scanner required
- run reference fixture tests

The parity test should not require physical USB hardware.

Example shape:

```yaml
name: Darktable Parity

on:
  pull_request:
    branches:
      - main
  workflow_dispatch:

permissions:
  contents: read

jobs:
  parity:
    runs-on: self-hosted

    steps:
      - uses: actions/checkout@v4

      - uses: dtolnay/rust-toolchain@stable

      - name: Run Rust parity fixture tests
        run: cargo test darktable_parity --locked -- --nocapture

      - name: Run Darktable CLI reference render
        shell: pwsh
        run: |
          $dt = "C:\Program Files\darktable\bin\darktable-cli.exe"
          if (!(Test-Path $dt)) {
            throw "Supported Darktable CLI not installed"
          }

          # Invoke fixture render here.
          # Compare result using project test utility.
```

---

## 8.5 What CI cannot prove

GitHub-hosted runners cannot validate:

- physical LS-40 USB communication
- WinUSB/Zadig state
- film loading
- frame discovery accuracy
- scan cancellation against real hardware
- scanner recovery after cancellation
- real acquisition stability
- OpenICE effectiveness on real infrared data

These require a separate **hardware acceptance gate**.

---

# Phase 9 — Hardware Acceptance Gate

## Goal
Make physical-scanner claims explicit and repeatable.

Create:

```text
docs/HARDWARE_ACCEPTANCE.md
```

Each release candidate should record:

```text
Commit:
Date:
Scanner:
Firmware:
Adapter:
Windows version:
WinUSB driver:
Darktable version:
Film strip:
Operator:
```

## Mandatory tests

### Scanner basics
- scanner detected
- film load detected
- discovery returns expected frame count
- eject works

### Scan modes
- 725 DPI single frame
- 2900 DPI single frame
- multi-frame strip
- IR cleaning
- selected multi-sampling mode

### Recovery
- cancel current frame
- next scan works
- stop after current
- USB retry path if reproducible

### Processing
- D-max
- scan bias
- WB selection
- paper black
- print exposure
- orientation
- XMP import into Darktable

### Export
- TIFF valid
- XMP valid
- Darktable opens sidecar
- frame numbering preserved

## Evidence
Store:
- console logs
- screenshots where useful
- parameter output
- generated XMP
- commit SHA

Avoid checking large copyrighted/personal scans into the public repository unless appropriate.

---

# Phase 10 — Release CI

## Goal
Produce a reproducible release artifact from a tagged commit.

File:

```text
.github/workflows/release.yml
```

Suggested:

```yaml
name: Release

on:
  push:
    tags:
      - "v*"

permissions:
  contents: write

jobs:
  build-windows:
    runs-on: windows-latest

    steps:
      - uses: actions/checkout@v4

      - uses: dtolnay/rust-toolchain@stable

      - uses: Swatinem/rust-cache@v2

      - name: Test
        run: cargo test --all-targets --all-features --locked

      - name: Build
        run: cargo build --release --locked

      - name: Stage release
        shell: pwsh
        run: |
          New-Item -ItemType Directory -Force dist
          Copy-Item target/release/coolscan-studio.exe dist/
          Copy-Item README.md dist/
          Copy-Item NOTICES.md dist/
          Copy-Item LICENSE* dist/ -ErrorAction SilentlyContinue

      - name: Create ZIP
        shell: pwsh
        run: |
          Compress-Archive -Path dist/* -DestinationPath coolscan-studio-windows-x64.zip

      - name: SHA256
        shell: pwsh
        run: |
          (Get-FileHash coolscan-studio-windows-x64.zip -Algorithm SHA256).Hash |
            Out-File coolscan-studio-windows-x64.zip.sha256

      - name: Upload workflow artifact
        uses: actions/upload-artifact@v4
        with:
          name: release-windows-x64
          path: |
            coolscan-studio-windows-x64.zip
            coolscan-studio-windows-x64.zip.sha256
```

A GitHub Release publishing step can be added once licensing and release policy are settled.

---

# 4. CI Trust Model for Future Reviews

When reviewing the repository in ChatGPT, the following evidence hierarchy should be used.

## Level 1 — Static review only
Evidence:
- source code
- commit history

Confidence:
- architecture and logic only

Not enough to claim:
- builds
- tests pass
- Windows support
- Darktable parity

---

## Level 2 — Green core CI
Required checks:
- Rustfmt
- Clippy
- Linux tests
- Windows tests
- Windows release build

Confidence:
- code compiles
- automated suite passes
- common Rust correctness problems reduced

Still not enough for:
- LS-40 hardware claims

---

## Level 3 — Green Darktable parity CI
Adds:
- fixed real scanner fixture
- Darktable CLI comparison

Confidence:
- processing and XMP compatibility are independently demonstrated

---

## Level 4 — Hardware acceptance recorded
Adds:
- physical LS-40 test
- exact commit SHA
- required scan workflow

Confidence:
- scanner behaviour is release-candidate validated

---

# 5. Branch Protection / Repository Rules

Once CI is running reliably, configure `main` so pull requests cannot merge unless required checks pass.

Recommended required checks:

```text
Rustfmt
Clippy
Unit & integration tests
Windows stable
Documentation build
Darktable parity
```

`Darktable parity` may initially remain non-required if it relies on an occasionally offline self-hosted runner. Once stable, make it required for changes touching:

```text
src/processing/**
src/darktable/**
src/tiff.rs
profiles/**
```

If GitHub rulesets support path-aware enforcement in the chosen configuration, use them; otherwise keep the parity workflow required globally once it is reliable.

Also recommended:
- require PR before merge
- require branch to be up-to-date before merge
- dismiss stale approvals after new commits
- disallow force-push to `main`
- disallow branch deletion
- optionally require signed commits later

---

# 6. CI Failure Policy

A failed required check should never be waved through without explanation.

Recommended policy:

### Rustfmt failure
Fix formatting.

### Clippy failure
Fix warning or explicitly justify a narrowly scoped `#[allow(...)]`.

### Unit test failure
Do not merge.

### Windows build failure
Do not merge.

### Darktable parity failure
Treat as processing regression until proven otherwise.

### Hardware acceptance failure
Do not tag a release containing scanner-path changes.

---

# 7. Test Organisation

Recommended test layers:

```text
src/**                  unit tests
tests/core_*.rs         pure integration tests
tests/xmp_*.rs          XMP serialization/parser tests
tests/parity_*.rs       real fixture parity tests
tests/ui_*.rs           headless review-state tests
tests/hardware_*        ignored/manual hardware tests where practical
```

Use Rust ignored tests for operations that require real hardware:

```rust
#[test]
#[ignore = "requires Nikon LS-40 hardware"]
fn ls40_cancel_and_recover() {
    ...
}
```

Run manually:

```powershell
cargo test -- --ignored --nocapture
```

CI should not pretend these hardware tests ran unless they actually ran on a self-hosted scanner machine.

---

# 8. Suggested PR Sequence

Keep remediation PRs small enough to review independently.

## PR 1 — Core correctness
- frame numbering
- retry scope
- all-frames-failed
- sample-limit alignment
- no auto-mock fallback
- disable no-op offset

## PR 2 — Roll/calibration model
- separate stock and calibration
- migrate JSON
- remove unverified D-min presets

## PR 3 — Processing dependency state
- auto/manual semantics
- recalculation tests

## PR 4 — Hardware cancellation
- control token
- callback abort
- stop-after-current
- hardware acceptance notes

## PR 5 — Per-frame streaming
- new scan callback/channel seam
- lower-memory workflow
- preserve ordering

## PR 6 — Darktable parity fixture
- real fixture
- picker reference values
- rendered-pixel references

## PR 7 — Export hardening
- XML writer
- ICC validation
- TIFF/XMP policy

## PR 8 — CI foundation
- `ci.yml`
- Windows workflow
- required checks

## PR 9 — Darktable CI
- self-hosted or reproducible runner
- parity workflow

## PR 10 — Release preparation
- README
- repository metadata
- notices/license correction
- release workflow

---

# 9. Definition of Done

Coolscan Studio should only be considered ready for an initial trusted release when:

- [ ] source frame numbers are preserved
- [ ] no known no-op GUI controls remain
- [ ] cancellation works on the physical LS-40
- [ ] stop-after-current works on the physical LS-40
- [ ] per-frame delivery works on real hardware
- [ ] zero successful frames returns an error
- [ ] roll calibration is separated from film-stock metadata
- [ ] no unverified generic D-min presets are shipped
- [ ] processing dependency recalculation is deterministic
- [ ] real LS-40/Darktable parity fixture passes
- [ ] XMP is XML-safe
- [ ] ICC availability is checked
- [ ] master/working output policy is documented
- [ ] GitHub CI is green
- [ ] Windows CI is green
- [ ] Darktable parity CI is green
- [ ] physical hardware acceptance is recorded against the release SHA
- [ ] licensing/notices are reconciled
- [ ] Cargo repository/readme metadata is correct
- [ ] release artifact is generated from CI, not manually

---

# 10. Recommended Immediate Next Step

Implement **PR 1 — Core correctness** first.

This deliberately avoids architectural upheaval and gives the new CI something meaningful to protect.

After PR 1, add the **core + Windows GitHub CI workflows immediately**, before beginning the larger hardware-streaming and Darktable-parity changes. That way every subsequent remediation PR is independently compiled and tested by GitHub rather than relying only on the implementation agent’s own local test claims.

The intended development rhythm after that is:

```text
small PR
  ↓
GitHub CI
  ↓
review
  ↓
merge
  ↓
next remediation phase
```

For scanner-path changes:

```text
small PR
  ↓
GitHub CI
  ↓
review
  ↓
LS-40 hardware acceptance
  ↓
merge / release gate
```

This gives us a much stronger basis for future repository reviews: we can inspect not only what the code claims to do, but also whether GitHub independently built and tested the exact commit being reviewed.
