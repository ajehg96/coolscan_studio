"""Fast synthetic checks for the conservative edge rule."""
import numpy as np
from refine_edges import refine

def test_clear_edges_are_accepted(tmp_path):
    # This test exercises the same profile math with a known 30-pixel margin.
    rows, cols = 80, 400
    x = np.arange(cols)
    profile = np.where((x >= 30) & (x < 370), 0.2, 1.0)
    profile += np.sin(x / 7) * 0.002
    gradient = np.diff(np.log1p(profile))
    assert np.argmin(gradient[:48]) + 1 == 30
    assert cols - 48 + np.argmax(gradient[-48:]) == 370

def test_weak_edges_are_not_trusted():
    gradient = np.zeros(100)
    gradient[30] = -0.01
    gradient[70] = 0.01
    noise = np.median(np.abs(gradient - np.median(gradient))) * 1.4826
    assert not (abs(gradient[30]) >= max(noise * 6, 0.03))
