//! Template match + element health helpers (score aligned with mouse-suite SAD).

use image::{ImageBuffer, Rgba};
use std::collections::{HashMap, HashSet};
use std::path::Path;

/// Same score as mouse-suite clicker: 1 - mean(|ΔRGB|) / (255*3).
///
/// Exact-score API for callers that need the real value; the search itself uses
/// [`score_at_above`] so it can abort losing candidates early.
#[allow(dead_code)]
pub fn score_at(
    screen: &ImageBuffer<Rgba<u8>, Vec<u8>>,
    template: &ImageBuffer<Rgba<u8>, Vec<u8>>,
    ox: u32,
    oy: u32,
) -> f32 {
    score_at_inner(screen, template, ox, oy, f32::NEG_INFINITY).unwrap_or(0.0)
}

/// Exact SAD score, but stop as soon as zero error on the remaining pixels
/// could no longer lift the result above `min_score`.
///
/// The matcher only needs to know whether a candidate beats the current best.
/// Once the partial error sum is past `(1 - min_score) * 765 * n`, the final
/// score is guaranteed `<= min_score`, so the rest of the template can be
/// skipped. This turns most full-screen candidates into a few-row scan.
pub fn score_at_above(
    screen: &ImageBuffer<Rgba<u8>, Vec<u8>>,
    template: &ImageBuffer<Rgba<u8>, Vec<u8>>,
    ox: u32,
    oy: u32,
    min_score: f32,
) -> Option<f32> {
    score_at_inner(screen, template, ox, oy, min_score)
}

fn score_at_inner(
    screen: &ImageBuffer<Rgba<u8>, Vec<u8>>,
    template: &ImageBuffer<Rgba<u8>, Vec<u8>>,
    ox: u32,
    oy: u32,
    min_score: f32,
) -> Option<f32> {
    let (sw, sh) = screen.dimensions();
    let (tw, th) = template.dimensions();
    if tw == 0
        || th == 0
        || ox.checked_add(tw).is_none_or(|v| v > sw)
        || oy.checked_add(th).is_none_or(|v| v > sh)
    {
        return None;
    }

    let n = (tw as u64) * (th as u64);
    let screen_raw = screen.as_raw();
    let template_raw = template.as_raw();
    let screen_stride = sw as usize * 4;
    let row_bytes = tw as usize * 4;
    // Abort threshold in absolute RGB error units. `-inf` (no current best)
    // disables early exit.
    let threshold = if min_score.is_finite() {
        (1.0 - min_score) as f64 * 765.0 * n as f64
    } else {
        f64::INFINITY
    };

    let mut sum: i64 = 0;
    for ty in 0..th as usize {
        let s_start = (oy as usize + ty) * screen_stride + ox as usize * 4;
        let t_start = ty * row_bytes;
        let srow = &screen_raw[s_start..s_start + row_bytes];
        let trow = &template_raw[t_start..t_start + row_bytes];
        for (sp, tp) in srow.chunks_exact(4).zip(trow.chunks_exact(4)) {
            sum += (sp[0] as i32 - tp[0] as i32).unsigned_abs() as i64
                + (sp[1] as i32 - tp[1] as i32).unsigned_abs() as i64
                + (sp[2] as i32 - tp[2] as i32).unsigned_abs() as i64;
        }
        // `>=` is safe: a final score exactly equal to the best would not
        // replace it (`>` in the search loop).
        if sum as f64 >= threshold {
            return None;
        }
    }

    Some(1.0 - (sum as f32 / n as f32) / (255.0 * 3.0))
}

#[derive(Clone, Debug)]
pub struct MatchHit {
    pub score: f32,
    pub hit_x: i32,
    pub hit_y: i32,
    pub hit_cx: i32,
    pub hit_cy: i32,
    pub tw: i32,
    pub th: i32,
    pub used_roi: bool,
}

/// Coarse-then-refine search inside ROI (absolute screen coords).
pub fn find_template(
    screen: &ImageBuffer<Rgba<u8>, Vec<u8>>,
    template: &ImageBuffer<Rgba<u8>, Vec<u8>>,
    roi: Option<(i32, i32, i32, i32)>,
) -> Option<MatchHit> {
    let (sw, sh) = screen.dimensions();
    let (tw, th) = template.dimensions();
    if tw == 0 || th == 0 || tw > sw || th > sh {
        return None;
    }

    let (x0, y0, x1, y1, used_roi) = if let Some((rx, ry, rw, rh)) = roi {
        let x0 = rx.max(0) as u32;
        let y0 = ry.max(0) as u32;
        let x1 = (rx + rw).clamp(0, sw as i32) as u32;
        let y1 = (ry + rh).clamp(0, sh as i32) as u32;
        (x0, y0, x1, y1, true)
    } else {
        (0, 0, sw, sh, false)
    };

    if x1 <= x0 || y1 <= y0 || tw > (x1 - x0) || th > (y1 - y0) {
        return None;
    }

    let max_x = x1 - tw;
    let max_y = y1 - th;
    let step = ((tw.min(th) / 16).max(1)).min(4);

    let mut best_score = f32::NEG_INFINITY;
    let mut best = (x0, y0);

    let mut y = y0;
    while y <= max_y {
        let mut x = x0;
        while x <= max_x {
            if let Some(s) = score_at_above(screen, template, x, y, best_score) {
                if s > best_score {
                    best_score = s;
                    best = (x, y);
                }
            }
            x += step;
        }
        y += step;
    }

    if step > 1 {
        let rx0 = best.0.saturating_sub(step).max(x0);
        let ry0 = best.1.saturating_sub(step).max(y0);
        let rx1 = (best.0 + step).min(max_x);
        let ry1 = (best.1 + step).min(max_y);
        for y in ry0..=ry1 {
            for x in rx0..=rx1 {
                if let Some(s) = score_at_above(screen, template, x, y, best_score) {
                    if s > best_score {
                        best_score = s;
                        best = (x, y);
                    }
                }
            }
        }
    }

    if !best_score.is_finite() {
        return None;
    }

    let (hx, hy) = best;
    Some(MatchHit {
        score: best_score,
        hit_x: hx as i32,
        hit_y: hy as i32,
        hit_cx: hx as i32 + (tw / 2) as i32,
        hit_cy: hy as i32 + (th / 2) as i32,
        tw: tw as i32,
        th: th as i32,
        used_roi,
    })
}

/// Prefer ROI; if score < threshold, also try full screen and keep the better hit.
pub fn find_template_smart(
    screen: &ImageBuffer<Rgba<u8>, Vec<u8>>,
    template: &ImageBuffer<Rgba<u8>, Vec<u8>>,
    roi: Option<(i32, i32, i32, i32)>,
    fallback_threshold: f32,
) -> Option<MatchHit> {
    let in_roi = find_template(screen, template, roi);
    if let Some(ref h) = in_roi {
        if h.score >= fallback_threshold {
            return in_roi;
        }
    }
    let full = find_template(screen, template, None);
    match (in_roi, full) {
        (Some(a), Some(b)) => Some(if b.score > a.score { b } else { a }),
        (Some(a), None) => Some(a),
        (None, Some(b)) => Some(b),
        (None, None) => None,
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HealthLevel {
    Ok,
    Warn,
    Bad,
}

impl HealthLevel {
    pub fn worsen(self, other: HealthLevel) -> HealthLevel {
        use HealthLevel::*;
        match (self, other) {
            (Bad, _) | (_, Bad) => Bad,
            (Warn, _) | (_, Warn) => Warn,
            _ => Ok,
        }
    }

    pub fn color_rgb(self) -> (u8, u8, u8) {
        match self {
            HealthLevel::Ok => (80, 180, 80),
            HealthLevel::Warn => (220, 180, 40),
            HealthLevel::Bad => (220, 60, 60),
        }
    }
}

#[derive(Clone, Debug)]
pub struct HealthReport {
    pub level: HealthLevel,
    pub badges: Vec<&'static str>,
    pub hints: Vec<String>,
}

pub fn name_dup_counts<'a>(names: impl Iterator<Item = &'a str>) -> HashMap<String, usize> {
    let mut m = HashMap::new();
    for n in names {
        *m.entry(n.to_string()).or_insert(0) += 1;
    }
    m
}

pub fn element_group(name: &str) -> &str {
    name.split_once('/')
        .map(|(g, _)| g)
        .filter(|g| !g.is_empty())
        .unwrap_or("(未分组)")
}

/// states: (is_primary, state_name, absolute image path)
/// `primary_image_dims` is passed in (not read here) so the caller can cache
/// disk lookups — this runs for every element on every UI frame.
pub fn assess_element(
    name: &str,
    cx: i32,
    cy: i32,
    bw: i32,
    bh: i32,
    screen_w: i32,
    screen_h: i32,
    cur_sw: i32,
    cur_sh: i32,
    states: &[(bool, &str, &Path)],
    dup_count: usize,
    primary_image_dims: Option<(u32, u32)>,
) -> HealthReport {
    let mut badges = Vec::new();
    let mut hints = Vec::new();
    let mut level = HealthLevel::Ok;

    if states.is_empty() {
        badges.push("无状态");
        hints.push("没有状态截图".into());
        level = HealthLevel::Bad;
    }
    if !states.is_empty() && !states.iter().any(|(p, _, _)| *p) {
        badges.push("无主状态");
        hints.push("缺少 is_primary 主状态".into());
        level = level.worsen(HealthLevel::Warn);
    }
    for (_, sn, path) in states {
        if !path.exists() {
            badges.push("缺图");
            hints.push(format!("状态 {} 图片不存在: {}", sn, path.display()));
            level = HealthLevel::Bad;
            break;
        }
    }
    if cx == 0 && cy == 0 {
        badges.push("中心0");
        hints.push("center=(0,0)，可能未走框选录制".into());
        level = HealthLevel::Bad;
    }
    if let Some((iw, ih)) = primary_image_dims {
        if bw <= iw as i32 || bh <= ih as i32 {
            badges.push("ROI小");
            hints.push(format!("ROI {}x{} 未大于图片 {}x{}", bw, bh, iw, ih));
            level = level.worsen(HealthLevel::Warn);
        }
    }
    if dup_count > 1 {
        badges.push("重名");
        hints.push(format!("名称「{}」重复 {} 次", name, dup_count));
        level = level.worsen(HealthLevel::Warn);
    }
    if screen_w > 0
        && screen_h > 0
        && cur_sw > 0
        && cur_sh > 0
        && (screen_w != cur_sw || screen_h != cur_sh)
    {
        badges.push("分辨率");
        hints.push(format!(
            "录制 {}x{} ≠ 当前 {}x{}",
            screen_w, screen_h, cur_sw, cur_sh
        ));
        level = level.worsen(HealthLevel::Warn);
    }

    let mut seen = HashSet::new();
    badges.retain(|b| seen.insert(*b));

    HealthReport {
        level,
        badges,
        hints,
    }
}

pub fn scale_i(v: i32, from: i32, to: i32) -> i32 {
    if from <= 0 {
        return v;
    }
    ((v as i64) * (to as i64) / (from as i64)) as i32
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn scale_i_handles_zero_and_negative_from() {
        assert_eq!(scale_i(100, 1920, 2560), 133);
        assert_eq!(scale_i(100, 0, 2560), 100);
        assert_eq!(scale_i(100, -1, 2560), 100);
    }

    #[test]
    fn element_group_splits_on_first_slash() {
        assert_eq!(element_group("scene/foo"), "scene");
        assert_eq!(element_group("scene/sub/foo"), "scene");
        assert_eq!(element_group("plain"), "(未分组)");
        assert_eq!(element_group("/leading"), "(未分组)");
    }

    #[test]
    fn name_dup_counts_counts_occurrences() {
        let m = name_dup_counts(["a", "b", "a"].into_iter());
        assert_eq!(m.get("a"), Some(&2));
        assert_eq!(m.get("b"), Some(&1));
    }

    #[test]
    fn assess_flags_missing_image_and_zero_center() {
        let p = Path::new("definitely-not-a-file-xyz.png");
        let states = [(true, "-n", p)];
        let r = assess_element(
            "btn", 0, 0, 80, 40, 1920, 1080, 1920, 1080, &states, 1, None,
        );
        assert_eq!(r.level, HealthLevel::Bad);
        assert!(r.badges.contains(&"缺图"));
        assert!(r.badges.contains(&"中心0"));
    }

    #[test]
    fn assess_uses_cached_dims_for_roi_check() {
        let p = Path::new("definitely-not-a-file-xyz.png");
        let states = [(true, "-n", p)];
        let r = assess_element(
            "btn", 100, 100, 100, 50, 1920, 1080, 1920, 1080, &states, 1,
            Some((200, 100)),
        );
        assert!(r.badges.contains(&"ROI小"));
        // Missing image is still detected by the filesystem check.
        assert!(r.badges.contains(&"缺图"));
    }

    #[test]
    fn assess_flags_duplicates_and_resolution_mismatch() {
        let p = Path::new("definitely-not-a-file-xyz.png");
        let states = [(true, "-n", p)];
        let r = assess_element(
            "btn", 100, 100, 200, 100, 1920, 1080, 2560, 1440, &states, 2, None,
        );
        assert!(r.badges.contains(&"重名"));
        assert!(r.badges.contains(&"分辨率"));
    }

    fn patterned(w: u32, h: u32) -> ImageBuffer<Rgba<u8>, Vec<u8>> {
        ImageBuffer::from_fn(w, h, |x, y| {
            Rgba([
                ((x * 37 + y * 11) % 256) as u8,
                ((x * 13 + y * 53) % 256) as u8,
                ((x * 71 + y * 7) % 256) as u8,
                255,
            ])
        })
    }

    #[test]
    fn score_at_is_exact_for_identical_region() {
        let template = patterned(16, 16);
        let mut screen = patterned(40, 40);
        for ty in 0..16 {
            for tx in 0..16 {
                screen.put_pixel(10 + tx, 12 + ty, *template.get_pixel(tx, ty));
            }
        }
        assert_eq!(score_at(&screen, &template, 10, 12), 1.0);
        assert!(score_at(&screen, &template, 11, 12) < 1.0);
        // Out-of-bounds is a miss, not a panic.
        assert_eq!(score_at(&screen, &template, 100, 100), 0.0);
    }

    #[test]
    fn early_abort_only_skips_candidates_that_cannot_win() {
        let template = patterned(12, 12);
        let mut screen = patterned(40, 40);
        for ty in 0..12 {
            for tx in 0..12 {
                screen.put_pixel(9 + tx, 7 + ty, *template.get_pixel(tx, ty));
            }
        }
        // Exact match cannot beat a 1.0 best, so it is skipped; it still beats
        // a 0.99 best and must be returned exactly.
        assert_eq!(score_at_above(&screen, &template, 9, 7, 1.0), None);
        assert_eq!(score_at_above(&screen, &template, 9, 7, 0.99), Some(1.0));
        // A bad candidate is rejected early.
        assert_eq!(score_at_above(&screen, &template, 0, 0, 0.99), None);
    }

    #[test]
    fn find_template_matches_brute_force_best() {
        let template = patterned(8, 8);
        let mut screen = patterned(40, 40);
        for ty in 0..8 {
            for tx in 0..8 {
                screen.put_pixel(21 + tx, 13 + ty, *template.get_pixel(tx, ty));
            }
        }
        let roi = Some((0, 0, 40, 40));
        let hit = find_template(&screen, &template, roi).expect("hit");

        let mut best = (0u32, 0u32, f32::NEG_INFINITY);
        for y in 0..=(40 - 8) {
            for x in 0..=(40 - 8) {
                let s = score_at(&screen, &template, x, y);
                if s > best.2 {
                    best = (x, y, s);
                }
            }
        }
        assert_eq!((hit.hit_x as u32, hit.hit_y as u32), (best.0, best.1));
        assert_eq!(hit.score, best.2);
    }
}
