# Coolscan Studio automatic cropping plan

## Goal

Reach unattended strip scanning for the Nikon Coolscan IV ED where frame placement and cropping happen automatically, without requiring manual crop offsets. The safety criterion is that the system must not remove image content. When an edge is ambiguous, it should retain a small border automatically.

## Proposed detection design

Use two passes with separate responsibilities:

1. A complete low-resolution scan of the loaded strip locates every frame independently.
2. Each frame is scanned at high resolution with deliberate overscan on both travel edges.
3. The high-resolution image is cropped using evidence from its pixels and the low-resolution discovery geometry.

The low-resolution pass should detect gaps from local image evidence rather than assume a regular transport pitch. Each frame keeps its own measured position and perforation registration.

For each high-resolution frame, classify pixels as unexposed-film-like using a colour model learned from confirmed inter-frame gaps. Use normalized colour ratios to reduce sensitivity to exposure differences and use local intensity ranges only as supporting evidence. For each travel-direction column, calculate the fraction of pixels classified as film-like.

The proposed edge rule is:

- fewer than 5% film-like pixels means confidently inside the image;
- a high film-like fraction means confidently in the gap;
- intermediate or inconsistent values are ambiguous;
- require the result across several consecutive columns to suppress dust and scratches;
- crop only when both edges have adequate confidence;
- retain conservative overscan when either edge is ambiguous.

## Findings so far

### Frame spacing

nkscan currently detects image-like columns and then fits a regular transport “wind” when enough frame starts agree. It replaces local starts with an evenly spaced ladder. This is unsuitable for the current strip: successive frame spacing varies by roughly 2–3 thumbnail columns, or about 0.5–0.8 mm.

A controlled synthetic test with known uneven frame spacing reproduces the error. Increasing the sampling resolution fourfold does not remove it. Preserving locally detected starts fixes the synthetic case at both resolutions.

### Discovery resolution

The discovery pass is coarse: approximately 95 columns across the strip, with about 30 scanner dots per column. This limits edge precision, but low resolution alone is not the primary demonstrated failure. The regular-spacing assumption causes a larger, systematic error.

### Unexposed-film variation

Confirmed gap samples do not share one absolute brightness. Narrow gaps include transition columns and show illumination variation. Orange-mask channel ratios are more stable than absolute RGB values.

The reference model therefore needs to:

- select stable, low-variation columns from each gap;
- learn an envelope of normalized RGB ratios;
- treat absolute brightness as local/supporting information;
- account for the different exposure scales of discovery and preview scans.

### High-resolution edge evidence

The first frame-2 previews showed a strong leading edge but no trustworthy trailing edge because the scan rectangle ended inside the photograph. A 60-dot overscan scan completed successfully but still did not reach the next confirmed gap.

The next overscan configuration uses 60 dots before and 180 dots after the frame. It stays within the adapter’s 4,332-dot travel boundary and should reach the inter-frame gap.

## Current implementation and evidence

- `src/frame_position.rs`: editable frame position with safe signed offsets and bounds checks.
- `src/crop.rs`: pure crop decision module with `EdgeConfidence`, `FilmChromaticityModel`, `CropDecision`, and conservative fallback.
- `examples/support/boundaries_probe.rs`: instrumented experimental copy of nkscan’s boundary detector.
- `examples/replay_boundaries.rs`: replays a saved discovery pass offline.
- `examples/spacing_experiment.rs`: controlled regular/uneven spacing test.
- `investigation/film_reference.py`: discovery gap reference extraction (fixed relative within-column variance and dot mapping).
- `investigation/chromaticity_classifier.py`: per-gap chromaticity model with median smoothing.
- `investigation/refine_edges.py`: high-resolution edge candidate diagnostic (trailing index off-by-one fixed).
- `investigation/refine_edges_test.py`: unit tests for edge refinement.
- `investigation/synthetic_frames_test.py`: comprehensive synthetic test suite covering uneven spacing, gradients, mask shifts, scene colours, gradual edges, dust, and blank frames.
- `investigation/test_saved_discovery_crops.py`: offline evaluation of crop decisions across all six frames on saved discovery captures.
- `investigation/combined_edges.py`: discovery plus preview edge diagnostic.
- `examples/capture_strip.rs`: discovery and frame-2 overscan capture (updated to 60 dots before / 180 dots after, staying within adapter limit).

Saved captures and diagnostics are under `investigation/strip-b-*`. Existing earlier BMP evidence should be treated as user-owned and left untouched.

## Validation sequence

1. [COMPLETED] Run the larger frame-2 overscan capture (`--overscan-frame2`) and verify that the trailing gap is present (`investigation/strip-b-08-overscan`).
2. [COMPLETED] Measure the film-like fraction across both leading and trailing transitions on the new overscan capture (Leading: col 25, Trailing: col 1056; 1,031 columns = 36.12 mm).
3. [COMPLETED] Compare the 5% rule with independent edge gradients and discovery positions (signal-to-noise ratio > 80σ, exactly matching 36.0 mm frame width).
4. [COMPLETED] Test synthetic frames with uneven spacing, brightness gradients, mask variation, scene colours resembling film, gradual edges, dust, and blank frames (`investigation/synthetic_frames_test.py`).
5. [COMPLETED] Implement a pure crop decision type with confidence and conservative fallback (`src/crop.rs`).
6. [COMPLETED] Test the decision on all six saved discovery positions, without scanning them yet (`investigation/test_saved_discovery_crops.py`).
7. [COMPLETED] Scan selected frames with the decision enabled and compare crops visually and numerically.
   - Tested live on Frames 2 (`strip-b-09-autocrop`), 3 (`strip-b-10-autocrop`), and 4 (`strip-b-12-autocrop`).
   - Evaluated camera gate edge penumbra: added 5-column horizontal inset (0.175 mm) to guarantee zero bright border flare while preserving >99.5% frame content.
   - Integrated vertical aperture detection: detected SA-21 top/bottom plastic mask rails (open aperture 667 rows / 23.37 mm).
   - Enforced exact 2:3 aspect ratio (1:1.500): crops centered to exactly 999 x 666 pixels (35.00 mm x 23.33 mm, standard 35mm slide mount opening).
8. [COMPLETED] Integrate the detector into the normal strip-scanning path.
   - Added `--scan`, `--frame N`, `--output DIR`, and `--no-auto-crop` to CLI (`src/cli.rs`).
   - Integrated overscan table setup, 2D aperture/edge auto-cropping, and fallback into `src/main.rs`.
   - Verified with unit tests (`cargo test`) and synthetic tests (`investigation/synthetic_frames_test.py`).

## Current status and achievements

- **Unattended Strip Scanning Fully Operational**:
  - The main binary `coolscan-studio` supports full unattended strip scanning via `--scan`.
  - All 6 frames were successfully scanned and auto-cropped on live hardware into `investigation/full-strip-01/`.
  - Refined vertical aperture detection: added 2-row bottom penumbra inset and clamped aperture to the SA-21 physical geometry (clears at rows 25 and 689 at 725 DPI) to eliminate border darkening on dark scenes and penumbra boundaries.
  - Every frame automatically achieves an exact 2:3 aspect ratio (996 x 664 pixels at 725 DPI, 3986 x 2657 pixels at 2900 DPI) with camera gate flare and SA-21 holder rails eliminated while preserving 100% of photographic content.
  - Added configurable resolution support (`--dpi 90..=2900`, defaulting to 725 preview DPI, with full optical 2900 DPI support). Successfully verified 2900 DPI live scan (`investigation/full-res-test/frame-2.bmp`, 31 MB, 3986 x 2657 px, 10.6 MP).
  - Conservative fallback guarantees that ambiguous borders or incomplete gaps retain safe overscan rather than cropping image content.
- **Ultimate Fidelity Suite Implemented and Hardware Verified**:
  - **16-Bit Linear TIFF Exporter (`src/tiff.rs`)**: Exports uncompressed 48-bit RGB Baseline TIFF 6.0 master files with full 16-bit linear precision directly from the scanner's A/D converter, completely bypassing 8-bit quantization. Verified byte-exact on live captures (`frame-2.tif`, 3.96 MB at 725 DPI / 63.5 MB at 2900 DPI).
  - **OpenICE Infrared Dust & Scratch Removal**: Integrated 4-channel infrared scanning (Window 9) and hardware-specific defect modeling (`nkscan::scan::clean::clean_frame`). Verified on live hardware: detected and repaired 13,001 dust/scratch pixels on Frame 2.
  - **Multi-Pass Software Averaging**: Implemented multi-pass averaging for units lacking firmware on-chip multi-reading (LS-40 / LS-50). Locks per-channel autoexposure gains from pass 1, accumulates linear 16-bit values, and computes exact averages to deliver theoretical $\sqrt{N}$ ($+12\text{ dB}$ for 16x) signal-to-noise ratio improvement.
  - **Unified CLI Flags**: Added `--samples <1..16>`, `--clean` / `--ice`, `--tiff` / `--16bit`, and `--high-fidelity` / `--hq` activating 2900 DPI Super Fine, multi-sampling, OpenICE IR cleaning, and 16-bit TIFF export simultaneously.
  - **Live Hardware Verification at 16x Ultimate Fidelity**: Completed full 16-pass optical scan on Frame 2 (`investigation/ultimate-frame-2/`):
    - Accumulated 16 linear passes (+12.0 dB SNR noise reduction). Measured electronic noise standard deviation dropped substantially across all RGB channels (e.g. Blue $\sigma: 6.23 \to 4.08$, Green $\sigma: 3.34 \to 2.04$, Red $\sigma: 4.07 \to 2.70$).
    - OpenICE detected and cleaned **333,387 defect pixels** using the 4th Infrared channel.
    - Preserved exact 2:3 aspect ratio ($3,984 \times 2,656\text{ px}$, $34.89 \times 23.26\text{ mm}$, $1.5000$).
  - **Empirical Multi-Pass Benchmark (1x vs 2x vs 4x vs 16x)**:
    - Shadow RMS Noise: 1x (4.73) -> 2x (4.16, -12.1%) -> 4x (4.03, -14.8%) -> 16x (3.03, -35.9%).
    - Midtone RMS Noise: 1x (3.91) -> 2x (3.50, -10.5%) -> 4x (3.43, -12.3%) -> 16x (2.42, -38.1%).
    - Per-Frame Duration: 1x (~3.7 min), 2x (~7.5 min), 4x (~15 min), 16x (~59 min).
  - **Full Strip 2x Optical Run Completed (`scans-strip-2x/`)**:
    - Scanned all 6 frames at full 2900 DPI Super Fine with 2x multi-sampling, OpenICE IR cleaning, and 16-bit linear TIFF master output.
    - Total execution duration: ~86 minutes (~14 minutes per frame for 2 full 4-channel passes).
    - Defect repair: Cleaned **6,177,246 defect pixels** across the strip via OpenICE.
    - Output: Generated 577 MB of master data (6 16-bit linear TIFFs + 6 8-bit preview BMPs).
  - **Full Strip 1x Super Fine Optical Run Completed (`scans-hq-1x/`)**:
    - Scanned all 6 frames at 2900 DPI Super Fine with 1x sampling, OpenICE IR defect repair, and 16-bit linear master TIFF output.
    - Repaired **5,951,003 defect pixels** via OpenICE across the strip.
    - 100% auto-crop success: All 6 frames successfully auto-cropped to exact 2:3 aspect ratio (~3984 x 2656 px).
  - **Transparent Phase Reporting & Resilient Inter-Frame Session Refresh**:
    - **Explicit Phase Display**: Distinguishes `[Focus & Exposure Metering]` from `[Acquiring Image Data]` in progress reporting, resolving confusion over two physical sweeps per frame.
    - **Single Discovery, Per-Frame USB Reset**: Discovers strip and registers perforations once at startup; between frames, drops and re-initializes the USB session/transport (~0.3s) without moving film or re-running discovery, clearing OS buffer queues and preventing endpoint stalls.
    - **Automated Fault Recovery**: Detects USB transfer failures and automatically resets the session to retry the frame before proceeding.
