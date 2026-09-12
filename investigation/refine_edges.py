"""Prototype automatic travel-edge refinement for one high-resolution preview.

The scan rectangle deliberately includes overscan. We use robust cross-row
gradients, require a strong edge relative to local noise, and otherwise keep
the registered rectangle. Coordinates are preview columns and converted to
scanner dots using the recorded scan DPI.
"""
from pathlib import Path
import json
import re
import sys
import numpy as np

def refine(folder: str) -> dict:
    root = Path(folder)
    meta = (root / 'frame-2.txt').read_text()
    rect = tuple(map(int, re.search(r'rect=Rect \{ top: (\d+), left: (\d+), bottom: (\d+), right: (\d+) \}', meta).groups()))
    dpi = int(re.search(r'dpi: (\d+)', meta).group(1))
    rows, cols, channels = map(int, re.search(r'rows=(\d+) cols=(\d+) channels=(\d+)', meta).groups())
    data = np.fromfile(root/'frame-2.raw', dtype='<u2').reshape(channels, rows, cols).astype(float)
    band = data[:, rows//8:rows-rows//8].mean(axis=1)
    # Log transmission is more stable than raw brightness across colour planes.
    profile = np.log1p(np.maximum(band, 1)).mean(axis=0)
    gradient = np.diff(profile)
    # Search only within overscan near each registered edge.
    search = max(8, int(round(cols * 0.12)))
    left_window = gradient[:search]
    right_window = gradient[-search:]
    left = int(np.argmin(left_window)) + 1
    right = cols - search + int(np.argmax(right_window))
    noise = float(np.median(np.abs(gradient - np.median(gradient)))) * 1.4826
    left_strength = float(abs(gradient[left-1]))
    right_strength = float(abs(gradient[right-1]))
    threshold = max(noise * 6, 0.03)
    accepted = left_strength >= threshold and right_strength >= threshold and right > left
    chosen = (left, right) if accepted else (0, cols)
    dots_per_column = 2900 / dpi
    result = {
        'folder': folder, 'preview_shape': [rows, cols], 'registered_rect': rect,
        'candidate_columns': [left, right], 'candidate_strength': [left_strength, right_strength],
        'noise': noise, 'threshold': threshold, 'accepted': accepted,
        'chosen_columns': list(chosen),
        'chosen_travel_dots': [round(v * dots_per_column) for v in chosen],
        'chosen_travel_mm': [round(v * 25.4 / dpi, 3) for v in chosen],
    }
    (root/'refined-edges.json').write_text(json.dumps(result, indent=2) + '\n')
    return result

if __name__ == '__main__':
    print(json.dumps([refine(folder) for folder in sys.argv[1:]], indent=2))
