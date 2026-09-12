# Automatic framing investigation — 5 September 2026

Goal: unattended strip scanning without manual crop offsets. New test strip B contains six exposed frames. Production detection is still the pinned nkscan implementation; all detector changes below are experiments.

## Findings supported by experiments

1. **Regular-spacing fitting can move correctly found boundaries.** The upstream detector first tiles the image, then fits a `Wind`. When at least three and two thirds of its starts agree, `Wind::ladder` replaces individual starts with an equally spaced sequence. Agreement tolerance is about one tenth of the pitch, much larger than a crop pixel.
2. **This happens on the new real strip.** In capture `strip-b-01`, the tiled starts are `[5,150,292,437,581,722]`; the final starts are `[5,148,292,435,579,722]`. Frames 2, 4 and 5 move two columns earlier. Each column is 30 optical dots, or 0.26276 mm at 2900 DPI; two columns are 0.5255 mm.
3. **Independent image evidence supports uneven spacing.** In repeat capture `strip-b-02`, strong downward transitions in log RGB transmission occur at columns `[5,150,292,436,581,723]`. Their spacings are `[145,142,144,145,142]` columns (approximately `[38.10,37.31,37.84,38.10,37.31]` mm). These are approximate sampled transitions, not subpixel ground truth. They show that forcing equal pitch is inappropriate here. The local tiling itself can still miss an edge by a column.
4. **Increasing resolution alone does not solve this failure.** The controlled synthetic experiment below changes only frame spacing. The even strip is exact at both sampling scales. On the uneven strip, the pinned detector displaces two known starts by three coarse pixels. At four times the resolution, the errors remain 2.5–3 coarse pixels in physical units. This isolates an algorithmic error from low-resolution quantization.
5. **Preserving local starts fixes that controlled test.** The experimental variant uses `wind.fill(&starts)` instead of `wind.ladder(...)`, retaining measured positions. All synthetic starts are exact at both scales. It is NOT a complete replacement: leading/trailing unexposed-frame inference, weak edges, variable frame lengths and overlap behavior require further tests.

## What remains unproved

- How much actual capture error is caused by thumbnail resolution. Discovery is 96 DPI with 30-dot pitch; previews are 725 DPI with 4-dot pitch. We have not captured an independently registered, higher-resolution whole-strip image, so the real resolution hypothesis is not yet isolated.
- Exact trailing edges for all six frames. Frame 1's trailing transition is especially weak in the thumbnail (about 0.016 log-transmission change near column 141, versus 0.18–0.51 at several other frame ends). A naive strongest-edge rule could confuse image content and film base.
- Thumbnail-to-preview registration. Original frame 2's preview shows about 34 pixels of left border, whereas the two-column thumbnail error predicts about 15 preview pixels. These are visual estimates; residual transport/registration and sampling effects need measurement. Corrected Type2 registration has been prepared but not yet run.
- Reliable complete-strip hardware operation: frame 4 in the original-rectangle preview batch took several minutes without completion, and that batch was stopped. This is separate from the deterministic offline spacing finding.

## Saved evidence

- `strip-b-01`: discovery plus three previews from an **invalid wider-window experiment**. Frame 2 is smeared; do not use its geometry as ground truth. Capture stopped, scanner abort succeeded and film presence was confirmed afterward.
- `strip-b-02`: repeat discovery and original nkscan rectangle previews for frames 1–3, visually normal. Frame 4 did not finish before the batch was stopped. Existing earlier strip-A files remain untouched.
- Each capture includes original planar little-endian 16-bit sample storage and text describing layout, dimensions and scanner capabilities. Discovery samples have 12 valid bits; preview samples are expanded to 16-bit full scale by nkscan. BMPs are display copies.
- `*-profile.csv` and `profiles.svg` are diagnostic transmission profiles. Central-band brightness alone is NOT a crop detector: a bright, flat image area can resemble film base.

## Reproduce offline

```powershell
cargo run --example replay_boundaries -- investigation/strip-b-01/discovery.raw 95 1155 12 137
cargo run --example spacing_experiment -- --assert-edges
cargo run --example spacing_experiment -- --preserve-edges --assert-edges
```

The second command intentionally fails on the pinned upstream detector, with a known-edge error. The third passes using the experimental local-edge variant. These commands do not open the scanner.

`examples/support/boundaries_probe.rs` is an instrumented experimental copy of nkscan `src/scan/boundaries.rs` at revision `4f276883e3971aac92fa58a3d8284994105f80d8`, with the upstream MIT license alongside it. The replay asserts that its unmodified decision path matches the actual dependency's starts and length.

## Proposed next increment

1. Confirm scanner recovery after the interrupted frame-4 run.
2. Run only frame 2 with the experimental local-start table. Update both the Type2 perforation registration and the returned rectangle; changing only a rectangle's top is not sufficient evidence of a physical move.
3. Compare left-border width with the original frame-2 preview. Keep the same scan recipe and output dimensions.
4. Separate safe acquisition bounds from final image crop. Capture a verified margin, then refine all four crop edges from a higher-resolution image, retaining measured local starts and using regular pitch only as a fallback for missing evidence.
5. Test weak/blank frames and different strips before claiming unattended cropping is solved. Confidence and a conservative fallback are necessary when film and picture are indistinguishable.

No upstream checkout or production detector has been modified. No commit has been created.

## Recovery and validation status

After stopping the second batch, normal `Session::open` did not return promptly. Source inspection showed that opening a session includes staging film. That recovery process was stopped. A direct `Abort` command then returned `Good`, but subsequent `TestUnitReady` checks continued to return `02h-04h-01h` (becoming ready). Further hardware experiments were suspended; the user should check the scanner before scanning resumes. Do not equate acceptance of abort with confirmed readiness.

The capture harness now permits discovery alone, `--frame2`, or the untested `--local-frame2` experiment; the six-preview batch option has been removed pending investigation of the frame-4 stall. `scanner_status --ready-only` queries readiness without the staging preamble; without that flag it first sends abort.

After restart, the scanner recovered to `Good`. A baseline frame-2 scan and a local-edge/perforation-registration frame-2 scan both completed with the same 725-DPI geometry. The measured left border changed from 40 pixels (1.40 mm) to 35 pixels (1.23 mm), while the scene framing remained visually the same. This is evidence that local registration changes the crop by about 0.17 mm, but it does not yet prove which border is the true image edge; both scans still include deliberate overscan.

Ten application unit tests passed. Formatting and Clippy across all targets passed. The synthetic upstream failure is intentional and recorded separately from the passing application suite. Replay and synthetic experiment logs are saved next to this report.

The first high-resolution edge-refinement prototype (`refine_edges.py`) found a strong leading edge in both frame-2 scans (columns 39 and 34), but no trustworthy trailing edge: the strongest candidate was weak and content-dependent. Its conservative rule rejected both refinements and retained the full registered rectangle. This supports separating acquisition overscan from final crop and requiring confidence on both edges before trimming.

The combined-scale prototype (`combined_edges.py`) confirms the practical shape of the solution: the preview provides a confident leading edge, while its trailing edge has no overscan and must be constrained by discovery geometry. For frame 2 the discovery transition is near column 286, but the preview's registered rectangle ends at its image boundary. The output therefore labels the trailing estimate `discovery-only` instead of pretending it has high-resolution confidence. This is a safe acquisition/crop boundary, not yet a finished cropper.

The 5% per-column classifier prototype (`film_fraction.py`) exposed a measurement issue: the leading pixels of these previews are bare gate/holder, not confirmed unexposed film. A reference learned from them is invalid and causes bright scene regions to be misclassified. A production classifier must receive its unexposed-film reference from confirmed low-resolution gap columns (or acquire explicit film overscan), then use the 5% rule only inside a discovery-bounded edge search.

The first discovery-reference extraction (`film_reference.py`) shows that unexposed gaps do not share one absolute brightness: across gap samples, channel spread is roughly 1.5–2.1 times the median because the narrow gaps include transition columns and illumination variation. Their orange-mask channel ratios are more stable. The classifier should therefore learn a per-gap chromaticity envelope and local intensity range, rather than subtract one global RGB value.

The synthetic per-gap classifier (`chromaticity_classifier.py`) passes brightness changes from 0.5× to 2×, moderate mask variation, scene colours, and dust contamination. Applying the first version to real discovery/preview data exposed two calibration requirements: raw intensities cannot transfer between the low-resolution discovery and high-resolution preview exposures, and transition columns must be excluded when learning the chromaticity envelope. The next implementation should normalize preview intensity locally and select gap cores by low within-column variation before fitting the model.

The gap-core filter and exposure-invariant chromaticity model are now implemented. Synthetic checks pass. The first physical overscan scan completed successfully, but 60 dots was insufficient to reach frame 2's actual inter-frame gap; the image remained content-filled at the trailing edge. The harness now uses 60 dots before and 210 dots after (270 dots total, within the 4332-dot adapter boundary) for `--overscan-frame2`. This should include the confirmed discovery gap and is the next hardware measurement.

Production code changes in this investigation are limited to extracting the existing BMP writer into `src/bmp.rs` for reuse. The experimental detector remains outside the application.
