# Changelog

All notable changes to this project are documented here.

## [0.7.3] — 2026-11-09

### Fixed
- UIA targets behind another window are no longer clicked through. `click --uia`
  and workflow `uia` targets now raise the target window (AttachThreadInput
  bypass, no-activate z-order fallback), re-read the control rect *after* the
  raise, and refuse to click when `WindowFromPoint` still reports a covering
  window (exit 3, `--force` overrides). Previously the click landed on whatever
  window happened to be on top — found on a live desktop where a terminal
  covered Explorer's address bar.
- Workflow `--dry-run` no longer raises windows; it has no desktop side effects.

### Verified
- End-to-end on a live desktop: UIA locate an Edit control → click to focus →
  type `HELLO OCR 2026` → OCR read-back `HELLO` at 96.3% confidence, exit 0.

## [0.7.2] — 2026-11-06

### Fixed
- `click --template` now actually resolves a template target (it was advertised
  but unimplemented). `--verify` works with either `--element` or `--template`.
- `click --verify` without `--element`/`--template` is a usage error instead of a
  silent 3.
- `--uia` requires `--window`; empty titles, minimized windows and zero-size
  controls are ignored (no more clicks at (0,0) or in a random window).
- OCR phrases must match within one OCR line; the returned box is the union of
  the matched words (previously words from different lines could be merged).
- `ocr --window` now crops the target window and offsets coordinates correctly.
- `--dry-run` only resolves targets; it no longer executes assertions.
- `backup` uses `VACUUM INTO` for a consistent snapshot and falls back to copy.
- Report paths no longer contain `data/..`.
- `type`/`key` with a target clicks it first to focus it.

## [0.7.1] — 2026-11-06

### Fixed
- Full-screen template searches are bounded to a ~3s budget per pass and use a
  coarser coarse pass for large templates. A 600×200 template with no ROI could
  previously run for minutes; it now returns the best hit found and exits 3 when
  below threshold. Prefer `--roi` for large templates.

## [0.7.0] — 2026-11-06

### Added
- `mouse-recorder.exe --cli <command>`: headless element management and UI
  automation, sharing the GUI database and image directory.
  - `list`, `get`, `record` (non-interactive), `find`, `verify`, `click`
  - `uia` target discovery (control name, automation id, class, type, rect)
  - `ocr` text lookup via the locally installed Tesseract
  - `run workflow.json|yaml` with click/type/key/wait/assert steps
- JSON and YAML workflows share one schema (schema_version 1).
- Every step writes a screenshot, score, elapsed time and error to
  `runs/<timestamp>/report.json`; exit code 4 on assertion failure.
- Adaptive template thresholds (0.50–0.98) based on consecutive evidence,
  stored in `runs/thresholds.json`.
- `--json` output for all commands; CLI attaches to the parent console.

### Notes
- OCR languages depend on installed Tesseract `tessdata` (this machine has
  `eng`; Chinese OCR needs a `chi_sim` language pack or Windows OCR later).
- UIA works for standard Win32/WPF/UWP controls; canvas/game UIs fall back to
  template/OCR.
- Recorded screen coordinates remain image-relative + origin; CLI converts to
  virtual-screen coordinates before input.

## [0.6.2] — 2026-11-06

### Fixed
- After a fullscreen capture/verify overlay, force the main window back to a
  sane on-screen physical rect. Viewport commands alone could leave it
  minimized/off-screen (`-32000`) after the overlay.
- Auto-focus the element/state/rename/export input box when its prompt opens.

## [0.6.1] — 2026-11-06

### Fixed
- Edit/save refuses to write geometry when the recorded virtual-desktop size
  differs from the current one, and the overlay tells you to rescale first.
  Previously this could overwrite coordinates with unscaled values.
- Startup crash recovery: a missing PNG is restored from its `.png.bak`; when
  both exist the backup is kept for manual inspection.
- Legacy path migration no longer guesses when two groups share a basename.
- Verify cancel now cooperatively stops the matcher at the next row instead of
  leaving a CPU-burning orphan worker.
- Element/state names are validated: no `..`, absolute paths, backslashes,
  drive/ADS colons, or Windows-invalid characters; one `group/name` level is
  allowed.
- Airtest helper resolves relative image paths and prefers screen coordinates.

## [0.6.0] — 2026-11-06

### Added
- Multi-monitor capture: grabs the whole virtual desktop, records the capture
  origin, converts rdev screen coordinates at the capture boundary, and exposes
  `origin_x` / `origin_y` in the DB, CSV, and Python exports.
- Grouped element names (`scene/foo`) now store image paths relative to the
  image directory (`scene/foo_-n.png`). Startup migrates legacy basename rows
  whose file lives in a subdirectory.
- Async verify: the screenshot stays on the UI thread and template matching runs
  in a worker thread, with a progress overlay, Esc cancel, and a generation
  guard that ignores stale results.

### Changed
- Capture commits are two-phase: the DB transaction is held open while the PNG
  is staged with its old version in `.bak`; DB commits first, then the backup is
  dropped. A failure rolls back the side that already moved.
- Edit/save and verify rebase recorded coordinates from the capture origin to
  the current virtual desktop.

### Fixed
- `save_edit_roi` now records the current full-desktop origin with the geometry.

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
