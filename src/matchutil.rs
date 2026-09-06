//! Template match + element health helpers (score aligned with mouse-suite SAD).

use image::{ImageBuffer, Rgba};
use std::collections::{HashMap, HashSet};
use std::path::Path;

/// Same score as mouse-suite clicker: 1 - mean(|ΔRGB|) / (255*3).
pub fn score_at(
    screen: &ImageBuffer<Rgba<u8>, Vec<u8>>,
    template: &ImageBuffer<Rgba<u8>, Vec<u8>>,
    ox: u32,
    oy: u32,
) -> f32 {
    let (tw, th) = template.dimensions();
    let mut sum = 0i64;
    let mut n = 0u32;
    for ty in 0..th {
        for tx in 0..tw {
            let sp = screen.get_pixel(ox + tx, oy + ty);
            let tp = template.get_pixel(tx, ty);
            sum += (sp[0] as i32 - tp[0] as i32).abs() as i64
                + (sp[1] as i32 - tp[1] as i32).abs() as i64
                + (sp[2] as i32 - tp[2] as i32).abs() as i64;
            n += 1;
        }
    }
    if n == 0 {
        return 0.0;
    }
    1.0 - (sum as f32 / n as f32) / (255.0 * 3.0)
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
            let s = score_at(screen, template, x, y);
            if s > best_score {
                best_score = s;
                best = (x, y);
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
                let s = score_at(screen, template, x, y);
                if s > best_score {
                    best_score = s;
                    best = (x, y);
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
    if let Some((_, _, path)) = states.iter().find(|(p, _, _)| *p).or(states.first()) {
        if let Ok((iw, ih)) = image::image_dimensions(path) {
            if bw <= iw as i32 || bh <= ih as i32 {
                badges.push("ROI小");
                hints.push(format!("ROI {}x{} 未大于图片 {}x{}", bw, bh, iw, ih));
                level = level.worsen(HealthLevel::Warn);
            }
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
