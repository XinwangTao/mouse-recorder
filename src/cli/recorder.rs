//! `--cli record-workflow`: record real clicks/typing and emit a workflow.
//!
//! Each click becomes a step whose target is a fallback chain: the UIA control
//! under the cursor first (cheap and exact), then a template crop of the click
//! area (works when accessibility is missing). Esc stops and writes the file.

use super::uia::UiaNode;
use super::{uia, Opts};
use crate::cli::workflow::{Step, Target, TargetSpec, Workflow};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

struct State {
    steps: Vec<Step>,
    text: String,
    clicks: u32,
    max_steps: usize,
    template: bool,
    out: PathBuf,
    tpl_dir: PathBuf,
    tpl_idx: u32,
    last_pos: (i32, i32),
    shift: bool,
}

impl State {
    fn flush_text(&mut self) {
        let text = std::mem::take(&mut self.text);
        if text.is_empty() {
            return;
        }
        let n = self.steps.len() + 1;
        self.steps.push(Step {
            id: format!("type_{n}"),
            action: "type".to_string(),
            text: Some(text),
            ..Default::default()
        });
    }

    fn push_key(&mut self, key: &str) {
        let n = self.steps.len() + 1;
        self.steps.push(Step {
            id: format!("key_{n}"),
            action: "key".to_string(),
            key: Some(key.to_string()),
            ..Default::default()
        });
    }
}

/// Build the target chain for one click. Pure so it can be unit tested.
fn target_for_click(
    x: i32,
    y: i32,
    node: Option<(UiaNode, Option<String>)>,
    template: Option<String>,
    screen_area: i64,
) -> TargetSpec {
    let mut chain: Vec<Target> = Vec::new();
    // A control covering half the screen (a shell pane, a webview host) is a
    // useless anchor: its centre is nowhere near what the user clicked.
    let node = node.filter(|(n, _)| {
        let area = n.w as i64 * n.h as i64;
        screen_area <= 0 || area * 2 <= screen_area
    });
    if let Some((n, window)) = node {
        // Prefer a stable automation id; fall back to name, then class.
        let mut t = Target {
            kind: "uia".to_string(),
            window: window.clone(),
            control_type: Some(n.control_type_name.to_string()),
            ..Default::default()
        };
        if !n.automation_id.is_empty() {
            t.automation_id = Some(n.automation_id.clone());
        } else if !n.name.is_empty() {
            t.name = Some(n.name.clone());
        } else if !n.class_name.is_empty() {
            t.class = Some(n.class_name.clone());
        } else {
            t = Target::default();
        }
        if t.kind == "uia" {
            chain.push(t);
        }
    }
    if let Some(path) = template {
        chain.push(Target {
            kind: "template".to_string(),
            path: Some(path),
            ..Default::default()
        });
    }
    match chain.len() {
        0 => TargetSpec::One(Target {
            kind: "coords".to_string(),
            x: Some(x),
            y: Some(y),
            ..Default::default()
        }),
        1 => TargetSpec::One(chain.remove(0)),
        _ => TargetSpec::Many(chain),
    }
}

fn capture_template(
    state: &mut State,
    screen: &super::Screen,
    x: i32,
    y: i32,
) -> Option<String> {
    let (ox, oy) = screen.origin;
    let ix = x - ox;
    let iy = y - oy;
    let (iw, ih) = (screen.img.width() as i32, screen.img.height() as i32);
    // 100x100 context box around the click, clamped to the captured image.
    let half = 50;
    let x0 = (ix - half).clamp(0, (iw - 1).max(0));
    let y0 = (iy - half).clamp(0, (ih - 1).max(0));
    let x1 = (ix + half).clamp(1, iw);
    let y1 = (iy + half).clamp(1, ih);
    if x1 - x0 < 8 || y1 - y0 < 8 {
        return None;
    }
    let crop = image::imageops::crop_imm(
        &screen.img,
        x0 as u32,
        y0 as u32,
        (x1 - x0) as u32,
        (y1 - y0) as u32,
    )
    .to_image();
    let _ = std::fs::create_dir_all(&state.tpl_dir);
    state.tpl_idx += 1;
    let path = state.tpl_dir.join(format!("tpl_{:03}.png", state.tpl_idx));
    if crop.save(&path).is_err() {
        return None;
    }
    Some(path.to_string_lossy().to_string())
}

/// Printable character for a key, honouring Shift. Non-ASCII input (an IME
/// composition, for example) cannot be reconstructed from key names; those
/// characters have to be added to the workflow as an explicit `type` step.
fn char_for(key: rdev::Key, shift: bool) -> Option<char> {
    use rdev::Key::*;
    let (base, shifted) = match key {
        KeyA => ('a', 'A'),
        KeyB => ('b', 'B'),
        KeyC => ('c', 'C'),
        KeyD => ('d', 'D'),
        KeyE => ('e', 'E'),
        KeyF => ('f', 'F'),
        KeyG => ('g', 'G'),
        KeyH => ('h', 'H'),
        KeyI => ('i', 'I'),
        KeyJ => ('j', 'J'),
        KeyK => ('k', 'K'),
        KeyL => ('l', 'L'),
        KeyM => ('m', 'M'),
        KeyN => ('n', 'N'),
        KeyO => ('o', 'O'),
        KeyP => ('p', 'P'),
        KeyQ => ('q', 'Q'),
        KeyR => ('r', 'R'),
        KeyS => ('s', 'S'),
        KeyT => ('t', 'T'),
        KeyU => ('u', 'U'),
        KeyV => ('v', 'V'),
        KeyW => ('w', 'W'),
        KeyX => ('x', 'X'),
        KeyY => ('y', 'Y'),
        KeyZ => ('z', 'Z'),
        Num0 => ('0', ')'),
        Num1 => ('1', '!'),
        Num2 => ('2', '@'),
        Num3 => ('3', '#'),
        Num4 => ('4', '$'),
        Num5 => ('5', '%'),
        Num6 => ('6', '^'),
        Num7 => ('7', '&'),
        Num8 => ('8', '*'),
        Num9 => ('9', '('),
        Space => (' ', ' '),
        Minus => ('-', '_'),
        Equal => ('=', '+'),
        LeftBracket => ('[', '{'),
        RightBracket => (']', '}'),
        BackSlash => ('\\', '|'),
        SemiColon => (';', ':'),
        Quote => ('\'', '"'),
        Comma => (',', '<'),
        Dot => ('.', '>'),
        Slash => ('/', '?'),
        BackQuote => ('`', '~'),
        _ => return None,
    };
    Some(if shift { shifted } else { base })
}

fn record_click(state: &mut State, x: i32, y: i32) {
    if state.steps.len() >= state.max_steps {
        return;
    }
    state.flush_text();
    state.clicks += 1;
    let screen = super::capture().ok();
    let template = match (&screen, state.template) {
        (Some(s), true) => capture_template(state, s, x, y),
        _ => None,
    };
    let screen_area = screen
        .as_ref()
        .map(|s| s.img.width() as i64 * s.img.height() as i64)
        .unwrap_or(0);
    let node = uia::node_at(x, y);
    let target = target_for_click(x, y, node, template, screen_area);
    let n = state.steps.len() + 1;
    state.steps.push(Step {
        id: format!("click_{n}"),
        action: "click".to_string(),
        target: Some(target),
        ..Default::default()
    });
}

fn finish(state: &State) -> i32 {
    let wf = Workflow {
        schema_version: 2,
        name: Some("recorded".to_string()),
        defaults: Default::default(),
        vars: HashMap::new(),
        steps: state.steps.clone(),
    };
    let yaml = match serde_yaml::to_string(&wf) {
        Ok(y) => y,
        Err(e) => {
            eprintln!("error: serialize workflow: {e}");
            return 5;
        }
    };
    if let Some(parent) = state.out.parent() {
        if !parent.as_os_str().is_empty() {
            let _ = std::fs::create_dir_all(parent);
        }
    }
    if let Err(e) = std::fs::write(&state.out, yaml) {
        eprintln!("error: write {}: {e}", state.out.display());
        return 5;
    }
    println!(
        "recorded {} step(s) -> {}",
        state.steps.len(),
        state.out.display()
    );
    0
}

pub fn run(args: &[String]) -> i32 {
    let opts = match Opts::parse(args) {
        Ok(o) => o,
        Err(e) => return super::usage(&e),
    };
    let out = PathBuf::from(
        opts.value("out")
            .unwrap_or_else(|| "workflow.yaml".to_string()),
    );
    let template = !opts.flag("no-template");
    let max_steps = opts
        .value("max-steps")
        .and_then(|v| v.parse::<usize>().ok())
        .unwrap_or(200);
    let tpl_dir = out
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join("templates");

    let state = Mutex::new(State {
        steps: Vec::new(),
        text: String::new(),
        clicks: 0,
        max_steps,
        template,
        out,
        tpl_dir,
        tpl_idx: 0,
        last_pos: (0, 0),
        shift: false,
    });

    eprintln!("recording: click/type as usual, Esc stops and writes the workflow");

    let result = rdev::listen(move |event| {
        let Ok(mut st) = state.lock() else { return };
        match event.event_type {
            rdev::EventType::MouseMove { x, y } => {
                st.last_pos = (x.round() as i32, y.round() as i32);
            }
            rdev::EventType::ButtonPress(rdev::Button::Left) => {
                let (x, y) = st.last_pos;
                record_click(&mut st, x, y);
            }
            rdev::EventType::KeyPress(rdev::Key::Escape) => {
                let code = finish(&st);
                std::process::exit(code);
            }
            rdev::EventType::KeyPress(rdev::Key::Return) => {
                st.flush_text();
                st.push_key("enter");
            }
            rdev::EventType::KeyPress(rdev::Key::Tab) => {
                st.flush_text();
                st.push_key("tab");
            }
            rdev::EventType::KeyPress(rdev::Key::Backspace) => {
                st.text.pop();
            }
            rdev::EventType::KeyPress(rdev::Key::ShiftLeft)
            | rdev::EventType::KeyPress(rdev::Key::ShiftRight) => {
                st.shift = true;
            }
            rdev::EventType::KeyRelease(rdev::Key::ShiftLeft)
            | rdev::EventType::KeyRelease(rdev::Key::ShiftRight) => {
                st.shift = false;
            }
            rdev::EventType::KeyPress(k) => {
                if let Some(c) = char_for(k, st.shift) {
                    st.text.push(c);
                }
            }
            _ => {}
        }
    });

    match result {
        Ok(()) => 0,
        Err(e) => {
            eprintln!("error: 无法启动输入监听（需要交互桌面）: {e:?}");
            5
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const AREA: i64 = 2560 * 1440;

    fn node(id: &str, name: &str, class: &str) -> UiaNode {
        UiaNode {
            name: name.to_string(),
            automation_id: id.to_string(),
            class_name: class.to_string(),
            control_type: 50000,
            control_type_name: "Button",
            x: 10,
            y: 20,
            w: 30,
            h: 40,
            enabled: true,
            offscreen: false,
            depth: 0,
        }
    }

    #[test]
    fn prefers_automation_id_then_name_then_class() {
        let t = target_for_click(1, 1, Some((node("btn1", "保存", "Button"), Some("App".into()))), None, AREA);
        match t {
            TargetSpec::One(o) => {
                assert_eq!(o.kind, "uia");
                assert_eq!(o.automation_id.as_deref(), Some("btn1"));
                assert_eq!(o.window.as_deref(), Some("App"));
            }
            _ => panic!("expected one target"),
        }
        let t = target_for_click(1, 1, Some((node("", "保存", "Button"), None)), None, AREA);
        match t {
            TargetSpec::One(o) => assert_eq!(o.name.as_deref(), Some("保存")),
            _ => panic!("expected one target"),
        }
        let t = target_for_click(1, 1, Some((node("", "", "Button"), None)), None, AREA);
        match t {
            TargetSpec::One(o) => assert_eq!(o.class.as_deref(), Some("Button")),
            _ => panic!("expected one target"),
        }
    }

    #[test]
    fn falls_back_to_coords_without_uia_or_template() {
        let t = target_for_click(300, 400, None, None, AREA);
        match t {
            TargetSpec::One(o) => {
                assert_eq!(o.kind, "coords");
                assert_eq!(o.x, Some(300));
                assert_eq!(o.y, Some(400));
            }
            _ => panic!("expected coords target"),
        }
    }

    #[test]
    fn chains_uia_then_template() {
        let t = target_for_click(
            1,
            1,
            Some((node("btn1", "", ""), Some("App".into()))),
            Some("templates/tpl_001.png".into()),
            AREA,
        );
        match t {
            TargetSpec::Many(v) => {
                assert_eq!(v.len(), 2);
                assert_eq!(v[0].kind, "uia");
                assert_eq!(v[1].kind, "template");
            }
            _ => panic!("expected fallback chain"),
        }
    }
}
