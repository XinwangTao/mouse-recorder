//! JSON/YAML click workflows with per-step evidence, retries and adaptive
//! template thresholds.

use crate::cli::{self, input, ocr, uia};
use crate::matchutil;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Instant;

#[derive(Deserialize, Serialize, Clone, Debug, Default)]
pub struct Defaults {
    pub timeout_ms: Option<u64>,
    pub retry: Option<u32>,
    pub threshold: Option<f32>,
}

#[derive(Deserialize, Serialize, Clone, Debug)]
pub struct Workflow {
    pub schema_version: u32,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub defaults: Defaults,
    pub steps: Vec<Step>,
}

#[derive(Deserialize, Serialize, Clone, Debug)]
pub struct Step {
    pub id: String,
    pub action: String,
    #[serde(default)]
    pub target: Option<Target>,
    #[serde(default)]
    pub expect: Option<Target>,
    #[serde(default)]
    pub text: Option<String>,
    #[serde(default)]
    pub key: Option<String>,
    #[serde(default)]
    pub ms: Option<u64>,
    #[serde(default)]
    pub retry: Option<u32>,
    #[serde(default)]
    pub on_fail: Option<String>,
    #[serde(default)]
    pub threshold: Option<f32>,
}

#[derive(Deserialize, Serialize, Clone, Debug, Default)]
pub struct Target {
    #[serde(rename = "type")]
    pub kind: String,
    pub name: Option<String>,
    pub state: Option<String>,
    pub path: Option<String>,
    pub window: Option<String>,
    pub text: Option<String>,
    pub automation_id: Option<String>,
    pub class: Option<String>,
    pub control_type: Option<String>,
    pub index: Option<usize>,
    pub x: Option<i32>,
    pub y: Option<i32>,
    pub roi: Option<[i32; 4]>,
    pub threshold: Option<f32>,
    pub lang: Option<String>,
}

#[derive(Serialize)]
struct Report {
    schema_version: u32,
    workflow: String,
    name: Option<String>,
    ok: bool,
    started_at: String,
    finished_at: String,
    report_dir: String,
    steps: Vec<StepReport>,
}

#[derive(Serialize)]
struct StepReport {
    id: String,
    action: String,
    ok: bool,
    attempts: u32,
    score: Option<f32>,
    elapsed_ms: u128,
    screenshot: Option<String>,
    target: Option<String>,
    error: Option<String>,
    threshold: Option<f32>,
    adaptive: Option<String>,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
struct ThresholdEntry {
    threshold: f32,
    fails: u32,
    oks: u32,
}

#[derive(Default)]
struct Adaptive {
    path: PathBuf,
    entries: HashMap<String, ThresholdEntry>,
}

impl Adaptive {
    fn load(path: PathBuf) -> Self {
        let entries = std::fs::read_to_string(&path)
            .ok()
            .and_then(|s| serde_json::from_str::<HashMap<String, ThresholdEntry>>(&s).ok())
            .unwrap_or_default();
        Self { path, entries }
    }

    fn threshold(&self, key: &str, base: f32) -> f32 {
        self.entries
            .get(key)
            .map(|e| e.threshold)
            .unwrap_or(base)
    }

    fn record(&mut self, key: &str, base: f32, ok: bool, score: Option<f32>) -> Option<String> {
        let e = self
            .entries
            .entry(key.to_string())
            .or_insert_with(|| ThresholdEntry {
                threshold: base,
                fails: 0,
                oks: 0,
            });
        let mut note = None;
        if ok {
            e.fails = 0;
            e.oks += 1;
            if e.oks >= 5 && e.threshold < 0.98 {
                e.threshold = (e.threshold + 0.01).min(0.98);
                e.oks = 0;
                note = Some(format!("threshold -> {:.2} (5 ok)", e.threshold));
            }
        } else if score.unwrap_or(0.0) >= 0.50 {
            e.oks = 0;
            e.fails += 1;
            if e.fails >= 3 && e.threshold > 0.50 {
                e.threshold = (e.threshold - 0.02).max(0.50);
                e.fails = 0;
                note = Some(format!("threshold -> {:.2} (3 low-score failures)", e.threshold));
            }
        }
        note
    }

    fn save(&self) {
        if self.entries.is_empty() {
            return;
        }
        if let Some(parent) = self.path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if let Ok(s) = serde_json::to_string_pretty(&self.entries) {
            let _ = std::fs::write(&self.path, s);
        }
    }
}

fn load_workflow(path: &str) -> Result<Workflow, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("read {path}: {e}"))?;
    let wf: Workflow = serde_yaml::from_str(&text).map_err(|e| format!("parse {path}: {e}"))?;
    if wf.schema_version != 1 {
        return Err(format!(
            "unsupported schema_version {} (expected 1)",
            wf.schema_version
        ));
    }
    if wf.steps.is_empty() {
        return Err("workflow has no steps".into());
    }
    Ok(wf)
}

fn selector(t: &Target) -> uia::Selector {
    uia::Selector {
        name: t.name.clone(),
        automation_id: t.automation_id.clone(),
        class_name: t.class.clone(),
        control_type: t.control_type.clone(),
        index: t.index,
    }
}

fn target_threshold(step: &Step, defaults: &Defaults) -> f32 {
    step.threshold
        .or_else(|| step.target.as_ref().and_then(|t| t.threshold))
        .or_else(|| step.expect.as_ref().and_then(|e| e.threshold))
        .or(defaults.threshold)
        .unwrap_or(0.85)
        .min(0.98)
        .max(0.50)
}

fn threshold_key(step: &Step) -> String {
    step.target
        .as_ref()
        .and_then(|t| t.name.clone())
        .unwrap_or_else(|| step.id.clone())
}

struct Resolved {
    x: i32,
    y: i32,
    score: Option<f32>,
    screen: cli::Screen,
    detail: String,
}

fn resolve_target(
    cfg: &cli::Config,
    conn: &rusqlite::Connection,
    step: &Step,
    t: &Target,
    threshold: f32,
    report_dir: &Path,
    interactive: bool,
) -> Result<Resolved, String> {
    let kind = if t.kind.is_empty() { "element" } else { t.kind.as_str() };
    let screen = cli::capture()?;
    match kind {
        "coords" => {
            let x = t.x.ok_or_else(|| "coords target needs x".to_string())?;
            let y = t.y.ok_or_else(|| "coords target needs y".to_string())?;
            Ok(Resolved {
                x,
                y,
                score: None,
                screen,
                detail: format!("coords {x},{y}"),
            })
        }
        "element" => {
            let name = t.name.as_deref().ok_or_else(|| "element target needs name".to_string())?;
            let elements = crate::load_elements(conn);
            let e = cli::find_element(&elements, name)
                .ok_or_else(|| format!("element not found: {name}"))?
                .clone();
            let r = cli::match_element(cfg, &screen, &e, t.state.as_deref(), threshold)?;
            if !r.found {
                return Err(format!(
                    "element {name} score {:.4} < {:.2}",
                    r.hit.score, threshold
                ));
            }
            Ok(Resolved {
                x: r.screen_cx,
                y: r.screen_cy,
                score: Some(r.hit.score),
                screen,
                detail: format!("element {name}"),
            })
        }
        "template" => {
            let path = t
                .path
                .as_deref()
                .ok_or_else(|| "template target needs path".to_string())?;
            let template = image::open(path)
                .map_err(|e| format!("load template {path}: {e}"))?
                .to_rgba8();
            let (sw, sh) = (screen.img.width() as i32, screen.img.height() as i32);
            let roi = t.roi.map(|r| {
                let (x, y, w, h) =
                    crate::clamp_drag_to_image(r[0], r[1], r[0] + r[2], r[1] + r[3], sw as u32, sh as u32)
                        .unwrap_or((0, 0, sw as u32, sh as u32));
                (x as i32, y as i32, w as i32, h as i32)
            });
            let hit = matchutil::find_template_smart(&screen.img, &template, roi, threshold)
                .ok_or_else(|| "template not found".to_string())?;
            if hit.score < threshold {
                return Err(format!(
                    "template score {:.4} < {:.2}",
                    hit.score, threshold
                ));
            }
            let (x, y) = screen.to_screen(hit.hit_cx, hit.hit_cy);
            Ok(Resolved {
                x,
                y,
                score: Some(hit.score),
                screen,
                detail: format!("template {path}"),
            })
        }
        "uia" => {
            let window = t.window.as_deref().unwrap_or("");
            if window.trim().is_empty() {
                return Err("UIA target needs window".into());
            }
            let sel = selector(t);
            // Raise first so the node rect is read from the settled window and
            // clicks/keystrokes reach it rather than whatever is on top.
            // Dry-run never touches the desktop, so it skips both steps.
            if interactive {
                uia::raise_window(window).map_err(|e| format!("raise window: {e}"))?;
            }
            let node = uia::find(window, &sel)?
                .ok_or_else(|| format!("UIA target not found in {window}"))?;
            if interactive {
                if let Some(blocker) = uia::occluder_at(window, node.cx(), node.cy())? {
                    return Err(format!(
                        "UIA target occluded by window: {blocker} ({} \"{}\")",
                        node.control_type_name, node.name
                    ));
                }
            }
            Ok(Resolved {
                x: node.cx(),
                y: node.cy(),
                score: None,
                screen,
                detail: format!("uia {} {:?}", node.control_type_name, node.name),
            })
        }
        "ocr" => {
            let text = t.text.as_deref().ok_or_else(|| "ocr target needs text".to_string())?;
            let lang = t.lang.as_deref().unwrap_or("eng");
            let img_path = report_dir.join(format!("target_{}.png", sanitize(&step.id)));
            std::fs::create_dir_all(report_dir).map_err(|e| e.to_string())?;
            screen.img.save(&img_path).map_err(|e| e.to_string())?;
            let hit = ocr::find_text(&img_path, text, lang)?
                .ok_or_else(|| format!("OCR text not found: {text}"))?;
            let (x, y) = screen.to_screen(hit.x + hit.w / 2, hit.y + hit.h / 2);
            Ok(Resolved {
                x,
                y,
                score: Some(hit.conf / 100.0),
                screen,
                detail: format!("ocr {text}"),
            })
        }
        other => Err(format!("unknown target type: {other}")),
    }
}

fn check_expect(
    cfg: &cli::Config,
    conn: &rusqlite::Connection,
    step: &Step,
    expect: &Target,
    threshold: f32,
    report_dir: &Path,
) -> Result<(bool, Option<f32>, String), String> {
    let kind = if expect.kind.is_empty() { "none" } else { expect.kind.as_str() };
    match kind {
        "none" => Ok((true, None, "none".into())),
        "window" => {
            let title = expect.window.as_deref().unwrap_or("");
            Ok((uia::window_exists(title), None, format!("window {title}")))
        }
        "template" | "element" => {
            let screen = cli::capture()?;
            if kind == "element" {
                let name = expect.name.as_deref().unwrap_or("");
                let elements = crate::load_elements(conn);
                let Some(e) = cli::find_element(&elements, name).cloned() else {
                    return Ok((false, None, format!("element not found: {name}")));
                };
                let r = cli::match_element(cfg, &screen, &e, expect.state.as_deref(), threshold)?;
                return Ok((
                    r.found,
                    Some(r.hit.score),
                    format!("element {name} score {:.4}", r.hit.score),
                ));
            }
            let path = expect.path.as_deref().unwrap_or("");
            let template = image::open(path)
                .map_err(|e| format!("load template {path}: {e}"))?
                .to_rgba8();
            let (sw, sh) = (screen.img.width() as i32, screen.img.height() as i32);
            let roi = expect.roi.map(|r| {
                let (x, y, w, h) =
                    crate::clamp_drag_to_image(r[0], r[1], r[0] + r[2], r[1] + r[3], sw as u32, sh as u32)
                        .unwrap_or((0, 0, sw as u32, sh as u32));
                (x as i32, y as i32, w as i32, h as i32)
            });
            let hit = matchutil::find_template_smart(&screen.img, &template, roi, threshold);
            let (found, score) = hit.map(|h| (h.score >= threshold, h.score)).unwrap_or((false, 0.0));
            Ok((found, Some(score), format!("template {path} score {score:.4}")))
        }
        "ocr" => {
            let text = expect.text.as_deref().unwrap_or("");
            let lang = expect.lang.as_deref().unwrap_or("eng");
            let screen = cli::capture()?;
            let img_path = report_dir.join(format!("expect_{}.png", sanitize(&step.id)));
            std::fs::create_dir_all(report_dir).map_err(|e| e.to_string())?;
            screen.img.save(&img_path).map_err(|e| e.to_string())?;
            let hit = ocr::find_text(&img_path, text, lang)?;
            match hit {
                Some(h) => Ok((
                    true,
                    Some(h.conf / 100.0),
                    format!("ocr {text} conf {:.1}", h.conf),
                )),
                None => Ok((false, None, format!("ocr not found: {text}"))),
            }
        }
        other => Err(format!("unknown expect type: {other}")),
    }
}

fn sanitize(id: &str) -> String {
    id.chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c } else { '_' })
        .collect()
}

fn save_step_screenshot(screen: &cli::Screen, report_dir: &Path, idx: usize, id: &str) -> Option<String> {
    let path = report_dir.join(format!("{:02}_{}.png", idx + 1, sanitize(id)));
    if screen.img.save(&path).is_ok() {
        Some(path.to_string_lossy().to_string())
    } else {
        None
    }
}

pub fn run(path: &str, json: bool, dry_run: bool, report_dir: Option<&str>) -> i32 {
    let wf = match load_workflow(path) {
        Ok(w) => w,
        Err(e) => {
            eprintln!("error: {e}");
            return 2;
        }
    };
    let (cfg, conn) = cli::open_db();
    let dir = cli::timestamp_dir(&cfg, report_dir);
    if let Err(e) = std::fs::create_dir_all(&dir) {
        eprintln!("error: create report dir {}: {e}", dir.display());
        return 5;
    }
    let thresholds_path = dir
        .parent()
        .map(|p| p.join("thresholds.json"))
        .unwrap_or_else(|| dir.join("thresholds.json"));
    let mut adaptive = Adaptive::load(thresholds_path);

    let started_at = chrono::Local::now().to_rfc3339();
    let mut reports = Vec::new();
    let mut all_ok = true;

    for (idx, step) in wf.steps.iter().enumerate() {
        let t0 = Instant::now();
        let retries = step.retry.or(wf.defaults.retry).unwrap_or(2);
        let base = target_threshold(step, &wf.defaults);
        let key = threshold_key(step);
        let threshold = adaptive.threshold(&key, base);
        let mut last_err = None;
        let mut attempts = 0u32;
        let mut score = None;
        let mut screenshot = None;
        let mut detail = None;
        let mut adaptive_note = None;

        for attempt in 0..=retries {
            attempts = attempt + 1;
            let result = (|| -> Result<(Option<f32>, Option<String>, cli::Screen), String> {
                let mut resolved: Option<Resolved> = None;
                if let Some(target) = &step.target {
                    let r = resolve_target(&cfg, &conn, step, target, threshold, &dir, !dry_run)?;
                    detail = Some(r.detail.clone());
                    score = r.score;
                    resolved = Some(r);
                }
                let action = step.action.to_ascii_lowercase();
                if !dry_run {
                    match action.as_str() {
                        "click" | "double_click" | "right_click" => {
                            let r = resolved.as_ref().ok_or_else(|| "click needs a target".to_string())?;
                            let button = if action == "right_click" {
                                input::Button::Right
                            } else {
                                input::Button::Left
                            };
                            input::click(r.x, r.y, button, action == "double_click")?;
                        }
                        "type" | "key" => {
                            // A target on type/key means "focus this control first".
                            if let Some(r) = resolved.as_ref() {
                                input::click(r.x, r.y, input::Button::Left, false)?;
                                std::thread::sleep(std::time::Duration::from_millis(120));
                            }
                            if action == "type" {
                                input::send_text(step.text.as_deref().unwrap_or(""))?;
                            } else {
                                input::send_key(step.key.as_deref().unwrap_or("enter"))?;
                            }
                        }
                        "wait" => {
                            std::thread::sleep(std::time::Duration::from_millis(
                                step.ms.unwrap_or(500),
                            ));
                        }
                        "assert" | "verify" => {}
                        other => return Err(format!("unknown action: {other}")),
                    }
                }
                if !dry_run {
                    if let Some(expect) = &step.expect {
                        std::thread::sleep(std::time::Duration::from_millis(120));
                        let (ok, exp_score, exp_detail) =
                            check_expect(&cfg, &conn, step, expect, threshold, &dir)?;
                        if score.is_none() {
                            score = exp_score;
                        }
                        detail = Some(match detail.as_ref() {
                            Some(d) => format!("{d}; {exp_detail}"),
                            None => exp_detail,
                        });
                        if !ok {
                            return Err("expectation failed".into());
                        }
                    }
                }
                let screen = cli::capture().unwrap_or_else(|_| {
                    resolved
                        .as_ref()
                        .map(|r| cli::Screen {
                            img: r.screen.img.clone(),
                            origin: r.screen.origin,
                        })
                        .unwrap_or_else(|| cli::Screen {
                            img: image::RgbaImage::new(1, 1),
                            origin: (0, 0),
                        })
                });
                Ok((score, detail.clone(), screen))
            })();

            match result {
                Ok((_s, _d, screen)) => {
                    screenshot = save_step_screenshot(&screen, &dir, idx, &step.id);
                    last_err = None;
                    adaptive_note = adaptive.record(&key, base, true, score);
                    break;
                }
                Err(e) => {
                    last_err = Some(e);
                    if attempt < retries {
                        std::thread::sleep(std::time::Duration::from_millis(250));
                    }
                }
            }
        }

        let ok = last_err.is_none();
        if !ok {
            adaptive_note = adaptive.record(&key, base, false, score);
        }
        if screenshot.is_none() {
            if let Ok(s) = cli::capture() {
                screenshot = save_step_screenshot(&s, &dir, idx, &step.id);
            }
        }
        reports.push(StepReport {
            id: step.id.clone(),
            action: step.action.clone(),
            ok,
            attempts,
            score,
            elapsed_ms: t0.elapsed().as_millis(),
            screenshot,
            target: detail,
            error: last_err,
            threshold: Some(threshold),
            adaptive: adaptive_note,
        });
        if !ok {
            all_ok = false;
            if step.on_fail.as_deref() != Some("continue") {
                break;
            }
        }
    }

    adaptive.save();
    let report = Report {
        schema_version: 1,
        workflow: path.to_string(),
        name: wf.name.clone(),
        ok: all_ok,
        started_at,
        finished_at: chrono::Local::now().to_rfc3339(),
        report_dir: dir.to_string_lossy().to_string(),
        steps: reports,
    };
    let report_path = dir.join("report.json");
    let _ = std::fs::write(
        &report_path,
        serde_json::to_string_pretty(&report).unwrap_or_default(),
    );

    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "ok": report.ok,
                "report": report_path.to_string_lossy(),
                "steps": report.steps,
            }))
            .unwrap_or_default()
        );
    } else {
        for s in &report.steps {
            println!(
                "[{}] {} {} attempts={} score={} {}ms {}",
                if s.ok { "ok" } else { "FAIL" },
                s.id,
                s.action,
                s.attempts,
                s.score.map(|v| format!("{v:.4}")).unwrap_or_else(|| "-".into()),
                s.elapsed_ms,
                s.error.clone().unwrap_or_default()
            );
            if let Some(a) = &s.adaptive {
                println!("    adaptive: {a}");
            }
        }
        println!(
            "{}  report: {}",
            if report.ok { "PASS" } else { "FAIL" },
            report_path.display()
        );
    }
    if report.ok {
        0
    } else {
        4
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn yaml_and_json_parse_same_schema() {
        let yaml = r#"
schema_version: 1
name: demo
defaults:
  threshold: 0.8
steps:
  - id: click1
    action: click
    target:
      type: element
      name: scene/btn
    expect:
      type: window
      window: Notepad
"#;
        let wf: Workflow = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(wf.steps.len(), 1);
        assert_eq!(wf.steps[0].target.as_ref().unwrap().name.as_deref(), Some("scene/btn"));
        let json = r#"{"schema_version":1,"steps":[{"id":"w","action":"wait","ms":10}]}"#;
        let wf2: Workflow = serde_yaml::from_str(json).unwrap();
        assert_eq!(wf2.steps[0].action, "wait");
    }

    #[test]
    fn adaptive_lowers_after_three_low_scores() {
        let mut a = Adaptive::default();
        assert!(a.record("k", 0.85, false, Some(0.80)).is_none());
        assert!(a.record("k", 0.85, false, Some(0.80)).is_none());
        let note = a.record("k", 0.85, false, Some(0.80));
        assert!(note.is_some());
        assert!((a.threshold("k", 0.85) - 0.83).abs() < 1e-5);
    }

    #[test]
    fn adaptive_never_crosses_bounds() {
        let mut a = Adaptive::default();
        for _ in 0..10 {
            a.record("k", 0.50, false, Some(0.49));
        }
        assert!((a.threshold("k", 0.50) - 0.50).abs() < 1e-5);
        for _ in 0..50 {
            a.record("k", 0.50, true, Some(0.99));
        }
        assert!(a.threshold("k", 0.50) <= 0.98);
    }
}
