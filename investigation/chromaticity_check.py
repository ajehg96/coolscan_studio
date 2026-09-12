"""Synthetic acceptance checks for the per-gap film model."""
import numpy as np
from chromaticity_classifier import FilmModel, edge_crossing

rng = np.random.default_rng(42)
base = np.array([560., 270., 155.])
gap = base * rng.uniform(.5, 2.0, (2000, 1)) * rng.uniform(.97, 1.03, (2000, 3))
model = FilmModel(gap)
assert model.classify(gap).mean() > .95
scene = np.array([300., 430., 500.]) * rng.uniform(.6, 1.4, (2000, 1))
assert model.classify(scene).mean() < .05
columns = [gap[rng.choice(len(gap), 100)] for _ in range(12)] + [scene[rng.choice(len(scene), 100)] for _ in range(20)]
cross, fractions = edge_crossing(columns, model)
assert cross == 12, (cross, fractions)
dust = gap[rng.choice(len(gap), 100)].copy(); dust[:3] = scene[:3]
assert model.classify(dust).mean() > .90
print('chromaticity checks passed')
