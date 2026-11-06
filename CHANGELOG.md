# Changelog

All notable changes to this project are documented here.

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
