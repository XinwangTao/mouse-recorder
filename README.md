# Mouse Recorder

A Windows desktop tool for capturing UI element screenshots with precise coordinate metadata. Built with Rust + egui.

## What it does

Records screen regions by drag-select, storing each region as a named **element** with:

- Center point coordinates `(center_x, center_y)` in physical pixels
- Bounding box `(x, y, width, height)`
- Screen resolution at capture time (for scaling on other resolutions)
- One or more named **state** screenshots per element (normal, hover, selected, disabled, etc.)

All data is stored in a SQLite database at `D:\elements\mouse_recorder.db`. Screenshots are saved to `D:\elements\`.

---

## Quick Start

```
cargo build --release
target\release\mouse-recorder.exe
```

Or use the management wrapper:

```
python manage.py record
```

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

> **Important:** Elements recorded this way get real coordinates. Images dropped manually into `D:\elements\` without going through this flow will have `center_x = 0, center_y = 0` in the database.

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

The temporary `capture_<timestamp>.png` file is renamed automatically. If the rename fails (e.g. cross-device), the original timestamped path is kept.

---

## Coordinates

Coordinates are in **physical pixels** — the raw values from the Windows low-level mouse hook multiplied by the DPI scale factor (`pixels_per_point`):

```
physical_x = logical_x × dpi_scale
```

So on a 125% DPI display, a logical position of `(800, 600)` is stored as `(1000, 750)`.

The `screen_width` / `screen_height` stored per element lets consumers rescale coordinates when replaying on a different resolution.

---

## Export to CSV

Click **Export CSV**, enter a filename. Saved to `D:\<filename>_<timestamp>.csv`.

| Column | Description |
|--------|-------------|
| `id` | Element ID |
| `x` | Center X (physical pixels) |
| `y` | Center Y (physical pixels) |
| `description` | Element name |
| `template_path` | Path to primary state screenshot |
| `original_width` | Screen width at capture time |
| `original_height` | Screen height at capture time |

---

## Management Tools

### `python manage.py <command>`

| Command | Description |
|---------|-------------|
| `record` | Launch the recorder (builds first if needed) |
| `build` | Compile the Rust binary |
| `list` | List all elements and states |
| `stats` | Database statistics |
| `verify` | Check DB integrity and validate image files |
| `export <file.csv>` | Safe export with validation |
| `backup` | Create timestamped DB backup |

### `python db_manager.py <command>`

Lower-level DB operations:

```
python db_manager.py list [name_filter]
python db_manager.py set-primary <element_id> <state_name>
python db_manager.py delete-state <element_id> <state_name>
python db_manager.py delete-element <element_id>
python db_manager.py export-clicker [output.csv]
python db_manager.py export-json [output.json] [--all-states]
python db_manager.py export-matrix [output.json]
python db_manager.py verify
python db_manager.py backup
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

Each element has exactly one primary state (`is_primary = 1`). This state is used in CSV exports.

---

## File Paths

| Path | Contents |
|------|----------|
| `D:\elements\mouse_recorder.db` | SQLite database |
| `D:\elements\<name>_<state>.png` | Screenshot files |
| `D:\<name>_<timestamp>.csv` | CSV exports |
| `D:\backups\mouse_recorder_<ts>.db` | DB backups (keeps last 5) |

---

## Build

```
cargo build --release
```

Requires Rust stable. The `unstable_grab` feature of `rdev` is enabled for event interception — no elevated privileges required on Windows.
