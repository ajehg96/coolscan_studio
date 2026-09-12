"""Estimate the fraction of unexposed-film-like pixels per preview column.

This is an analysis prototype. It does not alter scanner positions or crop files.
The reference is learned from the preview's confirmed leading overscan; colour
ratios are used so exposure level is less important than the orange mask.
"""
from pathlib import Path
import json, re, sys
import numpy as np

def run(folder: str):
    root = Path(folder)
    meta = (root/'frame-2.txt').read_text()
    rows, cols, channels = map(int, re.search(r'rows=(\d+) cols=(\d+) channels=(\d+)', meta).groups())
    data = np.fromfile(root/'frame-2.raw', dtype='<u2').reshape(channels, rows, cols).astype(float)
    band = data[:, rows//8:rows-rows//8]
    ref_file = root / 'film-reference.json'
    if ref_file.exists():
        ref_data = json.loads(ref_file.read_text())
        reference = np.array(ref_data['reference_rgb'])
        ref_source = 'discovery-gap-cores'
        chroma_center = np.array(ref_data['chroma_center'])
    else:
        reference = np.median(band[:, :, :24], axis=(1, 2))
        ref_source = 'preview-leading-overscan (fallback)'
        chroma_center = reference / max(reference.mean(), 1)

    pixels = band.transpose(1, 2, 0)
    means = np.maximum(pixels.mean(axis=2, keepdims=True), 1)
    colour_distance = np.max(np.abs(pixels / means - chroma_center), axis=2)
    # A pixel is in the gap/overscan if it matches the film mask chromaticity
    # or is in the high-transmission overscan/gate regime (> 40000 out of 65535).
    film_like = (colour_distance <= 0.08) | (pixels.mean(axis=2) >= 40000)
    fraction = film_like.mean(axis=0)
    # 5% threshold with a short run requirement suppresses dust and scratches.
    run_length = 8
    low = fraction < 0.05
    high = fraction > 0.50
    def first_run(values):
        for i in range(len(values)-run_length+1):
            if values[i:i+run_length].all(): return i
        return None
    # Discovery predicts frame 2 starts near 35 preview columns; do not let a
    # film-coloured region inside the scene win the global search.
    left_candidates = low[:max(24, min(cols, 120))]
    left = first_run(left_candidates)
    right_reversed = first_run(low[::-1])
    right = None if right_reversed is None else cols - right_reversed
    result = {
        'folder': folder, 'reference_rgb': reference.tolist(),
        'reference_source': ref_source,
        'film_like_fraction_range': [float(fraction.min()), float(fraction.max())],
        'threshold': 0.05, 'run_length_columns': run_length,
        'first_inside_image_column': left, 'last_inside_image_column': right,
        'left_border_mm': None if left is None else round(left*25.4/725, 3),
        'right_border_mm': None if right is None else round((cols-right)*25.4/725, 3),
        'columns_below_5_percent': int(low.sum()),
    }
    np.savetxt(root/'film-fraction.csv', np.column_stack([np.arange(cols), fraction]), delimiter=',', header='column,film_like_fraction', comments='')
    (root/'film-fraction.json').write_text(json.dumps(result, indent=2)+'\n')
    return result

if __name__ == '__main__':
    print(json.dumps([run(x) for x in sys.argv[1:]], indent=2))
