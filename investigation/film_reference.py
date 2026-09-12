"""Extract a robust unexposed-film reference from a discovery strip."""
from pathlib import Path
import json, re, sys
import numpy as np

def extract(folder: str):
    root = Path(folder)
    meta = (root/'discovery.txt').read_text()
    rows, cols, channels = map(int, re.search(r'rows=(\d+) cols=(\d+) channels=(\d+)', meta).groups())
    raw = np.fromfile(root/'discovery.raw', dtype='<u2').reshape(channels, rows, cols).astype(float)
    rects = re.findall(r'top:\s*(\d+),\s*left:\s*(\d+),\s*bottom:\s*(\d+),\s*right:\s*(\d+)', meta)
    if len(rects) < 2: raise ValueError('No frame rectangles in discovery metadata')
    # Metadata rectangles are scanner dots; discovery columns advance by line_pitch.
    pitch = int(re.search(r'line_pitch: (\d+)', meta).group(1))
    gaps = []
    for a, b in zip(rects, rects[1:]):
        end = int(a[2]); start = int(b[0])
        lo = max(0, int(np.ceil(end / pitch)))
        hi = min(cols, int(np.floor(start / pitch)))
        if hi > lo:
            # Keep the two lowest-variation columns from each narrow gap. This
            # excludes image tails and the transition itself.
            region = raw[:, rows//8:rows-rows//8, lo:hi]
            rel_var = (region.std(axis=1) / np.maximum(region.mean(axis=1), 1)).mean(axis=0)
            keep = np.argsort(rel_var)[:max(1, min(2, hi-lo))]
            gaps.extend((lo + int(i) for i in keep))
    if not gaps: raise ValueError('No inter-frame gap columns')
    band = raw[:, rows//8:rows-rows//8, gaps]
    # Median over rows and gap columns; percentiles describe natural variation.
    values = band.reshape(channels, -1)
    reference = np.median(values, axis=1)
    spread = np.percentile(values, [10, 90], axis=1)
    chroma = values / np.maximum(values.mean(axis=0, keepdims=True), 1)
    result = {
        'folder': folder, 'gap_columns': gaps, 'reference_rgb': reference.tolist(),
        'p10_rgb': spread[0].tolist(), 'p90_rgb': spread[1].tolist(),
        'relative_p90_minus_p10': (spread[1]/np.maximum(reference,1)-spread[0]/np.maximum(reference,1)).tolist(),
        'rows_used': [rows//8, rows-rows//8], 'line_pitch': pitch,
        'interpretation': 'absolute brightness varies across gaps; use channel ratios plus a local intensity envelope',
        'chroma_center': np.median(chroma, axis=1).tolist(),
        'chroma_p05': np.percentile(chroma, 5, axis=1).tolist(),
        'chroma_p95': np.percentile(chroma, 95, axis=1).tolist(),
    }
    (root/'film-reference.json').write_text(json.dumps(result, indent=2)+'\n')
    return result

if __name__ == '__main__':
    print(json.dumps([extract(x) for x in sys.argv[1:]], indent=2))
