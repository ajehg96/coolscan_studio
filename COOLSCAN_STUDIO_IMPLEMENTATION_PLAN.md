# Coolscan Studio — Implementation Plan
## Post-scan processing, Darktable preparation, and GUI architecture

**Repository:** `ajehg96/coolscan_studio`  
**Target branch baseline:** `main` as reviewed on 2026-09-12  
**Primary hardware:** Nikon COOLSCAN IV ED / LS-40 ED with SA-21  
**Current status:** unattended strip scanning, automatic crop, 16-bit TIFF output, OpenICE, multi-pass averaging, USB recovery, and CLI workflow are already working.

---

# 1. Purpose

Coolscan Studio has reached the point where the scanner-control problem is largely solved. The next stage should turn it from a capable CLI scanning engine into a reusable application architecture with a post-scan preparation workflow.

The goal is **not** to replace Darktable as a general image editor.

The goal is to make each scanned negative arrive in Darktable already technically prepared:

1. scanned reliably;
2. cropped safely;
3. colour-managed correctly for the LS-40;
4. inverted using repeatable negadoctor-compatible mathematics;
5. technically normalised where the operation is deterministic;
6. oriented correctly;
7. white-balanced from one human-selected neutral highlight;
8. accompanied by a Darktable-compatible processing recipe;
9. still backed by an untouched high-bit-depth scanner master.

The intended human interaction per frame should become approximately:

> **Check orientation → select a neutral highlight → review preview → accept**

Everything else that is mechanical should be automated.

---

# 2. Current architecture

The current source tree is approximately:

```text
src/
├── main.rs
├── cli.rs
├── boundaries.rs
├── crop.rs
├── frame_position.rs
├── bmp.rs
└── tiff.rs
```

The current `main.rs` owns most of the orchestration:

```text
CLI parsing
  ↓
scanner discovery
  ↓
Session open
  ↓
strip discovery / perforation registration
  ↓
frame scanning
  ├── focus
  ├── exposure metering
  ├── multi-pass averaging
  ├── OpenICE
  ├── USB retry
  └── session refresh
  ↓
auto crop
  ↓
BMP / TIFF output
```

This works well, but it creates an important constraint:

> The scanner pipeline currently produces files directly rather than returning a reusable frame result.

That is the main architectural seam to introduce before building a GUI.

---

# 3. Design principles

## 3.1 Preserve the proven scanner path

The existing LS-40 workflow is valuable and hardware-tested. Refactoring should initially be **behaviour-preserving**.

No colour-processing or GUI work should be mixed into the first scanner-pipeline refactor.

## 3.2 Separate acquisition from interpretation

The scanner should produce faithful high-bit-depth samples.

Negative inversion, orientation, white balance, print simulation, and Darktable preparation should happen in a separate processing stage.

## 3.3 Keep a master that can be reprocessed

The preferred output model is:

```text
scanner master
    = high-bit-depth scanner acquisition

working TIFF
    = optional cropped/oriented derivative

Darktable XMP
    = processing recipe
```

The exact long-term storage policy can remain configurable, but the architecture should not make destructive processing unavoidable.

## 3.4 Reproduce Darktable, do not approximate it

For the technical negadoctor controls, Coolscan Studio should use the same formulas and ordering as Darktable.

The sequence established from Darktable's implementation is:

```text
D-min / film base
      ↓
D max
      ↓
scan exposure bias
      ↓
shadow correction, if used
      ↓
highlight white balance
      ↓
paper black
      ↓
print exposure
      ↓
paper grade / paper gloss / creative edits
```

In particular:

- D max does not depend on highlight white balance.
- scan exposure bias does not depend on highlight white balance.
- paper black **does** depend on white-balance state.
- automatic print exposure **does** depend on the state preceding it.

Therefore, if highlight WB changes, paper black and print exposure must be recomputed.

## 3.5 Make hardware-free testing the default

Most development should be possible without the scanner attached.

Hardware should be needed only for a small set of explicit end-to-end smoke tests.

---

# 4. Target architecture

A practical target tree is:

```text
src/
├── lib.rs
│
├── scanner/
│   ├── mod.rs
│   ├── device.rs
│   ├── pipeline.rs
│   ├── recovery.rs
│   └── types.rs
│
├── framing/
│   ├── mod.rs
│   ├── boundaries.rs
│   ├── crop.rs
│   └── frame_position.rs
│
├── image/
│   ├── mod.rs
│   ├── samples.rs
│   ├── bmp.rs
│   └── tiff.rs
│
├── processing/
│   ├── mod.rs
│   ├── color.rs
│   ├── negadoctor.rs
│   ├── analysis.rs
│   ├── orientation.rs
│   └── roll.rs
│
├── darktable/
│   ├── mod.rs
│   ├── xmp.rs
│   └── style.rs
│
├── ui/
│   ├── mod.rs
│   ├── app.rs
│   ├── scan.rs
│   ├── review.rs
│   └── widgets.rs
│
└── bin/
    ├── coolscan-cli.rs
    └── coolscan-studio.rs
```

This is a target, not a requirement to perform one giant file move.

The migration should be incremental.

---

# 5. Core data model

The most important new internal type should be a frame result that exists independently of files on disk.

A first version could conceptually look like:

```rust
pub struct FrameArtifact {
    pub frame_number: usize,
    pub dpi: u16,

    pub samples: Samples,
    pub pass: Pass,

    pub crop: Option<CropDecision>,
    pub scan_metadata: ScanMetadata,
}
```

Later it can grow into:

```rust
pub struct PreparedFrame {
    pub source: FrameArtifact,
    pub roll: RollProfile,
    pub processing: ProcessingState,
    pub orientation: Orientation,
}
```

Suggested supporting types:

```rust
pub struct ScanMetadata {
    pub scanner_model: String,
    pub focus_position: Option<u16>,
    pub exposures: Option<[u32; 3]>,
    pub software_passes: u8,
    pub infrared_cleaned_pixels: Option<usize>,
}

pub struct RollProfile {
    pub name: String,
    pub film_stock: String,
    pub dmin: [f32; 3],
    pub scanner_profile_id: String,
}

pub struct ProcessingState {
    pub dmax: f32,
    pub scan_bias: f32,
    pub wb_high: [f32; 3],
    pub wb_low: [f32; 3],
    pub paper_black: f32,
    pub paper_grade: f32,
    pub paper_gloss: f32,
    pub print_exposure: f32,
}
```

The exact names can change. The key design requirement is that **scanner acquisition, processing state, and file output are separate concepts**.

---

# 6. Phase 0 — Freeze the known-good baseline

**Size:** Small  
**Risk:** Low

Before moving code, capture the present behaviour so future refactors can prove that they did not regress scanning.

## Work

- Record the current `main` commit as the scanner baseline.
- Run the complete Rust test suite.
- Run the existing synthetic crop tests.
- Record a representative CLI invocation for:
  - discovery only;
  - one 725 DPI scan;
  - one 2900 DPI scan;
  - full strip;
  - TIFF;
  - OpenICE;
  - multi-pass.
- Capture representative console output.
- Record hashes and dimensions for a small set of existing trusted output files where practical.

## Testing seam

No scanner code changes yet.

Create a small `tests/fixtures/` policy document describing which fixtures may be checked into Git and which must remain external because of size.

Avoid committing 60 MB TIFFs simply to create regression tests.

## Acceptance criteria

- `cargo test` passes.
- Existing investigation tests still pass.
- At least one known-good hardware scan command is documented.
- A baseline commit SHA is recorded in the implementation notes.

---

# 7. Phase 1 — Extract the scanner pipeline from `main.rs`

**Size:** Medium  
**Risk:** Medium  
**Purpose:** Create the seam required by both CLI and GUI.

This phase must not add post-processing.

## Work

Extract scanning orchestration into reusable Rust APIs.

Suggested interface:

```rust
pub struct ScanRequest {
    pub frames: FrameSelection,
    pub dpi: u16,
    pub samples: u8,
    pub clean: bool,
    pub auto_crop: bool,
}

pub struct StripScanResult {
    pub frames: Vec<FrameArtifact>,
    pub discovery: StripDiscoveryMetadata,
}

pub fn scan_strip(
    device: &Device,
    request: &ScanRequest,
    progress: impl FnMut(ScanEvent),
) -> Result<StripScanResult, ScanError>;
```

Introduce typed progress events instead of printing directly from the scanner core:

```rust
pub enum ScanEvent {
    Connecting,
    DiscoveringFrames,
    FrameStarted { frame: usize },
    Metering { frame: usize, percent: u8 },
    Acquiring { frame: usize, percent: u8 },
    Cleaning { frame: usize },
    Cropping { frame: usize },
    Retrying { frame: usize, attempt: usize },
    FrameComplete { frame: usize },
}
```

The CLI should translate these events into the exact or near-exact current console presentation.

## Important constraint

Do not let the scanner core call:

```rust
println!()
eprintln!()
std::process::exit()
```

Core code should return errors and events.

## Testing seam

### Unit tests

- `ScanRequest` validation.
- scan result metadata types.
- progress event ordering using a fake pipeline component where possible.

### Regression tests

CLI parser tests remain unchanged.

### Hardware seam

Run the same one-frame command before and after the refactor and compare:

- frame discovery count;
- crop geometry;
- TIFF dimensions;
- bit depth;
- scanner exposure metadata;
- cleaned-pixel count within expected run-to-run variation.

## Acceptance criteria

The CLI continues to perform all existing operations with no intentional behavioural change.

At the end of this phase:

```text
CLI → scanner::pipeline → FrameArtifact
```

must exist.

---

# 8. Phase 2 — Separate file output from scanning

**Size:** Small to Medium  
**Risk:** Low

At present, scanning and file writing are closely coupled.

Move output policy into a separate layer.

## Work

Introduce functions such as:

```rust
write_scanner_master(...)
write_working_tiff(...)
write_preview(...)
```

Add an output policy:

```rust
pub struct OutputPolicy {
    pub save_master: bool,
    pub save_cropped_tiff: bool,
    pub save_preview: bool,
}
```

The scanner pipeline should return data; the caller decides what to save.

## TIFF work

Extend the TIFF layer gradually rather than rewriting it immediately.

Potential future metadata:

- scanner make/model;
- resolution;
- frame number;
- acquisition parameters;
- orientation;
- ICC profile;
- software/version identifier.

Do **not** embed an ICC profile until colour-management behaviour is verified in Phase 4.

## Testing seam

The TIFF writer is already an excellent pure-function boundary.

Add tests for:

- dimensions;
- bit depth;
- sample ordering;
- orientation tag;
- metadata tags as each is added;
- exact byte-level sample preservation.

## Acceptance criteria

A frame can be scanned successfully without the pipeline itself writing a file.

---

# 9. Phase 3 — Formalise roll metadata

**Size:** Small  
**Risk:** Low

The processing work has shown that D-min is naturally a **roll-level property** rather than a frame-level property.

## Work

Introduce:

```rust
pub struct RollProfile {
    pub id: RollId,
    pub film_name: String,
    pub dmin: [f32; 3],
    pub scanner_profile: ScannerProfile,
}
```

Initially support manual creation.

For the first known roll:

```text
Film: Kodak Pro Image 100
D-min:
  R = 0.8965
  G = 0.9093
  B = 0.8816
Scanner profile:
  LS-4000 / LS-40 negative
```

Do not hard-code Pro Image into processing code.

Persist roll profiles in a simple application-owned format such as JSON/TOML.

## Future extension

Coolscan Studio may later estimate D-min from deliberately retained unexposed film regions or inter-frame gaps.

That should be a separate enhancement.

## Testing seam

- serialisation round trip;
- invalid D-min rejection;
- profile versioning;
- migration test when schema changes.

## Acceptance criteria

A scan session can be associated with a roll profile and every frame inherits it.

---

# 10. Phase 4 — Colour-management spike

**Size:** Medium  
**Risk:** High  
**Purpose:** Establish a trustworthy pixel domain before implementing negadoctor.

This is the most important technical research phase.

The current TIFF samples are scanner-device RGB.

Darktable applies the scanner input ICC before negadoctor.

Coolscan Studio therefore needs to reproduce that conversion before its analysis can match Darktable.

## Work

### 4.1 Select a colour-management implementation

Prefer a mature LittleCMS 2 based implementation.

Do a short dependency spike rather than committing immediately to a specific Rust wrapper.

Requirements:

- load the LS-40 negative ICC;
- accept 16-bit or floating-point RGB;
- preserve linear precision;
- transform into a known linear working colour space;
- process full-resolution frames efficiently;
- support Windows packaging.

### 4.2 Define the working colour space

For Darktable parity, determine exactly how to produce values equivalent to Darktable's working pipeline.

Candidate target:

```text
linear Rec.2020 RGB
```

But this must be verified empirically rather than assumed.

### 4.3 Add `processing::color`

Suggested API:

```rust
pub trait ColorTransform {
    fn transform_rgb(&self, rgb: [f32; 3]) -> [f32; 3];
}

pub struct ScannerColorPipeline { ... }
```

Provide an image-wide bulk transform.

## Critical test seam: Darktable parity

Create a tiny deterministic fixture from a real LS-40 scan:

- e.g. 32 × 32 or 64 × 64 pixels;
- preserve original scanner sample values;
- process the same fixture through Darktable;
- record the working-space values at selected coordinates if practicable.

Test Coolscan Studio against those reference values.

Tolerance should be defined explicitly.

For example:

```text
absolute channel error <= 1e-4
```

or an empirically justified equivalent.

## Acceptance criteria

Before negadoctor code is added, Coolscan Studio must be able to transform a known set of LS-40 pixels to values that match Darktable closely enough to make the picker formulas reproduce Darktable.

---

# 11. Phase 5 — Implement negadoctor mathematics as a pure Rust module

**Size:** Medium  
**Risk:** Medium

Create:

```text
src/processing/negadoctor.rs
```

This module should contain **no scanner code, no GUI code, and no file I/O**.

## Work

Model the parameters explicitly:

```rust
pub struct NegadoctorParams {
    pub dmin: [f32; 3],
    pub dmax: f32,
    pub offset: f32,
    pub wb_high: [f32; 3],
    pub wb_low: [f32; 3],
    pub paper_black: f32,
    pub paper_grade: f32,
    pub paper_gloss: f32,
    pub print_exposure: f32,
}
```

Port the relevant Darktable formulas carefully.

Required operations:

```rust
fn auto_dmax(...)
fn auto_scan_bias(...)
fn auto_highlight_wb(...)
fn auto_shadow_wb(...)
fn auto_paper_black(...)
fn auto_print_exposure(...)
fn render_positive(...)
```

Keep Darktable terminology in comments so future comparison remains easy.

## Do not optimise first

First objective: parity and readability.

SIMD / Rayon / GPU work can come later if needed.

## Testing seam: formula-level unit tests

Each operation should have deterministic tests with hand-constructed pixel extrema.

Examples:

- D max picks the channel maximum density.
- scan bias picks the required minimum ratio.
- highlight WB always normalises one channel to `1.0`.
- paper black changes when highlight WB changes.
- print exposure changes when paper black changes.

## Testing seam: golden Darktable values

Use values already observed during manual work as initial integration fixtures.

Examples from the current Pro Image roll include approximate frame outputs such as:

```text
D max:
  1.68
  2.56
  3.27

scan bias:
  -0.02
  +0.01
  +0.11

highlight WB examples:
  1.19 / 1.12 / 1.00
  1.24 / 1.17 / 1.00
  1.95 / 1.62 / 1.00
```

These are not unit-test constants by themselves; they become useful once linked to fixed fixture pixel data and the exact selected region.

## Acceptance criteria

Given the same transformed pixels, D-min, and sample rectangle, Coolscan Studio produces negadoctor values matching Darktable within an agreed tolerance.

---

# 12. Phase 6 — Automatic frame analysis

**Size:** Medium  
**Risk:** Medium

This phase implements the repeatable operations we have identified.

## Automatic operations

Using the complete cropped image area:

1. D max;
2. scan exposure bias.

After human WB selection:

3. paper black;
4. print exposure.

## API

```rust
pub struct TechnicalAnalysis {
    pub dmax: f32,
    pub scan_bias: f32,
}

pub fn analyse_pre_white_balance(
    image: &WorkingImage,
    roll: &RollProfile,
) -> TechnicalAnalysis;
```

Then:

```rust
pub fn finish_after_white_balance(
    image: &WorkingImage,
    params: &mut NegadoctorParams,
) {
    params.paper_black = auto_paper_black(...);
    params.print_exposure = auto_print_exposure(...);
}
```

This split should be explicit because it encodes the true dependency ordering.

## Whole-frame sampling semantics

Darktable's picker uses min/max/selected statistics, not simply the arithmetic mean.

Reproduce its semantics precisely.

Do not replace picker behaviour with percentiles unless there is a deliberate, separately tested design decision.

## Testing seam

- synthetic image with known extrema;
- real cropped fixture;
- parity against Darktable for each picker independently.

## Acceptance criteria

For a fixed frame and D-min, Coolscan Studio reproduces the same whole-frame automatic values as Darktable.

---

# 13. Phase 7 — Orientation model

**Size:** Small  
**Risk:** Low

Orientation is mathematically independent of the whole-frame extrema, but it matters strongly to the review experience.

## Work

Introduce:

```rust
pub enum Orientation {
    Normal,
    Rotate90,
    Rotate180,
    Rotate270,
}
```

Potentially add mirror states later only if a real use case appears.

Orientation should initially be metadata / view transformation rather than destructive pixel rotation.

## Strip-level convenience

Offer:

```text
Apply orientation to:
(•) this frame
( ) remaining frames
( ) whole strip
```

Film loaded into the SA-21 will often share orientation across the entire strip.

## Testing seam

- coordinate mapping;
- selection rectangle transformation;
- XMP orientation output;
- preview dimensions after 90° rotation.

## Acceptance criteria

A neutral-highlight selection made on a rotated preview maps to the correct underlying pixels.

This is critical.

---

# 14. Phase 8 — Review UI prototype

**Size:** Medium to Large  
**Risk:** Medium

Only now should GUI work begin.

The first GUI should be a **review tool for existing frame data**, not yet the full scanner controller.

That keeps the UI work testable without hardware.

## Suggested first screen

```text
┌──────────────────────────────────────────────────┐
│ Frame 3 of 6                          Pro Image 100│
├──────────────────────────────────────────────────┤
│                                                  │
│               positive preview                   │
│                                                  │
│        drag rectangle on neutral highlight       │
│                                                  │
├──────────────────────────────────────────────────┤
│   ↶ Rotate left   180°   Rotate right ↷          │
│                                                  │
│ Neutral reference: [ Select / Reselect ]         │
│                                                  │
│                    [ Accept & Next ]              │
└──────────────────────────────────────────────────┘
```

Advanced drawer:

```text
D-min             0.8965 / 0.9093 / 0.8816
D max             2.56
scan bias        +0.01
highlight WB      1.24 / 1.17 / 1.00
paper black      +0.10
paper grade       4.00
paper gloss       0.75
print exposure   -0.04 EV
```

## Selection behaviour

The UI should support:

- click-drag rectangle;
- visible overlay;
- immediate re-render after selection;
- undo/reselect;
- a clear indication if selection is too dark, clipped, or too small.

Do not attempt automatic neutral-object recognition in this phase.

## Testing seam

Use a mock `PreparedFrame` loaded from fixtures.

GUI development should not require the LS-40.

Test:

- rotation;
- rectangle mapping;
- parameter recalculation;
- switching frames;
- persistence of per-frame state.

## Acceptance criteria

A user can open a recorded frame, orient it, select a neutral area, and obtain a finished positive preview with automatic paper black and print exposure.

---

# 15. Phase 9 — Darktable sidecar research spike

**Size:** Small to Medium  
**Risk:** High

Do not build XMP generation from memory or assumption.

First determine exactly what Darktable writes for these modules.

## Work

Create a controlled TIFF and save several Darktable sidecars:

1. input profile only;
2. input profile + negadoctor default;
3. input profile + known D-min;
4. full tested frame;
5. orientation changes.

Diff the XMP files.

Document:

- module identifiers;
- module version numbers;
- parameter encoding;
- history ordering;
- orientation representation;
- style-specific fields;
- checksums/hashes if present.

## Testing seam

Create a fixture XMP generated by Darktable.

Build a parser/serializer test that:

```text
read → parse → serialise → semantic compare
```

Exact byte equality is not necessary if Darktable accepts the result and parameters round-trip.

## Acceptance criteria

Coolscan Studio can generate an XMP for a fixture image that Darktable opens with the expected modules and values.

---

# 16. Phase 10 — Darktable XMP writer

**Size:** Medium  
**Risk:** Medium after Phase 9

Implement:

```text
src/darktable/xmp.rs
```

The XMP should initially encode only what Coolscan Studio owns:

- scanner input profile;
- negadoctor values;
- orientation;
- possibly crop if Darktable needs to know one;
- processing history required for those modules.

Do not try to generate a large general-purpose Darktable history.

## Testing seam

### Structural tests

- valid XML;
- required namespaces;
- expected module history count.

### Round-trip test

1. Generate TIFF + XMP.
2. Open/import in Darktable.
3. Confirm:
   - LS-40 profile selected;
   - D-min correct;
   - D max correct;
   - scan bias correct;
   - highlight WB correct;
   - paper black correct;
   - print exposure correct;
   - orientation correct.

### Golden screenshot / manual acceptance

Maintain one or two reference frames where Coolscan Studio preview and Darktable rendering are visually and numerically compared.

## Acceptance criteria

An accepted frame can be opened in Darktable without repeating the technical setup steps.

---

# 17. Phase 11 — Integrate review UI with live scanning

**Size:** Large  
**Risk:** Medium

Now connect the proven scanner pipeline to the proven review UI.

## Workflow

```text
Start
 ↓
scanner detected
 ↓
film loaded
 ↓
discover strip
 ↓
scan frame
 ↓
auto crop
 ↓
technical analysis
 ↓
review screen
 ↓
orientation + neutral selection
 ↓
finish negadoctor auto calculations
 ↓
accept
 ↓
save TIFF + XMP
 ↓
next frame
```

## Important threading requirement

Scanner I/O must not run on the UI thread.

Use explicit worker messages/events.

The existing typed `ScanEvent` seam from Phase 1 should become the UI progress source.

## Cancellation

Design cancellation explicitly:

- cancel current scan;
- stop after current frame;
- eject film.

Do not rely on killing the process.

## Testing seam

Use a fake scanner backend that emits realistic progress events and canned frame data.

The whole GUI workflow should be testable without USB hardware.

## Acceptance criteria

A six-frame canned strip can be processed end-to-end in the GUI without scanner hardware.

Then repeat with the real LS-40.

---

# 18. Phase 12 — Full scanner GUI

**Size:** Large  
**Risk:** Medium

Only after the review flow works should the CLI controls be surfaced in the GUI.

## Controls

Basic mode:

```text
Resolution        2900 DPI
Dust removal      On
Quality           Standard / Fine / Ultimate
Output folder     ...
Film profile      Kodak Pro Image 100
```

Advanced mode:

```text
samples
interleaving
manual frame selection
auto-crop toggle
overscan diagnostics
scanner offset
```

The existing CLI should remain available as a power-user and debugging interface.

## Acceptance criteria

All existing CLI scan capabilities remain reachable, either directly or through an advanced GUI mode.

---

# 19. Phase 13 — Preview performance optimisation

**Size:** Medium  
**Risk:** Low

Only optimise after correctness.

A full 3984 × 2656 × float RGB processing chain may be unnecessary for every UI redraw.

## Strategy

Maintain:

```text
full-resolution master
        +
downscaled working preview
```

Run interactive operations on the preview.

Run final calculations carefully:

- extrema-dependent operations may need full-resolution data;
- preview-only calculation is acceptable only if validated.

Potential improvements:

- parallel pixel conversion;
- cached ICC-transformed image;
- cached pre-negadoctor density image;
- SIMD;
- GPU only if profiling justifies it.

## Acceptance criteria

Interactive orientation and WB selection feel immediate while final values remain equivalent to full-resolution processing.

---

# 20. Phase 14 — Packaging and Windows application delivery

**Size:** Medium  
**Risk:** Medium

The final Windows build must account for:

- WinUSB requirement;
- ICC profile licensing/notice;
- any LittleCMS runtime dependency;
- Darktable integration assumptions;
- scanner permissions;
- application configuration path.

## Work

- release build;
- version metadata;
- bundled notices;
- first-run scanner status diagnostics;
- optional Darktable path detection;
- export directory configuration;
- crash/error logging.

## Acceptance criteria

A clean Windows machine with the scanner's WinUSB driver configured can run Coolscan Studio without a Rust development environment.

---

# 21. Testing architecture

The project should explicitly maintain four testing layers.

## Layer A — Pure unit tests

No filesystem, no scanner.

Targets:

- crop maths;
- geometry;
- orientation;
- negadoctor formulas;
- roll profile validation;
- colour conversion helpers;
- parameter dependency logic.

Run on every commit.

## Layer B — Fixture integration tests

No scanner, filesystem allowed.

Use small recorded or synthetic images.

Targets:

```text
scanner samples
  → ICC
  → crop
  → negadoctor analysis
  → preview
  → XMP
```

Run on every commit or CI build.

## Layer C — Darktable parity tests

Darktable may be required.

Targets:

- ICC conversion equivalence;
- D max;
- scan bias;
- highlight WB;
- paper black;
- print exposure;
- XMP loading.

These may initially be manual or local scripted tests before being automated.

## Layer D — Hardware acceptance tests

Require LS-40.

Keep these few and explicit.

Suggested suite:

1. scanner list/open;
2. six-frame discovery;
3. one 725 DPI frame;
4. one 2900 DPI frame;
5. OpenICE scan;
6. 2× multi-pass;
7. full-strip unattended run;
8. GUI one-frame scan;
9. GUI full-strip scan.

Hardware tests should not be required for ordinary CI.

---

# 22. Important testing seams to preserve

## Scanner seam

```rust
trait ScannerBackend
```

Even if `nkscan` is used directly at first, design toward an abstraction that allows a recorded/fake backend.

## Colour seam

```rust
trait ColorTransform
```

Allows tests to substitute identity transforms or known matrices.

## Frame-processing seam

All negadoctor analysis should accept plain image buffers + parameters.

No GUI types.

## File-output seam

TIFF and XMP writers take complete data models and paths.

They do not know about scanner sessions.

## UI seam

UI reacts to state and events.

It should not contain scanner protocol logic.

These seams are more valuable than having many small files.

---

# 23. Error handling plan

Replace process-oriented failures with typed errors.

Suggested hierarchy:

```rust
pub enum AppError {
    Scanner(ScannerError),
    Processing(ProcessingError),
    Color(ColorError),
    Output(OutputError),
    Darktable(DarktableError),
}
```

Scanner recovery errors should retain useful context:

```text
frame number
scan phase
attempt number
USB state
whether reconnect succeeded
```

The GUI should show human-readable summaries while retaining detailed logs.

---

# 24. Logging

Introduce structured logging before the GUI lands.

Prefer events such as:

```text
scanner.open
strip.discovered frames=6
frame.scan.start frame=3 dpi=2900
frame.scan.retry frame=3 attempt=2
frame.crop.accepted width=3984 height=2656
frame.processing.dmax value=2.56
frame.processing.wb r=1.24 g=1.17 b=1.00
xmp.written path=...
```

The CLI can still print friendly progress separately.

---

# 25. Configuration and persistence

Avoid scattering application preferences through command-line defaults.

Introduce a config layer later in the project:

```text
scanner defaults
output folder
quality preset
Darktable path
default paper grade
default paper gloss
recent roll profiles
```

Roll profiles should be separate persisted entities, not hidden application preferences.

---

# 26. What should remain manual

The following should remain human-controlled until evidence justifies automation:

## Neutral highlight selection

The mathematics is deterministic.

The **choice of what is actually neutral in a scene is not**.

Keep this manual.

## Paper grade

4.0 and 4.5 both proved useful.

This is partly an interpretation/print choice.

Provide a default and a slider, not a hard-coded auto rule.

## Paper gloss

75% currently works well.

Treat it as a default.

Do not infer a different value automatically until there is a clear photographic reason.

## Final creative editing

Leave:

- local contrast;
- dodging/burning;
- colour grading;
- crop refinement;
- selective adjustments;
- retouching;

to Darktable.

---

# 27. What is safe to automate

Based on the experiments so far:

## Roll-wide

- scanner input profile;
- D-min.

## Frame-specific before human WB

- crop;
- D max;
- scan exposure bias.

## Frame-specific after human WB

- paper black;
- automatic print exposure.

## Convenience

- orientation propagation across strip;
- output naming;
- Darktable XMP generation.

---

# 28. Recommended PR / commit sequence

Avoid one enormous implementation branch.

A good sequence is:

### PR 1 — Pipeline extraction

No new behaviour.

```text
main.rs → scanner pipeline API
```

### PR 2 — Output separation

Scanner returns frames; output layer writes them.

### PR 3 — Roll profile model

Persistence + tests.

### PR 4 — Colour-management spike

ICC transform + parity fixtures.

### PR 5 — Negadoctor core

Pure Rust formulas + Darktable parity tests.

### PR 6 — Automatic technical analysis

D max + scan bias.

### PR 7 — Orientation + selection geometry

Pure model + tests.

### PR 8 — Review UI prototype

Load existing frames; no scanner.

### PR 9 — Post-WB auto calculations

paper black + print exposure.

### PR 10 — XMP research fixtures

Captured Darktable sidecars + documented schema.

### PR 11 — XMP writer

Round-trip verified.

### PR 12 — Live scanner integration

Connect review UI to scanner pipeline.

### PR 13 — Full GUI scan controls

CLI parity.

### PR 14 — Packaging

Windows release path.

---

# 29. Suggested first coding milestone

The best immediate milestone is:

> **Refactor the current scanner code so one successful scan returns a `FrameArtifact` without forcing file output.**

Nothing about image appearance should change.

A minimal success case:

```rust
let strip = scan_strip(&scanner, &request, progress)?;

for frame in strip.frames {
    println!(
        "frame {}: {}x{}",
        frame.frame_number,
        frame.pass.cols,
        frame.pass.rows
    );
}
```

Then the CLI output layer can explicitly call:

```rust
save_preview(&frame)?;
save_tiff(&frame)?;
```

Once this exists, nearly every later phase becomes easier.

---

# 30. Definition of done for the overall project stage

This implementation programme is complete when the following workflow succeeds:

1. Launch Coolscan Studio.
2. Scanner is detected.
3. Insert a negative strip.
4. App discovers all frames.
5. Select roll profile.
6. Scan strip.
7. Each frame is safely auto-cropped.
8. LS-40 colour transform is applied for preview/analysis.
9. Roll D-min is applied.
10. D max and scanner bias are calculated automatically.
11. Frame appears as a positive preview.
12. User corrects orientation.
13. User selects one neutral highlight.
14. Highlight WB is calculated.
15. Paper black and print exposure are recalculated automatically.
16. User reviews and accepts.
17. Master TIFF is preserved.
18. Darktable XMP is written.
19. Opening the frame in Darktable reproduces the Coolscan Studio result.
20. The user begins creative editing in Darktable rather than repeating technical setup.

---

# 31. Main risks

## Colour pipeline mismatch

**Risk:** Coolscan Studio's RGB values differ subtly from Darktable before negadoctor.

**Mitigation:** Do not proceed to XMP generation until ICC/working-space parity tests pass.

## XMP compatibility

**Risk:** Darktable sidecar history encoding changes between versions.

**Mitigation:** Version the XMP writer against tested Darktable versions and keep fixture sidecars.

## GUI/scanner coupling

**Risk:** USB or long-running scan work freezes the UI.

**Mitigation:** Scanner pipeline emits events and runs off the UI thread from the beginning.

## Refactor regression

**Risk:** Moving scanner code breaks a hardware path that currently works.

**Mitigation:** Phase 1 changes structure only, followed immediately by real LS-40 smoke testing.

## Huge test fixtures

**Risk:** Repository becomes filled with large TIFFs.

**Mitigation:** Use tiny pixel fixtures and synthetic images for CI; retain full-size captures externally or under investigation assets only when justified.

## Over-automation

**Risk:** Technical automation becomes aesthetic automation and removes film/scene character.

**Mitigation:** Keep neutral selection, paper grade, and creative decisions human-controlled.

---

# 32. Recommended next action

Start **Phase 0 and Phase 1 only**.

Do not add GUI dependencies yet.

The next code change should establish this boundary:

```text
current:
main.rs
  → scan
  → crop
  → write files

target:
CLI
  → ScanPipeline
       → Vec<FrameArtifact>
  → OutputPolicy
       → files
```

Once that refactor is hardware-verified, the processing subsystem can be built safely beside it without destabilising the scanner work that is already successful.
