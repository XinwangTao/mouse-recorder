# Changelog

All notable changes to this project are documented here.

## [0.5.2] — 2026-11-06

### Fixed
- Capture selection: intersect the drag rectangle with the captured image in
  `i64` before casting to `u32`. Negative coordinates on a monitor left/above
  the primary no longer wrap into a garbage crop; off-screen selections are
  rejected.
- Preview: clear the path-keyed texture on refresh so re-record/overwrite shows
  the new screenshot immediately.
- Rename: preflight every target filename and journal file moves, so a mid-way
  failure rolls back; metadata updates now commit in one transaction.
- Set-primary: both updates run in one transaction (no window with zero
  primary state).
- Input listener: retry `rdev::grab` after a failure instead of leaving the
  app unable to record for the rest of the session.

### Performance
- Template matching: abort SAD as soon as the remaining pixels cannot beat the
  current best, and scan raw row slices instead of bounds-checked `get_pixel`.
  Scores and hit coordinates are unchanged (verified against brute force).

## [0.5.1] — 2026-11-06

### Performance
- Cache the element health report instead of re-reading every state PNG
  (file stat + PNG header) for each element on every frame. The cache is
  rebuilt on refresh and when the screen resolution changes.
- Load elements and their states with a single `LEFT JOIN` query, replacing
  the previous N+1 per-element state queries.
- Add `idx_states_element_id` on `element_states(element_id)` for state lookups.
- Reuse one `DeviceState` probe per UI thread for the Esc fallback instead of
  constructing it on every frame (capture and ROI overlay).

### Fixed / robustness
- Log `config.toml` read/parse failures instead of silently falling back to
  defaults (defaults are still used).
- Release builds stay windowless, while `cargo test` now emits console output.

### Added
- 15 unit tests covering ROI math, health assessment, state-suffix cycling and
  DB element/state loading.

## [0.5.0] — 2026-09-06

- Hardened capture/DB paths, unique indexes, scaled verify ROI, repo layout.
