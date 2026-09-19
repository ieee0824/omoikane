# Fullscreen top-layer fixture

This fixture verifies that a fullscreen element ignores its authored size and
transform, fills the 160x120 viewport, and paints above an ordinary element
with the maximum `z-index`. The yellow marker must appear at `(60, 40)` on the
red fullscreen surface.

Refresh the checked-in baseline only after reviewing the generated image:

```bash
OMOIKANE_REFRESH_BASELINE=1 cargo test refresh_fullscreen_top_layer_baseline_png -- --ignored
cargo test fullscreen_top_layer_fixture_matches_local_baseline_png
```
