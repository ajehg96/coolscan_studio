"""Per-gap chromaticity classifier for film-like pixels."""
import numpy as np

class FilmModel:
    def __init__(self, gap_pixels):
        gap = np.asarray(gap_pixels, dtype=float)
        if gap.ndim != 2 or gap.shape[1] != 3 or len(gap) < 8:
            raise ValueError('need at least eight RGB gap pixels')
        chroma = gap / np.maximum(gap.mean(axis=1, keepdims=True), 1)
        self.center = np.median(chroma, axis=0)
        self.radius = max(float(np.percentile(np.abs(chroma-self.center), 95))*2, .025)
        intensity = gap.mean(axis=1)
        self.bounds = (float(np.percentile(intensity, 1)*.5), float(np.percentile(intensity, 99)*2))

    def classify(self, pixels):
        pixels = np.asarray(pixels, dtype=float)
        means = np.maximum(pixels.mean(axis=1, keepdims=True), 1)
        chroma_ok = np.max(np.abs(pixels/means-self.center), axis=1) <= self.radius
        # Exposure differs between discovery and preview; chromaticity is the
        # transferable feature. Intensity bounds remain diagnostic metadata.
        return chroma_ok

def edge_crossing(columns, model, threshold=.05, run=8):
    fractions = np.array([model.classify(column).mean() for column in columns])
    if len(fractions) >= 3:
        padded = np.pad(fractions, 1, mode='edge')
        smoothed = np.median([padded[:-2], padded[1:-1], padded[2:]], axis=0)
    else:
        smoothed = fractions
    low = smoothed < threshold
    for i in range(len(low)-run+1):
        if low[i:i+run].all(): return i, fractions
    return None, fractions
