#![windows_subsystem = "windows"]

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
use xcap::Monitor;

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

#[cfg(windows)]
fn set_window_exclude_from_capture(exclude: bool) {
    use std::ffi::OsStr;
    use std::os::windows::ffi::OsStrExt;
    use winapi::um::winuser::{FindWindowW, SetWindowDisplayAffinity, WDA_NONE};
    let title: Vec<u16> = OsStr::new("Mouse Recorder").encode_wide().chain(std::iter::once(0)).collect();
    unsafe {
        let hwnd = FindWindowW(std::ptr::null(), title.as_ptr());
        if !hwnd.is_null() {
            let affinity = if exclude { 0x11u32 } else { WDA_NONE };
            SetWindowDisplayAffinity(hwnd, affinity);
        }
    }
}

#[cfg(not(windows))]
fn set_window_exclude_from_capture(_exclude: bool) {}

fn get_next_state_suffix(current: &str) -> String {
    let suffixes = ["-n", "-s", "-h", "-d", "-f", "-a", "-p"];
    for i in 0..suffixes.len() - 1 {
        if current == suffixes[i] {
            return suffixes[i + 1].to_string();
        }
    }
    "-n".to_string()
}

fn get_resolution() -> (u32, u32) {
    Monitor::all()
        .ok()
        .and_then(|m| m.first().map(|mon| (mon.width().unwrap_or(1920), mon.height().unwrap_or(1080))))
        .unwrap_or((1920, 1080))
}

fn init_db(path: &str) -> Connection {
    if let Some(parent) = std::path::Path::new(path).parent() {
        fs::create_dir_all(parent).expect("Failed to create DB directory");
    }
    let conn = Connection::open(path).expect("Failed to open DB");
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
    ).expect("Fail DB init");
    conn
}

enum ScreenshotError {
    Validation(String),
    Capture(String),
}

impl ScreenshotError {
    fn into_string(self) -> String {
        match self { ScreenshotError::Validation(s) | ScreenshotError::Capture(s) => s }
    }
    fn is_retriable(&self) -> bool {
        matches!(self, ScreenshotError::Capture(_))
    }
}

fn take_screenshot(x1: i32, y1: i32, x2: i32, y2: i32, cfg: &Config) -> Result<String, ScreenshotError> {
    let px = x1.min(x2) as i32;
    let py = y1.min(y2) as i32;
    let pw = (x1 - x2).abs() as u32;
    let ph = (y1 - y2).abs() as u32;
    if pw == 0 || ph == 0 {
        return Err(ScreenshotError::Validation("Zero region".into()));
    }
    let ts = Local::now().format("%Y%m%d_%H%M%S_%3f");
    let img_dir = cfg.image_dir();
    fs::create_dir_all(&img_dir).map_err(|e| ScreenshotError::Capture(e.to_string()))?;
    let path = std::path::Path::new(&img_dir).join(format!("capture_{}.png", ts));

    let monitors = Monitor::all().map_err(|e| ScreenshotError::Capture(e.to_string()))?;
    if monitors.is_empty() {
        return Err(ScreenshotError::Capture("No monitor".into()));
    }

    let cx_pt = px + pw as i32 / 2;
    let cy_pt = py + ph as i32 / 2;
    let mon = monitors.iter()
        .find(|m| {
            let mx = m.x().unwrap_or(0);
            let my = m.y().unwrap_or(0);
            let mw = m.width().unwrap_or(1920) as i32;
            let mh = m.height().unwrap_or(1080) as i32;
            cx_pt >= mx && cx_pt < mx + mw && cy_pt >= my && cy_pt < my + mh
        })
        .unwrap_or(&monitors[0]);

    let mon_x = mon.x().unwrap_or(0);
    let mon_y = mon.y().unwrap_or(0);
    let local_px = (px - mon_x).max(0) as u32;
    let local_py = (py - mon_y).max(0) as u32;

    let full = mon.capture_image().map_err(|e| ScreenshotError::Capture(e.to_string()))?;

    let pw = pw.min(full.width().saturating_sub(local_px));
    let ph = ph.min(full.height().saturating_sub(local_py));
    if pw == 0 || ph == 0 {
        return Err(ScreenshotError::Validation("Region out of bounds".into()));
    }

    let mut cropped = image::ImageBuffer::new(pw, ph);
    for dy in 0..ph {
        for dx in 0..pw {
            cropped.put_pixel(dx, dy, *full.get_pixel(local_px + dx, local_py + dy));
        }
    }
    cropped.save(&path).map_err(|e| ScreenshotError::Capture(e.to_string()))?;
    Ok(path.to_string_lossy().to_string())
}

#[derive(Clone)]
struct ElementRec {
    id: i32,
    name: String,
    cx: i32, cy: i32,
    states: Vec<(String, bool)>,
}

fn load_elements(conn: &Connection) -> Vec<ElementRec> {
    let mut res = Vec::new();
    if let Ok(mut st) = conn.prepare("SELECT id,name,center_x,center_y FROM elements ORDER BY id") {
        if let Ok(rows) = st.query_map([], |r| Ok((r.get::<_,i32>(0)?, r.get::<_,String>(1)?, r.get::<_,i32>(2)?, r.get::<_,i32>(3)?))) {
            for row in rows.flatten() {
                let (id, name, cx, cy) = row;
                let mut states = Vec::new();
                if let Ok(mut s2) = conn.prepare("SELECT state_name,is_primary FROM element_states WHERE element_id=?1 ORDER BY id") {
                    if let Ok(r2) = s2.query_map(params![id], |r| Ok((r.get::<_,String>(0)?, r.get::<_,bool>(1)?))) {
                        for st in r2.flatten() { states.push(st); }
                    }
                }
                res.push(ElementRec { id, name, cx, cy, states });
            }
        }
    }
    res
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
        writeln!(f, "{},{},{},\"{}\",\"{}\",{},{}", id, cx, cy, name.replace('"', "\"\""), img.replace('"', "\"\""), sw, sh).map_err(|e| e.to_string())?;
    }
    Ok(path_s)
}

fn create_elem(conn: &Connection, name: &str, state: &str, x1: i32, y1: i32, x2: i32, y2: i32, img: &str, sw: u32, sh: u32) -> Result<i32, String> {
    let cx = x1.min(x2) + (x1 - x2).abs() / 2;
    let cy = y1.min(y2) + (y1 - y2).abs() / 2;
    let ts = Local::now().format("%Y%m%d_%H%M%S").to_string();
    let bx = x1.min(x2);
    let by = y1.min(y2);
    let bw = (x1 - x2).abs() as u32;
    let bh = (y1 - y2).abs() as u32;
    conn.execute(
        "INSERT INTO elements(name,center_x,center_y,bbox_x,bbox_y,bbox_width,bbox_height,screen_width,screen_height,created_at) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)",
        params![name, cx, cy, bx, by, bw, bh, sw, sh, ts]
    ).map_err(|e| e.to_string())?;
    let id = conn.last_insert_rowid() as i32;
    conn.execute(
        "INSERT INTO element_states(element_id,state_name,image_path,is_primary,created_at) VALUES(?1,?2,?3,1,?4)",
        params![id, state, img, ts]
    ).map_err(|e| e.to_string())?;
    Ok(id)
}

enum AppMode {
    Normal,
    Capturing,
    PendingScreenshot(i32,i32,i32,i32,mpsc::Receiver<Result<String,String>>,std::time::Instant),
    InputName(i32,i32,i32,i32,String),
    InputState(i32,i32,i32,i32,String,String,String),
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
    status: String,
    rx: mpsc::Receiver<CaptureMsg>,
    capture_flag: Arc<AtomicBool>,
    drag_start: Option<(i32,i32)>,
    drag_current: Option<(i32,i32)>,
    mouse_pos: Option<(i32,i32)>,
    capture_started_at: Option<std::time::Instant>,
    normal_window_size: Option<egui::Vec2>,
    forced_state_name: Option<String>,
    image_dir: String,
}

impl RecorderApp {
    fn refresh(&mut self) {
        self.elements = load_elements(&self.conn);
        self.last_id = self.elements.last().map(|e| e.id);
    }

    fn begin_capture(&mut self, ctx: &egui::Context, is_add_state: bool) {
        self.capture_flag.store(true, Ordering::Relaxed);
        self.drag_start = None;
        self.drag_current = None;
        self.capture_started_at = Some(std::time::Instant::now());
        self.mode = AppMode::Capturing;
        self.is_add_state_mode = is_add_state;
        self.status = "Click and drag on screen to select region...".into();
        self.normal_window_size = Some(ctx.screen_rect().size());
        ctx.send_viewport_cmd(egui::ViewportCommand::OuterPosition(egui::pos2(0.0, 0.0)));
        ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(egui::vec2(self.sw as f32 / self.scale, self.sh as f32 / self.scale)));
        ctx.send_viewport_cmd(egui::ViewportCommand::Decorations(false));
        ctx.send_viewport_cmd(egui::ViewportCommand::WindowLevel(egui::WindowLevel::AlwaysOnTop));
        set_window_exclude_from_capture(true);
    }

    fn end_capture(&mut self, ctx: &egui::Context) {
        self.capture_flag.store(false, Ordering::Relaxed);
        self.capture_started_at = None;
        self.drag_start = None;
        self.drag_current = None;
        self.mouse_pos = None;
        ctx.send_viewport_cmd(egui::ViewportCommand::Decorations(true));
        ctx.send_viewport_cmd(egui::ViewportCommand::WindowLevel(egui::WindowLevel::Normal));
        ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(egui::vec2(600.0, 450.0)));
        ctx.send_viewport_cmd(egui::ViewportCommand::OuterPosition(egui::pos2(100.0, 100.0)));
    }
}

impl eframe::App for RecorderApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        ctx.request_repaint_after(Duration::from_millis(16));
        self.scale = ctx.pixels_per_point();

        match &self.mode {
            AppMode::Capturing => {
                ctx.set_visuals({
                    let mut v = ctx.style().visuals.clone();
                    v.panel_fill = Color32::TRANSPARENT;
                    v.window_fill = Color32::TRANSPARENT;
                    v
                });
                if let Some(started) = self.capture_started_at {
                    if started.elapsed().as_secs() >= 30 {
                        self.end_capture(ctx);
                        self.mode = AppMode::Normal;
                        self.status = "Capture timed out (30s)".into();
                    }
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
                            let saved_start = self.drag_start.take();
                            self.end_capture(ctx);

                            if let Some((sx, sy)) = saved_start {
                                let scale = self.scale;
                                let px1 = (sx as f32 * scale) as i32;
                                let py1 = (sy as f32 * scale) as i32;
                                let px2 = (x as f32 * scale) as i32;
                                let py2 = (y as f32 * scale) as i32;
                                let cfg = self.config.db_path();
                                let cfg_img = self.image_dir.clone();
                                let (stx, srx) = mpsc::channel();
                                thread::spawn(move || {
                                    let cfg = Config { db_path: Some(cfg), image_dir: Some(cfg_img) };
                                    let deadline = std::time::Instant::now() + Duration::from_millis(500);
                                    loop {
                                        thread::sleep(Duration::from_millis(50));
                                        match take_screenshot(px1, py1, px2, py2, &cfg) {
                                            Ok(s) => { let _ = stx.send(Ok(s)); break; }
                                            Err(e) if !e.is_retriable() || std::time::Instant::now() >= deadline => {
                                                let _ = stx.send(Err(e.into_string())); break;
                                            }
                                            Err(_) => continue,
                                        }
                                    }
                                });
                                self.mode = AppMode::PendingScreenshot(px1, py1, px2, py2, srx, std::time::Instant::now());
                                self.status = "Taking screenshot...".into();
                            }
                        }
                    }
                }

                egui::CentralPanel::default()
                    .frame(egui::Frame::none().fill(Color32::TRANSPARENT))
                    .show(ctx, |ui| {
                        let painter = ui.painter();
                        let sw = self.sw as f32 / self.scale;
                        let sh = self.sh as f32 / self.scale;
                        let dim = Color32::from_black_alpha(160);
                        let cross_color = Color32::from_rgba_premultiplied(255, 255, 255, 180);

                        if let (Some((sx, sy)), Some((cx, cy))) = (self.drag_start, self.drag_current) {
                            let x1 = (sx.min(cx)) as f32;
                            let y1 = (sy.min(cy)) as f32;
                            let x2 = (sx.max(cx)) as f32;
                            let y2 = (sy.max(cy)) as f32;

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

                            let w = (x2 - x1) as i32;
                            let h = (y2 - y1) as i32;
                            let dims = format!(" {}x{} ", w, h);
                            let lx = if x2 + 4.0 + dims.len() as f32 * 8.5 < sw { x2 + 4.0 } else { x1 };
                            let ly = if y2 + 24.0 < sh { y2 + 4.0 } else { y1 - 24.0 };
                            let label_bg = Rect::from_min_size(Pos2::new(lx, ly), egui::vec2(dims.len() as f32 * 8.5, 20.0));
                            painter.rect_filled(label_bg, 3.0, Color32::from_rgb(30, 160, 255));
                            painter.text(label_bg.center(), egui::Align2::CENTER_CENTER, &dims, FontId::proportional(14.0), Color32::WHITE);
                        } else {
                            painter.rect_filled(Rect::from_min_max(Pos2::ZERO, Pos2::new(sw, sh)), 0.0, dim);

                            if let Some((mx, my)) = self.mouse_pos {
                                let mx = mx as f32;
                                let my = my as f32;
                                painter.line_segment([Pos2::new(0.0, my), Pos2::new(sw, my)], Stroke::new(1.0, cross_color));
                                painter.line_segment([Pos2::new(mx, 0.0), Pos2::new(mx, sh)], Stroke::new(1.0, cross_color));

                                let coord = format!(" ({}, {}) ", mx as i32, my as i32);
                                let tip_x = if mx + 80.0 < sw { mx + 12.0 } else { mx - 12.0 - coord.len() as f32 * 7.5 };
                                let tip_y = if my + 28.0 < sh { my + 12.0 } else { my - 28.0 };
                                let tip_bg = Rect::from_min_size(Pos2::new(tip_x, tip_y), egui::vec2(coord.len() as f32 * 7.5, 20.0));
                                painter.rect_filled(tip_bg, 3.0, Color32::from_black_alpha(200));
                                painter.text(tip_bg.center(), egui::Align2::CENTER_CENTER, &coord, FontId::proportional(13.0), Color32::WHITE);
                            }

                            let hint = "Click and drag to select   ESC to cancel";
                            painter.text(Pos2::new(sw / 2.0, sh - 40.0), egui::Align2::CENTER_CENTER, hint, FontId::proportional(16.0), Color32::from_white_alpha(200));
                        }

                        if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                            self.end_capture(ctx);
                            self.mode = AppMode::Normal;
                            self.status = "Cancelled".into();
                        }
                    });
            }
            AppMode::PendingScreenshot(x1, y1, x2, y2, rx, started) => {
                let x1 = *x1; let y1 = *y1; let x2 = *x2; let y2 = *y2;
                let timed_out = started.elapsed().as_secs() >= 3;
                if let Ok(result) = rx.try_recv() {
                    set_window_exclude_from_capture(false);
                    ctx.send_viewport_cmd(egui::ViewportCommand::OuterPosition(egui::pos2(100.0, 100.0)));
                    match result {
                        Ok(img) => {
                            if self.is_add_state_mode {
                                if let (Some(elem_name), Some(last_state)) =
                                    (self.last_element_name.clone(), self.last_state_name.clone())
                                {
                                    let next_state = if let Some(forced) = self.forced_state_name.take() {
                                        forced
                                    } else {
                                        get_next_state_suffix(&last_state)
                                    };
                                    let new_path = std::path::Path::new(&self.image_dir).join(format!("{}_{}.png", elem_name, next_state)).to_string_lossy().to_string();
                                    let img_path = if std::fs::rename(&img, &new_path).is_ok() { new_path } else { img };
                                    if let Some(id) = self.last_id {
                                        let ts = Local::now().format("%Y%m%d_%H%M%S").to_string();
                                        match self.conn.execute(
                                            "INSERT INTO element_states(element_id,state_name,image_path,is_primary,created_at) VALUES(?1,?2,?3,0,?4)",
                                            params![id, next_state, img_path, ts]
                                        ) {
                                            Ok(_) => {
                                                self.last_state_name = Some(next_state.clone());
                                                self.status = format!("State '{}' added!", next_state);
                                                self.refresh();
                                            }
                                            Err(e) => self.status = format!("Error: {}", e),
                                        }
                                    }
                                    self.mode = AppMode::Normal;
                                }
                            } else {
                                self.mode = AppMode::InputName(x1, y1, x2, y2, img);
                                self.input.clear();
                                self.status = "Enter element name:".into();
                            }
                        }
                        Err(e) => {
                            self.mode = AppMode::Normal;
                            self.status = format!("Screenshot failed: {}", e);
                        }
                    }
                } else if timed_out {
                    set_window_exclude_from_capture(false);
                    ctx.send_viewport_cmd(egui::ViewportCommand::OuterPosition(egui::pos2(100.0, 100.0)));
                    self.mode = AppMode::Normal;
                    self.status = "Screenshot timed out".into();
                }
            }
            _ => {
                egui::CentralPanel::default()
                    .frame(egui::Frame::central_panel(&ctx.style()))
                    .show(ctx, |ui| {
                    ui.heading("Mouse Recorder");
                    ui.label(format!("Resolution: {}x{}", self.sw, self.sh));
                    ui.separator();

                    ScrollArea::vertical().max_height(300.0).show(ui, |ui| {
                        if self.elements.is_empty() {
                            ui.colored_label(Color32::GRAY, "No elements. Click [New Element] to record.");
                        }
                        for e in &self.elements {
                            let lab = format!("#{} {} ({}, {})", e.id, e.name, e.cx, e.cy);
                            if Some(e.id) == self.last_id {
                                ui.colored_label(Color32::YELLOW, &lab);
                            } else {
                                ui.label(&lab);
                            }
                            for (sn, pr) in &e.states {
                                let m = if *pr { " [PRIMARY]" } else { "" };
                                ui.colored_label(Color32::DARK_GRAY, format!("  - {}{}", sn, m));
                            }
                        }
                    });

                    ui.separator();
                    ui.colored_label(Color32::GREEN, &self.status);

                    match &self.mode {
                        AppMode::Normal => {
                            ui.horizontal(|ui| {
                                if ui.button("New Element  [F5]").clicked() || ui.input(|i| i.key_pressed(egui::Key::F5)) {
                                    self.forced_state_name = None;
                                    self.begin_capture(ctx, false);
                                }
                                if self.last_id.is_some() {
                                    if ui.button("Add State  [F6]").clicked() || ui.input(|i| i.key_pressed(egui::Key::F6)) {
                                        self.forced_state_name = None;
                                        self.begin_capture(ctx, true);
                                    }
                                }
                                if ui.button("Export CSV").clicked() {
                                    self.mode = AppMode::InputExport;
                                    self.input.clear();
                                    self.status = "Enter export filename:".into();
                                }
                                if ui.button("Refresh").clicked() {
                                    self.refresh();
                                    self.status = "Refreshed".into();
                                }
                            });
                            if self.last_id.is_some() {
                                ui.separator();
                                ui.label("Quick state capture:");
                                ui.horizontal(|ui| {
                                    for (key, label, suffix) in [
                                        (egui::Key::F1, "F1  Normal (-n)",   "-n"),
                                        (egui::Key::F2, "F2  Selected (-s)", "-s"),
                                        (egui::Key::F3, "F3  Clicked (-c)",  "-c"),
                                        (egui::Key::F4, "F4  Click+Sel (-cs)", "-cs"),
                                    ] {
                                        let pressed = ui.input(|i| i.key_pressed(key));
                                        if ui.button(label).clicked() || pressed {
                                            self.forced_state_name = Some(suffix.to_string());
                                            self.begin_capture(ctx, true);
                                        }
                                    }
                                });
                            }
                        }
                        AppMode::Capturing => {
                            ui.colored_label(Color32::RED, "Click and drag on screen to select region");
                            if ui.button("Cancel").clicked() {
                                self.end_capture(ctx);
                                self.mode = AppMode::Normal;
                                self.status = "Cancelled".into();
                            }
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
                                        AppMode::InputName(x1,y1,x2,y2,img) if !val.is_empty() => {
                                            self.mode = AppMode::InputState(x1,y1,x2,y2,img,val,"-n".to_string());
                                            self.input.clear();
                                            self.status = "Enter state name (default: -n):".into();
                                        }
                                        AppMode::InputState(x1,y1,x2,y2,img,name,default_state) => {
                                            let state_name = if val.is_empty() { default_state } else { val };
                                            let new_path = std::path::Path::new(&self.image_dir).join(format!("{}_{}.png", name, state_name)).to_string_lossy().to_string();
                                            let img_path = if std::fs::rename(&img, &new_path).is_ok() {
                                                new_path
                                            } else {
                                                img
                                            };
                                            match create_elem(&self.conn, &name, &state_name, x1, y1, x2, y2, &img_path, self.sw, self.sh) {
                                                Ok(id) => {
                                                    self.last_id = Some(id);
                                                    self.last_element_name = Some(name.clone());
                                                    self.last_state_name = Some(state_name.clone());
                                                    self.status = format!("Element #{} created!", id);
                                                    self.refresh();
                                                }
                                                Err(e) => self.status = format!("Error: {}", e),
                                            }
                                            self.input.clear();
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
            .with_inner_size([600.0, 450.0])
            .with_title("Mouse Recorder")
            .with_transparent(true),
        ..Default::default()
    };
    eframe::run_native("Mouse Recorder", opts, Box::new(|_cc| Ok(Box::new(RecorderApp {
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
        status: "Ready. Click [New Element] to record".into(),
        rx,
        capture_flag: flag,
        drag_start: None,
        drag_current: None,
        mouse_pos: None,
        capture_started_at: None,
        normal_window_size: None,
        forced_state_name: None,
    }))))
}