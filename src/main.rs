#![windows_subsystem = "windows"]

mod matchutil;

use chrono::Local;
use eframe::egui::{self, Color32, ScrollArea, Stroke, Rect, Pos2, FontId};
use rdev::{grab, Event, EventType, Button};
use rusqlite::{params, Connection};
use serde::Deserialize;
use std::fs;
use std::io::Write;
use std::path::PathBuf;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

fn exe_dir() -> &'static std::path::Path {
    static DIR: OnceLock<PathBuf> = OnceLock::new();
    DIR.get_or_init(|| {
        let exe = std::env::current_exe().expect("Failed to get exe path");
        exe.parent().expect("Failed to get exe dir").to_path_buf()
    })
}

fn data_dir() -> &'static std::path::Path {
    static DIR: OnceLock<PathBuf> = OnceLock::new();
    DIR.get_or_init(|| exe_dir().join("data"))
}

#[derive(Deserialize)]
struct Config {
    db_path: Option<String>,
    image_dir: Option<String>,
}

impl Config {
    fn load() -> Self {
        let config_path = exe_dir().join("config.toml");
        fs::read_to_string(&config_path)
            .ok()
            .and_then(|content| toml::from_str(&content).ok())
            .unwrap_or_default()
    }

    fn resolve(&self, path: &str) -> PathBuf {
        let p = std::path::Path::new(path);
        if p.is_absolute() { p.to_path_buf() } else { exe_dir().join(p) }
    }

    fn db_path(&self) -> String {
        self.db_path.as_ref()
            .map(|p| self.resolve(p).to_string_lossy().to_string())
            .unwrap_or_else(|| data_dir().join("mouse_recorder.db").to_string_lossy().to_string())
    }

    fn image_dir(&self) -> String {
        self.image_dir.as_ref()
            .map(|p| self.resolve(p).to_string_lossy().to_string())
            .unwrap_or_else(|| data_dir().to_string_lossy().to_string())
    }
}

impl Default for Config {
    fn default() -> Self {
        Self { db_path: None, image_dir: None }
    }
}

fn log_error(msg: &str) {
    let log_path = data_dir().join("error.log");
    if let Ok(mut f) = fs::OpenOptions::new().create(true).append(true).open(&log_path) {
        let _ = writeln!(f, "[{}] {}", Local::now().format("%Y-%m-%d %H:%M:%S"), msg);
    }
}

/// Load a Windows CJK UI font so Chinese labels are not tofu/garbled.
fn install_cjk_fonts(ctx: &egui::Context) {
    let mut candidates: Vec<PathBuf> = Vec::new();
    if let Ok(root) = std::env::var("SystemRoot") {
        let fonts = PathBuf::from(root).join("Fonts");
        for name in [
            "msyh.ttc",   // Microsoft YaHei
            "msyhbd.ttc",
            "msjh.ttc",   // Microsoft JhengHei
            "simhei.ttf",
            "simsun.ttc",
            "simkai.ttf",
            "Deng.ttf",
        ] {
            candidates.push(fonts.join(name));
        }
    }
    candidates.extend([
        PathBuf::from(r"C:\Windows\Fonts\msyh.ttc"),
        PathBuf::from(r"C:\Windows\Fonts\simhei.ttf"),
    ]);

    let mut loaded: Option<(String, Vec<u8>)> = None;
    for path in candidates {
        if let Ok(bytes) = fs::read(&path) {
            if bytes.len() > 1024 {
                let name = path
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .unwrap_or("cjk")
                    .to_string();
                log_error(&format!("CJK font loaded: {}", path.display()));
                loaded = Some((name, bytes));
                break;
            }
        }
    }

    let Some((font_name, bytes)) = loaded else {
        log_error("CJK font NOT found — Chinese UI may show as boxes");
        return;
    };

    let mut fonts = egui::FontDefinitions::default();
    fonts.font_data.insert(
        font_name.clone(),
        std::sync::Arc::new(egui::FontData::from_owned(bytes)),
    );
    // Prefer CJK font first so Chinese glyphs resolve; Latin still falls back.
    if let Some(fam) = fonts.families.get_mut(&egui::FontFamily::Proportional) {
        fam.insert(0, font_name.clone());
    }
    if let Some(fam) = fonts.families.get_mut(&egui::FontFamily::Monospace) {
        fam.insert(0, font_name);
    }
    ctx.set_fonts(fonts);
}

fn setup_panic_hook() {
    std::panic::set_hook(Box::new(|info| {
        let msg = match info.payload().downcast_ref::<&str>() {
            Some(s) => s.to_string(),
            None => match info.payload().downcast_ref::<String>() {
                Some(s) => s.clone(),
                None => format!("{:?}", info),
            },
        };
        let loc = info.location().map(|l| l.to_string()).unwrap_or_default();
        let full = if loc.is_empty() { msg } else { format!("{} at {}", msg, loc) };
        log_error(&full);
    }));
}

fn get_next_state_suffix(current: &str) -> String {
    let suffixes = ["-n", "-s", "-h", "-d", "-f", "-a", "-p"];
    for i in 0..suffixes.len() - 1 {
        if current == suffixes[i] {
            return suffixes[i + 1].to_string();
        }
    }
    "-n".to_string()
}

#[cfg(windows)]
fn screen_size_px() -> (u32, u32) {
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        GetSystemMetrics, SM_CXSCREEN, SM_CYSCREEN,
    };
    unsafe {
        let w = GetSystemMetrics(SM_CXSCREEN);
        let h = GetSystemMetrics(SM_CYSCREEN);
        if w > 0 && h > 0 {
            (w as u32, h as u32)
        } else {
            (1920, 1080)
        }
    }
}

#[cfg(not(windows))]
fn screen_size_px() -> (u32, u32) {
    (1920, 1080)
}

fn get_resolution() -> (u32, u32) {
    screen_size_px()
}

/// One-shot desktop capture via GDI BitBlt.
/// DXGI Desktop Duplication (xcap) is for continuous streaming — wrong tool for this job.
#[cfg(windows)]
fn grab_screen_gdi() -> Result<image::RgbaImage, String> {
    use windows_sys::Win32::Graphics::Gdi::{
        BitBlt, CreateCompatibleBitmap, CreateCompatibleDC, DeleteDC, DeleteObject, GetDC,
        GetDIBits, ReleaseDC, SelectObject, BITMAPINFO, BITMAPINFOHEADER, BI_RGB, DIB_RGB_COLORS,
        SRCCOPY,
    };

    let (w_u, h_u) = screen_size_px();
    let w = w_u as i32;
    let h = h_u as i32;

    unsafe {
        let hdc = GetDC(std::ptr::null_mut());
        if hdc.is_null() {
            return Err("GetDC failed".into());
        }
        let hdc_mem = CreateCompatibleDC(hdc);
        if hdc_mem.is_null() {
            ReleaseDC(std::ptr::null_mut(), hdc);
            return Err("CreateCompatibleDC failed".into());
        }
        let hbmp = CreateCompatibleBitmap(hdc, w, h);
        if hbmp.is_null() {
            DeleteDC(hdc_mem);
            ReleaseDC(std::ptr::null_mut(), hdc);
            return Err("CreateCompatibleBitmap failed".into());
        }
        let old = SelectObject(hdc_mem, hbmp as _);
        if BitBlt(hdc_mem, 0, 0, w, h, hdc, 0, 0, SRCCOPY) == 0 {
            SelectObject(hdc_mem, old);
            DeleteObject(hbmp as _);
            DeleteDC(hdc_mem);
            ReleaseDC(std::ptr::null_mut(), hdc);
            return Err("BitBlt failed".into());
        }

        let mut bmi = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: w,
                biHeight: -h, // top-down
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB,
                biSizeImage: 0,
                biXPelsPerMeter: 0,
                biYPelsPerMeter: 0,
                biClrUsed: 0,
                biClrImportant: 0,
            },
            bmiColors: [std::mem::zeroed()],
        };
        let mut buf = vec![0u8; (w as usize) * (h as usize) * 4];
        let lines = GetDIBits(
            hdc_mem,
            hbmp,
            0,
            h as u32,
            buf.as_mut_ptr() as *mut _,
            &mut bmi,
            DIB_RGB_COLORS,
        );
        SelectObject(hdc_mem, old);
        DeleteObject(hbmp as _);
        DeleteDC(hdc_mem);
        ReleaseDC(std::ptr::null_mut(), hdc);
        if lines == 0 {
            return Err("GetDIBits failed".into());
        }

        // BGRA -> RGBA. BI_RGB often leaves A=0; egui unmultiplied then draws black.
        for px in buf.chunks_exact_mut(4) {
            px.swap(0, 2);
            px[3] = 255;
        }
        image::RgbaImage::from_raw(w_u, h_u, buf)
            .ok_or_else(|| "Invalid image buffer".to_string())
    }
}

#[cfg(not(windows))]
fn grab_screen_gdi() -> Result<image::RgbaImage, String> {
    Err("Screen capture only supported on Windows".into())
}

fn grab_primary_monitor() -> Result<image::RgbaImage, String> {
    grab_screen_gdi()
}

fn find_element_id_by_name(conn: &Connection, name: &str) -> Option<i32> {
    conn.query_row(
        "SELECT id FROM elements WHERE name=?1 LIMIT 1",
        params![name],
        |r| r.get::<_, i32>(0),
    )
    .ok()
}

fn state_exists(conn: &Connection, element_id: i32, state: &str) -> bool {
    conn.query_row(
        "SELECT 1 FROM element_states WHERE element_id=?1 AND state_name=?2 LIMIT 1",
        params![element_id, state],
        |_| Ok(()),
    )
    .is_ok()
}

/// Replace `dest` with `temp` without deleting the old file until the new one is in place.
fn place_image_at(temp: &str, dest: &str) -> Result<String, String> {
    if temp == dest {
        return Ok(dest.to_string());
    }
    let dest_path = std::path::Path::new(dest);
    let temp_path = std::path::Path::new(temp);
    if let Some(parent) = dest_path.parent() {
        fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }

    let bak = dest_path.with_extension("png.bak");
    let had_dest = dest_path.exists();
    if had_dest {
        if bak.exists() {
            let _ = fs::remove_file(&bak);
        }
        fs::rename(dest_path, &bak).map_err(|e| format!("backup old image: {e}"))?;
    }

    let placed = match fs::rename(temp_path, dest_path) {
        Ok(()) => Ok(()),
        Err(_) => {
            fs::copy(temp_path, dest_path)
                .map(|_| {
                    let _ = fs::remove_file(temp_path);
                })
                .map_err(|e| format!("copy image: {e}"))
        }
    };

    match placed {
        Ok(()) => {
            if had_dest {
                let _ = fs::remove_file(&bak);
            }
            Ok(dest.to_string())
        }
        Err(e) => {
            if had_dest {
                let _ = fs::rename(&bak, dest_path);
            }
            Err(e)
        }
    }
}

fn overwrite_elem(
    conn: &Connection,
    id: i32,
    state: &str,
    x1: i32,
    y1: i32,
    x2: i32,
    y2: i32,
    img: &str,
    sw: u32,
    sh: u32,
) -> Result<(), String> {
    let ix = x1.min(x2);
    let iy = y1.min(y2);
    let iw = (x1 - x2).abs();
    let ih = (y1 - y2).abs();
    let cx = ix + iw / 2;
    let cy = iy + ih / 2;
    let (bx, by, bw, bh) =
        pad_roi_from_image(ix, iy, iw, ih, sw as i32, sh as i32, adaptive_roi_pad(iw, ih));
    let ts = Local::now().format("%Y%m%d_%H%M%S").to_string();
    let fname = std::path::Path::new(img)
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or(img)
        .to_string();

    conn.execute(
        "UPDATE elements SET center_x=?1, center_y=?2, bbox_x=?3, bbox_y=?4,
         bbox_width=?5, bbox_height=?6, screen_width=?7, screen_height=?8 WHERE id=?9",
        params![cx, cy, bx, by, bw, bh, sw, sh, id],
    )
    .map_err(|e| e.to_string())?;

    if state_exists(conn, id, state) {
        conn.execute(
            "UPDATE element_states SET image_path=?1, created_at=?2 WHERE element_id=?3 AND state_name=?4",
            params![fname, ts, id, state],
        )
        .map_err(|e| e.to_string())?;
    } else {
        conn.execute(
            "INSERT INTO element_states(element_id,state_name,image_path,is_primary,created_at) VALUES(?1,?2,?3,0,?4)",
            params![id, state, fname, ts],
        )
        .map_err(|e| e.to_string())?;
    }
    Ok(())
}

fn init_db(path: &str) -> Connection {
    if let Some(parent) = std::path::Path::new(path).parent() {
        if let Err(e) = fs::create_dir_all(parent) {
            eprintln!("Failed to create DB directory: {e}");
            log_error(&format!("create DB dir: {e}"));
        }
    }
    let conn = match Connection::open(path) {
        Ok(c) => c,
        Err(e) => {
            log_error(&format!("open DB {path}: {e}"));
            panic!("Failed to open DB at {path}: {e}");
        }
    };
    let _ = conn.execute_batch(
        "PRAGMA foreign_keys=ON;
         PRAGMA busy_timeout=5000;
         PRAGMA journal_mode=WAL;",
    );
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS elements (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            name TEXT NOT NULL,
            center_x INTEGER NOT NULL, center_y INTEGER NOT NULL,
            bbox_x INTEGER NOT NULL, bbox_y INTEGER NOT NULL,
            bbox_width INTEGER NOT NULL, bbox_height INTEGER NOT NULL,
            screen_width INTEGER NOT NULL, screen_height INTEGER NOT NULL,
            created_at TEXT NOT NULL
        );
        CREATE TABLE IF NOT EXISTS element_states (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            element_id INTEGER NOT NULL,
            state_name TEXT NOT NULL, image_path TEXT NOT NULL,
            is_primary BOOLEAN NOT NULL DEFAULT 0,
            created_at TEXT NOT NULL,
            FOREIGN KEY (element_id) REFERENCES elements(id)
        );"
    ).unwrap_or_else(|e| {
        log_error(&format!("DB init schema: {e}"));
        panic!("Fail DB init: {e}");
    });
    // Unique indexes may fail on legacy DBs with duplicates — keep app usable.
    if let Err(e) = conn.execute_batch(
        "CREATE UNIQUE INDEX IF NOT EXISTS idx_elements_name ON elements(name);
         CREATE UNIQUE INDEX IF NOT EXISTS idx_states_elem_name
             ON element_states(element_id, state_name);",
    ) {
        log_error(&format!("unique index skipped (dup names?): {e}"));
    }
    conn
}

#[derive(Clone)]
struct StateRec {
    id: i32,
    name: String,
    image_path: String,
    is_primary: bool,
}

#[derive(Clone)]
struct ElementRec {
    id: i32,
    name: String,
    cx: i32,
    cy: i32,
    bx: i32,
    by: i32,
    bw: i32,
    bh: i32,
    screen_w: i32,
    screen_h: i32,
    created_at: String,
    states: Vec<StateRec>,
}

impl ElementRec {
    fn primary_or_first_state(&self) -> Option<&StateRec> {
        self.states
            .iter()
            .find(|s| s.is_primary)
            .or_else(|| self.states.first())
    }

    fn find_state(&self, name: &str) -> Option<&StateRec> {
        self.states.iter().find(|s| s.name == name)
    }
}

fn load_elements(conn: &Connection) -> Vec<ElementRec> {
    let mut res = Vec::new();
    let Ok(mut st) = conn.prepare(
        "SELECT id,name,center_x,center_y,bbox_x,bbox_y,bbox_width,bbox_height,
                screen_width,screen_height,created_at
         FROM elements ORDER BY id",
    ) else {
        return res;
    };
    let Ok(rows) = st.query_map([], |r| {
        Ok((
            r.get::<_, i32>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, i32>(2)?,
            r.get::<_, i32>(3)?,
            r.get::<_, i32>(4)?,
            r.get::<_, i32>(5)?,
            r.get::<_, i32>(6)?,
            r.get::<_, i32>(7)?,
            r.get::<_, i32>(8)?,
            r.get::<_, i32>(9)?,
            r.get::<_, String>(10)?,
        ))
    }) else {
        return res;
    };
    for row in rows.flatten() {
        let (id, name, cx, cy, bx, by, bw, bh, sw, sh, created_at) = row;
        let mut states = Vec::new();
        if let Ok(mut s2) = conn.prepare(
            "SELECT id,state_name,image_path,is_primary FROM element_states WHERE element_id=?1 ORDER BY id",
        ) {
            if let Ok(r2) = s2.query_map(params![id], |r| {
                Ok(StateRec {
                    id: r.get(0)?,
                    name: r.get(1)?,
                    image_path: r.get(2)?,
                    is_primary: r.get(3)?,
                })
            }) {
                for st in r2.flatten() {
                    states.push(st);
                }
            }
        }
        res.push(ElementRec {
            id,
            name,
            cx,
            cy,
            bx,
            by,
            bw,
            bh,
            screen_w: sw,
            screen_h: sh,
            created_at,
            states,
        });
    }
    res
}

fn png_size(path: &std::path::Path) -> Option<(u32, u32)> {
    image::image_dimensions(path).ok()
}

fn set_primary_state(conn: &Connection, element_id: i32, state_name: &str) -> Result<(), String> {
    conn.execute(
        "UPDATE element_states SET is_primary=0 WHERE element_id=?1",
        params![element_id],
    )
    .map_err(|e| e.to_string())?;
    let n = conn
        .execute(
            "UPDATE element_states SET is_primary=1 WHERE element_id=?1 AND state_name=?2",
            params![element_id, state_name],
        )
        .map_err(|e| e.to_string())?;
    if n == 0 {
        return Err("State not found".into());
    }
    Ok(())
}

fn delete_element(conn: &Connection, image_dir: &str, id: i32) -> Result<(), String> {
    let mut paths = Vec::new();
    {
        let mut st = conn
            .prepare("SELECT image_path FROM element_states WHERE element_id=?1")
            .map_err(|e| e.to_string())?;
        let rows = st
            .query_map(params![id], |r| r.get::<_, String>(0))
            .map_err(|e| e.to_string())?;
        for p in rows.flatten() {
            paths.push(p);
        }
    }
    conn.execute("DELETE FROM element_states WHERE element_id=?1", params![id])
        .map_err(|e| e.to_string())?;
    conn.execute("DELETE FROM elements WHERE id=?1", params![id])
        .map_err(|e| e.to_string())?;
    for p in paths {
        let full = std::path::Path::new(image_dir).join(&p);
        let _ = fs::remove_file(full);
    }
    Ok(())
}

fn delete_state(conn: &Connection, image_dir: &str, element_id: i32, state_name: &str) -> Result<(), String> {
    let img: String = conn
        .query_row(
            "SELECT image_path FROM element_states WHERE element_id=?1 AND state_name=?2",
            params![element_id, state_name],
            |r| r.get(0),
        )
        .map_err(|e| e.to_string())?;
    conn.execute(
        "DELETE FROM element_states WHERE element_id=?1 AND state_name=?2",
        params![element_id, state_name],
    )
    .map_err(|e| e.to_string())?;
    let full = std::path::Path::new(image_dir).join(&img);
    let _ = fs::remove_file(full);
    // If no states left, remove element shell
    let left: i32 = conn
        .query_row(
            "SELECT COUNT(*) FROM element_states WHERE element_id=?1",
            params![element_id],
            |r| r.get(0),
        )
        .unwrap_or(0);
    if left == 0 {
        conn.execute("DELETE FROM elements WHERE id=?1", params![element_id])
            .map_err(|e| e.to_string())?;
    } else {
        // Ensure at least one primary
        let has_primary: i32 = conn
            .query_row(
                "SELECT COUNT(*) FROM element_states WHERE element_id=?1 AND is_primary=1",
                params![element_id],
                |r| r.get(0),
            )
            .unwrap_or(0);
        if has_primary == 0 {
            let _ = conn.execute(
                "UPDATE element_states SET is_primary=1 WHERE id=(
                    SELECT id FROM element_states WHERE element_id=?1 ORDER BY id LIMIT 1
                )",
                params![element_id],
            );
        }
    }
    Ok(())
}

fn update_element_geometry(
    conn: &Connection,
    id: i32,
    cx: i32,
    cy: i32,
    bx: i32,
    by: i32,
    bw: i32,
    bh: i32,
    screen_w: i32,
    screen_h: i32,
) -> Result<(), String> {
    let bw = bw.max(1);
    let bh = bh.max(1);
    conn.execute(
        "UPDATE elements SET center_x=?1, center_y=?2, bbox_x=?3, bbox_y=?4,
         bbox_width=?5, bbox_height=?6, screen_width=?7, screen_height=?8 WHERE id=?9",
        params![cx, cy, bx, by, bw, bh, screen_w, screen_h, id],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

fn rename_element(
    conn: &Connection,
    image_dir: &str,
    id: i32,
    old_name: &str,
    new_name: &str,
) -> Result<(), String> {
    let new_name = new_name.trim();
    if new_name.is_empty() {
        return Err("名称不能为空".into());
    }
    if new_name == old_name {
        return Ok(());
    }
    if let Some(other) = find_element_id_by_name(conn, new_name) {
        if other != id {
            return Err(format!("名称「{}」已被 #{} 占用", new_name, other));
        }
    }
    let mut st = conn
        .prepare("SELECT state_name, image_path FROM element_states WHERE element_id=?1")
        .map_err(|e| e.to_string())?;
    let rows: Vec<(String, String)> = st
        .query_map(params![id], |r| Ok((r.get(0)?, r.get(1)?)))
        .map_err(|e| e.to_string())?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())?;
    drop(st);

    for (sn, path) in &rows {
        let old_path = std::path::Path::new(image_dir).join(path);
        let new_fname = format!("{}_{}.png", new_name, sn);
        let new_path = std::path::Path::new(image_dir).join(&new_fname);
        if old_path.exists() {
            if new_path.exists() && new_path != old_path {
                return Err(format!("目标图片已存在: {}", new_fname));
            }
            if let Err(e) = fs::rename(&old_path, &new_path) {
                fs::copy(&old_path, &new_path).map_err(|e2| {
                    format!("rename failed ({e}); copy also failed: {e2}")
                })?;
                let _ = fs::remove_file(&old_path);
            }
        }
        conn.execute(
            "UPDATE element_states SET image_path=?1 WHERE element_id=?2 AND state_name=?3",
            params![new_fname, id, sn],
        )
        .map_err(|e| e.to_string())?;
    }
    conn.execute(
        "UPDATE elements SET name=?1 WHERE id=?2",
        params![new_name, id],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

fn rescale_element_geometry(
    conn: &Connection,
    e: &ElementRec,
    new_sw: i32,
    new_sh: i32,
) -> Result<(), String> {
    if e.screen_w <= 0 || e.screen_h <= 0 {
        return Err("录制分辨率为 0，无法缩放".into());
    }
    if e.screen_w == new_sw && e.screen_h == new_sh {
        return Ok(());
    }
    let cx = matchutil::scale_i(e.cx, e.screen_w, new_sw);
    let cy = matchutil::scale_i(e.cy, e.screen_h, new_sh);
    let bx = matchutil::scale_i(e.bx, e.screen_w, new_sw);
    let by = matchutil::scale_i(e.by, e.screen_h, new_sh);
    let bw = matchutil::scale_i(e.bw, e.screen_w, new_sw).max(1);
    let bh = matchutil::scale_i(e.bh, e.screen_h, new_sh).max(1);
    update_element_geometry(conn, e.id, cx, cy, bx, by, bw, bh, new_sw, new_sh)
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum RoiHit {
    None,
    Center,
    Move,
    N,
    S,
    E,
    W,
    NE,
    NW,
    SE,
    SW,
}

#[derive(Clone, Copy)]
struct RoiDrag {
    hit: RoiHit,
    start_mx: i32,
    start_my: i32,
    start_bx: i32,
    start_by: i32,
    start_bw: i32,
    start_bh: i32,
    start_cx: i32,
    start_cy: i32,
}

const ROI_HANDLE_PX: i32 = 10;
const CENTER_HIT_PX: i32 = 14;
const ROI_MIN_SIZE: i32 = 4;

/// First-principles ROI margin for template matching (NCC / convolution search).
///
/// Match slides a W×H template inside ROI. Valid placements ≈
/// (ROI_W−W+1)×(ROI_H−H+1). If ROI==image → only 1 placement → zero tolerance.
///
/// Pad must cover expected position error δ (DPI rounding, AA, layout jitter),
/// but not so large that distractors raise false peaks / cost.
///
///   pad = clamp(max(FLOOR, 0.12·min(W,H)), FLOOR, CAP)
///
/// FLOOR≈12px ≈ typical UI jitter; CAP≈40px bounds large templates.
fn adaptive_roi_pad(iw: i32, ih: i32) -> i32 {
    const FLOOR: i32 = 12;
    const CAP: i32 = 40;
    let m = iw.min(ih).max(1) as f32;
    let rel = (m * 0.12).round() as i32;
    rel.max(FLOOR).min(CAP)
}

/// Template image rect on screen — anchored to click center.
fn image_rect_anchored(
    cx: i32,
    cy: i32,
    iw: i32,
    ih: i32,
    screen_w: i32,
    screen_h: i32,
) -> (i32, i32, i32, i32) {
    let iw = iw.max(1).min(screen_w.max(1));
    let ih = ih.max(1).min(screen_h.max(1));
    let ix = (cx - iw / 2).clamp(0, (screen_w - iw).max(0));
    let iy = (cy - ih / 2).clamp(0, (screen_h - ih).max(0));
    (ix, iy, iw, ih)
}

/// Expand image rect by `pad` to form search ROI (clamped to screen).
fn pad_roi_from_image(
    ix: i32,
    iy: i32,
    iw: i32,
    ih: i32,
    screen_w: i32,
    screen_h: i32,
    pad: i32,
) -> (i32, i32, i32, i32) {
    let bx = (ix - pad).max(0);
    let by = (iy - pad).max(0);
    let x2 = (ix + iw + pad).min(screen_w.max(ix + iw));
    let y2 = (iy + ih + pad).min(screen_h.max(iy + ih));
    let bw = (x2 - bx).max(iw).max(ROI_MIN_SIZE);
    let bh = (y2 - by).max(ih).max(ROI_MIN_SIZE);
    (bx, by, bw, bh)
}

/// Grow ROI so it fully contains the image rect (keeps ROI ≥ image).
fn ensure_roi_contains_image(
    bx: i32,
    by: i32,
    bw: i32,
    bh: i32,
    ix: i32,
    iy: i32,
    iw: i32,
    ih: i32,
    screen_w: i32,
    screen_h: i32,
) -> (i32, i32, i32, i32) {
    let x2 = bx + bw;
    let y2 = by + bh;
    let nx1 = bx.min(ix);
    let ny1 = by.min(iy);
    let nx2 = x2.max(ix + iw);
    let ny2 = y2.max(iy + ih);
    let bx = nx1.clamp(0, (screen_w - ROI_MIN_SIZE).max(0));
    let by = ny1.clamp(0, (screen_h - ROI_MIN_SIZE).max(0));
    let bw = (nx2 - bx).min(screen_w - bx).max(iw).max(ROI_MIN_SIZE);
    let bh = (ny2 - by).min(screen_h - by).max(ih).max(ROI_MIN_SIZE);
    (bx, by, bw, bh)
}

fn roi_hit_test(mx: i32, my: i32, bx: i32, by: i32, bw: i32, bh: i32, cx: i32, cy: i32) -> RoiHit {
    // Center has priority — click target is independent of ROI geometric center.
    if (mx - cx).abs() <= CENTER_HIT_PX && (my - cy).abs() <= CENTER_HIT_PX {
        return RoiHit::Center;
    }

    let x2 = bx + bw;
    let y2 = by + bh;
    let m = ROI_HANDLE_PX;
    let near_l = (mx - bx).abs() <= m;
    let near_r = (mx - x2).abs() <= m;
    let near_t = (my - by).abs() <= m;
    let near_b = (my - y2).abs() <= m;
    let in_x = mx >= bx - m && mx <= x2 + m;
    let in_y = my >= by - m && my <= y2 + m;
    let inside = mx >= bx && mx <= x2 && my >= by && my <= y2;

    if near_t && near_l && in_x && in_y {
        RoiHit::NW
    } else if near_t && near_r && in_x && in_y {
        RoiHit::NE
    } else if near_b && near_l && in_x && in_y {
        RoiHit::SW
    } else if near_b && near_r && in_x && in_y {
        RoiHit::SE
    } else if near_t && mx >= bx && mx <= x2 {
        RoiHit::N
    } else if near_b && mx >= bx && mx <= x2 {
        RoiHit::S
    } else if near_l && my >= by && my <= y2 {
        RoiHit::W
    } else if near_r && my >= by && my <= y2 {
        RoiHit::E
    } else if inside {
        RoiHit::Move
    } else {
        RoiHit::None
    }
}

fn apply_roi_drag(
    drag: &RoiDrag,
    mx: i32,
    my: i32,
    screen_w: i32,
    screen_h: i32,
) -> (i32, i32, i32, i32, i32, i32) {
    let dx = mx - drag.start_mx;
    let dy = my - drag.start_my;
    let mut bx = drag.start_bx;
    let mut by = drag.start_by;
    let mut bw = drag.start_bw;
    let mut bh = drag.start_bh;
    let mut cx = drag.start_cx;
    let mut cy = drag.start_cy;
    let mut x2 = bx + bw;
    let mut y2 = by + bh;

    match drag.hit {
        RoiHit::Center => {
            cx = drag.start_cx + dx;
            cy = drag.start_cy + dy;
        }
        RoiHit::Move => {
            bx += dx;
            by += dy;
            x2 = bx + bw;
            y2 = by + bh;
            cx = drag.start_cx + dx;
            cy = drag.start_cy + dy;
        }
        RoiHit::E => {
            x2 = drag.start_bx + drag.start_bw + dx;
        }
        RoiHit::W => {
            bx = drag.start_bx + dx;
        }
        RoiHit::S => {
            y2 = drag.start_by + drag.start_bh + dy;
        }
        RoiHit::N => {
            by = drag.start_by + dy;
        }
        RoiHit::SE => {
            x2 = drag.start_bx + drag.start_bw + dx;
            y2 = drag.start_by + drag.start_bh + dy;
        }
        RoiHit::SW => {
            bx = drag.start_bx + dx;
            y2 = drag.start_by + drag.start_bh + dy;
        }
        RoiHit::NE => {
            x2 = drag.start_bx + drag.start_bw + dx;
            by = drag.start_by + dy;
        }
        RoiHit::NW => {
            bx = drag.start_bx + dx;
            by = drag.start_by + dy;
        }
        RoiHit::None => {}
    }

    if drag.hit != RoiHit::Center {
        if x2 < bx {
            std::mem::swap(&mut bx, &mut x2);
        }
        if y2 < by {
            std::mem::swap(&mut by, &mut y2);
        }
        bw = (x2 - bx).max(ROI_MIN_SIZE);
        bh = (y2 - by).max(ROI_MIN_SIZE);
        bx = bx.clamp(0, (screen_w - ROI_MIN_SIZE).max(0));
        by = by.clamp(0, (screen_h - ROI_MIN_SIZE).max(0));
        bw = bw.min(screen_w - bx).max(ROI_MIN_SIZE);
        bh = bh.min(screen_h - by).max(ROI_MIN_SIZE);
    }

    cx = cx.clamp(0, (screen_w - 1).max(0));
    cy = cy.clamp(0, (screen_h - 1).max(0));
    (bx, by, bw, bh, cx, cy)
}

fn roi_cursor(hit: RoiHit) -> egui::CursorIcon {
    match hit {
        RoiHit::Center => egui::CursorIcon::AllScroll,
        RoiHit::Move => egui::CursorIcon::Grab,
        RoiHit::N | RoiHit::S => egui::CursorIcon::ResizeVertical,
        RoiHit::E | RoiHit::W => egui::CursorIcon::ResizeHorizontal,
        RoiHit::NE | RoiHit::SW => egui::CursorIcon::ResizeNeSw,
        RoiHit::NW | RoiHit::SE => egui::CursorIcon::ResizeNwSe,
        RoiHit::None => egui::CursorIcon::Default,
    }
}

fn export_csv(conn: &Connection, name: &str, cfg: &Config) -> Result<String, String> {
    let ts = Local::now().format("%Y%m%d_%H%M%S");
    let out_dir = cfg.image_dir();
    let path = std::path::Path::new(&out_dir).join(format!("{}_{}.csv", name, ts));
    let path_s = path.to_string_lossy().to_string();
    let mut f = fs::File::create(&path).map_err(|e| e.to_string())?;
    f.write_all(b"\xEF\xBB\xBF").map_err(|e| e.to_string())?;
    writeln!(f, "id,x,y,description,template_path,original_width,original_height").map_err(|e| e.to_string())?;
    let mut st = conn.prepare(
        "SELECT e.id,e.name,e.center_x,e.center_y,es.image_path,e.screen_width,e.screen_height
         FROM elements e JOIN element_states es ON e.id=es.element_id AND es.is_primary=1 ORDER BY e.id"
    ).map_err(|e| e.to_string())?;
    for row in st.query_map([], |r| Ok((r.get::<_,i32>(0)?, r.get::<_,String>(1)?, r.get::<_,i32>(2)?, r.get::<_,i32>(3)?, r.get::<_,String>(4)?, r.get::<_,i32>(5)?, r.get::<_,i32>(6)?))).map_err(|e| e.to_string())?.flatten() {
        let (id, name, cx, cy, img, sw, sh) = row;
        let img_path = std::path::Path::new(&out_dir).join(&img).to_string_lossy().to_string();
        writeln!(f, "{},{},{},\"{}\",\"{}\",{},{}", id, cx, cy, name.replace('"', "\"\""), img_path.replace('"', "\"\""), sw, sh).map_err(|e| e.to_string())?;
    }
    Ok(path_s)
}

fn create_elem(conn: &Connection, name: &str, state: &str, x1: i32, y1: i32, x2: i32, y2: i32, img: &str, sw: u32, sh: u32) -> Result<i32, String> {
    let ix = x1.min(x2);
    let iy = y1.min(y2);
    let iw = (x1 - x2).abs();
    let ih = (y1 - y2).abs();
    let cx = ix + iw / 2;
    let cy = iy + ih / 2;
    let (bx, by, bw, bh) =
        pad_roi_from_image(ix, iy, iw, ih, sw as i32, sh as i32, adaptive_roi_pad(iw, ih));
    let ts = Local::now().format("%Y%m%d_%H%M%S").to_string();
    let fname = std::path::Path::new(img).file_name()
        .and_then(|s| s.to_str())
        .unwrap_or(img)
        .to_string();
    let tx = conn.unchecked_transaction().map_err(|e| e.to_string())?;
    tx.execute(
        "INSERT INTO elements(name,center_x,center_y,bbox_x,bbox_y,bbox_width,bbox_height,screen_width,screen_height,created_at) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)",
        params![name, cx, cy, bx, by, bw, bh, sw, sh, ts]
    ).map_err(|e| e.to_string())?;
    let id = tx.last_insert_rowid() as i32;
    tx.execute(
        "INSERT INTO element_states(element_id,state_name,image_path,is_primary,created_at) VALUES(?1,?2,?3,1,?4)",
        params![id, state, fname, ts]
    ).map_err(|e| e.to_string())?;
    tx.commit().map_err(|e| e.to_string())?;
    Ok(id)
}

enum AppMode {
    Normal,
    Capturing,
    InputName(i32, i32, i32, i32, String),
    InputState(i32, i32, i32, i32, String, String, String),
    /// Name/image already exists — ask before overwriting DB + PNG.
    ConfirmOverwrite {
        x1: i32,
        y1: i32,
        x2: i32,
        y2: i32,
        temp_img: String,
        name: String,
        state_name: String,
        existing_id: Option<i32>,
        is_add_state: bool,
        message: String,
    },
    /// Inspect selected element's center / ROI / image size + preview.
    ViewDetail,
    /// Confirm deleting element or a single state.
    ConfirmDelete {
        element_id: i32,
        /// None = delete whole element; Some = delete one state.
        state_name: Option<String>,
        message: String,
    },
    /// Fullscreen overlay: desktop snapshot + ROI/center (no window transparency).
    /// `editable=false` = visual locate (view); `true` = drag/nudge and save.
    EditRoi {
        element_id: i32,
        editable: bool,
        /// Esc/save returns to ViewDetail when entered from details panel.
        return_to_detail: bool,
        bx: i32,
        by: i32,
        bw: i32,
        bh: i32,
        cx: i32,
        cy: i32,
        /// Template PNG size (image region); ROI must stay larger.
        img_w: i32,
        img_h: i32,
        orig_bx: i32,
        orig_by: i32,
        orig_bw: i32,
        orig_bh: i32,
        orig_cx: i32,
        orig_cy: i32,
        drag: Option<RoiDrag>,
    },
    /// Fullscreen: template match score + hit box vs recorded center/ROI.
    VerifyMatch {
        element_id: i32,
        score: f32,
        has_hit: bool,
        hit_x: i32,
        hit_y: i32,
        hit_cx: i32,
        hit_cy: i32,
        tw: i32,
        th: i32,
        bx: i32,
        by: i32,
        bw: i32,
        bh: i32,
        rec_cx: i32,
        rec_cy: i32,
        used_roi: bool,
    },
    /// Rename selected element (with collision check).
    InputRename,
    /// Side-by-side state image compare.
    CompareStates,
    InputExport,
}

enum CaptureMsg {
    Down(f64,f64),
    Up(f64,f64),
    Move(f64,f64),
}

fn start_listener(flag: Arc<AtomicBool>, tx: Sender<CaptureMsg>) {
    thread::spawn(move || {
        let last_pos: Arc<Mutex<(f64, f64)>> = Arc::new(Mutex::new((0.0, 0.0)));
        let last_pos_cb = Arc::clone(&last_pos);
        let cb = move |event: Event| -> Option<Event> {
            let capturing = flag.load(Ordering::Relaxed);
            match event.event_type {
                EventType::MouseMove { x, y } => {
                    if let Ok(mut pos) = last_pos_cb.lock() {
                        *pos = (x, y);
                    }
                    if capturing {
                        tx.send(CaptureMsg::Move(x, y)).ok();
                    }
                    Some(event)
                }
                EventType::ButtonPress(Button::Left) => {
                    if capturing {
                        if let Ok(pos) = last_pos_cb.lock() {
                            tx.send(CaptureMsg::Down(pos.0, pos.1)).ok();
                        }
                        None
                    } else {
                        Some(event)
                    }
                }
                EventType::ButtonRelease(Button::Left) => {
                    if capturing {
                        if let Ok(pos) = last_pos_cb.lock() {
                            tx.send(CaptureMsg::Up(pos.0, pos.1)).ok();
                        }
                        None
                    } else {
                        Some(event)
                    }
                }
                _ => Some(event),
            }
        };
        if let Err(e) = grab(cb) {
            log_error(&format!("rdev grab error: {:?}", e));
        }
    });
}

#[cfg(windows)]
fn win32_hwnd(frame: &eframe::Frame) -> Option<windows_sys::Win32::Foundation::HWND> {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    let handle = frame.window_handle().ok()?;
    match handle.as_raw() {
        RawWindowHandle::Win32(h) => Some(h.hwnd.get() as _),
        _ => None,
    }
}

/// Move window off-screen WITHOUT SW_HIDE — keeps the winit/egui event loop alive.
/// Fallback only when WDA_EXCLUDEFROMCAPTURE is unavailable.
#[cfg(windows)]
fn park_window_offscreen(frame: &eframe::Frame) -> bool {
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        SetWindowPos, SWP_NOACTIVATE, SWP_NOZORDER, SWP_SHOWWINDOW,
    };
    if let Some(hwnd) = win32_hwnd(frame) {
        unsafe {
            SetWindowPos(
                hwnd,
                std::ptr::null_mut(),
                -32000,
                -32000,
                2,
                2,
                SWP_NOZORDER | SWP_NOACTIVATE | SWP_SHOWWINDOW,
            );
        }
        true
    } else {
        false
    }
}

/// Exclude this window from screen capture (Win10 2004+).
/// First principles: don't hide/park — tell the OS to omit us from BitBlt/DXGI.
#[cfg(windows)]
fn set_exclude_from_capture(frame: &eframe::Frame, exclude: bool) -> bool {
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        SetWindowDisplayAffinity, WDA_EXCLUDEFROMCAPTURE, WDA_NONE,
    };
    if let Some(hwnd) = win32_hwnd(frame) {
        let affinity = if exclude {
            WDA_EXCLUDEFROMCAPTURE
        } else {
            WDA_NONE
        };
        unsafe { SetWindowDisplayAffinity(hwnd, affinity) != 0 }
    } else {
        false
    }
}

/// Win32: z-order / visibility only. Size & position owned by egui ViewportCommand
/// (logical points) to avoid DPI physical/logical fights.
#[cfg(windows)]
fn place_window_fullscreen(frame: &eframe::Frame) -> bool {
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        SetWindowPos, ShowWindow, HWND_TOPMOST, SWP_NOMOVE, SWP_NOSIZE, SWP_SHOWWINDOW, SW_RESTORE,
        SW_SHOW,
    };
    if let Some(hwnd) = win32_hwnd(frame) {
        unsafe {
            ShowWindow(hwnd, SW_SHOW);
            ShowWindow(hwnd, SW_RESTORE);
            SetWindowPos(
                hwnd,
                HWND_TOPMOST,
                0,
                0,
                0,
                0,
                SWP_NOMOVE | SWP_NOSIZE | SWP_SHOWWINDOW,
            );
        }
        true
    } else {
        false
    }
}

#[cfg(windows)]
fn place_window_normal(frame: &eframe::Frame) -> bool {
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        SetWindowPos, ShowWindow, HWND_NOTOPMOST, SWP_NOMOVE, SWP_NOSIZE, SWP_SHOWWINDOW, SW_RESTORE,
        SW_SHOW,
    };
    if let Some(hwnd) = win32_hwnd(frame) {
        unsafe {
            ShowWindow(hwnd, SW_SHOW);
            ShowWindow(hwnd, SW_RESTORE);
            SetWindowPos(
                hwnd,
                HWND_NOTOPMOST,
                0,
                0,
                0,
                0,
                SWP_NOMOVE | SWP_NOSIZE | SWP_SHOWWINDOW,
            );
        }
        true
    } else {
        false
    }
}

#[cfg(not(windows))]
fn park_window_offscreen(_frame: &eframe::Frame) -> bool { false }
#[cfg(not(windows))]
fn set_exclude_from_capture(_frame: &eframe::Frame, _exclude: bool) -> bool { false }
#[cfg(not(windows))]
fn place_window_fullscreen(_frame: &eframe::Frame) -> bool { false }
#[cfg(not(windows))]
fn place_window_normal(_frame: &eframe::Frame) -> bool { false }

fn escape_key_down() -> bool {
    use device_query::{DeviceQuery, DeviceState, Keycode};
    DeviceState::new().get_keys().contains(&Keycode::Escape)
}

struct RecorderApp {
    conn: Connection,
    config: Config,
    elements: Vec<ElementRec>,
    last_id: Option<i32>,
    last_element_name: Option<String>,
    last_state_name: Option<String>,
    is_add_state_mode: bool,
    sw: u32, sh: u32,
    scale: f32,
    mode: AppMode,
    input: String,
    /// Case-insensitive substring filter for the element list.
    name_filter: String,
    /// Scene group (`name` prefix before `/`); empty or "全部" = all.
    group_filter: String,
    /// Re-record current element+state: skip name UI, force overwrite.
    recapture: bool,
    /// Loaded textures for CompareStates: (state label, texture).
    compare_tex: Vec<(String, egui::TextureHandle)>,
    status: String,
    rx: mpsc::Receiver<CaptureMsg>,
    capture_flag: Arc<AtomicBool>,
    drag_start: Option<(i32,i32)>,
    drag_current: Option<(i32,i32)>,
    mouse_pos: Option<(i32,i32)>,
    capture_started_at: Option<std::time::Instant>,
    forced_state_name: Option<String>,
    image_dir: String,
    pre_capture: Option<(egui::TextureHandle, image::RgbaImage)>,
    /// Preview texture for list/detail (path key to avoid reload churn).
    preview: Option<(String, egui::TextureHandle)>,
    /// Template PNG drawn inside image-region on locate/edit overlay.
    overlay_template: Option<egui::TextureHandle>,
}

impl RecorderApp {
    fn refresh(&mut self) {
        self.elements = load_elements(&self.conn);
        self.sync_last_selection();
    }

    fn sync_last_selection(&mut self) {
        if let Some(id) = self.last_id {
            if let Some(e) = self.elements.iter().find(|el| el.id == id) {
                self.last_element_name = Some(e.name.clone());
                if self
                    .last_state_name
                    .as_ref()
                    .map(|n| e.find_state(n).is_some())
                    .unwrap_or(false)
                {
                    // keep current state selection
                } else {
                    self.last_state_name = e.primary_or_first_state().map(|s| s.name.clone());
                }
                return;
            }
        }
        if self.last_id.is_some() && !self.elements.iter().any(|e| Some(e.id) == self.last_id) {
            self.last_id = self.elements.last().map(|e| e.id);
            if let Some(e) = self.elements.last() {
                self.last_element_name = Some(e.name.clone());
                self.last_state_name = e.primary_or_first_state().map(|s| s.name.clone());
            } else {
                self.last_element_name = None;
                self.last_state_name = None;
            }
        }
    }

    fn selected_element(&self) -> Option<&ElementRec> {
        self.last_id
            .and_then(|id| self.elements.iter().find(|e| e.id == id))
    }

    fn selected_state(&self) -> Option<&StateRec> {
        let e = self.selected_element()?;
        if let Some(n) = &self.last_state_name {
            e.find_state(n).or_else(|| e.primary_or_first_state())
        } else {
            e.primary_or_first_state()
        }
    }

    /// Indices into `self.elements` matching name + group filters.
    fn filtered_indices(&self) -> Vec<usize> {
        let q = self.name_filter.trim().to_lowercase();
        let g = self.group_filter.trim();
        let all_groups = g.is_empty() || g == "全部";
        self.elements
            .iter()
            .enumerate()
            .filter(|(_, e)| {
                let name_ok = q.is_empty() || e.name.to_lowercase().contains(&q);
                let group_ok = all_groups || matchutil::element_group(&e.name) == g;
                name_ok && group_ok
            })
            .map(|(i, _)| i)
            .collect()
    }

    fn health_for(&self, e: &ElementRec, dup: &std::collections::HashMap<String, usize>) -> matchutil::HealthReport {
        let states: Vec<(bool, &str, std::path::PathBuf)> = e
            .states
            .iter()
            .map(|s| {
                (
                    s.is_primary,
                    s.name.as_str(),
                    std::path::Path::new(&self.image_dir).join(&s.image_path),
                )
            })
            .collect();
        let refs: Vec<(bool, &str, &std::path::Path)> = states
            .iter()
            .map(|(p, n, path)| (*p, *n, path.as_path()))
            .collect();
        let dup_count = dup.get(&e.name).copied().unwrap_or(1);
        matchutil::assess_element(
            &e.name,
            e.cx,
            e.cy,
            e.bw,
            e.bh,
            e.screen_w,
            e.screen_h,
            self.sw as i32,
            self.sh as i32,
            &refs,
            dup_count,
        )
    }

    fn unique_groups(&self) -> Vec<String> {
        let mut g: Vec<String> = self
            .elements
            .iter()
            .map(|e| matchutil::element_group(&e.name).to_string())
            .collect();
        g.sort();
        g.dedup();
        g
    }

    fn navigate_filtered(&mut self, delta: i32) {
        let idxs = self.filtered_indices();
        if idxs.is_empty() {
            return;
        }
        let cur_pos = self
            .last_id
            .and_then(|id| idxs.iter().position(|&i| self.elements[i].id == id));
        let next = match cur_pos {
            Some(p) => {
                let n = idxs.len() as i32;
                (((p as i32) + delta).rem_euclid(n)) as usize
            }
            None => {
                if delta >= 0 {
                    0
                } else {
                    idxs.len() - 1
                }
            }
        };
        self.last_id = Some(self.elements[idxs[next]].id);
        self.sync_last_selection();
        self.preview = None;
    }

    fn ensure_preview(&mut self, ctx: &egui::Context) {
        let Some((state_id, image_name)) = self.selected_state().map(|s| (s.id, s.image_path.clone())) else {
            self.preview = None;
            return;
        };
        let path = std::path::Path::new(&self.image_dir)
            .join(&image_name)
            .to_string_lossy()
            .to_string();
        if self
            .preview
            .as_ref()
            .map(|(p, _)| p == &path)
            .unwrap_or(false)
        {
            return;
        }
        self.preview = None;
        let Ok(img) = image::open(&path) else {
            return;
        };
        let rgba = img.to_rgba8();
        let size = [rgba.width() as usize, rgba.height() as usize];
        let color = egui::ColorImage::from_rgba_unmultiplied(size, rgba.as_raw());
        let tex = ctx.load_texture(
            format!("preview-{}", state_id),
            color,
            egui::TextureOptions::NEAREST,
        );
        self.preview = Some((path, tex));
    }

    /// Draw selected-state image preview into `ui` (loads texture if needed).
    fn show_preview_panel(&mut self, ctx: &egui::Context, ui: &mut egui::Ui, max_w: f32, max_h: f32) {
        self.ensure_preview(ctx);
        ui.group(|ui| {
            ui.set_min_width(max_w.min(320.0));
            ui.label(egui::RichText::new("Preview 预览").strong());
            if let Some(e) = self.selected_element() {
                let st = self.selected_state();
                ui.label(format!(
                    "#{}  {}  {}",
                    e.id,
                    e.name,
                    st.map(|s| s.name.as_str()).unwrap_or("-")
                ));
                let img_name = st.map(|s| s.image_path.as_str()).unwrap_or("");
                if !img_name.is_empty() {
                    ui.label(
                        egui::RichText::new(img_name)
                            .small()
                            .color(Color32::GRAY),
                    );
                }
            } else {
                ui.colored_label(Color32::GRAY, "未选择元件");
                return;
            }

            if let Some((_, tex)) = &self.preview {
                let tw = tex.size()[0] as f32;
                let th = tex.size()[1] as f32;
                let scale = (max_w / tw).min(max_h / th).min(4.0);
                let scale = if scale.is_finite() && scale > 0.0 {
                    scale
                } else {
                    1.0
                };
                ui.add(
                    egui::Image::new((tex.id(), egui::vec2(tw * scale, th * scale)))
                        .bg_fill(Color32::from_gray(30)),
                );
                ui.label(
                    egui::RichText::new(format!("{}×{} px", tw as i32, th as i32))
                        .small()
                        .color(Color32::GRAY),
                );
            } else if let Some(st) = self.selected_state() {
                let path = std::path::Path::new(&self.image_dir).join(&st.image_path);
                ui.colored_label(
                    Color32::RED,
                    format!("无法加载: {}", path.display()),
                );
            }
        });
    }

    fn location_summary_text(&self) -> String {
        let Some(e) = self.selected_element() else {
            return String::new();
        };
        let st = self.selected_state();
        let img_path = st
            .map(|s| {
                std::path::Path::new(&self.image_dir)
                    .join(&s.image_path)
                    .to_string_lossy()
                    .to_string()
            })
            .unwrap_or_default();
        let (iw, ih) = png_size(std::path::Path::new(&img_path)).unwrap_or((0, 0));
        let (ix, iy, _, _) =
            image_rect_anchored(e.cx, e.cy, iw as i32, ih as i32, e.screen_w, e.screen_h);
        format!(
            "name={}\ncenter=({}, {})\nimage_rect=[{}, {}, {}, {}]  (x,y,w,h)\nroi=[{}, {}, {}, {}]  (x,y,w,h)\nroi_end=({}, {})\nimage_size={}x{}\nscreen={}x{}\nstate={}\nimage={}",
            e.name,
            e.cx,
            e.cy,
            ix,
            iy,
            iw,
            ih,
            e.bx,
            e.by,
            e.bw,
            e.bh,
            e.bx + e.bw,
            e.by + e.bh,
            iw,
            ih,
            e.screen_w,
            e.screen_h,
            st.map(|s| s.name.as_str()).unwrap_or("-"),
            img_path,
        )
    }

    /// Grab desktop into `pre_capture` (same path as Capturing). Avoids broken
    /// Windows window-transparency which otherwise paints the overlay all black.
    fn capture_desktop_bg(&mut self, ctx: &egui::Context, frame: &eframe::Frame) -> Result<(), String> {
        let excluded = set_exclude_from_capture(frame, true);
        if !excluded {
            log_error("capture_desktop_bg: exclude failed, parking off-screen");
            let _ = park_window_offscreen(frame);
            thread::sleep(Duration::from_millis(60));
        }
        let img = grab_primary_monitor()?;
        self.sw = img.width();
        self.sh = img.height();
        self.scale = ctx.pixels_per_point().max(0.1);
        self.pre_capture = None;
        let size = [img.width() as usize, img.height() as usize];
        let color_img = egui::ColorImage::from_rgba_unmultiplied(size, img.as_raw());
        let tex = ctx.load_texture(
            format!("locate-bg-{}", Local::now().format("%H%M%S%3f")),
            color_img,
            egui::TextureOptions::NEAREST,
        );
        self.pre_capture = Some((tex, img));
        Ok(())
    }

    fn begin_view_locate(&mut self, ctx: &egui::Context, frame: &eframe::Frame) {
        self.begin_roi_overlay(ctx, frame, false);
    }

    fn begin_edit_roi(&mut self, ctx: &egui::Context, frame: &eframe::Frame) {
        self.begin_roi_overlay(ctx, frame, true);
    }

    /// Load template texture for current selection; returns (img_w, img_h).
    fn load_overlay_template(&mut self, ctx: &egui::Context, element_id: i32) -> (i32, i32) {
        let img_path = self
            .selected_state()
            .map(|s| {
                std::path::Path::new(&self.image_dir)
                    .join(&s.image_path)
                    .to_string_lossy()
                    .to_string()
            })
            .unwrap_or_default();
        let (iw_u, ih_u) = png_size(std::path::Path::new(&img_path)).unwrap_or((0, 0));
        let img_w = iw_u.max(1) as i32;
        let img_h = ih_u.max(1) as i32;
        self.overlay_template = None;
        if let Ok(img) = image::open(&img_path) {
            let rgba = img.to_rgba8();
            let size = [rgba.width() as usize, rgba.height() as usize];
            let color = egui::ColorImage::from_rgba_unmultiplied(size, rgba.as_raw());
            self.overlay_template = Some(ctx.load_texture(
                format!(
                    "tpl-{}-{}",
                    element_id,
                    Local::now().format("%H%M%S%3f")
                ),
                color,
                egui::TextureOptions::NEAREST,
            ));
        }
        (img_w, img_h)
    }

    fn geometry_for_element(
        &self,
        e: &ElementRec,
        img_w: i32,
        img_h: i32,
    ) -> (i32, i32, i32, i32, i32, i32) {
        let screen_w = self.sw as i32;
        let screen_h = self.sh as i32;
        let (ix, iy, iw, ih) =
            image_rect_anchored(e.cx, e.cy, img_w, img_h, screen_w, screen_h);
        let mut bx = e.bx;
        let mut by = e.by;
        let mut bw = e.bw.max(ROI_MIN_SIZE);
        let mut bh = e.bh.max(ROI_MIN_SIZE);
        if bw < iw + 2 || bh < ih + 2 {
            let pad = adaptive_roi_pad(iw, ih);
            let (nbx, nby, nbw, nbh) =
                pad_roi_from_image(ix, iy, iw, ih, screen_w, screen_h, pad);
            bx = nbx;
            by = nby;
            bw = nbw;
            bh = nbh;
        } else {
            let (nbx, nby, nbw, nbh) =
                ensure_roi_contains_image(bx, by, bw, bh, ix, iy, iw, ih, screen_w, screen_h);
            bx = nbx;
            by = nby;
            bw = nbw;
            bh = nbh;
        }
        (bx, by, bw, bh, e.cx, e.cy)
    }

    /// Refresh overlay geometry/template for current selection (keep desktop bg).
    fn refresh_overlay_selection(
        &mut self,
        ctx: &egui::Context,
        editable: bool,
        return_to_detail: bool,
    ) {
        let Some(e) = self.selected_element().cloned() else {
            return;
        };
        let (img_w, img_h) = self.load_overlay_template(ctx, e.id);
        let (bx, by, bw, bh, cx, cy) = self.geometry_for_element(&e, img_w, img_h);
        self.mode = AppMode::EditRoi {
            element_id: e.id,
            editable,
            return_to_detail,
            bx,
            by,
            bw,
            bh,
            cx,
            cy,
            img_w,
            img_h,
            orig_bx: bx,
            orig_by: by,
            orig_bw: bw,
            orig_bh: bh,
            orig_cx: cx,
            orig_cy: cy,
            drag: None,
        };
        let st = self
            .selected_state()
            .map(|s| s.name.as_str())
            .unwrap_or("-");
        let idx = self
            .elements
            .iter()
            .position(|x| x.id == e.id)
            .map(|i| i + 1)
            .unwrap_or(0);
        let n = self.elements.len();
        self.status = if editable {
            format!(
                "Edit「{}」[{}/{}] state={} — ↑↓换元件 · ←→换状态",
                e.name, idx, n, st
            )
        } else {
            format!(
                "View「{}」[{}/{}] state={} — ↑↓换元件 · ←→换状态 · E编辑 · Esc关闭",
                e.name, idx, n, st
            )
        };
        ctx.request_repaint();
    }

    /// Browse next/prev element while overlay is open (wrap). `delta` = ±1.
    fn browse_overlay_element(&mut self, ctx: &egui::Context, delta: i32) {
        if self.elements.is_empty() {
            return;
        }
        let (editable, return_to_detail) = match &self.mode {
            AppMode::EditRoi {
                editable,
                return_to_detail,
                ..
            } => (*editable, *return_to_detail),
            _ => return,
        };
        // Editing geometry: only allow browse in view mode (avoid losing unsaved edits silently).
        if editable {
            return;
        }
        let cur = self.last_id.and_then(|id| {
            self.elements.iter().position(|e| e.id == id)
        });
        let n = self.elements.len() as i32;
        let idx = cur.unwrap_or(0) as i32;
        let next = (idx + delta).rem_euclid(n) as usize;
        self.last_id = Some(self.elements[next].id);
        self.sync_last_selection();
        self.preview = None;
        self.refresh_overlay_selection(ctx, false, return_to_detail);
    }

    /// Browse next/prev state of current element (wrap). View mode only.
    fn browse_overlay_state(&mut self, ctx: &egui::Context, delta: i32) {
        let (editable, return_to_detail, element_id) = match &self.mode {
            AppMode::EditRoi {
                editable,
                return_to_detail,
                element_id,
                ..
            } => (*editable, *return_to_detail, *element_id),
            _ => return,
        };
        if editable {
            return;
        }
        let Some(e) = self.elements.iter().find(|x| x.id == element_id) else {
            return;
        };
        if e.states.is_empty() {
            return;
        }
        let cur = self
            .last_state_name
            .as_ref()
            .and_then(|n| e.states.iter().position(|s| &s.name == n))
            .unwrap_or(0);
        let n = e.states.len() as i32;
        let next = (cur as i32 + delta).rem_euclid(n) as usize;
        self.last_state_name = Some(e.states[next].name.clone());
        self.preview = None;
        self.refresh_overlay_selection(ctx, false, return_to_detail);
    }

    fn begin_roi_overlay(&mut self, ctx: &egui::Context, frame: &eframe::Frame, editable: bool) {
        let Some(_e) = self.selected_element().cloned() else {
            self.status = "Select an element first".into();
            return;
        };
        let return_to_detail = match &self.mode {
            AppMode::ViewDetail => true,
            AppMode::EditRoi {
                return_to_detail, ..
            } => *return_to_detail,
            _ => false,
        };
        if let Err(err) = self.capture_desktop_bg(ctx, frame) {
            self.status = format!("Screenshot failed: {err}");
            self.restore_main_window(ctx, frame);
            log_error(&format!("begin_roi_overlay: {err}"));
            return;
        }
        self.refresh_overlay_selection(ctx, editable, return_to_detail);
        self.enter_fullscreen_overlay(ctx, frame);
    }

    fn enter_fullscreen_overlay(&mut self, ctx: &egui::Context, frame: &eframe::Frame) {
        let logical_w = self.sw as f32 / self.scale.max(0.1);
        let logical_h = self.sh as f32 / self.scale.max(0.1);
        // Opaque window + painted desktop snapshot (transparency is unreliable on Win).
        // Size/pos: egui ViewportCommand only (logical). Win32 only toggles TOPMOST.
        ctx.send_viewport_cmd(egui::ViewportCommand::Transparent(false));
        ctx.send_viewport_cmd(egui::ViewportCommand::Decorations(false));
        ctx.send_viewport_cmd(egui::ViewportCommand::OuterPosition(egui::pos2(0.0, 0.0)));
        ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(egui::vec2(logical_w, logical_h)));
        ctx.send_viewport_cmd(egui::ViewportCommand::WindowLevel(egui::WindowLevel::AlwaysOnTop));
        ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
        ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
        let _ = place_window_fullscreen(frame);
        ctx.request_repaint();
    }

    fn exit_edit_roi(&mut self, ctx: &egui::Context, frame: &eframe::Frame, to_detail: bool) {
        self.pre_capture = None;
        self.overlay_template = None;
        self.restore_main_window(ctx, frame);
        self.mode = if to_detail {
            AppMode::ViewDetail
        } else {
            AppMode::Normal
        };
    }

    fn save_edit_roi(&mut self, ctx: &egui::Context, frame: &eframe::Frame) {
        let (id, bx, by, bw, bh, cx, cy, return_to_detail) = match &self.mode {
            AppMode::EditRoi {
                element_id,
                bx,
                by,
                bw,
                bh,
                cx,
                cy,
                editable,
                return_to_detail,
                ..
            } => {
                if !*editable {
                    return;
                }
                (
                    *element_id,
                    *bx,
                    *by,
                    *bw,
                    *bh,
                    *cx,
                    *cy,
                    *return_to_detail,
                )
            }
            _ => return,
        };
        match update_element_geometry(
            &self.conn,
            id,
            cx,
            cy,
            bx,
            by,
            bw,
            bh,
            self.sw as i32,
            self.sh as i32,
        ) {
            Ok(()) => {
                self.refresh();
                self.preview = None;
                self.exit_edit_roi(ctx, frame, return_to_detail);
                self.status = format!(
                    "Saved geometry (image unchanged): center=({}, {})  ROI=({},{}) {}x{}",
                    cx, cy, bx, by, bw, bh
                );
            }
            Err(e) => self.status = format!("Save failed: {e}"),
        }
    }

    fn begin_capture(&mut self, ctx: &egui::Context, frame: &eframe::Frame, is_add_state: bool) {
        self.capture_flag.store(false, Ordering::Relaxed);
        self.drag_start = None;
        self.drag_current = None;
        self.mouse_pos = None;
        self.capture_started_at = None;
        self.is_add_state_mode = is_add_state;
        self.pre_capture = None;
        self.status = "Capturing desktop...".into();
        while self.rx.try_recv().is_ok() {}

        // First principles: exclude ourselves from capture instead of racing DWM with park/hide.
        let excluded = set_exclude_from_capture(frame, true);
        if excluded {
            log_error("begin_capture: WDA_EXCLUDEFROMCAPTURE ok");
        } else {
            // Older OS fallback: park off-screen briefly so BitBlt doesn't include us.
            log_error("begin_capture: exclude failed, parking off-screen");
            let _ = park_window_offscreen(frame);
            thread::sleep(Duration::from_millis(60));
        }

        let img = match grab_primary_monitor() {
            Ok(img) => img,
            Err(e) => {
                self.mode = AppMode::Normal;
                self.status = format!("Screenshot failed: {e}");
                self.restore_main_window(ctx, frame);
                log_error(&format!("begin_capture: {e}"));
                return;
            }
        };

        self.show_capture_overlay(ctx, frame, img, is_add_state);
    }

    fn show_capture_overlay(
        &mut self,
        ctx: &egui::Context,
        frame: &eframe::Frame,
        img: image::RgbaImage,
        is_add_state: bool,
    ) {
        // Image dimensions are ground truth for crop + stored resolution.
        self.sw = img.width();
        self.sh = img.height();
        self.scale = ctx.pixels_per_point().max(0.1);

        self.pre_capture = None;
        let size = [img.width() as usize, img.height() as usize];
        // One bulk convert — no per-pixel iterator alloc churn.
        let color_img = egui::ColorImage::from_rgba_unmultiplied(size, img.as_raw());
        let tex = ctx.load_texture(
            format!("desktop-bg-{}", Local::now().format("%H%M%S%3f")),
            color_img,
            egui::TextureOptions::NEAREST, // pixel-perfect for UI templates
        );
        self.pre_capture = Some((tex, img));

        self.is_add_state_mode = is_add_state;
        self.capture_flag.store(true, Ordering::Relaxed);
        self.capture_started_at = Some(std::time::Instant::now());
        self.status = "Click and drag on screen to select region...".into();
        self.mode = AppMode::Capturing;

        let logical_w = self.sw as f32 / self.scale;
        let logical_h = self.sh as f32 / self.scale;
        ctx.send_viewport_cmd(egui::ViewportCommand::Decorations(false));
        ctx.send_viewport_cmd(egui::ViewportCommand::OuterPosition(egui::pos2(0.0, 0.0)));
        ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(egui::vec2(logical_w, logical_h)));
        ctx.send_viewport_cmd(egui::ViewportCommand::WindowLevel(egui::WindowLevel::AlwaysOnTop));
        ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
        ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
        let _ = place_window_fullscreen(frame);
        ctx.request_repaint();
        log_error(&format!(
            "overlay shown {}x{} scale={:.2}",
            self.sw, self.sh, self.scale
        ));
    }

    fn restore_main_window(&mut self, ctx: &egui::Context, frame: &eframe::Frame) {
        let _ = set_exclude_from_capture(frame, true); // keep exclude for next shot
        ctx.send_viewport_cmd(egui::ViewportCommand::Transparent(false));
        ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
        ctx.send_viewport_cmd(egui::ViewportCommand::Decorations(true));
        ctx.send_viewport_cmd(egui::ViewportCommand::WindowLevel(egui::WindowLevel::Normal));
        ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(egui::vec2(920.0, 700.0)));
        ctx.send_viewport_cmd(egui::ViewportCommand::OuterPosition(egui::pos2(100.0, 100.0)));
        ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
        ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
        let _ = place_window_normal(frame);
    }

    fn end_capture(&mut self, ctx: &egui::Context, frame: &eframe::Frame) {
        self.capture_flag.store(false, Ordering::Relaxed);
        self.capture_started_at = None;
        self.drag_start = None;
        self.drag_current = None;
        self.mouse_pos = None;
        self.pre_capture = None; // free GPU texture + full desktop buffer
        self.restore_main_window(ctx, frame);
        log_error("end_capture done: restored window");
    }

    /// Returns Some((existing_id, message)) when name/image/state already exists.
    fn check_name_conflict(
        &self,
        name: &str,
        state_name: &str,
        is_add_state: bool,
        existing_id_hint: Option<i32>,
    ) -> Option<(Option<i32>, String)> {
        let dest = std::path::Path::new(&self.image_dir).join(format!("{}_{}.png", name, state_name));
        let file_exists = dest.exists();
        let existing_id = existing_id_hint.or_else(|| find_element_id_by_name(&self.conn, name));
        let state_clash = existing_id
            .map(|id| state_exists(&self.conn, id, state_name))
            .unwrap_or(false);

        if !file_exists && !state_clash && !(existing_id.is_some() && !is_add_state) {
            return None;
        }

        let mut parts = Vec::new();
        if existing_id.is_some() && !is_add_state {
            parts.push(format!("元素名「{}」已存在", name));
        }
        if state_clash {
            parts.push(format!("状态「{}」已存在", state_name));
        }
        if file_exists {
            parts.push(format!(
                "图片「{}_{}.png」已存在",
                name, state_name
            ));
        }
        let message = format!(
            "{}。是否用新截图和坐标覆盖旧数据？",
            parts.join("；")
        );
        Some((existing_id, message))
    }

    fn commit_named_capture(
        &mut self,
        x1: i32,
        y1: i32,
        x2: i32,
        y2: i32,
        temp_img: &str,
        name: &str,
        state_name: &str,
        existing_id: Option<i32>,
        is_add_state: bool,
        overwrite: bool,
    ) -> Result<String, String> {
        let dest = std::path::Path::new(&self.image_dir)
            .join(format!("{}_{}.png", name, state_name))
            .to_string_lossy()
            .to_string();
        // DB first with planned final path; only then replace the PNG so a failed
        // insert/update never leaves a replaced image with stale metadata.
        let planned = dest.clone();

        let result = if is_add_state {
            let id = existing_id
                .or(self.last_id)
                .ok_or_else(|| "No element selected".to_string())?;
            if overwrite || state_exists(&self.conn, id, state_name) {
                overwrite_elem(
                    &self.conn, id, state_name, x1, y1, x2, y2, &planned, self.sw, self.sh,
                )?;
            } else {
                let fname = std::path::Path::new(&planned)
                    .file_name()
                    .and_then(|s| s.to_str())
                    .unwrap_or(&planned)
                    .to_string();
                let ts = Local::now().format("%Y%m%d_%H%M%S").to_string();
                self.conn
                    .execute(
                        "INSERT INTO element_states(element_id,state_name,image_path,is_primary,created_at) VALUES(?1,?2,?3,0,?4)",
                        params![id, state_name, fname, ts],
                    )
                    .map_err(|e| e.to_string())?;
            }
            self.last_id = Some(id);
            self.last_element_name = Some(name.to_string());
            self.last_state_name = Some(state_name.to_string());
            Ok(format!("State '{}' saved (#{})", state_name, id))
        } else if overwrite {
            if let Some(id) = existing_id.or_else(|| find_element_id_by_name(&self.conn, name)) {
                overwrite_elem(
                    &self.conn, id, state_name, x1, y1, x2, y2, &planned, self.sw, self.sh,
                )?;
                self.last_id = Some(id);
                self.last_element_name = Some(name.to_string());
                self.last_state_name = Some(state_name.to_string());
                Ok(format!("Element #{} overwritten!", id))
            } else {
                let id = create_elem(
                    &self.conn, name, state_name, x1, y1, x2, y2, &planned, self.sw, self.sh,
                )?;
                self.last_id = Some(id);
                self.last_element_name = Some(name.to_string());
                self.last_state_name = Some(state_name.to_string());
                Ok(format!("Element #{} created (replaced image)!", id))
            }
        } else {
            let id = create_elem(
                &self.conn, name, state_name, x1, y1, x2, y2, &planned, self.sw, self.sh,
            )?;
            self.last_id = Some(id);
            self.last_element_name = Some(name.to_string());
            self.last_state_name = Some(state_name.to_string());
            Ok(format!("Element #{} created!", id))
        };

        match result {
            Ok(msg) => {
                place_image_at(temp_img, &dest)?;
                self.refresh();
                Ok(msg)
            }
            Err(e) => Err(e),
        }
    }

    fn begin_recapture(&mut self, ctx: &egui::Context, frame: &eframe::Frame) {
        let Some(name) = self.last_element_name.clone() else {
            self.status = "未选择元件".into();
            return;
        };
        let Some(state) = self.last_state_name.clone() else {
            self.status = "未选择状态".into();
            return;
        };
        let _ = name;
        self.recapture = true;
        self.forced_state_name = Some(state);
        self.begin_capture(ctx, frame, true);
        self.status = "重录：框选后将覆盖当前状态截图与坐标".into();
    }

    fn begin_verify_match(&mut self, ctx: &egui::Context, frame: &eframe::Frame) {
        let Some(e) = self.selected_element().cloned() else {
            self.status = "未选择元件".into();
            return;
        };
        let Some(st) = self.selected_state().cloned() else {
            self.status = "无状态可验配".into();
            return;
        };
        let path = std::path::Path::new(&self.image_dir).join(&st.image_path);
        let template = match image::open(&path) {
            Ok(img) => img.to_rgba8(),
            Err(err) => {
                self.status = format!("无法加载模板: {err}");
                return;
            }
        };

        self.status = "验配中：截屏并搜索…".into();
        let excluded = set_exclude_from_capture(frame, true);
        if !excluded {
            let _ = park_window_offscreen(frame);
            thread::sleep(Duration::from_millis(60));
        }
        let screen = match grab_primary_monitor() {
            Ok(img) => img,
            Err(err) => {
                self.restore_main_window(ctx, frame);
                self.status = format!("截屏失败: {err}");
                return;
            }
        };
        self.sw = screen.width();
        self.sh = screen.height();
        self.scale = ctx.pixels_per_point().max(0.1);

        let (bx, by, bw, bh, rec_cx, rec_cy) = if e.screen_w > 0
            && e.screen_h > 0
            && (e.screen_w != self.sw as i32 || e.screen_h != self.sh as i32)
        {
            (
                matchutil::scale_i(e.bx, e.screen_w, self.sw as i32),
                matchutil::scale_i(e.by, e.screen_h, self.sh as i32),
                matchutil::scale_i(e.bw, e.screen_w, self.sw as i32).max(1),
                matchutil::scale_i(e.bh, e.screen_h, self.sh as i32).max(1),
                matchutil::scale_i(e.cx, e.screen_w, self.sw as i32),
                matchutil::scale_i(e.cy, e.screen_h, self.sh as i32),
            )
        } else {
            (e.bx, e.by, e.bw.max(1), e.bh.max(1), e.cx, e.cy)
        };
        let roi = Some((bx, by, bw, bh));
        let hit = matchutil::find_template_smart(&screen, &template, roi, 0.85);

        let size = [screen.width() as usize, screen.height() as usize];
        let color_img = egui::ColorImage::from_rgba_unmultiplied(size, screen.as_raw());
        let tex = ctx.load_texture(
            format!("verify-bg-{}", Local::now().format("%H%M%S%3f")),
            color_img,
            egui::TextureOptions::NEAREST,
        );
        self.pre_capture = Some((tex, screen));
        let _ = self.load_overlay_template(ctx, e.id);

        let (score, has_hit, hit_x, hit_y, hit_cx, hit_cy, tw, th, used_roi) = match hit {
            Some(h) if h.score >= 0.55 => (
                h.score,
                true,
                h.hit_x,
                h.hit_y,
                h.hit_cx,
                h.hit_cy,
                h.tw,
                h.th,
                h.used_roi,
            ),
            Some(h) => (
                h.score,
                false,
                h.hit_x,
                h.hit_y,
                h.hit_cx,
                h.hit_cy,
                h.tw,
                h.th,
                h.used_roi,
            ),
            None => (0.0, false, 0, 0, 0, 0, template.width() as i32, template.height() as i32, true),
        };

        self.mode = AppMode::VerifyMatch {
            element_id: e.id,
            score,
            has_hit,
            hit_x,
            hit_y,
            hit_cx,
            hit_cy,
            tw,
            th,
            bx,
            by,
            bw,
            bh,
            rec_cx,
            rec_cy,
            used_roi,
        };
        self.status = if has_hit {
            format!(
                "验配 score={:.3} hit=({},{}) recorded=({},{}) {}",
                score,
                hit_cx,
                hit_cy,
                rec_cx,
                rec_cy,
                if used_roi { "ROI" } else { "全屏" }
            )
        } else {
            "验配失败：未找到模板".into()
        };

        let logical_w = self.sw as f32 / self.scale;
        let logical_h = self.sh as f32 / self.scale;
        ctx.send_viewport_cmd(egui::ViewportCommand::Decorations(false));
        ctx.send_viewport_cmd(egui::ViewportCommand::OuterPosition(egui::pos2(0.0, 0.0)));
        ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(egui::vec2(logical_w, logical_h)));
        ctx.send_viewport_cmd(egui::ViewportCommand::WindowLevel(egui::WindowLevel::AlwaysOnTop));
        ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
        ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
        let _ = place_window_fullscreen(frame);
        ctx.request_repaint();
    }

    fn exit_verify_match(&mut self, ctx: &egui::Context, frame: &eframe::Frame) {
        self.pre_capture = None;
        self.overlay_template = None;
        self.restore_main_window(ctx, frame);
        self.mode = AppMode::Normal;
    }

    fn load_compare_textures(&mut self, ctx: &egui::Context) {
        self.compare_tex.clear();
        let Some(e) = self.selected_element().cloned() else {
            return;
        };
        for st in &e.states {
            let path = std::path::Path::new(&self.image_dir).join(&st.image_path);
            if let Ok(img) = image::open(&path) {
                let rgba = img.to_rgba8();
                let size = [rgba.width() as usize, rgba.height() as usize];
                let color = egui::ColorImage::from_rgba_unmultiplied(size, rgba.as_raw());
                let tex = ctx.load_texture(
                    format!("cmp-{}-{}", e.id, st.name),
                    color,
                    egui::TextureOptions::NEAREST,
                );
                let label = format!(
                    "{}{}  {}×{}",
                    st.name,
                    if st.is_primary { " *" } else { "" },
                    rgba.width(),
                    rgba.height()
                );
                self.compare_tex.push((label, tex));
            } else {
                let miss = egui::ColorImage {
                    size: [1, 1],
                    pixels: vec![Color32::RED],
                };
                self.compare_tex.push((
                    format!("{} (缺图)", st.name),
                    ctx.load_texture(
                        format!("cmp-miss-{}-{}", e.id, st.name),
                        miss,
                        egui::TextureOptions::NEAREST,
                    ),
                ));
            }
        }
    }

    fn open_data_folder(&self) {
        let dir = &self.image_dir;
        #[cfg(windows)]
        {
            let _ = std::process::Command::new("explorer").arg(dir).spawn();
        }
        #[cfg(not(windows))]
        {
            let _ = dir;
        }
    }
}

impl eframe::App for RecorderApp {
    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        match self.mode {
            AppMode::Capturing | AppMode::EditRoi { .. } | AppMode::VerifyMatch { .. } => {
                [0.0, 0.0, 0.0, 1.0]
            }
            _ => {
                let c = _visuals.panel_fill;
                [
                    c.r() as f32 / 255.0,
                    c.g() as f32 / 255.0,
                    c.b() as f32 / 255.0,
                    1.0,
                ]
            }
        }
    }

    fn update(&mut self, ctx: &egui::Context, frame: &mut eframe::Frame) {
        self.scale = ctx.pixels_per_point().max(0.1);
        match &self.mode {
            AppMode::Capturing => ctx.request_repaint(), // drain rdev promptly
            AppMode::EditRoi { drag: Some(_), .. } => ctx.request_repaint(),
            AppMode::EditRoi { .. } | AppMode::VerifyMatch { .. } => {
                ctx.request_repaint_after(Duration::from_millis(50));
            }
            _ => {}
        }

        match &self.mode {
            AppMode::VerifyMatch { .. } => {
                let (
                    element_id,
                    score,
                    has_hit,
                    hit_x,
                    hit_y,
                    hit_cx,
                    hit_cy,
                    tw,
                    th,
                    bx,
                    by,
                    bw,
                    bh,
                    rec_cx,
                    rec_cy,
                    used_roi,
                ) = if let AppMode::VerifyMatch {
                    element_id,
                    score,
                    has_hit,
                    hit_x,
                    hit_y,
                    hit_cx,
                    hit_cy,
                    tw,
                    th,
                    bx,
                    by,
                    bw,
                    bh,
                    rec_cx,
                    rec_cy,
                    used_roi,
                } = &self.mode
                {
                    (
                        *element_id,
                        *score,
                        *has_hit,
                        *hit_x,
                        *hit_y,
                        *hit_cx,
                        *hit_cy,
                        *tw,
                        *th,
                        *bx,
                        *by,
                        *bw,
                        *bh,
                        *rec_cx,
                        *rec_cy,
                        *used_roi,
                    )
                } else {
                    unreachable!()
                };

                let s = self.scale;
                let to_ui = |v: i32| v as f32 / s;
                let name = self
                    .elements
                    .iter()
                    .find(|e| e.id == element_id)
                    .map(|e| e.name.clone())
                    .unwrap_or_else(|| "?".into());

                if ctx.input(|i| {
                    i.key_pressed(egui::Key::Escape)
                        || i.key_pressed(egui::Key::Enter)
                        || i.key_pressed(egui::Key::Space)
                }) || escape_key_down()
                {
                    self.exit_verify_match(ctx, frame);
                    self.status = format!("验配结束 score={:.3}", score);
                    return;
                }

                egui::CentralPanel::default()
                    .frame(egui::Frame::none().fill(Color32::BLACK))
                    .show(ctx, |ui| {
                        let painter = ui.painter();
                        let sw = to_ui(self.sw as i32);
                        let sh = to_ui(self.sh as i32);
                        if let Some((tex, _)) = &self.pre_capture {
                            let bg = Rect::from_min_max(Pos2::ZERO, Pos2::new(sw, sh));
                            painter.image(
                                tex.id(),
                                bg,
                                Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)),
                                Color32::WHITE,
                            );
                        }
                        // Dim outside ROI
                        let roi_r = Rect::from_min_size(
                            Pos2::new(to_ui(bx), to_ui(by)),
                            egui::vec2(to_ui(bw), to_ui(bh)),
                        );
                        painter.rect_filled(
                            Rect::from_min_max(Pos2::ZERO, Pos2::new(sw, sh)),
                            0.0,
                            Color32::from_black_alpha(100),
                        );
                        if let Some((tex, _)) = &self.pre_capture {
                            painter.image(
                                tex.id(),
                                roi_r,
                                Rect::from_min_max(
                                    Pos2::new(bx as f32 / self.sw as f32, by as f32 / self.sh as f32),
                                    Pos2::new(
                                        (bx + bw) as f32 / self.sw as f32,
                                        (by + bh) as f32 / self.sh as f32,
                                    ),
                                ),
                                Color32::WHITE,
                            );
                        }
                        painter.rect_stroke(roi_r, 0.0, Stroke::new(2.0, Color32::GREEN));

                        let score_color = if score >= 0.90 {
                            Color32::LIGHT_GREEN
                        } else if score >= 0.80 {
                            Color32::YELLOW
                        } else {
                            Color32::LIGHT_RED
                        };

                        if has_hit {
                            let hit_r = Rect::from_min_size(
                                Pos2::new(to_ui(hit_x), to_ui(hit_y)),
                                egui::vec2(to_ui(tw), to_ui(th)),
                            );
                            painter.rect_stroke(hit_r, 0.0, Stroke::new(3.0, Color32::from_rgb(0, 200, 255)));
                            painter.circle_filled(
                                Pos2::new(to_ui(hit_cx), to_ui(hit_cy)),
                                6.0,
                                Color32::from_rgb(0, 200, 255),
                            );
                            painter.circle_stroke(
                                Pos2::new(to_ui(hit_cx), to_ui(hit_cy)),
                                10.0,
                                Stroke::new(2.0, Color32::WHITE),
                            );
                        }
                        // Recorded center
                        painter.circle_filled(
                            Pos2::new(to_ui(rec_cx), to_ui(rec_cy)),
                            5.0,
                            Color32::YELLOW,
                        );
                        painter.circle_stroke(
                            Pos2::new(to_ui(rec_cx), to_ui(rec_cy)),
                            9.0,
                            Stroke::new(2.0, Color32::from_rgb(200, 120, 0)),
                        );

                        egui::Area::new(egui::Id::new("verify_toolbar"))
                            .fixed_pos(Pos2::new(16.0, 16.0))
                            .order(egui::Order::Foreground)
                            .show(ctx, |ui| {
                                egui::Frame::none()
                                    .fill(Color32::from_black_alpha(200))
                                    .inner_margin(egui::Margin::same(10.0))
                                    .show(ui, |ui| {
                                        ui.label(
                                            egui::RichText::new(format!(
                                                "验配「{}」  score={:.3}  {}  Esc/Enter关闭",
                                                name,
                                                score,
                                                if used_roi { "搜索:ROI优先" } else { "搜索:全屏" }
                                            ))
                                            .color(score_color)
                                            .size(16.0),
                                        );
                                        if has_hit {
                                            let dx = hit_cx - rec_cx;
                                            let dy = hit_cy - rec_cy;
                                            ui.label(
                                                egui::RichText::new(format!(
                                                    "命中中心 ({}, {})  录制中心 ({}, {})  Δ=({}, {})",
                                                    hit_cx, hit_cy, rec_cx, rec_cy, dx, dy
                                                ))
                                                .color(Color32::WHITE),
                                            );
                                            ui.label(
                                                egui::RichText::new(
                                                    "青框=匹配命中  黄点=录制中心  绿框=ROI",
                                                )
                                                .small()
                                                .color(Color32::GRAY),
                                            );
                                        } else {
                                            ui.colored_label(Color32::LIGHT_RED, "未找到匹配");
                                        }
                                    });
                            });
                    });
            }
            AppMode::EditRoi { .. } => {
                let (
                    element_id,
                    editable,
                    return_to_detail,
                    mut bx,
                    mut by,
                    mut bw,
                    mut bh,
                    mut cx,
                    mut cy,
                    img_w,
                    img_h,
                    orig_bx,
                    orig_by,
                    orig_bw,
                    orig_bh,
                    orig_cx,
                    orig_cy,
                    mut drag,
                ) = if let AppMode::EditRoi {
                    element_id,
                    editable,
                    return_to_detail,
                    bx,
                    by,
                    bw,
                    bh,
                    cx,
                    cy,
                    img_w,
                    img_h,
                    orig_bx,
                    orig_by,
                    orig_bw,
                    orig_bh,
                    orig_cx,
                    orig_cy,
                    drag,
                } = &self.mode
                {
                    (
                        *element_id,
                        *editable,
                        *return_to_detail,
                        *bx,
                        *by,
                        *bw,
                        *bh,
                        *cx,
                        *cy,
                        *img_w,
                        *img_h,
                        *orig_bx,
                        *orig_by,
                        *orig_bw,
                        *orig_bh,
                        *orig_cx,
                        *orig_cy,
                        *drag,
                    )
                } else {
                    unreachable!()
                };

                let s = self.scale;
                let to_ui = |v: i32| v as f32 / s;
                let to_phys = |v: f32| (v * s).round() as i32;
                let screen_w = self.sw as i32;
                let screen_h = self.sh as i32;

                let mut save = false;
                let mut cancel = false;
                let mut reset = false;
                let mut switch_edit = false;
                let mut to_detail = false;
                let mut browse_elem: i32 = 0;
                let mut browse_state: i32 = 0;
                ctx.input(|i| {
                    if editable && i.key_pressed(egui::Key::Enter) {
                        save = true;
                    }
                    if i.key_pressed(egui::Key::Escape) {
                        cancel = true;
                    }
                    if editable && i.key_pressed(egui::Key::R) && i.modifiers.ctrl {
                        reset = true;
                    }
                    if !editable
                        && (i.key_pressed(egui::Key::E) || i.key_pressed(egui::Key::F8))
                    {
                        switch_edit = true;
                    }
                    if !editable && i.key_pressed(egui::Key::D) {
                        to_detail = true;
                    }
                    // View mode: ↑↓ browse elements, ←→ browse states (same desktop snapshot).
                    if !editable && !i.modifiers.ctrl {
                        if i.key_pressed(egui::Key::ArrowDown)
                            || i.key_pressed(egui::Key::PageDown)
                        {
                            browse_elem = 1;
                        }
                        if i.key_pressed(egui::Key::ArrowUp) || i.key_pressed(egui::Key::PageUp)
                        {
                            browse_elem = -1;
                        }
                        if i.key_pressed(egui::Key::ArrowRight) {
                            browse_state = 1;
                        }
                        if i.key_pressed(egui::Key::ArrowLeft) {
                            browse_state = -1;
                        }
                    }
                    if editable {
                        let step = if i.modifiers.shift { 10 } else { 1 };
                        if i.modifiers.ctrl {
                            if i.key_pressed(egui::Key::ArrowLeft) {
                                bx -= step;
                            }
                            if i.key_pressed(egui::Key::ArrowRight) {
                                bx += step;
                            }
                            if i.key_pressed(egui::Key::ArrowUp) {
                                by -= step;
                            }
                            if i.key_pressed(egui::Key::ArrowDown) {
                                by += step;
                            }
                        } else {
                            if i.key_pressed(egui::Key::ArrowLeft) {
                                cx -= step;
                            }
                            if i.key_pressed(egui::Key::ArrowRight) {
                                cx += step;
                            }
                            if i.key_pressed(egui::Key::ArrowUp) {
                                cy -= step;
                            }
                            if i.key_pressed(egui::Key::ArrowDown) {
                                cy += step;
                            }
                        }
                    }
                });
                // Level-triggered Esc fallback when focus quirks drop key_pressed events.
                if escape_key_down() {
                    cancel = true;
                }
                if browse_elem != 0 {
                    self.browse_overlay_element(ctx, browse_elem);
                    return;
                }
                if browse_state != 0 {
                    self.browse_overlay_state(ctx, browse_state);
                    return;
                }
                bx = bx.clamp(0, (screen_w - ROI_MIN_SIZE).max(0));
                by = by.clamp(0, (screen_h - ROI_MIN_SIZE).max(0));
                bw = bw.min(screen_w - bx).max(ROI_MIN_SIZE);
                bh = bh.min(screen_h - by).max(ROI_MIN_SIZE);
                cx = cx.clamp(0, (screen_w - 1).max(0));
                cy = cy.clamp(0, (screen_h - 1).max(0));
                if reset {
                    bx = orig_bx;
                    by = orig_by;
                    bw = orig_bw;
                    bh = orig_bh;
                    cx = orig_cx;
                    cy = orig_cy;
                    drag = None;
                }
                // Keep ROI ≥ image region (image anchored to center).
                {
                    let (ix, iy, iw, ih) =
                        image_rect_anchored(cx, cy, img_w, img_h, screen_w, screen_h);
                    let (nbx, nby, nbw, nbh) = ensure_roi_contains_image(
                        bx, by, bw, bh, ix, iy, iw, ih, screen_w, screen_h,
                    );
                    bx = nbx;
                    by = nby;
                    bw = nbw;
                    bh = nbh;
                }

                let pointer = ctx.input(|i| {
                    (
                        i.pointer.interact_pos(),
                        i.pointer.primary_pressed(),
                        i.pointer.primary_released(),
                    )
                });

                let hover_hit = if editable {
                    if let Some(pos) = pointer.0 {
                        let mx = to_phys(pos.x);
                        let my = to_phys(pos.y);
                        if let Some(d) = drag {
                            let (nbx, nby, nbw, nbh, ncx, ncy) =
                                apply_roi_drag(&d, mx, my, screen_w, screen_h);
                            bx = nbx;
                            by = nby;
                            bw = nbw;
                            bh = nbh;
                            cx = ncx;
                            cy = ncy;
                            d.hit
                        } else {
                            roi_hit_test(mx, my, bx, by, bw, bh, cx, cy)
                        }
                    } else {
                        RoiHit::None
                    }
                } else {
                    RoiHit::None
                };

                if editable && pointer.1 {
                    if let Some(pos) = pointer.0 {
                        let mx = to_phys(pos.x);
                        let my = to_phys(pos.y);
                        let hit = roi_hit_test(mx, my, bx, by, bw, bh, cx, cy);
                        if hit != RoiHit::None {
                            drag = Some(RoiDrag {
                                hit,
                                start_mx: mx,
                                start_my: my,
                                start_bx: bx,
                                start_by: by,
                                start_bw: bw,
                                start_bh: bh,
                                start_cx: cx,
                                start_cy: cy,
                            });
                        }
                    }
                }
                if pointer.2 {
                    drag = None;
                }

                if editable {
                    ctx.set_cursor_icon(roi_cursor(if drag.is_some() {
                        drag.map(|d| d.hit).unwrap_or(hover_hit)
                    } else {
                        hover_hit
                    }));
                }

                self.mode = AppMode::EditRoi {
                    element_id,
                    editable,
                    return_to_detail,
                    bx,
                    by,
                    bw,
                    bh,
                    cx,
                    cy,
                    img_w,
                    img_h,
                    orig_bx,
                    orig_by,
                    orig_bw,
                    orig_bh,
                    orig_cx,
                    orig_cy,
                    drag,
                };

                if save {
                    self.save_edit_roi(ctx, frame);
                    return;
                }
                if cancel {
                    self.exit_edit_roi(ctx, frame, return_to_detail);
                    self.status = if editable {
                        "ROI edit cancelled".into()
                    } else {
                        "View closed".into()
                    };
                    return;
                }
                if to_detail {
                    self.exit_edit_roi(ctx, frame, true);
                    return;
                }
                if switch_edit {
                    if let AppMode::EditRoi { editable, .. } = &mut self.mode {
                        *editable = true;
                    }
                    self.status = "Edit mode — drag ROI / arrows for center · Enter save".into();
                    return;
                }

                egui::CentralPanel::default()
                    .frame(egui::Frame::none().fill(Color32::BLACK))
                    .show(ctx, |ui| {
                        let painter = ui.painter();
                        let sw_ui = to_ui(screen_w);
                        let sh_ui = to_ui(screen_h);

                        // Desktop snapshot background — required; transparency is unreliable.
                        if let Some((tex, _)) = &self.pre_capture {
                            let bg_rect = Rect::from_min_max(Pos2::ZERO, Pos2::new(sw_ui, sh_ui));
                            painter.image(
                                tex.id(),
                                bg_rect,
                                Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)),
                                Color32::WHITE,
                            );
                        }

                        let (ix, iy, iw, ih) =
                            image_rect_anchored(cx, cy, img_w, img_h, screen_w, screen_h);
                        let rx1 = to_ui(bx);
                        let ry1 = to_ui(by);
                        let rx2 = to_ui(bx + bw);
                        let ry2 = to_ui(by + bh);
                        let ix1 = to_ui(ix);
                        let iy1 = to_ui(iy);
                        let ix2 = to_ui(ix + iw);
                        let iy2 = to_ui(iy + ih);

                        // Dim outside ROI (search region).
                        let dim = Color32::from_black_alpha(if editable { 100 } else { 70 });
                        painter.rect_filled(
                            Rect::from_min_max(Pos2::ZERO, Pos2::new(sw_ui, ry1)),
                            0.0,
                            dim,
                        );
                        painter.rect_filled(
                            Rect::from_min_max(Pos2::new(0.0, ry2), Pos2::new(sw_ui, sh_ui)),
                            0.0,
                            dim,
                        );
                        painter.rect_filled(
                            Rect::from_min_max(Pos2::new(0.0, ry1), Pos2::new(rx1, ry2)),
                            0.0,
                            dim,
                        );
                        painter.rect_filled(
                            Rect::from_min_max(Pos2::new(rx2, ry1), Pos2::new(sw_ui, ry2)),
                            0.0,
                            dim,
                        );

                        // Image region: paint template PNG + cyan border.
                        let img_rect = Rect::from_min_max(Pos2::new(ix1, iy1), Pos2::new(ix2, iy2));
                        if let Some(tex) = &self.overlay_template {
                            painter.image(
                                tex.id(),
                                img_rect,
                                Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)),
                                Color32::WHITE,
                            );
                        } else {
                            painter.rect_filled(
                                img_rect,
                                0.0,
                                Color32::from_rgba_unmultiplied(0, 180, 255, 40),
                            );
                        }
                        painter.rect_stroke(
                            img_rect,
                            0.0,
                            Stroke::new(2.0, Color32::from_rgb(0, 200, 255)),
                        );
                        painter.text(
                            Pos2::new(ix1 + 4.0, iy1 + 2.0),
                            egui::Align2::LEFT_TOP,
                            "IMG",
                            FontId::proportional(12.0),
                            Color32::from_rgb(0, 220, 255),
                        );

                        // ROI (search) — green, outer.
                        let roi_rect = Rect::from_min_max(Pos2::new(rx1, ry1), Pos2::new(rx2, ry2));
                        painter.rect_stroke(
                            roi_rect,
                            0.0,
                            Stroke::new(2.5, Color32::from_rgb(0, 220, 90)),
                        );
                        painter.rect_stroke(roi_rect, 0.0, Stroke::new(1.0, Color32::WHITE));
                        painter.text(
                            Pos2::new(rx1 + 4.0, ry1 + 2.0),
                            egui::Align2::LEFT_TOP,
                            "ROI",
                            FontId::proportional(12.0),
                            Color32::from_rgb(80, 255, 120),
                        );

                        if editable {
                            let hs = 8.0;
                            let handles = [
                                (rx1, ry1),
                                (rx2, ry1),
                                (rx1, ry2),
                                (rx2, ry2),
                                ((rx1 + rx2) / 2.0, ry1),
                                ((rx1 + rx2) / 2.0, ry2),
                                (rx1, (ry1 + ry2) / 2.0),
                                (rx2, (ry1 + ry2) / 2.0),
                            ];
                            for (hx, hy) in handles {
                                painter.rect_filled(
                                    Rect::from_center_size(Pos2::new(hx, hy), egui::vec2(hs, hs)),
                                    1.0,
                                    Color32::WHITE,
                                );
                                painter.rect_stroke(
                                    Rect::from_center_size(Pos2::new(hx, hy), egui::vec2(hs, hs)),
                                    1.0,
                                    Stroke::new(1.0, Color32::from_rgb(0, 160, 70)),
                                );
                            }
                        }

                        let c = Pos2::new(to_ui(cx), to_ui(cy));
                        painter.circle_filled(c, 5.0, Color32::from_rgb(255, 220, 0));
                        painter.circle_stroke(c, 12.0, Stroke::new(2.5, Color32::YELLOW));
                        painter.line_segment(
                            [Pos2::new(c.x - 18.0, c.y), Pos2::new(c.x + 18.0, c.y)],
                            Stroke::new(2.0, Color32::YELLOW),
                        );
                        painter.line_segment(
                            [Pos2::new(c.x, c.y - 18.0), Pos2::new(c.x, c.y + 18.0)],
                            Stroke::new(2.0, Color32::YELLOW),
                        );

                        egui::Area::new(egui::Id::new("roi_edit_toolbar"))
                            .fixed_pos(Pos2::new(16.0, 16.0))
                            .order(egui::Order::Foreground)
                            .show(ctx, |ui| {
                                egui::Frame::none()
                                    .fill(Color32::from_black_alpha(210))
                                    .rounding(6.0)
                                    .inner_margin(egui::Margin::same(10.0))
                                    .show(ui, |ui| {
                                        ui.set_min_width(640.0);
                                        ui.label(
                                            egui::RichText::new(format!(
                                                "{}  Center ({}, {})  |  IMG {}x{}  |  ROI ({},{}) {}x{}",
                                                if editable { "EDIT" } else { "VIEW" },
                                                cx, cy, iw, ih, bx, by, bw, bh
                                            ))
                                            .color(Color32::WHITE)
                                            .size(14.0),
                                        );
                                        if editable {
                                            ui.label(
                                                egui::RichText::new(
                                                    "Cyan=image (template) | Green=ROI (must be larger) | Yellow=center | Enter save geometry",
                                                )
                                                .color(Color32::LIGHT_GRAY)
                                                .size(12.0),
                                            );
                                            ui.horizontal(|ui| {
                                                if ui.button("Save 保存 (Enter)").clicked() {
                                                    save = true;
                                                }
                                                if ui.button("Reset 重置").clicked() {
                                                    reset = true;
                                                }
                                                if ui.button("Cancel 取消 (Esc)").clicked() {
                                                    cancel = true;
                                                }
                                            });
                                        } else {
                                            ui.label(
                                                egui::RichText::new(
                                                    "↑↓ 换元件 | ←→ 换状态 | E/F8 编辑 | Esc 关闭",
                                                )
                                                .color(Color32::LIGHT_GRAY)
                                                .size(12.0),
                                            );
                                            ui.horizontal(|ui| {
                                                if ui.button("◀ Prev 上一个").clicked() {
                                                    browse_elem = -1;
                                                }
                                                if ui.button("Next 下一个 ▶").clicked() {
                                                    browse_elem = 1;
                                                }
                                                if ui.button("Edit 编辑 (E/F8)").clicked() {
                                                    switch_edit = true;
                                                }
                                                if ui.button("Details 详情 (D)").clicked() {
                                                    to_detail = true;
                                                }
                                                if ui.button("Close 关闭 (Esc)").clicked() {
                                                    cancel = true;
                                                }
                                            });
                                        }
                                    });
                            });
                    });

                if save {
                    self.save_edit_roi(ctx, frame);
                } else if cancel {
                    self.exit_edit_roi(ctx, frame, return_to_detail);
                    self.status = if editable {
                        "ROI edit cancelled".into()
                    } else {
                        "View closed".into()
                    };
                } else if to_detail {
                    self.exit_edit_roi(ctx, frame, true);
                } else if browse_elem != 0 {
                    self.browse_overlay_element(ctx, browse_elem);
                } else if browse_state != 0 {
                    self.browse_overlay_state(ctx, browse_state);
                } else if switch_edit {
                    if let AppMode::EditRoi { editable, .. } = &mut self.mode {
                        *editable = true;
                    }
                    self.status = "Edit mode — drag ROI / arrows for center · Enter save".into();
                } else if reset {
                    if let AppMode::EditRoi {
                        bx,
                        by,
                        bw,
                        bh,
                        cx,
                        cy,
                        orig_bx,
                        orig_by,
                        orig_bw,
                        orig_bh,
                        orig_cx,
                        orig_cy,
                        drag,
                        ..
                    } = &mut self.mode
                    {
                        *bx = *orig_bx;
                        *by = *orig_by;
                        *bw = *orig_bw;
                        *bh = *orig_bh;
                        *cx = *orig_cx;
                        *cy = *orig_cy;
                        *drag = None;
                    }
                }
            }
            AppMode::Capturing => {

                if let Some(started) = self.capture_started_at {
                    if started.elapsed().as_secs() >= 30 {
                        self.end_capture(ctx, frame);
                        self.recapture = false;
                        self.mode = AppMode::Normal;
                        self.status = "Capture timed out (30s)".into();
                    }
                }
                if !matches!(self.mode, AppMode::Capturing) {
                    return;
                }
                while let Ok(msg) = self.rx.try_recv() {
                    match msg {
                        CaptureMsg::Down(x, y) => {
                            self.drag_start = Some((x as i32, y as i32));
                            self.drag_current = Some((x as i32, y as i32));
                            self.status = format!("Drag started ({},{}), release to finish", x as i32, y as i32);
                        }
                        CaptureMsg::Move(x, y) => {
                            self.mouse_pos = Some((x as i32, y as i32));
                            if self.drag_start.is_some() {
                                self.drag_current = Some((x as i32, y as i32));
                            }
                        }
                        CaptureMsg::Up(x, y) => {
                            log_error("capture up start");
                            let saved_start = self.drag_start.take();
                            let pre_img = self.pre_capture.take();
                            self.capture_flag.store(false, Ordering::Relaxed);
                            self.capture_started_at = None;
                            self.drag_start = None;
                            self.drag_current = None;
                            self.mouse_pos = None;

                            let result = match (saved_start, pre_img) {
                                (Some((sx, sy)), Some((_tex, full))) => {
                                    let px = sx.min(x as i32);
                                    let py = sy.min(y as i32);
                                    let pw = (sx - x as i32).abs() as u32;
                                    let ph = (sy - y as i32).abs() as u32;
                                    if pw == 0 || ph == 0 {
                                        Err("Zero region".into())
                                    } else {
                                        let lpx = (px as u32).min(full.width().saturating_sub(1));
                                        let lpy = (py as u32).min(full.height().saturating_sub(1));
                                        let lpw = pw.min(full.width().saturating_sub(lpx));
                                        let lph = ph.min(full.height().saturating_sub(lpy));
                                        if lpw == 0 || lph == 0 {
                                            Err("Region out of bounds".into())
                                        } else {
                                            let ts = Local::now().format("%Y%m%d_%H%M%S_%3f");
                                            let _ = fs::create_dir_all(&self.image_dir);
                                            let path = std::path::Path::new(&self.image_dir)
                                                .join(format!("capture_{}.png", ts));
                                            let path_s = path.to_string_lossy().to_string();
                                            let cropped =
                                                image::imageops::crop_imm(&full, lpx, lpy, lpw, lph)
                                                    .to_image();
                                            match cropped.save(&path) {
                                                Ok(_) => Ok((
                                                    lpx as i32,
                                                    lpy as i32,
                                                    (lpx + lpw) as i32,
                                                    (lpy + lph) as i32,
                                                    path_s,
                                                )),
                                                Err(e) => Err(format!("Save failed: {}", e)),
                                            }
                                        }
                                    }
                                }
                                (None, _) => Err("Cancelled".into()),
                                (_, None) => Err("No pre-captured image".into()),
                            };

                            self.end_capture(ctx, frame);

                            match result {
                                Ok((x1, y1, x2, y2, img_path)) => {
                                    log_error(&format!("capture success: {}", img_path));
                                    if self.is_add_state_mode {
                                        if let (Some(elem_name), Some(last_state)) = (
                                            self.last_element_name.clone(),
                                            self.last_state_name.clone(),
                                        ) {
                                            // Re-record: always overwrite current state.
                                            if self.recapture {
                                                self.recapture = false;
                                                let state = self
                                                    .forced_state_name
                                                    .take()
                                                    .unwrap_or(last_state);
                                                let existing_id = self.last_id;
                                                match self.commit_named_capture(
                                                    x1,
                                                    y1,
                                                    x2,
                                                    y2,
                                                    &img_path,
                                                    &elem_name,
                                                    &state,
                                                    existing_id,
                                                    true,
                                                    true,
                                                ) {
                                                    Ok(msg) => {
                                                        self.status = format!("重录完成: {msg}");
                                                    }
                                                    Err(e) => {
                                                        self.status = format!("重录失败: {e}");
                                                    }
                                                }
                                                self.mode = AppMode::Normal;
                                            } else {
                                            let next_state = self
                                                .forced_state_name
                                                .take()
                                                .unwrap_or_else(|| get_next_state_suffix(&last_state));
                                            let existing_id = self.last_id;
                                            if let Some((eid, message)) = self.check_name_conflict(
                                                &elem_name,
                                                &next_state,
                                                true,
                                                existing_id,
                                            ) {
                                                self.mode = AppMode::ConfirmOverwrite {
                                                    x1,
                                                    y1,
                                                    x2,
                                                    y2,
                                                    temp_img: img_path,
                                                    name: elem_name,
                                                    state_name: next_state,
                                                    existing_id: eid.or(existing_id),
                                                    is_add_state: true,
                                                    message,
                                                };
                                                self.status = "Name conflict — confirm overwrite".into();
                                            } else {
                                                match self.commit_named_capture(
                                                    x1, y1, x2, y2, &img_path, &elem_name, &next_state,
                                                    existing_id, true, false,
                                                ) {
                                                    Ok(msg) => self.status = msg,
                                                    Err(e) => self.status = format!("Error: {e}"),
                                                }
                                                self.mode = AppMode::Normal;
                                            }
                                            }
                                        } else {
                                            self.recapture = false;
                                            self.status = "No element selected".into();
                                            self.mode = AppMode::Normal;
                                        }
                                    } else {
                                        self.mode = AppMode::InputName(x1, y1, x2, y2, img_path);
                                        self.input.clear();
                                        self.status = "Enter element name:".into();
                                    }
                                }
                                Err(e) => {
                                    self.recapture = false;
                                    self.mode = AppMode::Normal;
                                    self.status = e;
                                }
                            }
                        }
                    }
                }

                // Mode may have left Capturing inside Up/timeout — do not paint black overlay.
                if !matches!(self.mode, AppMode::Capturing) {
                    return;
                }

                egui::CentralPanel::default()
                    .frame(egui::Frame::none().fill(Color32::BLACK))
                    .show(ctx, |ui| {
                        let painter = ui.painter();
                        let s = self.scale;
                        // Overlay is sized in logical points; rdev gives physical pixels.
                        let to_ui = |v: i32| v as f32 / s;
                        let sw = to_ui(self.sw as i32);
                        let sh = to_ui(self.sh as i32);

                        if let Some((tex, _)) = &self.pre_capture {
                            let bg_rect = Rect::from_min_max(Pos2::ZERO, Pos2::new(sw, sh));
                            painter.image(
                                tex.id(),
                                bg_rect,
                                Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)),
                                Color32::WHITE,
                            );
                        }

                        let dim = Color32::from_black_alpha(160);
                        let cross_color = Color32::from_rgba_unmultiplied(255, 255, 255, 180);

                        if let (Some((sx, sy)), Some((cx, cy))) = (self.drag_start, self.drag_current) {
                            let x1 = to_ui(sx.min(cx));
                            let y1 = to_ui(sy.min(cy));
                            let x2 = to_ui(sx.max(cx));
                            let y2 = to_ui(sy.max(cy));

                            painter.rect_filled(Rect::from_min_max(Pos2::new(0.0, 0.0), Pos2::new(sw,  y1)), 0.0, dim);
                            painter.rect_filled(Rect::from_min_max(Pos2::new(0.0, y2),  Pos2::new(sw,  sh)), 0.0, dim);
                            painter.rect_filled(Rect::from_min_max(Pos2::new(0.0, y1),  Pos2::new(x1,  y2)), 0.0, dim);
                            painter.rect_filled(Rect::from_min_max(Pos2::new(x2,  y1),  Pos2::new(sw,  y2)), 0.0, dim);

                            let sel = Rect::from_min_max(Pos2::new(x1, y1), Pos2::new(x2, y2));
                            painter.rect_stroke(sel, 0.0, Stroke::new(2.0, Color32::WHITE));
                            painter.rect_stroke(sel, 0.0, Stroke::new(1.0, Color32::from_rgb(30, 160, 255)));

                            for (px, py) in [(x1,y1),(x2,y1),(x1,y2),(x2,y2)] {
                                painter.rect_filled(
                                    Rect::from_center_size(Pos2::new(px, py), egui::vec2(8.0, 8.0)),
                                    0.0, Color32::WHITE,
                                );
                            }
                            for (px, py) in [(x1,(y1+y2)/2.0),(x2,(y1+y2)/2.0),((x1+x2)/2.0,y1),((x1+x2)/2.0,y2)] {
                                painter.rect_filled(
                                    Rect::from_center_size(Pos2::new(px, py), egui::vec2(6.0, 6.0)),
                                    0.0, Color32::WHITE,
                                );
                            }

                            let w = (sx - cx).abs();
                            let h = (sy - cy).abs();
                            let dims = format!(" {}x{} ", w, h);
                            let lx = if x2 + 4.0 + dims.len() as f32 * 8.5 < sw { x2 + 4.0 } else { x1 };
                            let ly = if y2 + 24.0 < sh { y2 + 4.0 } else { y1 - 24.0 };
                            let label_bg = Rect::from_min_size(Pos2::new(lx, ly), egui::vec2(dims.len() as f32 * 8.5, 20.0));
                            painter.rect_filled(label_bg, 3.0, Color32::from_rgb(30, 160, 255));
                            painter.text(label_bg.center(), egui::Align2::CENTER_CENTER, &dims, FontId::proportional(14.0), Color32::WHITE);
                        } else {
                            painter.rect_filled(Rect::from_min_max(Pos2::ZERO, Pos2::new(sw, sh)), 0.0, dim);

                            if let Some((mx, my)) = self.mouse_pos {
                                let mx = to_ui(mx);
                                let my = to_ui(my);
                                painter.line_segment([Pos2::new(0.0, my), Pos2::new(sw, my)], Stroke::new(1.0, cross_color));
                                painter.line_segment([Pos2::new(mx, 0.0), Pos2::new(mx, sh)], Stroke::new(1.0, cross_color));

                                let coord = format!(" ({}, {}) ", (mx * s) as i32, (my * s) as i32);
                                let tip_x = if mx + 80.0 < sw { mx + 12.0 } else { mx - 12.0 - coord.len() as f32 * 7.5 };
                                let tip_y = if my + 28.0 < sh { my + 12.0 } else { my - 28.0 };
                                let tip_bg = Rect::from_min_size(Pos2::new(tip_x, tip_y), egui::vec2(coord.len() as f32 * 7.5, 20.0));
                                painter.rect_filled(tip_bg, 3.0, Color32::from_black_alpha(200));
                                painter.text(tip_bg.center(), egui::Align2::CENTER_CENTER, &coord, FontId::proportional(13.0), Color32::WHITE);
                            }

                            let hint = "Click and drag to select   ESC to cancel";
                            painter.text(Pos2::new(sw / 2.0, sh - 40.0), egui::Align2::CENTER_CENTER, hint, FontId::proportional(16.0), Color32::from_white_alpha(200));
                        }

                        if ui.input(|i| i.key_pressed(egui::Key::Escape)) || escape_key_down() {
                            self.end_capture(ctx, frame);
                            self.recapture = false;
                            self.mode = AppMode::Normal;
                            self.status = "Cancelled".into();
                        }
                    });
            }
            _ => {
                egui::CentralPanel::default()
                    .frame(egui::Frame::central_panel(&ctx.style()))
                    .show(ctx, |ui| {
                    ui.heading("Mouse Recorder");
                    ui.label(format!("Resolution: {}x{}", self.sw, self.sh));
                    ui.separator();

                    let mut search_focused = false;
                    ui.horizontal(|ui| {
                        ui.label("搜索:");
                        let resp = ui.add(
                            egui::TextEdit::singleline(&mut self.name_filter)
                                .desired_width(220.0)
                                .hint_text("按名称筛选…"),
                        );
                        search_focused = resp.has_focus();
                        if !self.name_filter.is_empty() {
                            if ui.button("清除").clicked() {
                                self.name_filter.clear();
                            }
                        }
                        ui.label("分组:");
                        let groups = self.unique_groups();
                        let cur_g = if self.group_filter.is_empty() {
                            "全部".to_string()
                        } else {
                            self.group_filter.clone()
                        };
                        egui::ComboBox::from_id_salt("group_filter")
                            .selected_text(&cur_g)
                            .width(120.0)
                            .show_ui(ui, |ui| {
                                if ui
                                    .selectable_label(self.group_filter.is_empty(), "全部")
                                    .clicked()
                                {
                                    self.group_filter.clear();
                                }
                                for g in &groups {
                                    if ui
                                        .selectable_label(self.group_filter.as_str() == g, g)
                                        .clicked()
                                    {
                                        self.group_filter = g.clone();
                                    }
                                }
                            });
                        let n_match = self.filtered_indices().len();
                        ui.label(
                            egui::RichText::new(format!(
                                "{}/{}",
                                n_match,
                                self.elements.len()
                            ))
                            .color(Color32::GRAY),
                        );
                        if search_focused
                            && ui.input(|i| i.key_pressed(egui::Key::Enter))
                        {
                            let idxs = self.filtered_indices();
                            if let Some(&i) = idxs.first() {
                                let id = self.elements[i].id;
                                self.last_id = Some(id);
                                self.sync_last_selection();
                                self.preview = None;
                                self.status = format!("Selected #{}", id);
                            }
                        }
                    });

                    // Resolution mismatch banner
                    if let Some(e) = self.selected_element().cloned() {
                        if e.screen_w > 0
                            && e.screen_h > 0
                            && (e.screen_w != self.sw as i32 || e.screen_h != self.sh as i32)
                        {
                            ui.horizontal(|ui| {
                                ui.colored_label(
                                    Color32::YELLOW,
                                    format!(
                                        "分辨率: 录制 {}x{} ≠ 当前 {}x{}",
                                        e.screen_w, e.screen_h, self.sw, self.sh
                                    ),
                                );
                                if ui.button("缩放到当前屏").clicked() {
                                    match rescale_element_geometry(
                                        &self.conn,
                                        &e,
                                        self.sw as i32,
                                        self.sh as i32,
                                    ) {
                                        Ok(()) => {
                                            self.refresh();
                                            self.status = format!(
                                                "#{} 几何已缩放到 {}x{}",
                                                e.id, self.sw, self.sh
                                            );
                                        }
                                        Err(err) => self.status = format!("缩放失败: {err}"),
                                    }
                                }
                            });
                        }
                    }

                    let list_h = 260.0;
                    let preview_w = 300.0;
                    let dups = matchutil::name_dup_counts(
                        self.elements.iter().map(|e| e.name.as_str()),
                    );
                    ui.horizontal(|ui| {
                        let list_w = (ui.available_width() - preview_w - 12.0).max(260.0);
                        ui.vertical(|ui| {
                            ui.set_width(list_w);
                            ScrollArea::vertical()
                                .max_height(list_h)
                                .id_salt("elem_list")
                                .show(ui, |ui| {
                                    if self.elements.is_empty() {
                                        ui.colored_label(
                                            Color32::GRAY,
                                            "No elements. Click [New Element] to record.",
                                        );
                                    }
                                    let idxs = self.filtered_indices();
                                    if !self.elements.is_empty() && idxs.is_empty() {
                                        ui.colored_label(
                                            Color32::YELLOW,
                                            "无匹配 — 试试缩短关键词或切换分组",
                                        );
                                    }
                                    let mut clicked_id: Option<i32> = None;
                                    let mut clicked_state: Option<(i32, String)> = None;
                                    for &i in &idxs {
                                        let e = &self.elements[i];
                                        let selected = Some(e.id) == self.last_id;
                                        let health = self.health_for(e, &dups);
                                        let (hr, hg, hb) = health.level.color_rgb();
                                        let badge = if health.badges.is_empty() {
                                            String::new()
                                        } else {
                                            format!("[{}] ", health.badges.join("|"))
                                        };
                                        let lab = format!(
                                            "{}#{} {}  center({}, {})",
                                            badge, e.id, e.name, e.cx, e.cy
                                        );
                                        let resp = ui.selectable_label(
                                            selected,
                                            egui::RichText::new(lab)
                                                .color(Color32::from_rgb(hr, hg, hb)),
                                        );
                                        let resp = if health.hints.is_empty() {
                                            resp
                                        } else {
                                            resp.on_hover_text(health.hints.join("\n"))
                                        };
                                        if resp.clicked() {
                                            clicked_id = Some(e.id);
                                        }
                                        for st in &e.states {
                                            let state_sel = selected
                                                && self.last_state_name.as_deref()
                                                    == Some(st.name.as_str());
                                            let m = if st.is_primary { " [PRIMARY]" } else { "" };
                                            let slab = format!(
                                                "    {}  {}{}",
                                                st.name, st.image_path, m
                                            );
                                            if ui.selectable_label(state_sel, &slab).clicked() {
                                                clicked_state = Some((e.id, st.name.clone()));
                                            }
                                        }
                                    }
                                    if let Some(id) = clicked_id {
                                        self.last_id = Some(id);
                                        self.sync_last_selection();
                                        self.preview = None;
                                        self.status = format!("Selected #{}", id);
                                    }
                                    if let Some((id, sn)) = clicked_state {
                                        self.last_id = Some(id);
                                        self.last_state_name = Some(sn);
                                        self.sync_last_selection();
                                        self.preview = None;
                                        self.status = "State selected".into();
                                    }
                                });
                        });
                        ui.vertical(|ui| {
                            ui.set_width(preview_w);
                            self.show_preview_panel(ctx, ui, preview_w - 16.0, list_h - 40.0);
                        });
                    });

                    ui.separator();
                    ui.colored_label(Color32::GREEN, &self.status);

                    match &self.mode {
                        AppMode::Normal => {
                            // Global shortcuts when an element is selected
                            if self.last_id.is_some() {
                                if ui.input(|i| i.key_pressed(egui::Key::F7)) {
                                    self.begin_view_locate(ctx, frame);
                                }
                                if ui.input(|i| i.key_pressed(egui::Key::F8)) {
                                    self.begin_edit_roi(ctx, frame);
                                }
                                if ui.input(|i| i.key_pressed(egui::Key::F9)) {
                                    let text = self.location_summary_text();
                                    if !text.is_empty() {
                                        ctx.copy_text(text);
                                        self.status = "Copied coords [F9]".into();
                                    }
                                }
                                if ui.input(|i| i.key_pressed(egui::Key::F11)) {
                                    self.begin_verify_match(ctx, frame);
                                }
                                if ui.input(|i| i.key_pressed(egui::Key::F12)) {
                                    self.begin_recapture(ctx, frame);
                                }
                            }
                            // List navigate with Up/Down (skip while typing in search)
                            if !search_focused {
                                if ui.input(|i| i.key_pressed(egui::Key::ArrowDown)) {
                                    self.navigate_filtered(1);
                                    if let Some(id) = self.last_id {
                                        self.status = format!("Selected #{}", id);
                                    }
                                }
                                if ui.input(|i| i.key_pressed(egui::Key::ArrowUp)) {
                                    self.navigate_filtered(-1);
                                    if let Some(id) = self.last_id {
                                        self.status = format!("Selected #{}", id);
                                    }
                                }
                            }

                            ui.horizontal(|ui| {
                                if ui.button("New Element  [F5]").clicked()
                                    || ui.input(|i| i.key_pressed(egui::Key::F5))
                                {
                                    self.recapture = false;
                                    self.forced_state_name = None;
                                    self.begin_capture(ctx, frame, false);
                                }
                                if self.last_id.is_some() {
                                    if ui.button("Add State  [F6]").clicked()
                                        || ui.input(|i| i.key_pressed(egui::Key::F6))
                                    {
                                        self.recapture = false;
                                        self.forced_state_name = None;
                                        self.begin_capture(ctx, frame, true);
                                    }
                                    if ui.button("验配 [F11]").clicked() {
                                        self.begin_verify_match(ctx, frame);
                                    }
                                    if ui.button("重录 [F12]").clicked() {
                                        self.begin_recapture(ctx, frame);
                                    }
                                    if ui.button("改名").clicked() {
                                        self.input = self
                                            .last_element_name
                                            .clone()
                                            .unwrap_or_default();
                                        self.mode = AppMode::InputRename;
                                        self.status = "输入新名称:".into();
                                    }
                                    if ui.button("状态对比").clicked() {
                                        self.load_compare_textures(ctx);
                                        self.mode = AppMode::CompareStates;
                                        self.status = "状态对比".into();
                                    }
                                    if ui.button("View Loc 查看 [F7]").clicked() {
                                        self.begin_view_locate(ctx, frame);
                                    }
                                    if ui.button("Details 详情 [F10]").clicked()
                                        || ui.input(|i| i.key_pressed(egui::Key::F10))
                                    {
                                        self.mode = AppMode::ViewDetail;
                                        self.status = "Element details".into();
                                    }
                                    if ui.button("Edit ROI 编辑 [F8]").clicked() {
                                        self.begin_edit_roi(ctx, frame);
                                    }
                                    if ui.button("Copy 复制 [F9]").clicked() {
                                        let text = self.location_summary_text();
                                        ctx.copy_text(text);
                                        self.status = "Copied coords".into();
                                    }
                                }
                                if ui.button("Export CSV").clicked() {
                                    self.mode = AppMode::InputExport;
                                    self.input.clear();
                                    self.status = "Enter export filename:".into();
                                }
                                if ui.button("打开目录").clicked() {
                                    self.open_data_folder();
                                    self.status = format!("Opened {}", self.image_dir);
                                }
                                if ui.button("Refresh").clicked() {
                                    self.refresh();
                                    self.preview = None;
                                    self.status = "Refreshed".into();
                                }
                            });
                            ui.label(
                                egui::RichText::new(
                                    "F5新建 F6状态 F7查看 F8编辑 F9复制 F10详情 F11验配 F12重录 | ↑↓筛选列表",
                                )
                                .small()
                                .color(Color32::GRAY),
                            );
                            if self.last_id.is_some() {
                                ui.separator();
                                ui.label("Quick state capture:");
                                ui.horizontal(|ui| {
                                    for (key, label, suffix) in [
                                        (egui::Key::F1, "F1  Normal (-n)", "-n"),
                                        (egui::Key::F2, "F2  Selected (-s)", "-s"),
                                        (egui::Key::F3, "F3  Clicked (-c)", "-c"),
                                        (egui::Key::F4, "F4  Click+Sel (-cs)", "-cs"),
                                    ] {
                                        let pressed = ui.input(|i| i.key_pressed(key));
                                        if ui.button(label).clicked() || pressed {
                                            self.recapture = false;
                                            self.forced_state_name = Some(suffix.to_string());
                                            self.begin_capture(ctx, frame, true);
                                        }
                                    }
                                });
                            }
                        }
                        AppMode::InputRename => {
                            ui.label("改名 Rename（重名会拦截）:");
                            ui.text_edit_singleline(&mut self.input);
                            ui.horizontal(|ui| {
                                if ui.button("确认改名").clicked()
                                    || ui.input(|i| i.key_pressed(egui::Key::Enter))
                                {
                                    if let (Some(id), Some(old)) =
                                        (self.last_id, self.last_element_name.clone())
                                    {
                                        let new_name = self.input.clone();
                                        match rename_element(
                                            &self.conn,
                                            &self.image_dir,
                                            id,
                                            &old,
                                            &new_name,
                                        ) {
                                            Ok(()) => {
                                                self.refresh();
                                                self.preview = None;
                                                self.mode = AppMode::Normal;
                                                self.input.clear();
                                                self.status =
                                                    format!("已改名 #{} → {}", id, new_name.trim());
                                            }
                                            Err(e) => self.status = format!("改名失败: {e}"),
                                        }
                                    }
                                }
                                if ui.button("取消").clicked()
                                    || ui.input(|i| i.key_pressed(egui::Key::Escape))
                                {
                                    self.mode = AppMode::Normal;
                                    self.input.clear();
                                    self.status = "Cancelled".into();
                                }
                            });
                        }
                        AppMode::CompareStates => {
                            let title = self
                                .selected_element()
                                .map(|e| format!("状态对比 — #{} {}", e.id, e.name))
                                .unwrap_or_else(|| "状态对比".into());
                            ui.heading(title);
                            if self.compare_tex.is_empty() {
                                ui.colored_label(Color32::GRAY, "无状态图片");
                            } else {
                                ScrollArea::horizontal().show(ui, |ui| {
                                    ui.horizontal(|ui| {
                                        for (label, tex) in &self.compare_tex {
                                            ui.group(|ui| {
                                                ui.label(label);
                                                let tw = tex.size()[0] as f32;
                                                let th = tex.size()[1] as f32;
                                                let scale =
                                                    (160.0 / tw).min(120.0 / th).min(4.0).max(0.1);
                                                ui.image((
                                                    tex.id(),
                                                    egui::vec2(tw * scale, th * scale),
                                                ));
                                            });
                                        }
                                    });
                                });
                            }
                            if ui.button("关闭").clicked()
                                || ui.input(|i| i.key_pressed(egui::Key::Escape))
                            {
                                self.compare_tex.clear();
                                self.mode = AppMode::Normal;
                                self.status = "Ready".into();
                            }
                        }
                        AppMode::ViewDetail => {
                            // Shortcuts while viewing
                            if ui.input(|i| i.key_pressed(egui::Key::F7)) {
                                self.begin_view_locate(ctx, frame);
                            }
                            if ui.input(|i| i.key_pressed(egui::Key::F8) || i.key_pressed(egui::Key::E))
                            {
                                self.begin_edit_roi(ctx, frame);
                            }
                            if ui.input(|i| {
                                i.key_pressed(egui::Key::F9) || i.key_pressed(egui::Key::C)
                            }) {
                                let text = self.location_summary_text();
                                ctx.copy_text(text);
                                self.status = "Copied coords".into();
                            }
                            if let Some(e) = self.selected_element().cloned() {
                                let st = self.selected_state().cloned();
                                let img_path = st
                                    .as_ref()
                                    .map(|s| {
                                        std::path::Path::new(&self.image_dir)
                                            .join(&s.image_path)
                                            .to_string_lossy()
                                            .to_string()
                                    })
                                    .unwrap_or_default();
                                let (iw, ih) =
                                    png_size(std::path::Path::new(&img_path)).unwrap_or((0, 0));

                                ui.heading(format!("#{}  {}", e.id, e.name));
                                ui.separator();
                                ui.horizontal(|ui| {
                                    ui.vertical(|ui| {
                                        ui.set_min_width(360.0);
                                        ui.label(format!("1. Center 中心点:  ({}, {})", e.cx, e.cy));
                                        ui.label(format!(
                                            "2. Image 图片区域:  {} x {} px    file: {}    (anchored to center)",
                                            iw,
                                            ih,
                                            st.as_ref().map(|s| s.image_path.as_str()).unwrap_or("-")
                                        ));
                                        ui.label(format!(
                                            "3. ROI 搜索范围:  x={}  y={}  w={}  h={}    end=({}, {})  (should be > image)",
                                            e.bx,
                                            e.by,
                                            e.bw,
                                            e.bh,
                                            e.bx + e.bw,
                                            e.by + e.bh
                                        ));
                                        if e.bw <= iw as i32 || e.bh <= ih as i32 {
                                            ui.colored_label(
                                                Color32::YELLOW,
                                                format!(
                                                    "提示: ROI 未大于图片；打开查看/编辑时会按自适应边距外扩 (pad≈{}px)",
                                                    adaptive_roi_pad(iw as i32, ih as i32)
                                                ),
                                            );
                                        }
                                        ui.label(format!(
                                            "Screen 分辨率: {}x{}    State 状态: {}{}",
                                            e.screen_w,
                                            e.screen_h,
                                            st.as_ref().map(|s| s.name.as_str()).unwrap_or("-"),
                                            if st.as_ref().map(|s| s.is_primary).unwrap_or(false) {
                                                " [PRIMARY]"
                                            } else {
                                                ""
                                            }
                                        ));
                                        ui.label(format!("Created 创建: {}", e.created_at));
                                    });
                                    self.show_preview_panel(ctx, ui, 320.0, 220.0);
                                });

                                ui.separator();
                                ui.horizontal(|ui| {
                                    if ui.button("Copy 复制 [F9/C]").clicked() {
                                        let text = self.location_summary_text();
                                        ctx.copy_text(text);
                                        self.status = "Copied to clipboard".into();
                                    }
                                    if ui.button("Locate 屏幕定位 [F7]").clicked() {
                                        self.begin_view_locate(ctx, frame);
                                    }
                                    if ui.button("Edit ROI 编辑 [F8/E]").clicked() {
                                        self.begin_edit_roi(ctx, frame);
                                    }
                                    if let Some(st) = &st {
                                        if !st.is_primary
                                            && ui.button("Set Primary 设为主状态").clicked()
                                        {
                                            match set_primary_state(
                                                &self.conn, e.id, &st.name,
                                            ) {
                                                Ok(()) => {
                                                    self.refresh();
                                                    self.status =
                                                        format!("Primary -> {}", st.name);
                                                }
                                                Err(err) => {
                                                    self.status = format!("Error: {err}");
                                                }
                                            }
                                        }
                                        if ui.button("Delete State 删除状态").clicked() {
                                            self.mode = AppMode::ConfirmDelete {
                                                element_id: e.id,
                                                state_name: Some(st.name.clone()),
                                                message: format!(
                                                    "Delete state「{}」and its image?",
                                                    st.name
                                                ),
                                            };
                                        }
                                    }
                                    if ui.button("Delete Element 删除元件").clicked() {
                                        self.mode = AppMode::ConfirmDelete {
                                            element_id: e.id,
                                            state_name: None,
                                            message: format!(
                                                "Delete element「{}」and ALL states/images?",
                                                e.name
                                            ),
                                        };
                                    }
                                    if ui.button("Close 关闭 [Esc]").clicked()
                                        || ui.input(|i| i.key_pressed(egui::Key::Escape))
                                    {
                                        self.mode = AppMode::Normal;
                                        self.status = "Ready".into();
                                    }
                                });
                            } else {
                                ui.label("No element selected 未选择元件");
                                if ui.button("Close 关闭").clicked() {
                                    self.mode = AppMode::Normal;
                                }
                            }
                        }
                        AppMode::ConfirmDelete {
                            element_id,
                            state_name,
                            message,
                        } => {
                            let element_id = *element_id;
                            let state_name = state_name.clone();
                            let message = message.clone();
                            ui.colored_label(Color32::YELLOW, &message);
                            ui.horizontal(|ui| {
                                if ui.button("Confirm Delete 确认删除").clicked() {
                                    let result = match &state_name {
                                        Some(sn) => {
                                            delete_state(&self.conn, &self.image_dir, element_id, sn)
                                        }
                                        None => {
                                            delete_element(&self.conn, &self.image_dir, element_id)
                                        }
                                    };
                                    match result {
                                        Ok(()) => {
                                            self.preview = None;
                                            self.refresh();
                                            if state_name.is_none() {
                                                self.last_id = self.elements.last().map(|e| e.id);
                                            }
                                            self.sync_last_selection();
                                            self.mode = AppMode::Normal;
                                            self.status = "Deleted".into();
                                        }
                                        Err(e) => {
                                            self.status = format!("Delete error: {e}");
                                            self.mode = AppMode::Normal;
                                        }
                                    }
                                }
                                if ui.button("Cancel 取消").clicked() {
                                    self.mode = AppMode::ViewDetail;
                                }
                            });
                        }
                        AppMode::Capturing => {
                            ui.colored_label(Color32::RED, "Click and drag on screen to select region");
                            if ui.button("Cancel").clicked() {
                                self.end_capture(ctx, frame);
                                self.recapture = false;
                                self.mode = AppMode::Normal;
                                self.status = "Cancelled".into();
                            }
                        }
                        AppMode::ConfirmOverwrite {
                            x1,
                            y1,
                            x2,
                            y2,
                            temp_img,
                            name,
                            state_name,
                            existing_id,
                            is_add_state,
                            message,
                        } => {
                            let x1 = *x1;
                            let y1 = *y1;
                            let x2 = *x2;
                            let y2 = *y2;
                            let temp_img = temp_img.clone();
                            let name = name.clone();
                            let state_name = state_name.clone();
                            let existing_id = *existing_id;
                            let is_add_state = *is_add_state;
                            let message = message.clone();
                            ui.colored_label(Color32::YELLOW, &message);
                            ui.horizontal(|ui| {
                                if ui.button("Overwrite 覆盖").clicked() {
                                    match self.commit_named_capture(
                                        x1, y1, x2, y2, &temp_img, &name, &state_name,
                                        existing_id, is_add_state, true,
                                    ) {
                                        Ok(msg) => self.status = msg,
                                        Err(e) => self.status = format!("Error: {e}"),
                                    }
                                    self.mode = AppMode::Normal;
                                    self.input.clear();
                                }
                                if ui.button("Keep Old 取消").clicked() {
                                    let _ = fs::remove_file(&temp_img);
                                    self.mode = AppMode::Normal;
                                    self.input.clear();
                                    self.status = "Cancelled — kept existing data".into();
                                }
                            });
                        }
                        _ => {
                            let prompt = match &self.mode {
                                AppMode::InputName(..) => "Element name:",
                                AppMode::InputState(..) => "State name (default: -n):",
                                AppMode::InputExport => "Export filename (no extension):",
                                _ => "",
                            };
                            ui.label(prompt);
                            let resp = ui.text_edit_singleline(&mut self.input);
                            ui.horizontal(|ui| {
                                if ui.button("OK").clicked() || (resp.has_focus() && ui.input_mut(|i| i.key_pressed(egui::Key::Enter))) {
                                    let val = self.input.trim().to_string();
                                    let old = std::mem::replace(&mut self.mode, AppMode::Normal);
                                    match old {
                                        AppMode::InputName(x1, y1, x2, y2, img) if !val.is_empty() => {
                                            self.mode = AppMode::InputState(
                                                x1, y1, x2, y2, img, val, "-n".to_string(),
                                            );
                                            self.input.clear();
                                            self.status = "Enter state name (default: -n):".into();
                                        }
                                        AppMode::InputState(x1, y1, x2, y2, img, name, default_state) => {
                                            let state_name =
                                                if val.is_empty() { default_state } else { val };
                                            if let Some((eid, message)) = self.check_name_conflict(
                                                &name, &state_name, false, None,
                                            ) {
                                                self.mode = AppMode::ConfirmOverwrite {
                                                    x1,
                                                    y1,
                                                    x2,
                                                    y2,
                                                    temp_img: img,
                                                    name,
                                                    state_name,
                                                    existing_id: eid,
                                                    is_add_state: false,
                                                    message,
                                                };
                                                self.status = "Name conflict — confirm overwrite".into();
                                            } else {
                                                match self.commit_named_capture(
                                                    x1, y1, x2, y2, &img, &name, &state_name,
                                                    None, false, false,
                                                ) {
                                                    Ok(msg) => self.status = msg,
                                                    Err(e) => self.status = format!("Error: {e}"),
                                                }
                                                self.input.clear();
                                            }
                                        }
                                        AppMode::InputExport if !val.is_empty() => {
                                            match export_csv(&self.conn, &val, &self.config) {
                                                Ok(p) => self.status = format!("Exported: {}", p),
                                                Err(e) => self.status = format!("Export error: {}", e),
                                            }
                                            self.input.clear();
                                        }
                                        _ => {
                                            self.status = "Cancelled".into();
                                            self.input.clear();
                                        }
                                    }
                                }
                                if ui.button("Cancel").clicked() {
                                    self.mode = AppMode::Normal;
                                    self.input.clear();
                                    self.status = "Cancelled".into();
                                }
                            });
                        }
                    }
                });
            }
        }
    }
}

fn main() -> eframe::Result {
    setup_panic_hook();
    let cfg = Config::load();
    let db_path = cfg.db_path();
    let image_dir = cfg.image_dir();
    let conn = init_db(&db_path);
    let (sw, sh) = get_resolution();
    let elems = load_elements(&conn);
    let last = elems.last().map(|e| e.id);
    let flag = Arc::new(AtomicBool::new(false));
    let (tx, rx) = mpsc::channel();
    start_listener(flag.clone(), tx);
    let opts = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([920.0, 700.0])
            .with_title("Mouse Recorder"),
        ..Default::default()
    };
    eframe::run_native("Mouse Recorder", opts, Box::new(|cc| {
        install_cjk_fonts(&cc.egui_ctx);
        let mut app = RecorderApp {
            conn,
            config: cfg,
            image_dir,
            elements: elems,
            last_id: last,
            last_element_name: None,
            last_state_name: None,
            is_add_state_mode: false,
            sw, sh,
            scale: 1.0,
            mode: AppMode::Normal,
            input: String::new(),
            name_filter: String::new(),
            group_filter: String::new(),
            recapture: false,
            compare_tex: Vec::new(),
            status: "Ready. Click [New Element] to record".into(),
            rx,
            capture_flag: flag,
            drag_start: None,
            drag_current: None,
            mouse_pos: None,
            capture_started_at: None,
            forced_state_name: None,
            pre_capture: None,
            preview: None,
            overlay_template: None,
        };
        app.sync_last_selection();
        Ok(Box::new(app))
    }))
}