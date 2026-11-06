# Mouse Recorder

A Windows desktop tool for capturing UI element screenshots with precise coordinate metadata. Built with Rust + egui.

## What it does

Records screen regions by drag-select, storing each region as a named **element** with:

- Center point coordinates `(center_x, center_y)` in physical pixels
- Bounding box `(x, y, width, height)`
- Screen resolution at capture time (for scaling on other resolutions)
- One or more named **state** screenshots per element (normal, hover, selected, disabled, etc.)

All data is stored under `./data/` (see `config.toml`): SQLite at `data/mouse_recorder.db`, screenshots beside it.

---

## Quick Start

```
cargo build --release
target\release\mouse-recorder.exe
```

Or use the management wrapper:

```
python scripts/manage.py record
```

---

## CLI / UI Automation (`--cli`)

The same executable runs headless commands for scripts and agents:

```
mouse-recorder.exe --cli <command> [options]
```

| Command | What it does |
|---------|--------------|
| `list [--filter X] [--json]` | List elements and states |
| `get <name> [--state S] [--json]` | Show one element |
| `record --name NAME --rect x,y,w,h [--state S] [--overwrite]` | Capture a region without the GUI; a new state is added when the element exists |
| `rename <old> <new>` | Rename an element (files + DB) |
| `delete <name> [--state S]` | Delete an element or one state |
| `set-primary <name> <state>` | Set the primary state |
| `export --format csv\|json [--out PATH]` | Export elements |
| `backup [--out DIR]` | Checkpoint and copy the SQLite DB |
| `find --element NAME[:STATE]` / `--template PATH` | Template search on the current screen |
| `verify ...` | Same match, exit 3 when the score is below threshold |
| `click --element NAME[:STATE]` / `--template PATH` / `--uia` / `--coords x,y` | Click a target; `--verify` re-checks afterwards |
| `uia --window TITLE [--name X] [--automation-id X] [--class X] [--control-type X]` | Dump/filter Windows UIA controls with screen rects |
| `ocr --text TEXT [--image PATH] [--lang eng]` | OCR text lookup (local Tesseract) |
| `run workflow.json\|yaml [--json]` | Execute a click workflow |

All geometry uses the same image-relative + `origin` model as the GUI; the CLI
converts to virtual-screen coordinates before moving the mouse. `--rect`,
`--coords` and `--roi` are always **virtual-screen** pixels.

> Paths are resolved relative to the executable directory (`config.toml`,
> `data/`). Use the packaged layout (exe beside `config.toml` + `data/`) or copy
> the exe there; running `target/release/mouse-recorder.exe` uses
> `target/release/data`.

### Workflow format (JSON or YAML, schema_version 1)

```yaml
schema_version: 1
name: login
defaults:
  threshold: 0.85
  retry: 2
steps:
  - id: open
    action: click
    target:
      type: element          # element | template | uia | ocr | coords
      name: scene/login_btn
    expect:
      type: window           # none | window | element | template | ocr
      window: "登录"
  - id: submit
    action: click
    target:
      type: uia
      window: "登录"
      control_type: Button
      name: "确定"
    expect:
      type: none
```

Each step writes a screenshot, score, elapsed time and error to
`runs/<timestamp>/report.json`; exit code 4 means an assertion failed. Adaptive
thresholds (0.50–0.98) are stored in `runs/thresholds.json` and adjust only
after consecutive evidence.

OCR uses the locally installed Tesseract; available languages depend on its
`tessdata` (this machine has `eng`). UIA covers standard Win32/WPF/UWP
controls; canvas/game UIs fall back to template or OCR.

Full-screen template search is bounded to ~3s per pass and uses a coarser scan
for large templates. Pass `--roi x,y,w,h` (virtual-screen pixels) for large
templates or when you know the target region.

---

## Recording Elements

### New element — F5

1. Launch `mouse-recorder.exe`
2. Click **New Element** or press **F5**
3. The app enters capture mode (window goes fullscreen transparent, other apps stop receiving mouse events)
4. Drag a rectangle over the target UI element
5. Enter an **element name** (e.g. `submit_button`)
6. Enter a **state name** (default: `-n`)
7. The element is saved with its screenshot, center coordinates, bounding box, and screen resolution

### Browse & search

- **Name search**: type in the search box above the list (case-insensitive substring). Shows `matched/total`. Press **Enter** in the search box to jump to the first match; **清除** clears the filter.
- **Group filter**: names with `scene/foo` style go under group `scene`; others under `(未分组)`.
- **Image preview**: selecting an element/state shows a live thumbnail on the right (also in Details / F10).
- **Health badges**: list items tinted with `[缺图|中心0|ROI小|重名|分辨率|…]`; hover for details.
- **Up / Down**: navigate within the *filtered* list (disabled while the search box is focused).

### Verify / maintain

| Action | Key | What it does |
|--------|-----|----------------|
| 验配 | F11 | Screenshot + template search in ROI (fallback full screen); overlay shows score, hit box (cyan), recorded center (yellow) |
| 重录 | F12 | Re-capture current element+state, overwrite image/geometry, keep id/name |
| 改名 | — | Rename element + image files; blocks collisions |
| 状态对比 | — | Side-by-side state thumbnails |
| 缩放到当前屏 | — | Banner when recorded resolution ≠ current; scales center/ROI |
| 打开目录 | — | Open image/DB folder in Explorer |

> **Important:** Elements recorded this way get real coordinates. Images dropped manually into `data/` without going through this flow will have `center_x = 0, center_y = 0` in the database.

### Add a state to existing element — F6

With an element selected (highlighted yellow in the list):

1. Click **Add State** or press **F6**
2. Drag-select the same element in a different visual state
3. The new state image is saved and linked to the element

### Quick state shortcuts (F1–F4)

These capture a new state image immediately without prompting for a state name:

| Key | State suffix | Meaning |
|-----|-------------|---------|
| F1 | `-n` | Normal |
| F2 | `-s` | Selected |
| F3 | `-c` | Clicked |
| F4 | `-cs` | Click + Selected |

### Capture mode behavior

- Mouse events (move, left click, drag) are **intercepted** — other applications will not receive them during capture
- The window becomes a fullscreen transparent overlay with crosshair and live coordinate tooltip
- A selection rectangle shows live dimensions while dragging
- Auto-cancels after **30 seconds** if no drag is completed
- Press **Escape** to cancel at any time

---

## Screenshot Naming

Screenshots are named `<element_name>_<state_name>.png` after both names are confirmed.

Example: element `bangong`, state `-n` → `D:\elements\bangong_-n.png`

Names may use one grouping level (`scene/foo`), which creates `data/scene/foo_-n.png` and stores the relative path in the DB. Path escapes (`..`, absolute paths, backslashes) and Windows-invalid filename characters are rejected before anything is written.

The temporary `capture_<timestamp>.png` file is renamed automatically. If the rename fails (e.g. cross-device), the original timestamped path is kept.

Committed captures keep the previous image as `<file>.png.bak` until the DB transaction commits. On startup, a `.png.bak` whose destination is missing is restored (crash recovery); if both files exist, the backup is kept for manual inspection and is replaced by the next capture of the same name.

---

## Coordinates

Coordinates are in **physical pixels**. The app captures the whole Windows
virtual desktop (all monitors), so the captured image is the desktop bounding
box:

- Geometry (`center`, `ROI`, image rect) is stored **relative to the captured
  image**, starting at `(0, 0)`.
- `origin_x` / `origin_y` record where that image starts in Windows
  virtual-screen coordinates (negative when a monitor sits left of/above the
  primary).
- To get a mouse-click coordinate: `screen_x = center_x + origin_x`.

Exports (CSV, `db_manager.py`) already add the origin, so their `x` / `y` are
virtual-screen pixels usable for `SetCursorPos` / automation tools.

`screen_width` / `screen_height` store the virtual desktop size at capture time;
consumers can rescale when replaying on a different resolution.

> Mixed-DPI monitor layouts: the overlay uses one DPI scale for the whole
> virtual desktop. Recording on layouts whose monitors use different scaling
> factors may be off by the per-monitor DPI difference; same-scale layouts are
> exact. Re-record after changing the monitor arrangement.

---

## Export to CSV

Click **Export CSV**, enter a filename. Saved under `data/<filename>_<timestamp>.csv`.

| Column | Description |
|--------|-------------|
| `id` | Element ID |
| `x` | Center X (virtual-screen physical pixels = stored center + origin) |
| `y` | Center Y (virtual-screen physical pixels) |
| `description` | Element name |
| `template_path` | Path to primary state screenshot |
| `original_width` | Virtual desktop width at capture time |
| `original_height` | Virtual desktop height at capture time |
| `origin_x` | Image origin X in virtual-screen pixels |
| `origin_y` | Image origin Y in virtual-screen pixels |

---

## Management Tools

### `python scripts/manage.py <command>`

| Command | Description |
|---------|-------------|
| `record` | Launch the recorder (builds first if needed) |
| `build` | Compile the Rust binary |
| `list` | List all elements and states |
| `stats` | Database statistics |
| `verify` | Check DB integrity and validate image files |
| `export <file.csv>` | Safe export with validation |
| `backup` | Create timestamped DB backup |

### `python scripts/db_manager.py <command>`

Lower-level DB operations:

```
python scripts/db_manager.py list [name_filter]
python scripts/db_manager.py set-primary <element_id> <state_name>
python scripts/db_manager.py delete-state <element_id> <state_name>
python scripts/db_manager.py delete-element <element_id>
python scripts/db_manager.py export-clicker [output.csv]
python scripts/db_manager.py export-json [output.json] [--all-states]
python scripts/db_manager.py export-matrix [output.json]
python scripts/db_manager.py verify
python scripts/db_manager.py backup
```

---

## Database Schema

```sql
CREATE TABLE elements (
    id           INTEGER PRIMARY KEY AUTOINCREMENT,
    name         TEXT    NOT NULL,
    center_x     INTEGER NOT NULL,
    center_y     INTEGER NOT NULL,
    bbox_x       INTEGER NOT NULL,
    bbox_y       INTEGER NOT NULL,
    bbox_width   INTEGER NOT NULL,
    bbox_height  INTEGER NOT NULL,
    screen_width  INTEGER NOT NULL,
    screen_height INTEGER NOT NULL,
    origin_x     INTEGER NOT NULL DEFAULT 0,
    origin_y     INTEGER NOT NULL DEFAULT 0,
    created_at   TEXT    NOT NULL
);

CREATE TABLE element_states (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    element_id  INTEGER NOT NULL REFERENCES elements(id),
    state_name  TEXT    NOT NULL,
    image_path  TEXT    NOT NULL,
    is_primary  BOOLEAN NOT NULL DEFAULT 0,
    created_at  TEXT    NOT NULL
);
```

Each element has exactly one primary state (`is_primary = 1`). This state is used in CSV exports. `image_path` is relative to the image directory (for example `scene/foo_-n.png`); legacy basename-only rows are migrated to the relative form at startup when the file is found in a subdirectory.

---

## File Paths

| Path | Contents |
|------|----------|
| `data/mouse_recorder.db` | SQLite database |
| `data/<name>_<state>.png` | Screenshot files |
| `data/backups/` | DB backups |
| `dist/latest/` | Current packaged release |
| `docs/` | Guides and API reference |
| `scripts/` | Python management tools |

---

## Build

```
cargo build --release
```

Requires Rust stable. The `unstable_grab` feature of `rdev` is enabled for event interception — no elevated privileges required on Windows.

### Tests

```
cargo test
```

Pure helpers (ROI math, health assessment, DB loading) are covered by unit tests; no display or recorded data is needed.

### Performance notes

- The element list renders from a cached health report per element (image existence + PNG dimensions are read on refresh/screen change, not every frame).
- State loading uses one `LEFT JOIN` query instead of one query per element.
- `element_states(element_id)` is indexed.
- Template matching aborts a candidate as soon as its remaining pixels cannot beat the current best, and scans raw pixel rows. Scores and hit coordinates match the brute-force SAD result.
- Capture intersects the drag rectangle in 64-bit space before converting to image coordinates, so negative multi-monitor coordinates cannot wrap.

See [CHANGELOG.md](CHANGELOG.md) for release notes.
