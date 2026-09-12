import numpy as np
from PIL import Image

def generate_inset_bmps(uncropped_path, out_prefix, lead, trail):
    im = Image.open(uncropped_path)
    arr = np.array(im)
    for inset in [2, 3, 5]:
        l = lead + inset
        r = trail - inset
        cropped_arr = arr[:, l:r, :]
        cropped_im = Image.fromarray(cropped_arr)
        out_path = f"{out_prefix}-inset{inset}.bmp"
        cropped_im.save(out_path)
        print(f"Saved {out_path} ({cropped_im.size[0]} x {cropped_im.size[1]}, {(r-l)/725*25.4:.2f} mm)")

generate_inset_bmps("investigation/strip-b-09-autocrop/frame-2-uncropped.bmp", "investigation/strip-b-09-autocrop/frame-2", 29, 1059)
generate_inset_bmps("investigation/strip-b-10-autocrop/frame-3-uncropped.bmp", "investigation/strip-b-10-autocrop/frame-3", 18, 1045)
generate_inset_bmps("investigation/strip-b-12-autocrop/frame-4-uncropped.bmp", "investigation/strip-b-12-autocrop/frame-4", 39, 1063)
