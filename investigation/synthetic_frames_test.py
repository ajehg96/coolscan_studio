"""Synthetic test suite for edge detection and film classification under challenging conditions.

Tests Step 4 of investigation/PLAN.md:
- Uneven spacing
- Brightness gradients (lamp falloff / vignetting)
- Mask variation (shifts in orange mask chromaticity)
- Scene colours resembling film (warm sunsets, orange objects)
- Gradual edges (unsharp or transition zones)
- Dust and scratches
- Blank frames
"""
import numpy as np
import pytest
from chromaticity_classifier import FilmModel, edge_crossing


def make_film_gap(rng, base_rgb, n_pixels, spread=0.03, brightness_range=(0.8, 1.2)):
    """Generate unexposed film base pixels with natural noise and brightness variation."""
    b = rng.uniform(brightness_range[0], brightness_range[1], (n_pixels, 1))
    noise = rng.normal(1.0, spread, (n_pixels, 3))
    return base_rgb * b * noise


def test_film_model_with_brightness_gradients():
    """Illumination falloff / lamp gradients across columns should not break classification."""
    rng = np.random.default_rng(101)
    base_rgb = np.array([550.0, 260.0, 140.0])

    # Reference learned from a flat gap
    ref_gap = make_film_gap(rng, base_rgb, 2000)
    model = FilmModel(ref_gap)

    # Gap columns subjected to a 3x gradient from edge to center
    cols = []
    for factor in np.linspace(0.4, 1.8, 15):
        col = make_film_gap(rng, base_rgb * factor, 100)
        cols.append(col)

    # All gap columns under gradient should still classify as film
    for i, col in enumerate(cols):
        fraction = model.classify(col).mean()
        assert fraction >= 0.90, f"Column {i} with factor {factor} had low film fraction: {fraction}"


def test_film_model_with_mask_variation():
    """Orange mask variation along the strip (up to +/- 5% chromaticity shift)."""
    rng = np.random.default_rng(102)
    base_rgb = np.array([550.0, 260.0, 140.0])

    ref_gap = make_film_gap(rng, base_rgb, 2000)
    model = FilmModel(ref_gap)

    # Slight shift in blue and red ratios due to emulsion / processing variation
    shifted_base = base_rgb * np.array([1.03, 0.98, 0.97])
    shifted_gap = make_film_gap(rng, shifted_base, 500)
    assert model.classify(shifted_gap).mean() >= 0.85


def test_scene_colors_resembling_film():
    """Scene objects with warm orange tones (e.g. sunset/sand) vs true unexposed mask."""
    rng = np.random.default_rng(103)
    base_rgb = np.array([550.0, 260.0, 140.0])
    model = FilmModel(make_film_gap(rng, base_rgb, 2000))

    # A warm scene element (e.g. golden hour light, autumn foliage)
    # Warm scene is yellow-red, but typically has more green/blue than film base
    warm_scene_col = np.array([500.0, 320.0, 190.0]) * rng.uniform(0.7, 1.1, (100, 1))
    assert model.classify(warm_scene_col).mean() < 0.05

    # A scene column containing an orange object covering 40% of the height,
    # with the rest being other scene content
    mixed_col = np.empty((100, 3))
    mixed_col[:40] = make_film_gap(rng, base_rgb, 40)  # orange object
    mixed_col[40:] = np.array([150.0, 200.0, 250.0]) * rng.uniform(0.8, 1.2, (60, 1))  # sky
    # The column as a whole has 40% film-like pixels, but inside-image rule (< 5%) rejects it
    # and bounded search prevents considering it outside the overscan zone.
    fraction = model.classify(mixed_col).mean()
    assert fraction < 0.50, f"Mixed scene column film fraction was too high: {fraction}"


def test_gradual_edge_transition():
    """Gradual transition over 4-6 columns between gap and image."""
    rng = np.random.default_rng(104)
    base_rgb = np.array([550.0, 260.0, 140.0])
    scene_rgb = np.array([200.0, 220.0, 240.0])
    model = FilmModel(make_film_gap(rng, base_rgb, 2000))

    columns = []
    # 10 pure gap columns
    for _ in range(10):
        columns.append(make_film_gap(rng, base_rgb, 100))
    # 5 transition columns (alpha blends from 1.0 down to 0.0)
    for alpha in np.linspace(0.8, 0.1, 5):
        blend = alpha * base_rgb + (1 - alpha) * scene_rgb
        columns.append(blend * rng.uniform(0.9, 1.1, (100, 1)))
    # 20 pure scene columns
    for _ in range(20):
        columns.append(scene_rgb * rng.uniform(0.8, 1.2, (100, 1)))

    cross, fractions = edge_crossing(columns, model, threshold=0.05, run=6)
    # Edge crossing should safely preserve transition content
    assert cross is not None
    assert 10 <= cross <= 17, f"Edge crossing at unexpected column: {cross}"


def test_dust_and_scratches_suppression():
    """Dust in the gap and scratches in the frame should not trigger false edges."""
    rng = np.random.default_rng(105)
    base_rgb = np.array([550.0, 260.0, 140.0])
    scene_rgb = np.array([180.0, 200.0, 220.0])
    model = FilmModel(make_film_gap(rng, base_rgb, 2000))

    columns = []
    # 15 gap columns with dust specks in columns 3 and 7 (dark pixels in gap)
    for col_idx in range(15):
        col = make_film_gap(rng, base_rgb, 100)
        if col_idx in (3, 7):
            col[:15] = [50.0, 50.0, 50.0]  # dust speck covering 15% of column
        columns.append(col)

    # 20 scene columns with a bright scratch in column 22
    for col_idx in range(15, 35):
        col = scene_rgb * rng.uniform(0.8, 1.2, (100, 1))
        if col_idx == 22:
            col[:20] = base_rgb  # bright scratch with film-like color
        columns.append(col)

    cross, fractions = edge_crossing(columns, model, threshold=0.05, run=8)
    assert cross == 15, f"Expected clean edge at column 15, got {cross}"


def test_blank_frame_detection():
    """A completely blank / unexposed frame should be detected and not cropped."""
    rng = np.random.default_rng(106)
    base_rgb = np.array([550.0, 260.0, 140.0])
    model = FilmModel(make_film_gap(rng, base_rgb, 2000))

    # All columns are film base
    columns = [make_film_gap(rng, base_rgb, 100) for _ in range(50)]
    cross, fractions = edge_crossing(columns, model, threshold=0.05, run=8)
    # For a blank frame, fraction never drops below 5%
    assert cross is None, "Blank frame should have no inside-image edge crossing"
    assert (fractions > 0.85).all()


def test_uneven_spacing_bounds():
    """Variable frame spacing does not prevent gap detection when local evidence is used."""
    rng = np.random.default_rng(107)
    base_rgb = np.array([550.0, 260.0, 140.0])
    model = FilmModel(make_film_gap(rng, base_rgb, 2000))

    # Test narrow gap (6 columns) and wide gap (18 columns)
    for gap_len in [6, 12, 18]:
        gap_cols = [make_film_gap(rng, base_rgb, 100) for _ in range(gap_len)]
        fractions = [model.classify(col).mean() for col in gap_cols]
        assert all(f > 0.85 for f in fractions)
