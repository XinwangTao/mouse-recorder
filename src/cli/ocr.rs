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

type LineKey = (u32, u32, u32, u32);
type LineWords<'a> = (LineKey, Vec<&'a OcrWord>);

#[derive(Clone, Debug)]
struct OcrWord {
    hit: OcrHit,
    line: LineKey,
}

/// Word boxes from `tesseract ... tsv`, grouped by page/block/paragraph/line.
fn words_with_lines(image: &Path, lang: &str) -> Result<Vec<OcrWord>, String> {
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
        if cols.len() < 12 || cols[0] != "5" {
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
        let line_key = (
            cols[1].parse::<u32>().unwrap_or(0),
            cols[2].parse::<u32>().unwrap_or(0),
            cols[3].parse::<u32>().unwrap_or(0),
            cols[4].parse::<u32>().unwrap_or(i as u32),
        );
        words.push(OcrWord {
            hit: OcrHit {
                text: word,
                x,
                y,
                w,
                h,
                conf,
            },
            line: line_key,
        });
    }
    Ok(words)
}

/// Word boxes (flat, reading order).
#[allow(dead_code)]
pub fn word_boxes(image: &Path, lang: &str) -> Result<Vec<OcrHit>, String> {
    Ok(words_with_lines(image, lang)?
        .into_iter()
        .map(|w| w.hit)
        .collect())
}

/// Find a word or phrase on an image. Phrases only match word sequences on the
/// same OCR line; the result box is the union of the matched words.
pub fn find_text(image: &Path, query: &str, lang: &str) -> Result<Option<OcrHit>, String> {
    let words = words_with_lines(image, lang)?;
    Ok(match_words(&words, query))
}

fn match_words(words: &[OcrWord], query: &str) -> Option<OcrHit> {
    let query = query.trim();
    if query.is_empty() {
        return None;
    }
    let lower = query.to_lowercase();
    if !lower.contains(' ') {
        return words
            .iter()
            .find(|w| w.hit.text.to_lowercase().contains(&lower))
            .map(|w| w.hit.clone());
    }

    // Group by line, preserving first-seen order.
    let mut lines: Vec<LineWords> = Vec::new();
    for w in words {
        if let Some((_, v)) = lines.iter_mut().find(|(k, _)| *k == w.line) {
            v.push(w);
        } else {
            lines.push((w.line, vec![w]));
        }
    }

    let first_token = lower.split_whitespace().next().unwrap_or("");
    for (_, line) in lines {
        if line.is_empty() {
            continue;
        }
        let joined = line
            .iter()
            .map(|w| w.hit.text.as_str())
            .collect::<Vec<_>>()
            .join(" ")
            .to_lowercase();
        if !joined.contains(&lower) {
            continue;
        }
        let start = line
            .iter()
            .position(|w| w.hit.text.to_lowercase().contains(first_token))
            .unwrap_or(0);
        let mut x = line[start].hit.x;
        let mut y = line[start].hit.y;
        let mut x2 = x + line[start].hit.w;
        let mut y2 = y + line[start].hit.h;
        let mut conf = line[start].hit.conf;
        let mut used = Vec::new();
        let mut matched = String::new();
        for w in line.iter().skip(start) {
            used.push(w.hit.text.clone());
            matched = if matched.is_empty() {
                w.hit.text.clone()
            } else {
                format!("{matched} {}", w.hit.text)
            };
            x = x.min(w.hit.x);
            y = y.min(w.hit.y);
            x2 = x2.max(w.hit.x + w.hit.w);
            y2 = y2.max(w.hit.y + w.hit.h);
            conf = conf.min(w.hit.conf);
            if matched.to_lowercase().contains(&lower) {
                break;
            }
        }
        return Some(OcrHit {
            text: used.join(" "),
            x,
            y,
            w: x2 - x,
            h: y2 - y,
            conf,
        });
    }
    None
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

#[cfg(test)]
mod tests {
    use super::*;

    fn w(text: &str, x: i32, line: u32) -> OcrWord {
        OcrWord {
            hit: OcrHit {
                text: text.to_string(),
                x,
                y: (line as i32) * 20,
                w: 20,
                h: 12,
                conf: 90.0,
            },
            line: (0, 0, 0, line),
        }
    }

    #[test]
    fn phrase_must_be_on_one_line() {
        let words = vec![w("HELLO", 10, 1), w("WORLD", 40, 1), w("HELLO", 10, 2), w("THERE", 40, 2)];
        let hit = match_words(&words, "hello world").unwrap();
        assert_eq!(hit.text, "HELLO WORLD");
        assert_eq!(hit.x, 10);
        assert_eq!(hit.w, 50);
        assert!(match_words(&words, "world there").is_none());
        assert_eq!(match_words(&words, "hello").unwrap().text, "HELLO");
        assert!(match_words(&words, "").is_none());
    }
}
