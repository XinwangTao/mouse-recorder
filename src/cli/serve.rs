//! `--cli serve`: one long-lived process answering command requests on stdin.
//!
//! Agents pay the process start + DB open cost once instead of per call. Each
//! request is one JSON line:
//!
//! ```text
//! {"id": 7, "args": ["find", "--element", "scene/btn", "--json"]}
//! ```
//!
//! and each response is one JSON line on stdout:
//!
//! ```text
//! {"mrctl":1,"id":7,"code":0,"out":"<captured stdout>","err":"<captured stderr>"}
//! ```
//!
//! Command output is captured by pointing the process stdout/stderr handles at
//! temp files for the duration of the call (Rust resolves the handle on every
//! write, so `println!` lands in the file).

use std::io::{BufRead, Write};
use std::path::PathBuf;

#[cfg(windows)]
struct Capture {
    out_path: PathBuf,
    err_path: PathBuf,
    old_out: *mut core::ffi::c_void,
    old_err: *mut core::ffi::c_void,
    _out_file: std::fs::File,
    _err_file: std::fs::File,
}

#[cfg(windows)]
fn begin_capture(seq: u32) -> std::io::Result<Capture> {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::System::Console::{
        GetStdHandle, SetStdHandle, STD_ERROR_HANDLE, STD_OUTPUT_HANDLE,
    };

    let dir = std::env::temp_dir().join("mrctl-serve");
    std::fs::create_dir_all(&dir)?;
    let out_path = dir.join(format!("{}_{seq}.out", std::process::id()));
    let err_path = dir.join(format!("{}_{seq}.err", std::process::id()));
    let out_file = std::fs::File::create(&out_path)?;
    let err_file = std::fs::File::create(&err_path)?;
    unsafe {
        let old_out = GetStdHandle(STD_OUTPUT_HANDLE);
        let old_err = GetStdHandle(STD_ERROR_HANDLE);
        SetStdHandle(STD_OUTPUT_HANDLE, out_file.as_raw_handle() as _);
        SetStdHandle(STD_ERROR_HANDLE, err_file.as_raw_handle() as _);
        Ok(Capture {
            out_path,
            err_path,
            old_out,
            old_err,
            _out_file: out_file,
            _err_file: err_file,
        })
    }
}

#[cfg(windows)]
fn end_capture(c: Capture) -> (String, String) {
    use windows_sys::Win32::System::Console::{SetStdHandle, STD_ERROR_HANDLE, STD_OUTPUT_HANDLE};
    let _ = std::io::stdout().flush();
    let _ = std::io::stderr().flush();
    unsafe {
        SetStdHandle(STD_OUTPUT_HANDLE, c.old_out);
        SetStdHandle(STD_ERROR_HANDLE, c.old_err);
    }
    let out = std::fs::read_to_string(&c.out_path).unwrap_or_default();
    let err = std::fs::read_to_string(&c.err_path).unwrap_or_default();
    let _ = std::fs::remove_file(&c.out_path);
    let _ = std::fs::remove_file(&c.err_path);
    (out, err)
}

#[cfg(not(windows))]
struct Capture;
#[cfg(not(windows))]
fn begin_capture(_seq: u32) -> std::io::Result<Capture> {
    Ok(Capture)
}
#[cfg(not(windows))]
fn end_capture(_c: Capture) -> (String, String) {
    (String::new(), String::new())
}

pub fn run() -> i32 {
    eprintln!("mrctl serve: one JSON request per line on stdin, Esc/Ctrl-C or EOF exits");
    let stdin = std::io::stdin();
    let mut seq = 0u32;
    for line in stdin.lock().lines() {
        let Ok(line) = line else { break };
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if line == "quit" || line == "shutdown" {
            break;
        }
        let (id, args) = match parse_request(line) {
            Ok(v) => v,
            Err(e) => {
                let resp = serde_json::json!({
                    "mrctl": 1, "id": serde_json::Value::Null, "code": 2, "error": e
                });
                println!("{resp}");
                let _ = std::io::stdout().flush();
                continue;
            }
        };
        if args.first().map(|s| s.as_str()) == Some("serve") {
            let resp = serde_json::json!({
                "mrctl": 1, "id": id, "code": 2, "error": "nested serve is not supported"
            });
            println!("{resp}");
            let _ = std::io::stdout().flush();
            continue;
        }
        seq += 1;
        let capture = begin_capture(seq);
        let code = match &capture {
            Ok(_) => super::exec(&args),
            Err(_) => super::exec(&args),
        };
        let (out, err) = match capture {
            Ok(c) => end_capture(c),
            Err(_) => (String::new(), String::new()),
        };
        let resp = serde_json::json!({
            "mrctl": 1, "id": id, "code": code, "out": out, "err": err
        });
        println!("{resp}");
        let _ = std::io::stdout().flush();
    }
    0
}

/// Accept `{"id":1,"args":["find",...]}` and a bare `["find",...]` array.
fn parse_request(line: &str) -> Result<(serde_json::Value, Vec<String>), String> {
    let v: serde_json::Value =
        serde_json::from_str(line).map_err(|e| format!("bad request json: {e}"))?;
    let (id, arr) = match &v {
        serde_json::Value::Array(a) => (serde_json::Value::Null, a),
        serde_json::Value::Object(o) => (
            o.get("id").cloned().unwrap_or(serde_json::Value::Null),
            o.get("args")
                .and_then(|a| a.as_array())
                .ok_or_else(|| "request needs an 'args' array".to_string())?,
        ),
        _ => return Err("request must be a JSON object or array".into()),
    };
    let mut args = Vec::with_capacity(arr.len());
    for a in arr {
        match a {
            serde_json::Value::String(s) => args.push(s.clone()),
            other => args.push(other.to_string()),
        }
    }
    if args.is_empty() {
        return Err("empty args".into());
    }
    Ok((id, args))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_object_and_array_requests() {
        let (id, args) = parse_request(r#"{"id":3,"args":["find","--element","a"]}"#).unwrap();
        assert_eq!(id, serde_json::json!(3));
        assert_eq!(args, vec!["find", "--element", "a"]);
        let (id, args) = parse_request(r#"["list"]"#).unwrap();
        assert!(id.is_null());
        assert_eq!(args, vec!["list"]);
        assert!(parse_request("{}").is_err());
        assert!(parse_request("nope").is_err());
    }
}
