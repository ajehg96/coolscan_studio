"""Test crop decisions on all six frames across saved discovery captures.

Validates Step 6 of investigation/PLAN.md:
"Test the decision on all six saved discovery positions, without scanning them yet."
"""
import json
import re
import sys
from pathlib import Path
import numpy as np

sys.path.insert(0, str(Path(__file__).parent))
from chromaticity_classifier import FilmModel, edge_crossing
from film_reference import extract


def evaluate_discovery_folder(folder_path: str):
    root = Path(folder_path)
    if not (root / 'discovery.raw').exists() or not (root / 'discovery.txt').exists():
        return None

    # Extract film reference from inter-frame gaps
    ref = extract(str(root))
    meta = (root / 'discovery.txt').read_text()

    rows, cols, channels = map(
        int, re.search(r'rows=(\d+) cols=(\d+) channels=(\d+)', meta).groups()
    )
    raw = np.fromfile(root / 'discovery.raw', dtype='<u2').reshape(channels, rows, cols).astype(float)
    pitch = int(re.search(r'line_pitch: (\d+)', meta).group(1))

    # Parse detected frame rectangles (in scanner dots)
    rects = re.findall(
        r'top:\s*(\d+),\s*left:\s*(\d+),\s*bottom:\s*(\d+),\s*right:\s*(\d+)', meta
    )
    if len(rects) < 6:
        return None

    # Train chromaticity model on confirmed gap pixels
    gap_cols = ref['gap_columns']
    gap_pixels = raw[:, rows // 8 : rows - rows // 8, gap_cols].reshape(3, -1).T
    model = FilmModel(gap_pixels)

    # Compute film fraction across all columns in discovery strip
    band = raw[:, rows // 8 : rows - rows // 8]
    band_cols = band.transpose(2, 1, 0)  # (cols, rows_band, 3)
    fractions = np.array([model.classify(col).mean() for col in band_cols])

    # 3-tap median smoothing
    padded = np.pad(fractions, 1, mode='edge')
    smoothed = np.median([padded[:-2], padded[1:-1], padded[2:]], axis=0)

    top0 = int(rects[0][0])
    frame_results = []

    for idx, r in enumerate(rects[:6], 1):
        top, left_dot, bottom, right_dot = map(int, r)
        # Convert dot addresses to discovery column indices relative to top0
        start_col = max(0, int(round((top - top0) / pitch)))
        end_col = min(cols, int(round((bottom - top0) / pitch)))

        # Frame search window: includes margins before start and after end
        margin = 12  # in coarse discovery columns (~3.6mm)
        search_start = max(0, start_col - margin)
        search_end = min(cols, end_col + margin)

        frame_fractions = smoothed[search_start:search_end]
        frame_len = len(frame_fractions)

        # Leading edge search in window
        expected_leading_rel = start_col - search_start
        leading_win = frame_fractions[:min(frame_len, expected_leading_rel + 8)]
        leading_cross = None
        for i in range(len(leading_win) - 4):
            if (leading_win[i:i + 4] < 0.05).all():
                leading_cross = search_start + i
                break

        # Trailing edge search in window
        expected_trailing_rel = end_col - search_start
        trailing_win_start = max(0, expected_trailing_rel - 8)
        trailing_cross = None
        for i in range(frame_len - 1, trailing_win_start + 3, -1):
            if (frame_fractions[i - 4:i] < 0.05).all():
                # Check that there is gap evidence after this point
                remaining = frame_fractions[i:]
                if len(remaining) > 0 and (remaining > 0.05).any():
                    trailing_cross = search_start + i
                    break

        leading_confident = leading_cross is not None
        trailing_confident = trailing_cross is not None
        accepted = leading_confident and trailing_confident and trailing_cross > leading_cross

        chosen_cols = (
            (leading_cross, trailing_cross)
            if accepted
            else (start_col, end_col)
        )

        frame_results.append({
            'frame': idx,
            'detected_cols': [start_col, end_col],
            'leading_cross': leading_cross,
            'trailing_cross': trailing_cross,
            'leading_confident': leading_confident,
            'trailing_confident': trailing_confident,
            'accepted': accepted,
            'chosen_cols': list(chosen_cols),
            'length_cols': chosen_cols[1] - chosen_cols[0],
            'length_mm': round((chosen_cols[1] - chosen_cols[0]) * pitch * 25.4 / 2900, 2),
        })

    return {
        'folder': folder_path,
        'model_center': model.center.tolist(),
        'model_radius': float(model.radius),
        'frames': frame_results,
    }


def main():
    folders = [
        'investigation/strip-b-01',
        'investigation/strip-b-02',
        'investigation/strip-b-03-baseline',
        'investigation/strip-b-04-local',
        'investigation/strip-b-06-overscan',
        'investigation/strip-b-07-overscan',
    ]

    all_results = {}
    for f in folders:
        res = evaluate_discovery_folder(f)
        if res is not None:
            all_results[f] = res
            print(f"\n=== Results for {f} ===")
            for fr in res['frames']:
                status = "CONFIDENT" if fr['accepted'] else "CONSERVATIVE FALLBACK"
                lead_str = f"col {fr['leading_cross']}" if fr['leading_confident'] else "ambiguous"
                trail_str = f"col {fr['trailing_cross']}" if fr['trailing_confident'] else "ambiguous"
                print(
                    f"  Frame {fr['frame']}: {status} | Leading: {lead_str} | Trailing: {trail_str} "
                    f"| Chosen: {fr['chosen_cols']} ({fr['length_mm']} mm)"
                )

    Path('investigation/discovery_crop_decisions.json').write_text(
        json.dumps(all_results, indent=2) + '\n'
    )
    print("\nSaved full evaluation to investigation/discovery_crop_decisions.json")


if __name__ == '__main__':
    main()
