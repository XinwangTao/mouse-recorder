//! `mouse-recorder.exe --cli ...` — headless element management, target
//! discovery, clicking, workflow runs and evidence reports.

use crate::matchutil::{self, MatchHit};
use crate::{Config, ElementRec};
use image::RgbaImage;
use rusqlite::params;
use serde::Serialize;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

pub mod input;
pub mod ocr;
pub mod uia;
pub mod workflow;

const VERSION: &str = env!("CARGO_PKG_VERSION");

pub fn attach_console() {
    #[cfg(windows)]
    {
        use std::os::windows::io::AsRawHandle;
        use windows_sys::Win32::Foundation::INVALID_HANDLE_VALUE;
        use windows_sys::Win32::System::Console::{
            AllocConsole, AttachConsole, GetStdHandle, SetStdHandle, STD_ERROR_HANDLE,
            STD_OUTPUT_HANDLE, ATTACH_PARENT_PROCESS,
        };
        // Keep inherited pipes/files; only attach when stdout is missing.
        let current = unsafe { GetStdHandle(STD_OUTPUT_HANDLE) };
        if current.is_null() || current == INVALID_HANDLE_VALUE {
            unsafe {
                if AttachConsole(ATTACH_PARENT_PROCESS) == 0 {
                    let _ = AllocConsole();
                }
            }
            if let Ok(f) = std::fs::OpenOptions::new().write(true).open("CONOUT$") {
                let h = f.as_raw_handle() as _;
                unsafe {
                    SetStdHandle(STD_OUTPUT_HANDLE, h);
                    SetStdHandle(STD_ERROR_HANDLE, h);
                }
                std::mem::forget(f);
            }
        }
    }
}

fn enable_dpi_awareness() {
    #[cfg(windows)]
    unsafe {
        use windows_sys::Win32::UI::HiDpi::{
            SetProcessDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
        };
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
    }
}

pub fn run(args: Vec<String>) -> i32 {
    enable_dpi_awareness();
    if args.is_empty() || args[0] == "help" || args[0] == "--help" || args[0] == "-h" {
        print_help();
        return if args.is_empty() { 2 } else { 0 };
    }
    let cmd = args[0].as_str();
    let rest = &args[1..];
    let result = match cmd {
        "list" => cmd_list(rest),
        "get" => cmd_get(rest),
        "record" => cmd_record(rest),
        "rename" => cmd_rename(rest),
        "delete" => cmd_delete(rest),
        "set-primary" => cmd_set_primary(rest),
        "export" => cmd_export(rest),
        "backup" => cmd_backup(rest),
        "find" => cmd_find(rest),
        "verify" => cmd_verify(rest),
        "click" => cmd_click(rest),
        "ocr" => cmd_ocr(rest),
        "uia" => cmd_uia(rest),
        "run" => {
            let opts = match Opts::parse(rest) {
                Ok(o) => o,
                Err(e) => return usage(&e),
            };
            let Some(file) = opts.positional.first() else {
                return usage("run 需要工作流文件路径");
            };
            let json = opts.flag("json");
            let dry_run = opts.flag("dry-run");
            let report_dir = opts.value("report-dir");
            workflow::run(file, json, dry_run, report_dir.as_deref())
        }
        "version" => {
            println!("mrctl {VERSION}");
            0
        }
        other => usage(&format!("未知子命令: {other}")),
    };
    if result != 0 && cmd == "help" {
        print_help();
    }
    result
}

fn print_help() {
    println!(
        r#"mouse-recorder --cli <command> [options]

Commands:
  list [--filter X] [--json]             List elements and states
  get <name> [--state S] [--json]        Show one element
  record --name NAME --rect x,y,w,h      Capture a region without the GUI
         [--state S] [--overwrite] [--json]
  rename <old> <new>                     Rename an element (files + DB)
  delete <name> [--state S]              Delete an element or one state
  set-primary <name> <state>             Set the primary state
  export --format csv|json [--out PATH]  Export elements
  backup [--out DIR]                     Checkpoint + copy the SQLite DB
  find  (--element NAME[:STATE] | --template PATH)
         [--threshold F] [--roi x,y,w,h] [--json]
  verify (--element NAME[:STATE] | --template PATH)
         [--threshold F] [--json]
  click  (--element NAME[:STATE] | --template PATH | --uia | --coords x,y)
         [--window TITLE] [--double] [--right] [--verify] [--json]
  ocr    --text TEXT [--image PATH | --window TITLE]
         [--lang eng] [--json]
  uia    --window TITLE [--json] [--max N] [--depth N]
         [--name X] [--automation-id X] [--class X] [--control-type X]
  run <workflow.json|yaml> [--json] [--dry-run] [--report-dir DIR]
  version | help

Exit codes: 0 ok, 2 usage, 3 target not found / low score, 4 assertion failed
"#
    );
}

fn usage(msg: &str) -> i32 {
    eprintln!("error: {msg}");
    eprintln!("run `mouse-recorder.exe --cli help` for usage");
    2
}

fn fail(code: i32, msg: impl std::fmt::Display) -> i32 {
    eprintln!("error: {msg}");
    code
}

fn emit_json<T: Serialize>(v: &T) {
    println!("{}", serde_json::to_string_pretty(v).unwrap_or_else(|e| {
        format!("{{\"error\":\"{}\"}}", e)
    }));
}

pub(crate) struct Opts {
    flags: HashMap<String, bool>,
    values: HashMap<String, String>,
    pub positional: Vec<String>,
}

const BOOL_FLAGS: &[&str] = &[
    "json", "double", "right", "verify", "overwrite", "dry-run", "uia",
];

impl Opts {
    pub(crate) fn parse(args: &[String]) -> Result<Self, String> {
        let mut flags = HashMap::new();
        let mut values = HashMap::new();
        let mut positional = Vec::new();
        let mut i = 0;
        while i < args.len() {
            let a = &args[i];
            if a == "--" {
                positional.extend(args[i + 1..].iter().cloned());
                break;
            }
            if let Some(rest) = a.strip_prefix("--") {
                let (key, inline) = match rest.split_once('=') {
                    Some((k, v)) => (k, Some(v.to_string())),
                    None => (rest, None),
                };
                let key = key.to_string();
                if BOOL_FLAGS.contains(&key.as_str()) {
                    flags.insert(key, true);
                } else if let Some(v) = inline {
                    values.insert(key, v);
                } else {
                    let Some(next) = args.get(i + 1) else {
                        return Err(format!("--{key} 缺少值"));
                    };
                    values.insert(key.clone(), next.clone());
                    i += 1;
                }
            } else {
                positional.push(a.clone());
            }
            i += 1;
        }
        Ok(Self {
            flags,
            values,
            positional,
        })
    }

    fn flag(&self, k: &str) -> bool {
        self.flags.get(k).copied().unwrap_or(false)
    }

    fn value(&self, k: &str) -> Option<String> {
        self.values.get(k).cloned()
    }

    fn require(&self, k: &str) -> Result<String, String> {
        self.value(k).ok_or_else(|| format!("缺少 --{k}"))
    }
}

// ---------------------------------------------------------------------------
// Shared screen/DB helpers (also used by the workflow module)

pub(crate) struct Screen {
    pub img: RgbaImage,
    pub origin: (i32, i32),
}

impl Screen {
    pub fn to_screen(&self, x: i32, y: i32) -> (i32, i32) {
        (x + self.origin.0, y + self.origin.1)
    }
}

pub(crate) fn capture() -> Result<Screen, String> {
    let (ox, oy, _, _) = crate::virtual_screen_rect();
    let img = crate::grab_virtual_desktop()?;
    Ok(Screen {
        img,
        origin: (ox, oy),
    })
}

pub(crate) fn open_db() -> (Config, rusqlite::Connection) {
    let cfg = Config::load();
    let conn = crate::init_db(&cfg.db_path());
    let dir = cfg.image_dir();
    // Same startup maintenance as the GUI: crash recovery + legacy path migration.
    crate::recover_image_backups(&dir);
    crate::migrate_image_paths(&conn, &dir);
    (cfg, conn)
}

pub(crate) fn find_element<'a>(
    elements: &'a [ElementRec],
    name: &str,
) -> Option<&'a ElementRec> {
    elements.iter().find(|e| e.name == name)
}

pub(crate) fn element_state<'a>(
    e: &'a ElementRec,
    state: Option<&str>,
) -> Option<&'a crate::StateRec> {
    match state {
        Some(s) => e.find_state(s),
        None => e.primary_or_first_state(),
    }
}

/// Screen coordinates of the recorded click center, rebased to the current
/// virtual desktop size/origin.
pub(crate) fn element_center_screen(
    e: &ElementRec,
    cur_w: i32,
    cur_h: i32,
    origin: (i32, i32),
) -> (i32, i32) {
    let same = e.screen_w == cur_w && e.screen_h == cur_h;
    let (mut cx, mut cy) = if same || e.screen_w <= 0 || e.screen_h <= 0 {
        (e.cx, e.cy)
    } else {
        (
            matchutil::scale_i(e.cx, e.screen_w, cur_w),
            matchutil::scale_i(e.cy, e.screen_h, cur_h),
        )
    };
    if same {
        cx += e.origin_x - origin.0;
        cy += e.origin_y - origin.1;
    }
    (
        cx.clamp(0, (cur_w - 1).max(0)),
        cy.clamp(0, (cur_h - 1).max(0)),
    )
}

pub(crate) fn element_roi_image(
    e: &ElementRec,
    cur_w: i32,
    cur_h: i32,
    origin: (i32, i32),
) -> (i32, i32, i32, i32) {
    let same = e.screen_w == cur_w && e.screen_h == cur_h;
    let (mut bx, mut by, mut bw, mut bh) = if same || e.screen_w <= 0 || e.screen_h <= 0 {
        (e.bx, e.by, e.bw.max(1), e.bh.max(1))
    } else {
        (
            matchutil::scale_i(e.bx, e.screen_w, cur_w),
            matchutil::scale_i(e.by, e.screen_h, cur_h),
            matchutil::scale_i(e.bw, e.screen_w, cur_w).max(1),
            matchutil::scale_i(e.bh, e.screen_h, cur_h).max(1),
        )
    };
    if same {
        bx += e.origin_x - origin.0;
        by += e.origin_y - origin.1;
    }
    bx = bx.clamp(0, (cur_w - 2).max(0));
    by = by.clamp(0, (cur_h - 2).max(0));
    bw = bw.min(cur_w - bx).max(1);
    bh = bh.min(cur_h - by).max(1);
    (bx, by, bw, bh)
}

pub(crate) fn state_image_path(cfg: &Config, e: &ElementRec, state: Option<&str>) -> Option<PathBuf> {
    let st = element_state(e, state)?;
    let p = Path::new(&cfg.image_dir()).join(&st.image_path);
    Some(p)
}

pub(crate) struct MatchResult {
    pub hit: MatchHit,
    pub screen_cx: i32,
    pub screen_cy: i32,
    pub found: bool,
}

pub(crate) fn match_element(
    cfg: &Config,
    screen: &Screen,
    e: &ElementRec,
    state: Option<&str>,
    threshold: f32,
) -> Result<MatchResult, String> {
    let path = state_image_path(cfg, e, state)
        .ok_or_else(|| format!("element {} has no state image", e.name))?;
    let template = image::open(&path)
        .map_err(|err| format!("load template {}: {err}", path.display()))?
        .to_rgba8();
    let (sw, sh) = (screen.img.width() as i32, screen.img.height() as i32);
    let roi = element_roi_image(e, sw, sh, screen.origin);
    let hit = matchutil::find_template_smart(&screen.img, &template, Some(roi), threshold)
        .ok_or_else(|| "template search failed".to_string())?;
    let found = hit.score >= threshold;
    let (screen_cx, screen_cy) = screen.to_screen(hit.hit_cx, hit.hit_cy);
    Ok(MatchResult {
        hit,
        screen_cx,
        screen_cy,
        found,
    })
}

pub(crate) fn match_template_file(
    screen: &Screen,
    path: &str,
    roi: Option<&str>,
    threshold: f32,
) -> Result<Option<MatchHit>, String> {
    let template = image::open(path)
        .map_err(|e| format!("load template {path}: {e}"))?
        .to_rgba8();
    let (sw, sh) = (screen.img.width() as i32, screen.img.height() as i32);
    let roi_img = match roi {
        Some(r) => {
            let (x, y, w, h) = parse_rect(r)?;
            let ix0 = x - screen.origin.0;
            let iy0 = y - screen.origin.1;
            let (bx, by, bw, bh) = crate::clamp_drag_to_image(
                ix0,
                iy0,
                ix0 + w,
                iy0 + h,
                sw as u32,
                sh as u32,
            )
            .ok_or_else(|| "ROI 不在当前虚拟桌面内".to_string())?;
            Some((bx as i32, by as i32, bw as i32, bh as i32))
        }
        None => None,
    };
    Ok(matchutil::find_template_smart(
        &screen.img,
        &template,
        roi_img,
        threshold,
    ))
}

pub(crate) fn parse_rect(s: &str) -> Result<(i32, i32, i32, i32), String> {
    let parts: Vec<i32> = s
        .split(',')
        .map(|p| p.trim().parse::<i32>())
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| format!("矩形格式应为 x,y,w,h: {s}"))?;
    if parts.len() != 4 {
        return Err(format!("矩形格式应为 x,y,w,h: {s}"));
    }
    Ok((parts[0], parts[1], parts[2], parts[3]))
}

pub(crate) fn timestamp_dir(cfg: &Config, report_dir: Option<&str>) -> PathBuf {
    let base = report_dir.map(PathBuf::from).unwrap_or_else(|| {
        let image_dir = cfg.image_dir();
        let img = Path::new(&image_dir);
        img.parent().unwrap_or(img).join("runs")
    });
    let ts = chrono::Local::now().format("%Y%m%d_%H%M%S");
    base.join(ts.to_string())
}

// ---------------------------------------------------------------------------
// Commands

#[derive(Serialize)]
struct StateOut {
    state: String,
    image_path: String,
    image: String,
    is_primary: bool,
    exists: bool,
}

#[derive(Serialize)]
struct ElementOut {
    id: i32,
    name: String,
    center_x: i32,
    center_y: i32,
    screen_x: i32,
    screen_y: i32,
    bbox: [i32; 4],
    screen: [i32; 2],
    origin: [i32; 2],
    states: Vec<StateOut>,
}

fn element_out(cfg: &Config, e: &ElementRec) -> ElementOut {
    let dir = cfg.image_dir();
    ElementOut {
        id: e.id,
        name: e.name.clone(),
        center_x: e.cx,
        center_y: e.cy,
        screen_x: e.cx + e.origin_x,
        screen_y: e.cy + e.origin_y,
        bbox: [e.bx, e.by, e.bw, e.bh],
        screen: [e.screen_w, e.screen_h],
        origin: [e.origin_x, e.origin_y],
        states: e
            .states
            .iter()
            .map(|s| {
                let full = Path::new(&dir).join(&s.image_path);
                StateOut {
                    state: s.name.clone(),
                    image_path: s.image_path.clone(),
                    image: full.to_string_lossy().to_string(),
                    is_primary: s.is_primary,
                    exists: full.exists(),
                }
            })
            .collect(),
    }
}

fn cmd_list(args: &[String]) -> i32 {
    let opts = match Opts::parse(args) {
        Ok(o) => o,
        Err(e) => return usage(&e),
    };
    let filter = opts.value("filter").unwrap_or_default().to_lowercase();
    let (cfg, conn) = open_db();
    let elements: Vec<ElementOut> = crate::load_elements(&conn)
        .iter()
        .filter(|e| filter.is_empty() || e.name.to_lowercase().contains(&filter))
        .map(|e| element_out(&cfg, e))
        .collect();
    if opts.flag("json") {
        emit_json(&elements);
    } else {
        for e in &elements {
            println!(
                "#{} {}  center=({}, {})  screen=({}, {})  origin=({}, {})  roi={:?}",
                e.id,
                e.name,
                e.center_x,
                e.center_y,
                e.screen_x,
                e.screen_y,
                e.origin[0],
                e.origin[1],
                e.bbox
            );
            for s in &e.states {
                println!(
                    "    {:<8} {}  [{}]",
                    s.state,
                    s.image,
                    if s.exists { "ok" } else { "MISSING" }
                );
            }
        }
    }
    0
}

fn cmd_get(args: &[String]) -> i32 {
    let opts = match Opts::parse(args) {
        Ok(o) => o,
        Err(e) => return usage(&e),
    };
    let Some(name) = opts.positional.first() else {
        return usage("get 需要元件名");
    };
    let state = opts.value("state");
    let (cfg, conn) = open_db();
    let elements = crate::load_elements(&conn);
    let Some(e) = find_element(&elements, name) else {
        return fail(3, format!("元件不存在: {name}"));
    };
    if state.is_some() && element_state(e, state.as_deref()).is_none() {
        return fail(3, format!("状态不存在: {name} {:?}", state));
    }
    if opts.flag("json") {
        emit_json(&element_out(&cfg, e));
    } else {
        let out = element_out(&cfg, e);
        println!("#{} {}", out.id, out.name);
        println!("image center: ({}, {})", out.center_x, out.center_y);
        println!("screen center: ({}, {})", out.screen_x, out.screen_y);
        println!("bbox: {:?}  screen: {:?}  origin: {:?}", out.bbox, out.screen, out.origin);
        for s in &out.states {
            println!("  {:<8} {} [{}]", s.state, s.image, if s.exists { "ok" } else { "MISSING" });
        }
    }
    0
}

fn cmd_record(args: &[String]) -> i32 {
    let opts = match Opts::parse(args) {
        Ok(o) => o,
        Err(e) => return usage(&e),
    };
    let name = match opts.require("name") {
        Ok(v) => v,
        Err(e) => return usage(&e),
    };
    let rect = match opts.require("rect") {
        Ok(v) => v,
        Err(e) => return usage(&e),
    };
    let state = opts.value("state").unwrap_or_else(|| "-n".to_string());
    if let Err(e) = crate::validate_element_name(&name) {
        return usage(&e);
    }
    if let Err(e) = crate::validate_state_name(&state) {
        return usage(&e);
    }
    let (rx, ry, rw, rh) = match parse_rect(&rect) {
        Ok(v) => v,
        Err(e) => return usage(&e),
    };
    let screen = match capture() {
        Ok(s) => s,
        Err(e) => return fail(5, e),
    };
    let (sw, sh) = (screen.img.width() as i32, screen.img.height() as i32);
    let (ox, oy) = screen.origin;
    // --rect is in virtual-screen pixels; the capture is image-relative.
    let ix0 = rx - ox;
    let iy0 = ry - oy;
    let Some((ix_u, iy_u, iw_u, ih_u)) =
        crate::clamp_drag_to_image(ix0, iy0, ix0 + rw, iy0 + rh, sw as u32, sh as u32)
    else {
        return fail(3, "矩形不在当前虚拟桌面内");
    };
    let (ix, iy, iw, ih) = (ix_u as i32, iy_u as i32, iw_u as i32, ih_u as i32);
    let cropped = image::imageops::crop_imm(&screen.img, ix_u, iy_u, iw_u, ih_u).to_image();
    let (cfg, conn) = open_db();
    let dir = cfg.image_dir();
    let dest = Path::new(&dir)
        .join(format!("{}_{}.png", name, state))
        .to_string_lossy()
        .to_string();
    let db_img = crate::db_image_path(&dir, &dest);
    let temp = Path::new(&dir).join(format!(
        "capture_{}.png",
        chrono::Local::now().format("%Y%m%d_%H%M%S_%3f")
    ));
    if let Some(parent) = temp.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Err(e) = cropped.save(&temp) {
        return fail(5, format!("save temp: {e}"));
    }
    let temp_s = temp.to_string_lossy().to_string();

    let tx = match conn.unchecked_transaction() {
        Ok(t) => t,
        Err(e) => return fail(5, format!("begin tx: {e}")),
    };
    let existing = crate::find_element_id_by_name(&conn, &name);
    let overwrite = opts.flag("overwrite");
    let db_result: Result<i32, String> = match existing {
        Some(id) => {
            if crate::state_exists(&tx, id, &state) {
                if overwrite {
                    crate::overwrite_elem(
                        &tx, id, &state, ix, iy, ix + iw, iy + ih, &db_img, sw as u32,
                        sh as u32, ox, oy,
                    )
                    .map(|_| id)
                } else {
                    Err(format!(
                        "状态已存在: {name} [{state}]（加 --overwrite 覆盖）"
                    ))
                }
            } else {
                // Add a new state to an existing element, keeping geometry.
                let ts = chrono::Local::now().format("%Y%m%d_%H%M%S").to_string();
                tx.execute(
                    "INSERT INTO element_states(element_id,state_name,image_path,is_primary,created_at) VALUES(?1,?2,?3,0,?4)",
                    params![id, state, db_img, ts],
                )
                .map_err(|e| e.to_string())
                .map(|_| id)
            }
        }
        None => crate::create_elem_in_tx(
            &tx, &name, &state, ix, iy, ix + iw, iy + ih, &db_img, sw as u32, sh as u32,
            ox, oy,
        ),
    };
    let id = match db_result {
        Ok(id) => id,
        Err(e) => {
            let _ = std::fs::remove_file(&temp);
            return fail(5, e);
        }
    };
    let placed = match crate::place_image_hold(&temp_s, &dest) {
        Ok(p) => p,
        Err(e) => return fail(5, e),
    };
    if let Err(e) = tx.commit() {
        placed.rollback();
        return fail(5, format!("commit: {e}"));
    }
    placed.commit();
    let out = serde_json::json!({
        "id": id, "name": name, "state": state, "image": dest, "db_image_path": db_img,
        "rect": [ix, iy, iw, ih], "origin": [ox, oy]
    });
    if opts.flag("json") {
        emit_json(&out);
    } else {
        println!(
            "recorded #{} {} [{}] -> {} ({}x{})",
            id, name, state, dest, iw, ih
        );
    }
    0
}

fn template_from_opts(
    cfg: &Config,
    conn: &rusqlite::Connection,
    opts: &Opts,
) -> Result<(PathBuf, Option<ElementRec>, Option<String>), String> {
    if let Some(path) = opts.value("template") {
        return Ok((PathBuf::from(path), None, None));
    }
    let Some(spec) = opts.value("element") else {
        return Err("需要 --element NAME[:STATE] 或 --template PATH".into());
    };
    let (name, state) = match spec.split_once(':') {
        Some((n, s)) => (n.to_string(), Some(s.to_string())),
        None => (spec, opts.value("state")),
    };
    let elements = crate::load_elements(conn);
    let e = find_element(&elements, &name)
        .ok_or_else(|| format!("元件不存在: {name}"))?
        .clone();
    let p = state_image_path(cfg, &e, state.as_deref())
        .ok_or_else(|| format!("元件 {} 没有可用状态图", name))?;
    Ok((p, Some(e), state))
}

fn do_match(opts: &Opts, json: bool) -> i32 {
    let (cfg, conn) = open_db();
    let (path, element, _state) = match template_from_opts(&cfg, &conn, opts) {
        Ok(v) => v,
        Err(e) => return usage(&e),
    };
    let threshold = opts
        .value("threshold")
        .and_then(|v| v.parse::<f32>().ok())
        .unwrap_or(0.85);
    let screen = match capture() {
        Ok(s) => s,
        Err(e) => return fail(5, e),
    };
    let (sw, sh) = (screen.img.width() as i32, screen.img.height() as i32);
    let template = match image::open(&path) {
        Ok(img) => img.to_rgba8(),
        Err(e) => return fail(5, format!("load template {}: {e}", path.display())),
    };
    let roi = if let Some(r) = opts.value("roi") {
        match parse_rect(&r) {
            Ok((x, y, w, h)) => {
                let ix0 = x - screen.origin.0;
                let iy0 = y - screen.origin.1;
                let (ix, iy, iw, ih) =
                    crate::clamp_drag_to_image(ix0, iy0, ix0 + w, iy0 + h, sw as u32, sh as u32)
                        .unwrap_or((0, 0, sw as u32, sh as u32));
                Some((ix as i32, iy as i32, iw as i32, ih as i32))
            }
            Err(e) => return usage(&e),
        }
    } else if let Some(e) = &element {
        Some(element_roi_image(e, sw, sh, screen.origin))
    } else {
        None
    };
    let hit = matchutil::find_template_smart(&screen.img, &template, roi, threshold);
    let Some(hit) = hit else {
        if json {
            emit_json(&serde_json::json!({"found": false, "score": 0.0}));
        } else {
            println!("not found");
        }
        return 3;
    };
    let found = hit.score >= threshold;
    let (cx, cy) = screen.to_screen(hit.hit_cx, hit.hit_cy);
    let out = serde_json::json!({
        "found": found, "score": hit.score, "threshold": threshold,
        "hit": [hit.hit_x, hit.hit_y], "center": [cx, cy],
        "center_image": [hit.hit_cx, hit.hit_cy],
        "template_size": [hit.tw, hit.th], "used_roi": hit.used_roi,
        "origin": [screen.origin.0, screen.origin.1],
    });
    if json {
        emit_json(&out);
    } else {
        println!(
            "{} score={:.4} center=({}, {}) threshold={:.2} roi={}",
            if found { "found" } else { "low" },
            hit.score,
            cx,
            cy,
            threshold,
            if hit.used_roi { "yes" } else { "full" }
        );
    }
    if found {
        0
    } else {
        3
    }
}

fn cmd_rename(args: &[String]) -> i32 {
    let opts = match Opts::parse(args) {
        Ok(o) => o,
        Err(e) => return usage(&e),
    };
    let (Some(old), Some(new)) = (opts.positional.first(), opts.positional.get(1)) else {
        return usage("rename 需要 <old> <new>");
    };
    let (cfg, conn) = open_db();
    let Some(id) = crate::find_element_id_by_name(&conn, old) else {
        return fail(3, format!("元件不存在: {old}"));
    };
    match crate::rename_element(&conn, &cfg.image_dir(), id, old, new) {
        Ok(()) => {
            if opts.flag("json") {
                emit_json(&serde_json::json!({"ok": true, "id": id, "old": old, "new": new}));
            } else {
                println!("renamed #{id}: {old} -> {new}");
            }
            0
        }
        Err(e) => fail(5, e),
    }
}

fn cmd_delete(args: &[String]) -> i32 {
    let opts = match Opts::parse(args) {
        Ok(o) => o,
        Err(e) => return usage(&e),
    };
    let Some(name) = opts.positional.first() else {
        return usage("delete 需要 <name> [--state S]");
    };
    let (cfg, conn) = open_db();
    let Some(id) = crate::find_element_id_by_name(&conn, name) else {
        return fail(3, format!("元件不存在: {name}"));
    };
    let state = opts.value("state");
    let result = match state.as_deref() {
        Some(s) => crate::delete_state(&conn, &cfg.image_dir(), id, s),
        None => crate::delete_element(&conn, &cfg.image_dir(), id),
    };
    match result {
        Ok(()) => {
            if opts.flag("json") {
                emit_json(&serde_json::json!({
                    "ok": true, "id": id, "name": name, "state": state
                }));
            } else {
                println!(
                    "deleted #{} {}{}",
                    id,
                    name,
                    state.map(|s| format!(" [{s}]")).unwrap_or_default()
                );
            }
            0
        }
        Err(e) => fail(5, e),
    }
}

fn cmd_set_primary(args: &[String]) -> i32 {
    let opts = match Opts::parse(args) {
        Ok(o) => o,
        Err(e) => return usage(&e),
    };
    let Some(name) = opts.positional.first() else {
        return usage("set-primary 需要 <name> <state>");
    };
    let state = opts
        .positional
        .get(1)
        .cloned()
        .or_else(|| opts.value("state"));
    let Some(state) = state else {
        return usage("set-primary 需要 <name> <state>");
    };
    let (_cfg, conn) = open_db();
    let Some(id) = crate::find_element_id_by_name(&conn, name) else {
        return fail(3, format!("元件不存在: {name}"));
    };
    match crate::set_primary_state(&conn, id, &state) {
        Ok(()) => {
            if opts.flag("json") {
                emit_json(&serde_json::json!({
                    "ok": true, "id": id, "name": name, "state": state
                }));
            } else {
                println!("primary #{id} {name} -> {state}");
            }
            0
        }
        Err(e) => fail(5, e),
    }
}

fn cmd_export(args: &[String]) -> i32 {
    let opts = match Opts::parse(args) {
        Ok(o) => o,
        Err(e) => return usage(&e),
    };
    let format = opts
        .value("format")
        .unwrap_or_else(|| "json".to_string())
        .to_lowercase();
    let out = opts.value("out");
    let (cfg, conn) = open_db();
    let elements = crate::load_elements(&conn);
    match format.as_str() {
        "csv" => {
            let base = out.unwrap_or_else(|| "export".to_string());
            match crate::export_csv(&conn, &base, &cfg) {
                Ok(path) => {
                    println!("{path}");
                    0
                }
                Err(e) => fail(5, e),
            }
        }
        "json" => {
            let rows: Vec<ElementOut> = elements.iter().map(|e| element_out(&cfg, e)).collect();
            let text = serde_json::to_string_pretty(&rows).unwrap_or_default();
            match out {
                Some(path) => match std::fs::write(&path, text) {
                    Ok(()) => {
                        println!("{path}");
                        0
                    }
                    Err(e) => fail(5, format!("write {path}: {e}")),
                },
                None => {
                    println!("{text}");
                    0
                }
            }
        }
        other => usage(&format!("未知导出格式: {other}（csv|json）")),
    }
}

fn cmd_backup(args: &[String]) -> i32 {
    let opts = match Opts::parse(args) {
        Ok(o) => o,
        Err(e) => return usage(&e),
    };
    let (cfg, conn) = open_db();
    let _ = conn.execute_batch("PRAGMA wal_checkpoint(FULL);");
    let dir = opts
        .value("out")
        .map(PathBuf::from)
        .unwrap_or_else(|| Path::new(&cfg.image_dir()).join("backups"));
    if let Err(e) = std::fs::create_dir_all(&dir) {
        return fail(5, format!("create {}: {e}", dir.display()));
    }
    let ts = chrono::Local::now().format("%Y%m%d_%H%M%S");
    let dest = dir.join(format!("mouse_recorder_{ts}.db"));
    // VACUUM INTO produces a consistent snapshot even with WAL readers.
    let escaped = dest.to_string_lossy().replace('\'', "''");
    match conn.execute_batch(&format!("VACUUM INTO '{escaped}';")) {
        Ok(()) => {
            println!("{}", dest.display());
            0
        }
        Err(vacuum_err) => {
            eprintln!("warning: VACUUM INTO failed ({vacuum_err}); falling back to copy");
            let _ = conn.execute_batch("PRAGMA wal_checkpoint(FULL);");
            match std::fs::copy(cfg.db_path(), &dest) {
                Ok(_) => {
                    println!("{}", dest.display());
                    0
                }
                Err(e) => fail(5, format!("backup: {e}")),
            }
        }
    }
}

fn cmd_find(args: &[String]) -> i32 {
    let opts = match Opts::parse(args) {
        Ok(o) => o,
        Err(e) => return usage(&e),
    };
    let json = opts.flag("json");
    do_match(&opts, json)
}

fn cmd_verify(args: &[String]) -> i32 {
    let opts = match Opts::parse(args) {
        Ok(o) => o,
        Err(e) => return usage(&e),
    };
    let json = opts.flag("json");
    do_match(&opts, json)
}

fn cmd_click(args: &[String]) -> i32 {
    let opts = match Opts::parse(args) {
        Ok(o) => o,
        Err(e) => return usage(&e),
    };
    let json = opts.flag("json");
    let (cfg, conn) = open_db();
    let threshold = opts
        .value("threshold")
        .and_then(|v| v.parse::<f32>().ok())
        .unwrap_or(0.85);
    let verify = opts.flag("verify");
    if verify && opts.value("element").is_none() && opts.value("template").is_none() {
        return usage("--verify 需要 --element 或 --template");
    }

    let (x, y, score) = if let Some(c) = opts.value("coords") {
        match parse_rect(&format!("{c},0,0")) {
            Ok((x, y, _, _)) => (x, y, None),
            Err(_) => return usage("--coords 格式应为 x,y"),
        }
    } else if opts.flag("uia") {
        let win = opts.value("window").unwrap_or_default();
        if win.trim().is_empty() {
            return usage("--uia 需要 --window");
        }
        let Some(sel) = uia::Selector::from_opts(&opts) else {
            return usage("--uia 需要 --name / --automation-id / --class / --control-type");
        };
        match uia::find(&win, &sel) {
            Ok(Some(n)) => (n.cx(), n.cy(), None),
            Ok(None) => return fail(3, "UIA 未找到目标控件"),
            Err(e) => return fail(5, e),
        }
    } else if let Some(path) = opts.value("template") {
        let screen = match capture() {
            Ok(s) => s,
            Err(e) => return fail(5, e),
        };
        match match_template_file(&screen, &path, opts.value("roi").as_deref(), threshold) {
            Ok(Some(h)) if h.score >= threshold => {
                let (cx, cy) = screen.to_screen(h.hit_cx, h.hit_cy);
                (cx, cy, Some(h.score))
            }
            Ok(Some(h)) => {
                if json {
                    emit_json(&serde_json::json!({
                        "ok": false, "reason": "low_score", "score": h.score,
                        "threshold": threshold
                    }));
                } else {
                    println!("not clicking: score {:.4} < {:.2}", h.score, threshold);
                }
                return 3;
            }
            Ok(None) => return fail(3, "template not found"),
            Err(e) => return fail(5, e),
        }
    } else {
        let spec = match opts.value("element") {
            Some(v) => v,
            None => return usage("click 需要 --element / --template / --uia / --coords"),
        };
        let (name, state) = match spec.split_once(':') {
            Some((n, s)) => (n.to_string(), Some(s.to_string())),
            None => (spec, opts.value("state")),
        };
        let elements = crate::load_elements(&conn);
        let Some(e) = find_element(&elements, &name).cloned() else {
            return fail(3, format!("元件不存在: {name}"));
        };
        if verify {
            let screen = match capture() {
                Ok(s) => s,
                Err(e) => return fail(5, e),
            };
            match match_element(&cfg, &screen, &e, state.as_deref(), threshold) {
                Ok(r) if r.found => (r.screen_cx, r.screen_cy, Some(r.hit.score)),
                Ok(r) => {
                    if json {
                        emit_json(&serde_json::json!({
                            "ok": false, "reason": "low_score", "score": r.hit.score,
                            "threshold": threshold
                        }));
                    } else {
                        println!("not clicking: score {:.4} < {:.2}", r.hit.score, threshold);
                    }
                    return 3;
                }
                Err(e) => return fail(5, e),
            }
        } else {
            let screen = match capture() {
                Ok(s) => s,
                Err(e) => return fail(5, e),
            };
            let (sw, sh) = (screen.img.width() as i32, screen.img.height() as i32);
            let (cx, cy) = element_center_screen(&e, sw, sh, screen.origin);
            (cx, cy, None)
        }
    };

    let button = if opts.flag("right") {
        input::Button::Right
    } else {
        input::Button::Left
    };
    let double = opts.flag("double");
    if let Err(e) = input::click(x, y, button, double) {
        return fail(5, e);
    }

    if verify {
        std::thread::sleep(std::time::Duration::from_millis(180));
        let screen = match capture() {
            Ok(s) => s,
            Err(e) => return fail(5, e),
        };
        if let Some(path) = opts.value("template") {
            match match_template_file(&screen, &path, opts.value("roi").as_deref(), threshold) {
                Ok(hit) => {
                    let (found, score, after) = match hit {
                        Some(h) => {
                            let (cx, cy) = screen.to_screen(h.hit_cx, h.hit_cy);
                            (h.score >= threshold, h.score, (cx, cy))
                        }
                        None => (false, 0.0, (0, 0)),
                    };
                    if json {
                        emit_json(&serde_json::json!({
                            "ok": found, "clicked": [x, y], "score": score,
                            "threshold": threshold, "after_center": after
                        }));
                    } else {
                        println!(
                            "clicked ({}, {}) verify score={:.4} {}",
                            x,
                            y,
                            score,
                            if found { "ok" } else { "FAILED" }
                        );
                    }
                    if found { 0 } else { 4 }
                }
                Err(e) => fail(5, e),
            }
        } else {
            let spec = opts.value("element").unwrap_or_default();
            let (name, state) = match spec.split_once(':') {
                Some((n, s)) => (n.to_string(), Some(s.to_string())),
                None => (spec, opts.value("state")),
            };
            let elements = crate::load_elements(&conn);
            if let Some(e) = find_element(&elements, &name).cloned() {
                match match_element(&cfg, &screen, &e, state.as_deref(), threshold) {
                    Ok(r) => {
                        if json {
                            emit_json(&serde_json::json!({
                                "ok": r.found, "clicked": [x, y], "score": r.hit.score,
                                "threshold": threshold, "after_center": [r.screen_cx, r.screen_cy]
                            }));
                        } else {
                            println!(
                                "clicked ({}, {}) verify score={:.4} {}",
                                x,
                                y,
                                r.hit.score,
                                if r.found { "ok" } else { "FAILED" }
                            );
                        }
                        if r.found { 0 } else { 4 }
                    }
                    Err(e) => fail(5, e),
                }
            } else {
                3
            }
        }
    } else {
        if json {
            emit_json(&serde_json::json!({
                "ok": true, "clicked": [x, y], "double": double,
                "button": if opts.flag("right") { "right" } else { "left" },
                "score": score
            }));
        } else {
            println!("clicked ({}, {})", x, y);
        }
        0
    }
}

fn cmd_ocr(args: &[String]) -> i32 {
    let opts = match Opts::parse(args) {
        Ok(o) => o,
        Err(e) => return usage(&e),
    };
    let text = match opts.require("text") {
        Ok(v) => v,
        Err(e) => return usage(&e),
    };
    let lang = opts.value("lang").unwrap_or_else(|| "eng".to_string());
    let mut origin = (0, 0);
    let image = if let Some(p) = opts.value("image") {
        PathBuf::from(p)
    } else if let Some(win) = opts.value("window") {
        let screen = match capture() {
            Ok(s) => s,
            Err(e) => return fail(5, e),
        };
        let (wx, wy, ww, wh) = match uia::window_rect(&win) {
            Ok(v) => v,
            Err(e) => return fail(5, e),
        };
        let (ox, oy) = screen.origin;
        let ix0 = (wx - ox).max(0);
        let iy0 = (wy - oy).max(0);
        let ix1 = (wx + ww - ox).min(screen.img.width() as i32);
        let iy1 = (wy + wh - oy).min(screen.img.height() as i32);
        if ix1 <= ix0 || iy1 <= iy0 {
            return fail(3, "窗口不在当前虚拟桌面内");
        }
        origin = (ox + ix0, oy + iy0);
        let cropped = image::imageops::crop_imm(
            &screen.img,
            ix0 as u32,
            iy0 as u32,
            (ix1 - ix0) as u32,
            (iy1 - iy0) as u32,
        )
        .to_image();
        let (cfg, _) = open_db();
        let dir = timestamp_dir(&cfg, None);
        let _ = std::fs::create_dir_all(&dir);
        let p = dir.join("ocr_window.png");
        if let Err(e) = cropped.save(&p) {
            return fail(5, format!("save window crop: {e}"));
        }
        p
    } else {
        let screen = match capture() {
            Ok(s) => s,
            Err(e) => return fail(5, e),
        };
        origin = screen.origin;
        let (cfg, _) = open_db();
        let dir = timestamp_dir(&cfg, None);
        let _ = std::fs::create_dir_all(&dir);
        let p = dir.join("ocr_screen.png");
        if let Err(e) = screen.img.save(&p) {
            return fail(5, format!("save screenshot: {e}"));
        }
        p
    };
    match ocr::find_text(&image, &text, &lang) {
        Ok(Some(hit)) => {
            let rect = [hit.x + origin.0, hit.y + origin.1, hit.w, hit.h];
            if opts.flag("json") {
                emit_json(&serde_json::json!({
                    "found": true, "text": hit.text, "score": hit.conf,
                    "rect": rect,
                    "center": [rect[0] + rect[2] / 2, rect[1] + rect[3] / 2],
                    "origin": [origin.0, origin.1]
                }));
            } else {
                println!(
                    "found '{}' conf={:.1} rect=({},{},{},{})",
                    hit.text, hit.conf, rect[0], rect[1], rect[2], rect[3]
                );
            }
            0
        }
        Ok(None) => {
            if opts.flag("json") {
                emit_json(&serde_json::json!({"found": false, "text": text}));
            } else {
                println!("not found: {text}");
            }
            3
        }
        Err(e) => fail(5, e),
    }
}

fn cmd_uia(args: &[String]) -> i32 {
    let opts = match Opts::parse(args) {
        Ok(o) => o,
        Err(e) => return usage(&e),
    };
    let window = match opts.require("window") {
        Ok(v) => v,
        Err(e) => return usage(&e),
    };
    let max = opts
        .value("max")
        .and_then(|v| v.parse::<usize>().ok())
        .unwrap_or(400);
    let depth = opts
        .value("depth")
        .and_then(|v| v.parse::<usize>().ok())
        .unwrap_or(8);
    let selector = uia::Selector::from_opts(&opts);
    let has_selector = selector.is_some();
    match uia::dump(&window, max, depth, selector) {
        Ok(nodes) => {
            if opts.flag("json") {
                emit_json(&nodes);
            } else {
                for n in &nodes {
                    println!(
                        "{}{} name={:?} id={:?} class={:?} rect=({},{},{},{}){}",
                        "  ".repeat(n.depth),
                        n.control_type_name,
                        n.name,
                        n.automation_id,
                        n.class_name,
                        n.x,
                        n.y,
                        n.w,
                        n.h,
                        if n.offscreen { " [offscreen]" } else { "" }
                    );
                }
            }
            if has_selector && nodes.is_empty() {
                3
            } else {
                0
            }
        }
        Err(e) => fail(5, e),
    }
}
