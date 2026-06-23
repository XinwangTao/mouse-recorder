# Critical Fixes Required in src/main.rs

## Issue 1: Transaction Integrity (CRITICAL)

**Problem**: Screenshot can fail after database record is created, causing data inconsistency.

**Current code** (lines 392-428):
```rust
fn create_new_element(...) -> Result<i32, Box<dyn std::error::Error>> {
    let (bbox, center_x, center_y) = calculate_region_info(...)?;
    
    let timestamp = chrono::Local::now().format("%Y%m%d_%H%M%S");
    let image_filename = format!("D:\\elements\\{}_{}.png", ...);
    
    fs::create_dir_all("D:\\elements")?;
    
    // BUG: If this fails, we've already calculated everything
    capture_and_save_region(..., &image_filename)?;
    
    // BUG: If capture succeeds but this fails, we have orphaned image
    app_state.db.execute("INSERT INTO elements ...")?;
    
    let element_id = app_state.db.last_insert_rowid() as i32;
    
    // BUG: If this fails, we have element without primary state
    app_state.db.execute("INSERT INTO element_states ...")?;
    
    Ok(element_id)
}
```

**Fixed code**:
```rust
fn create_new_element(...) -> Result<i32, Box<dyn std::error::Error>> {
    let (bbox, center_x, center_y) = calculate_region_info(...)?;
    
    let timestamp = chrono::Local::now().format("%Y%m%d_%H%M%S");
    let image_filename = format!("D:\\elements\\{}_{}.png", ...);
    
    fs::create_dir_all("D:\\elements")?;
    
    // STEP 1: Capture image FIRST (most likely to fail)
    capture_and_save_region(..., &image_filename)?;
    
    // STEP 2: Start transaction
    let tx = app_state.db.transaction()?;
    
    // STEP 3: Insert element
    tx.execute(
        "INSERT INTO elements (...) VALUES (?1, ?2, ...)",
        params![name, center_x, ...]
    )?;
    
    let element_id = tx.last_insert_rowid() as i32;
    
    // STEP 4: Insert state
    tx.execute(
        "INSERT INTO element_states (...) VALUES (?1, ?2, ...)",
        params![element_id, state_name, image_filename, ...]
    )?;
    
    // STEP 5: Commit transaction (all-or-nothing)
    tx.commit()?;
    
    Ok(element_id)
}
```

**Same fix needed for**: `add_element_state()` function

## Issue 2: Path Configuration (HIGH PRIORITY)

**Problem**: All paths are hardcoded to `D:\`

**Changes needed**:

1. Add Config struct at the top of main.rs:
```rust
#[derive(Debug, Clone, Deserialize)]
struct Config {
    db_path: String,
    image_dir: String,
    backup_dir: String,
    auto_backup: bool,
    max_backups: usize,
}

impl Config {
    fn load() -> Self {
        let config_path = "config.toml";
        if let Ok(content) = fs::read_to_string(config_path) {
            toml::from_str(&content).unwrap_or_else(|e| {
                eprintln!("Warning: Invalid config.toml, using defaults: {}", e);
                Self::default()
            })
        } else {
            Self::default()
        }
    }
}

impl Default for Config {
    fn default() -> Self {
        Self {
            db_path: "D:\\elements\\mouse_recorder.db".to_string(),
            image_dir: "D:\\elements".to_string(),
            backup_dir: "D:\\backups".to_string(),
            auto_backup: true,
            max_backups: 5,
        }
    }
}
```

2. Update AppState to include config:
```rust
struct AppState {
    db: Connection,
    mode: RecordMode,
    screen_resolution: ScreenResolution,
    last_element_id: Option<i32>,
    config: Config,  // ADD THIS
}
```

3. Replace all hardcoded paths:
```rust
// BEFORE
let db_path = "D:\\elements\\mouse_recorder.db";
let image_filename = format!("D:\\elements\\{}_{}.png", ...);
fs::create_dir_all("D:\\elements")?;

// AFTER
let db_path = &app_state.config.db_path;
let image_filename = format!("{}\\{}_{}.png", app_state.config.image_dir, ...);
fs::create_dir_all(&app_state.config.image_dir)?;
```

## Issue 3: Export Validation (MEDIUM PRIORITY)

**Problem**: CSV export doesn't verify images exist

**Fix in export_to_clicker()**:
```rust
fn export_to_clicker(conn: &Connection, base_filename: &str) -> Result<String, Box<dyn std::error::Error>> {
    // ... existing query code ...
    
    let mut missing_images = Vec::new();
    
    for row in rows {
        let (id, name, center_x, center_y, image_path) = row?;
        
        // VALIDATE IMAGE EXISTS
        if !Path::new(&image_path).exists() {
            missing_images.push(format!("Element #{}: {}", id, image_path));
        }
        
        let escaped_name = name.replace('"', "\"\"");
        let escaped_path = image_path.replace('"', "\"\"");
        writeln!(file, ...)?;
    }
    
    // WARN USER IF IMAGES MISSING
    if !missing_images.is_empty() {
        eprintln!("Warning: {} image(s) not found:", missing_images.len());
        for img in &missing_images {
            eprintln!("  - {}", img);
        }
        eprintln!("mouse-clicker will fall back to coordinates for these elements.");
    }
    
    Ok(csv_path)
}
```

## Issue 4: Auto-Backup on Startup (LOW PRIORITY)

**Add to main() before database operations**:
```rust
fn main() -> io::Result<()> {
    let config = Config::load();
    
    // AUTO-BACKUP IF ENABLED
    if config.auto_backup && Path::new(&config.db_path).exists() {
        if let Err(e) = create_backup(&config) {
            eprintln!("Warning: Backup failed: {}", e);
        }
    }
    
    let conn = init_database(&config.db_path)?;
    // ... rest of code ...
}

fn create_backup(config: &Config) -> std::io::Result<()> {
    fs::create_dir_all(&config.backup_dir)?;
    
    let timestamp = chrono::Local::now().format("%Y%m%d_%H%M%S");
    let backup_path = format!("{}\\mouse_recorder_{}.db", config.backup_dir, timestamp);
    
    fs::copy(&config.db_path, &backup_path)?;
    
    // CLEANUP OLD BACKUPS
    let mut backups: Vec<_> = fs::read_dir(&config.backup_dir)?
        .filter_map(|e| e.ok())
        .filter(|e| e.path().extension().and_then(|s| s.to_str()) == Some("db"))
        .collect();
    
    backups.sort_by_key(|e| e.metadata().unwrap().modified().unwrap());
    
    if backups.len() > config.max_backups {
        for old in &backups[..backups.len() - config.max_backups] {
            let _ = fs::remove_file(old.path());
        }
    }
    
    Ok(())
}
```

## Testing Checklist

After applying fixes:

1. [ ] Compile with `cargo build`
2. [ ] Test config.toml loading (modify paths, verify used)
3. [ ] Test transaction rollback (kill process during capture, verify no orphan DB records)
4. [ ] Test missing image warning in export
5. [ ] Test auto-backup creates files in backup_dir
6. [ ] Test backup cleanup keeps only max_backups files
7. [ ] Verify CSV format with mouse-clicker

## Priority Order

1. **CRITICAL**: Fix transaction integrity (Issue 1)
2. **HIGH**: Add path configuration (Issue 2)
3. **MEDIUM**: Add export validation (Issue 3)
4. **LOW**: Add auto-backup (Issue 4)
