"""Combine discovery geometry with a high-resolution frame preview.

The preview is registered to the frame rectangle and therefore may have no
trailing overscan. In that case the discovery transition supplies a bounded,
lower-confidence trailing estimate; the result is marked accordingly.
"""
from pathlib import Path
import json, re, sys
import numpy as np

def parse_rect(text):
    return tuple(map(int, re.search(r'rect=Rect \{ top: (\d+), left: (\d+), bottom: (\d+), right: (\d+) \}', text).groups()))

def run(discovery_dir, preview_dir):
    droot, proot = Path(discovery_dir), Path(preview_dir)
    dmeta = (droot/'discovery.txt').read_text()
    pmeta = (proot/'frame-2.txt').read_text()
    dro, dco, dc = map(int, re.search(r'rows=(\d+) cols=(\d+) channels=(\d+)', dmeta).groups())
    pro, pco, pc = map(int, re.search(r'rows=(\d+) cols=(\d+) channels=(\d+)', pmeta).groups())
    dpi = int(re.search(r'dpi: (\d+)', pmeta).group(1))
    d = np.fromfile(droot/'discovery.raw', dtype='<u2').reshape(dc, dro, dco).astype(float)
    p = np.fromfile(proot/'frame-2.raw', dtype='<u2').reshape(pc, pro, pco).astype(float)
    dprofile = np.log1p(np.maximum(d[:, dro//8: dro-dro//8].mean(axis=1), 1)).mean(axis=0)
    pprofile = np.log1p(np.maximum(p[:, pro//8: pro-pro//8].mean(axis=1), 1)).mean(axis=0)
    dg = np.diff(dprofile)
    # nkscan's frame 2 registration in discovery columns and preview pixels.
    drects = re.findall(r'top:\s*(\d+),\s*left:\s*(\d+),\s*bottom:\s*(\d+),\s*right:\s*(\d+)', dmeta)
    if len(drects) < 6: raise ValueError('discovery metadata has fewer than six rectangles')
    dtop, _, dbottom, _ = map(int, drects[1])
    frame_start = int(round((dtop - int(drects[0][0])) / 30))
    # Locate the strongest transition near frame 2's expected end.
    # Use the known frame format: discovery start + its rectangle height/30.
    start_col = int(round((dtop - int(drects[0][0])) / 30))
    length_col = max(1, round((dbottom-dtop)/30))
    end_col = min(dco-1, start_col + length_col)
    lo, hi = max(0, end_col-10), min(dco-1, end_col+10)
    candidate_d = lo + int(np.argmax(dg[lo:hi])) + 1
    pg = np.diff(pprofile)
    search = max(8, pco // 8)
    left = int(np.argmin(pg[:search])) + 1
    right = pco - search + int(np.argmax(pg[-search:]))
    noise = float(np.median(np.abs(pg-np.median(pg))) * 1.4826)
    threshold = max(noise * 6, 0.03)
    left_strength = float(abs(pg[left-1]))
    right_strength = float(abs(pg[right-1]))

    trailing_has_overscan = right_strength >= threshold and right > left and (pco - right) >= 4
    trailing_conf = 'high-res-confident' if trailing_has_overscan else 'discovery-only'
    chosen_right = right if trailing_has_overscan else pco

    result = {
        'discovery_frame2_start_column': start_col,
        'discovery_frame2_expected_end_column': end_col,
        'discovery_trailing_candidate_column': candidate_d,
        'preview_leading_candidate_column': left,
        'preview_trailing_candidate_column': right if trailing_has_overscan else None,
        'preview_width_columns': pco,
        'preview_trailing_overscan': trailing_has_overscan,
        'leading_confident': left_strength >= threshold,
        'trailing_confidence': trailing_conf,
        'estimated_frame_columns': [left, chosen_right],
        'estimated_frame_mm': [round(left*25.4/dpi, 3), round(chosen_right*25.4/dpi, 3)],
    }
    (proot/'combined-edges.json').write_text(json.dumps(result, indent=2)+'\n')
    return result

if __name__ == '__main__':
    print(json.dumps([run(a,b) for a,b in zip(sys.argv[1::2],sys.argv[2::2])], indent=2))
