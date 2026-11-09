//! Windows UI Automation discovery for `--cli uia` / `--uia` targets.

use serde::Serialize;
use windows::core::Result as WResult;
use windows::Win32::Foundation::{BOOL, HWND, LPARAM};
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED,
};
use windows::Win32::UI::Accessibility::{
    CUIAutomation, IUIAutomation, IUIAutomationElement, IUIAutomationTreeWalker,
};
use windows::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GetWindowTextLengthW, GetWindowTextW, IsIconic, IsWindowVisible,
};

#[derive(Serialize, Clone, Debug)]
pub struct UiaNode {
    pub name: String,
    pub automation_id: String,
    pub class_name: String,
    pub control_type: i32,
    pub control_type_name: &'static str,
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
    pub enabled: bool,
    pub offscreen: bool,
    pub depth: usize,
}

impl UiaNode {
    pub fn cx(&self) -> i32 {
        self.x + self.w / 2
    }
    pub fn cy(&self) -> i32 {
        self.y + self.h / 2
    }
}

#[derive(Clone, Default, Debug)]
pub struct Selector {
    pub name: Option<String>,
    pub automation_id: Option<String>,
    pub class_name: Option<String>,
    pub control_type: Option<String>,
    pub index: Option<usize>,
}

impl Selector {
    pub fn from_opts(opts: &crate::cli::Opts) -> Option<Self> {
        let s = Self {
            name: opts.value("name"),
            automation_id: opts.value("automation-id"),
            class_name: opts.value("class"),
            control_type: opts.value("control-type"),
            index: opts.value("index").and_then(|v| v.parse().ok()),
        };
        if s.name.is_none()
            && s.automation_id.is_none()
            && s.class_name.is_none()
            && s.control_type.is_none()
        {
            None
        } else {
            Some(s)
        }
    }

    fn matches(&self, n: &UiaNode) -> bool {
        let name_ok = self
            .name
            .as_ref()
            .map(|q| n.name.to_lowercase().contains(&q.to_lowercase()))
            .unwrap_or(true);
        let id_ok = self
            .automation_id
            .as_ref()
            .map(|q| n.automation_id.eq_ignore_ascii_case(q))
            .unwrap_or(true);
        let class_ok = self
            .class_name
            .as_ref()
            .map(|q| n.class_name.eq_ignore_ascii_case(q))
            .unwrap_or(true);
        let type_ok = self
            .control_type
            .as_ref()
            .map(|q| n.control_type_name.eq_ignore_ascii_case(q))
            .unwrap_or(true);
        name_ok && id_ok && class_ok && type_ok
    }
}

fn control_type_name(id: i32) -> &'static str {
    match id {
        50000 => "Button",
        50001 => "Calendar",
        50002 => "CheckBox",
        50003 => "ComboBox",
        50004 => "Edit",
        50005 => "Hyperlink",
        50006 => "Image",
        50007 => "ListItem",
        50008 => "List",
        50009 => "Menu",
        50010 => "MenuBar",
        50011 => "MenuItem",
        50012 => "ProgressBar",
        50013 => "RadioButton",
        50014 => "ScrollBar",
        50015 => "Slider",
        50016 => "Spinner",
        50017 => "StatusBar",
        50018 => "Tab",
        50019 => "TabItem",
        50020 => "Text",
        50021 => "ToolBar",
        50022 => "ToolTip",
        50023 => "Tree",
        50024 => "TreeItem",
        50025 => "Custom",
        50026 => "Group",
        50027 => "Thumb",
        50028 => "DataGrid",
        50029 => "DataItem",
        50030 => "Document",
        50031 => "SplitButton",
        50032 => "Window",
        50033 => "Pane",
        50034 => "Header",
        50035 => "HeaderItem",
        50036 => "Table",
        50037 => "TitleBar",
        50038 => "Separator",
        50039 => "SemanticZoom",
        50040 => "AppBar",
        _ => "Unknown",
    }
}

unsafe extern "system" fn enum_window(hwnd: HWND, lparam: LPARAM) -> BOOL {
    let out = &mut *(lparam.0 as *mut Vec<HWND>);
    if IsWindowVisible(hwnd).as_bool() && !IsIconic(hwnd).as_bool() {
        out.push(hwnd);
    }
    BOOL(1)
}

fn window_title(hwnd: HWND) -> String {
    let len = unsafe { GetWindowTextLengthW(hwnd) };
    if len <= 0 {
        return String::new();
    }
    let mut buf = vec![0u16; (len + 1) as usize];
    let n = unsafe { GetWindowTextW(hwnd, &mut buf) };
    String::from_utf16_lossy(&buf[..n.max(0) as usize])
}

fn find_window(title: &str) -> Result<HWND, String> {
    let q = title.trim().to_lowercase();
    if q.is_empty() {
        return Err("window title is empty".into());
    }
    let mut windows: Vec<HWND> = Vec::new();
    unsafe {
        let _ = EnumWindows(Some(enum_window), LPARAM(&mut windows as *mut _ as isize));
    }
    for hwnd in windows {
        let t = window_title(hwnd);
        if !t.is_empty() && t.to_lowercase().contains(&q) {
            return Ok(hwnd);
        }
    }
    Err(format!("未找到窗口: {title}"))
}

pub fn window_rect(title: &str) -> Result<(i32, i32, i32, i32), String> {
    use windows::Win32::UI::WindowsAndMessaging::GetWindowRect;
    let hwnd = find_window(title)?;
    let mut r = windows::Win32::Foundation::RECT::default();
    unsafe { GetWindowRect(hwnd, &mut r) }.map_err(|e| format!("GetWindowRect: {e}"))?;
    let w = r.right - r.left;
    let h = r.bottom - r.top;
    if w <= 0 || h <= 0 {
        return Err(format!("窗口矩形无效: {title}"));
    }
    Ok((r.left, r.top, w, h))
}

fn read_node(el: &IUIAutomationElement, depth: usize) -> UiaNode {
    let name = unsafe { el.CurrentName() }
        .map(|b| b.to_string())
        .unwrap_or_default();
    let automation_id = unsafe { el.CurrentAutomationId() }
        .map(|b| b.to_string())
        .unwrap_or_default();
    let class_name = unsafe { el.CurrentClassName() }
        .map(|b| b.to_string())
        .unwrap_or_default();
    let control_type = unsafe { el.CurrentControlType() }
        .map(|c| c.0)
        .unwrap_or(0);
    let (x, y, w, h) = match unsafe { el.CurrentBoundingRectangle() } {
        Ok(r) => (r.left, r.top, r.right - r.left, r.bottom - r.top),
        Err(_) => (0, 0, 0, 0),
    };
    let enabled = unsafe { el.CurrentIsEnabled() }
        .map(|b| b.as_bool())
        .unwrap_or(true);
    let offscreen = unsafe { el.CurrentIsOffscreen() }
        .map(|b| b.as_bool())
        .unwrap_or(false);
    UiaNode {
        name,
        automation_id,
        class_name,
        control_type,
        control_type_name: control_type_name(control_type),
        x,
        y,
        w,
        h,
        enabled,
        offscreen,
        depth,
    }
}

fn walk(
    walker: &IUIAutomationTreeWalker,
    parent: &IUIAutomationElement,
    level: usize,
    max_depth: usize,
    max_nodes: usize,
    selector: &Option<Selector>,
    out: &mut Vec<UiaNode>,
) {
    if level > max_depth || out.len() >= max_nodes {
        return;
    }
    let mut child: WResult<IUIAutomationElement> =
        unsafe { walker.GetFirstChildElement(parent) };
    while let Ok(el) = child {
        if out.len() >= max_nodes {
            break;
        }
        let node = read_node(&el, level);
        let matched = selector
            .as_ref()
            .map(|s| s.matches(&node))
            .unwrap_or(true);
        if matched && !node.offscreen && node.w > 0 && node.h > 0 {
            out.push(node);
        }
        walk(walker, &el, level + 1, max_depth, max_nodes, selector, out);
        child = unsafe { walker.GetNextSiblingElement(&el) };
    }
}

fn automation() -> Result<IUIAutomation, String> {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
    }
    unsafe { CoCreateInstance(&CUIAutomation, None, CLSCTX_INPROC_SERVER) }
        .map_err(|e| format!("CoCreateInstance(UIAutomation): {e}"))
}

pub fn dump(
    window: &str,
    max_nodes: usize,
    max_depth: usize,
    selector: Option<Selector>,
) -> Result<Vec<UiaNode>, String> {
    let hwnd = find_window(window)?;
    let automation = automation()?;
    let root = unsafe { automation.ElementFromHandle(hwnd) }
        .map_err(|e| format!("ElementFromHandle: {e}"))?;
    let walker = unsafe { automation.ControlViewWalker() }
        .map_err(|e| format!("ControlViewWalker: {e}"))?;
    let mut out = Vec::new();
    walk(
        &walker,
        &root,
        0,
        max_depth,
        max_nodes,
        &selector,
        &mut out,
    );
    if selector.is_some() {
        if let Some(idx) = selector.as_ref().and_then(|s| s.index) {
            if idx >= out.len() {
                return Ok(Vec::new());
            }
            out = vec![out[idx].clone()];
        }
    }
    Ok(out)
}

pub fn find(window: &str, selector: &Selector) -> Result<Option<UiaNode>, String> {
    let sel = Some(selector.clone());
    let nodes = dump(window, 400, 12, sel)?;
    Ok(nodes.into_iter().next())
}

pub fn window_exists(title: &str) -> bool {
    find_window(title).is_ok()
}

/// Raise `title` to the foreground so synthetic clicks and keystrokes land on
/// it rather than on whatever happens to be on top.
///
/// UIA can read a covered window; SendInput cannot reach it. Windows only lets
/// the foreground process steal focus, so we attach to the current foreground
/// thread first (the documented bypass) and fall back to a no-activate z-order
/// bump.
pub fn raise_window(title: &str) -> Result<(), String> {
    use windows::Win32::System::Threading::{AttachThreadInput, GetCurrentThreadId};
    use windows::Win32::UI::WindowsAndMessaging::{
        BringWindowToTop, GetForegroundWindow, GetWindowThreadProcessId, SetForegroundWindow,
        SetWindowPos, ShowWindow, HWND_TOP, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SWP_SHOWWINDOW,
        SW_RESTORE,
    };
    let hwnd = find_window(title)?;
    unsafe {
        if IsIconic(hwnd).as_bool() {
            let _ = ShowWindow(hwnd, SW_RESTORE);
        }
        if GetForegroundWindow() == hwnd {
            return Ok(());
        }
        let fg = GetForegroundWindow();
        let fg_tid = GetWindowThreadProcessId(fg, None);
        let me = GetCurrentThreadId();
        let attached = fg_tid != 0 && fg_tid != me && AttachThreadInput(me, fg_tid, BOOL(1)).as_bool();
        let _ = BringWindowToTop(hwnd);
        let activated = SetForegroundWindow(hwnd).as_bool();
        if attached {
            let _ = AttachThreadInput(me, fg_tid, BOOL(0));
        }
        if !activated || GetForegroundWindow() != hwnd {
            // Best effort: at least put it on top so a click reaches it.
            let _ = SetWindowPos(
                hwnd,
                HWND_TOP,
                0,
                0,
                0,
                0,
                SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | SWP_SHOWWINDOW,
            );
        }
    }
    // Give the compositor a moment to finish the z-order change, otherwise
    // WindowFromPoint still reports the old top window.
    std::thread::sleep(std::time::Duration::from_millis(150));
    Ok(())
}

/// `Ok(None)` when the point really hits `title`'s window, otherwise the title
/// of the window that would receive the click. Guards against clicking through
/// to a covering window when a UIA target is occluded.
pub fn occluder_at(title: &str, x: i32, y: i32) -> Result<Option<String>, String> {
    use windows::Win32::UI::WindowsAndMessaging::{GetAncestor, WindowFromPoint, GA_ROOT};
    let hwnd = find_window(title)?;
    let hit = unsafe { WindowFromPoint(windows::Win32::Foundation::POINT { x, y }) };
    if hit.0.is_null() {
        return Ok(Some("<无窗口>".to_string()));
    }
    let root = unsafe { GetAncestor(hit, GA_ROOT) };
    if hit == hwnd || root == hwnd {
        return Ok(None);
    }
    let name = window_title(root);
    Ok(Some(if name.is_empty() {
        "<无标题窗口>".to_string()
    } else {
        name
    }))
}
