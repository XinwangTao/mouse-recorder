//! SendInput helpers for the CLI (clicks, Unicode text, key chords).

#[derive(Clone, Copy, Debug)]
pub enum Button {
    Left,
    Right,
}

#[cfg(windows)]
fn send_input(input: &windows_sys::Win32::UI::Input::KeyboardAndMouse::INPUT) -> bool {
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::SendInput;
    let sent = unsafe {
        SendInput(
            1,
            input,
            std::mem::size_of::<windows_sys::Win32::UI::Input::KeyboardAndMouse::INPUT>() as i32,
        )
    };
    sent == 1
}

#[cfg(windows)]
fn mouse_event(flags: u32) -> bool {
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
        INPUT, INPUT_MOUSE, MOUSEINPUT,
    };
    let mut input: INPUT = unsafe { std::mem::zeroed() };
    input.r#type = INPUT_MOUSE;
    input.Anonymous.mi = MOUSEINPUT {
        dx: 0,
        dy: 0,
        mouseData: 0,
        dwFlags: flags,
        time: 0,
        dwExtraInfo: 0,
    };
    send_input(&input)
}

#[cfg(windows)]
fn key_event(vk: u16, scan: u16, flags: u32) -> bool {
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
        INPUT, INPUT_KEYBOARD, KEYBDINPUT,
    };
    let mut input: INPUT = unsafe { std::mem::zeroed() };
    input.r#type = INPUT_KEYBOARD;
    input.Anonymous.ki = KEYBDINPUT {
        wVk: vk,
        wScan: scan,
        dwFlags: flags,
        time: 0,
        dwExtraInfo: 0,
    };
    send_input(&input)
}

#[cfg(windows)]
pub fn click(x: i32, y: i32, button: Button, double: bool) -> Result<(), String> {
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
        MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP, MOUSEEVENTF_RIGHTDOWN, MOUSEEVENTF_RIGHTUP,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::SetCursorPos;

    if unsafe { SetCursorPos(x, y) } == 0 {
        return Err(format!("SetCursorPos({x},{y}) failed"));
    }
    std::thread::sleep(std::time::Duration::from_millis(40));
    let (down, up) = match button {
        Button::Left => (MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP),
        Button::Right => (MOUSEEVENTF_RIGHTDOWN, MOUSEEVENTF_RIGHTUP),
    };
    let clicks = if double { 2 } else { 1 };
    for i in 0..clicks {
        if !mouse_event(down) || !mouse_event(up) {
            return Err("SendInput mouse failed".into());
        }
        if i + 1 < clicks {
            std::thread::sleep(std::time::Duration::from_millis(70));
        }
    }
    Ok(())
}

#[cfg(not(windows))]
pub fn click(_x: i32, _y: i32, _button: Button, _double: bool) -> Result<(), String> {
    Err("input is Windows-only".into())
}

#[cfg(windows)]
pub fn send_text(text: &str) -> Result<(), String> {
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::{KEYEVENTF_KEYUP, KEYEVENTF_UNICODE};
    for unit in text.encode_utf16() {
        if !key_event(0, unit, KEYEVENTF_UNICODE)
            || !key_event(0, unit, KEYEVENTF_UNICODE | KEYEVENTF_KEYUP)
        {
            return Err("SendInput text failed".into());
        }
        std::thread::sleep(std::time::Duration::from_millis(6));
    }
    Ok(())
}

#[cfg(not(windows))]
pub fn send_text(_text: &str) -> Result<(), String> {
    Err("input is Windows-only".into())
}

fn vk_for(name: &str) -> Option<u16> {
    let name = name.trim().to_ascii_lowercase();
    Some(match name.as_str() {
        "enter" | "return" => 0x0D,
        "tab" => 0x09,
        "esc" | "escape" => 0x1B,
        "space" => 0x20,
        "backspace" | "bs" => 0x08,
        "delete" | "del" => 0x2E,
        "insert" | "ins" => 0x2D,
        "home" => 0x24,
        "end" => 0x23,
        "pageup" | "pgup" => 0x21,
        "pagedown" | "pgdn" => 0x22,
        "up" => 0x26,
        "down" => 0x28,
        "left" => 0x25,
        "right" => 0x27,
        "f1" => 0x70,
        "f2" => 0x71,
        "f3" => 0x72,
        "f4" => 0x73,
        "f5" => 0x74,
        "f6" => 0x75,
        "f7" => 0x76,
        "f8" => 0x77,
        "f9" => 0x78,
        "f10" => 0x79,
        "f11" => 0x7A,
        "f12" => 0x7B,
        _ => {
            let mut chars = name.chars();
            let (Some(c), None) = (chars.next(), chars.next()) else {
                return None;
            };
            let c = c.to_ascii_uppercase();
            if c.is_ascii_alphanumeric() {
                return Some(c as u16);
            }
            return None;
        }
    })
}

#[cfg(windows)]
fn modifier_vk(name: &str) -> Option<u16> {
    Some(match name {
        "ctrl" | "control" => 0x11,
        "shift" => 0x10,
        "alt" => 0x12,
        "win" | "meta" => 0x5B,
        _ => return None,
    })
}

/// `enter`, `ctrl+a`, `shift+tab`, ...
#[cfg(windows)]
pub fn send_key(spec: &str) -> Result<(), String> {
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::KEYEVENTF_KEYUP;
    let parts: Vec<&str> = spec.split('+').map(|p| p.trim()).filter(|p| !p.is_empty()).collect();
    if parts.is_empty() {
        return Err("empty key".into());
    }
    let mut mods = Vec::new();
    for p in &parts[..parts.len() - 1] {
        let vk = modifier_vk(&p.to_ascii_lowercase())
            .ok_or_else(|| format!("unknown modifier: {p}"))?;
        if !key_event(vk, 0, 0) {
            return Err("SendInput modifier failed".into());
        }
        mods.push(vk);
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    let last = parts[parts.len() - 1];
    let vk = vk_for(last).ok_or_else(|| format!("unknown key: {last}"))?;
    if !key_event(vk, 0, 0) || !key_event(vk, 0, KEYEVENTF_KEYUP) {
        return Err("SendInput key failed".into());
    }
    for m in mods.into_iter().rev() {
        let _ = key_event(m, 0, KEYEVENTF_KEYUP);
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    Ok(())
}

#[cfg(not(windows))]
pub fn send_key(_spec: &str) -> Result<(), String> {
    Err("input is Windows-only".into())
}
