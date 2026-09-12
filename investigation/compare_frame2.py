"""Measure the leading gap on frame 2 from independent row-wise edges.

This deliberately narrow diagnostic is not a general automatic cropper.
"""
from pathlib import Path
import json
import re
import sys
import numpy as np

results = []
for folder in sys.argv[1:]:
    root = Path(folder)
    meta = (root / 'frame-2.txt').read_text()
    rows, cols, channels = map(int, re.search(r'rows=(\d+) cols=(\d+) channels=(\d+)', meta).groups())
    data = np.fromfile(root/'frame-2.raw', dtype='<u2').reshape(channels, rows, cols).astype(float)
    # Exclude holder edges. The first 100 columns bracket the known leading gap.
    band = data[1, rows//8:rows-rows//8, :100]
    smooth = (band[:, :-2] + band[:, 1:-1] + band[:, 2:]) / 3
    gradient = np.diff(np.log1p(smooth), axis=1)
    edges = np.argmin(gradient, axis=1) + 2
    quantiles = np.percentile(edges, [10, 50, 90]).tolist()
    result = dict(folder=folder, rows=rows, cols=cols,
                  edge_column_p10_median_p90=quantiles,
                  median_border_mm=quantiles[1]*25.4/725)
    results.append(result)
print(json.dumps(results, indent=2))
