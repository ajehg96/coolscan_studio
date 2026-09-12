import numpy as np
from PIL import Image

for f, path in [
    ('Frame 2', 'investigation/strip-b-09-autocrop/frame-2-cropped.bmp'),
    ('Frame 3', 'investigation/strip-b-10-autocrop/frame-3-cropped.bmp'),
    ('Frame 4', 'investigation/strip-b-12-autocrop/frame-4-cropped.bmp'),
]:
    im = Image.open(path)
    arr = np.array(im, dtype=float)
    row_mean = arr.mean(axis=(1, 2))
    print(f'=== {f} ===')
    print("Top rows 18..27:")
    for r in range(18, 28):
        print(f"  row {r:3d}: {row_mean[r]:.1f}")
    
    print("Bottom rows 685..716:")
    for r in range(685, 717):
        print(f"  row {r:3d}: {row_mean[r]:.1f}")
    print()
