//! OCR via the locally installed Tesseract CLI (no extra Rust runtime).

use serde::Serialize;
use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Serialize, Clone, Debug)]
pub struct OcrHit {
    pub text: String,
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
    pub conf: f32,
}

fn tesseract_exe() -> Option<PathBuf> {
    if let Ok(p) = std::env::var("TESSERACT") {
        let p = PathBuf::from(p);
        if p.is_file() {
            return Some(p);
        }
    }
    let common = Path::new(r"C:\Program Files\Tesseract-OCR\tesseract.exe");
    if common.is_file() {
        return Some(common.to_path_buf());
    }
    Some(PathBuf::from("tesseract"))
}

/// Word boxes from `tesseract ... tsv`.
pub fn word_boxes(image: &Path, lang: &str) -> Result<Vec<OcrHit>, String> {
    let exe = tesseract_exe().ok_or_else(|| "Tesseract not found".to_string())?;
    let out = Command::new(&exe)
        .arg(image)
        .arg("stdout")
        .args(["-l", lang, "--psm", "11", "tsv"])
        .output()
        .map_err(|e| format!("run {}: {e}", exe.display()))?;
    if !out.status.success() {
        return Err(format!(
            "tesseract failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    let text = String::from_utf8_lossy(&out.stdout);
    let mut words = Vec::new();
    for (i, line) in text.lines().enumerate() {
        if i == 0 {
            continue;
        }
        let cols: Vec<&str> = line.split('\t').collect();
        if cols.len() < 12 {
            continue;
        }
        if cols[0] != "5" {
            continue; // level 5 = word
        }
        let Ok(x) = cols[6].parse::<i32>() else { continue };
        let Ok(y) = cols[7].parse::<i32>() else { continue };
        let Ok(w) = cols[8].parse::<i32>() else { continue };
        let Ok(h) = cols[9].parse::<i32>() else { continue };
        let conf = cols[10].parse::<f32>().unwrap_or(-1.0);
        let word = cols[11].trim().to_string();
        if word.is_empty() {
            continue;
        }
        words.push(OcrHit {
            text: word,
            x,
            y,
            w,
            h,
            conf,
        });
    }
    Ok(words)
}

/// Find a word or phrase on an image. Phrases return the union of the words on
/// the matching line.
pub fn find_text(image: &Path, query: &str, lang: &str) -> Result<Option<OcrHit>, String> {
    let query = query.trim();
    if query.is_empty() {
        return Ok(None);
    }
    let words = word_boxes(image, lang)?;
    let lower = query.to_lowercase();
    // Single token: exact/contains word match.
    if !lower.contains(' ') {
        if let Some(w) = words
            .iter()
            .find(|w| w.text.to_lowercase().contains(&lower))
        {
            return Ok(Some(w.clone()));
        }
    }
    // Phrase: group words by line and search the joined text.
    let text = words
        .iter()
        .map(|w| w.text.as_str())
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase();
    if !text.contains(&lower) {
        return Ok(None);
    }
    // Approximate: union of all word boxes whose text participates. For a
    // phrase, start from the first word matching the first token.
    let first = lower.split_whitespace().next().unwrap_or(&lower);
    let Some(start) = words
        .iter()
        .position(|w| w.text.to_lowercase().contains(first))
    else {
        return Ok(None);
    };
    let mut x = words[start].x;
    let mut y = words[start].y;
    let mut x2 = words[start].x + words[start].w;
    let mut y2 = words[start].y + words[start].h;
    let mut conf = words[start].conf;
    let mut used = vec![words[start].text.clone()];
    let mut remaining = lower[first.len()..].trim().to_string();
    for w in words.iter().skip(start + 1) {
        if remaining.is_empty() {
            break;
        }
        let wt = w.text.to_lowercase();
        if remaining.starts_with(&wt) || wt.contains(remaining.split_whitespace().next().unwrap_or("")) {
            x = x.min(w.x);
            y = y.min(w.y);
            x2 = x2.max(w.x + w.w);
            y2 = y2.max(w.y + w.h);
            conf = conf.min(w.conf);
            used.push(w.text.clone());
            remaining = remaining[wt.len().min(remaining.len())..].trim().to_string();
        }
    }
    Ok(Some(OcrHit {
        text: used.join(" "),
        x,
        y,
        w: x2 - x,
        h: y2 - y,
        conf,
    }))
}

#[allow(dead_code)]
pub fn text(image: &Path, lang: &str) -> Result<String, String> {
    let exe = tesseract_exe().ok_or_else(|| "Tesseract not found".to_string())?;
    let out = Command::new(&exe)
        .arg(image)
        .arg("stdout")
        .args(["-l", lang, "--psm", "11"])
        .output()
        .map_err(|e| format!("run {}: {e}", exe.display()))?;
    if !out.status.success() {
        return Err(format!(
            "tesseract failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    Ok(String::from_utf8_lossy(&out.stdout).to_string())
}
