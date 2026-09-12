import numpy as np
from PIL import Image

def test_insets(name, uncropped_path, lead, trail):
    im = Image.open(uncropped_path)
    arr = np.array(im)
    
    print(f"=== {name} ===")
    print(f"Base crop columns: [{lead}, {trail}), width = {trail - lead} cols ({ (trail - lead) / 725 * 25.4:.2f} mm)")
    
    for inset in [0, 1, 2, 3, 4, 5, 6]:
        l = lead + inset
        r = trail - inset
        cropped = arr[:, l:r, :]
        w = r - l
        mm = w / 725 * 25.4
        
        # Edge brightness:
        first_col_mean = cropped[:, 0, :].mean()
        second_col_mean = cropped[:, 1, :].mean()
        penultimate_col_mean = cropped[:, -2, :].mean()
        last_col_mean = cropped[:, -1, :].mean()
        
        print(f"Inset {inset:2d}: [{l:4d}, {r:4d}] w={w:4d} ({mm:.2f} mm) | left edges: [{first_col_mean:.1f}, {second_col_mean:.1f}] | right edges: [{penultimate_col_mean:.1f}, {last_col_mean:.1f}]")
    print()

test_insets("Frame 2", "investigation/strip-b-09-autocrop/frame-2-uncropped.bmp", 29, 1059)
test_insets("Frame 3", "investigation/strip-b-10-autocrop/frame-3-uncropped.bmp", 18, 1045)
test_insets("Frame 4", "investigation/strip-b-12-autocrop/frame-4-uncropped.bmp", 39, 1063)
