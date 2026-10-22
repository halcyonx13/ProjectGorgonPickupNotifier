//! System font lookup (fontdb) and anti-aliased text drawing (fontdue) into tiny-skia pixmaps.

use std::collections::HashMap;
use tiny_skia::Pixmap;

/// All installed fonts. Scanning takes a moment, so build this once and keep it.
pub struct FontSystem {
    db: fontdb::Database,
}

const FALLBACK_FAMILIES: &[&str] =
    &["Segoe UI", "Arial", "Tahoma", "Verdana", "DejaVu Sans", "Liberation Sans", "Noto Sans"];

impl FontSystem {
    pub fn new() -> FontSystem {
        let mut db = fontdb::Database::new();
        db.load_system_fonts();
        // Optional extra font directory (handy for testing without system fonts).
        if let Some(dir) = std::env::var_os("PGPN_FONT_DIR") {
            db.load_fonts_dir(dir);
        }
        FontSystem { db }
    }

    /// Sorted, de-duplicated family names (English names where available).
    pub fn families(&self) -> Vec<String> {
        let mut names: Vec<String> =
            self.db.faces().filter_map(|f| f.families.first().map(|(n, _)| n.clone())).collect();
        names.sort_by_key(|a| a.to_lowercase());
        names.dedup_by(|a, b| a.eq_ignore_ascii_case(b));
        names
    }

    fn canonical_family(&self, wanted: &str) -> Option<String> {
        let wanted = wanted.trim();
        self.db.faces().find_map(|f| {
            f.families.iter().find(|(n, _)| n.eq_ignore_ascii_case(wanted)).map(|(n, _)| n.clone())
        })
    }

    /// Load the best match for a family/weight/style, falling back to common UI fonts.
    pub fn load(&self, family: &str, bold: bool, italic: bool) -> Option<(fontdue::Font, String)> {
        let mut candidates: Vec<String> = Vec::new();
        if let Some(c) = self.canonical_family(family) {
            candidates.push(c);
        }
        for f in FALLBACK_FAMILIES {
            if let Some(c) = self.canonical_family(f) {
                candidates.push(c);
            }
        }
        let weight = if bold { fontdb::Weight::BOLD } else { fontdb::Weight::NORMAL };
        let style = if italic { fontdb::Style::Italic } else { fontdb::Style::Normal };
        for name in &candidates {
            let families = [fontdb::Family::Name(name)];
            let q = fontdb::Query { families: &families, weight, stretch: fontdb::Stretch::Normal, style };
            if let Some(id) = self.db.query(&q)
                && let Some(font) = self.font_from_id(id)
            {
                return Some((font, name.clone()));
            }
        }
        // Anything at all.
        let id = self.db.faces().next()?.id;
        let name = self.db.face(id)?.families.first()?.0.clone();
        Some((self.font_from_id(id)?, name))
    }

    fn font_from_id(&self, id: fontdb::ID) -> Option<fontdue::Font> {
        self.db
            .with_face_data(id, |data, index| {
                fontdue::Font::from_bytes(
                    data,
                    fontdue::FontSettings { collection_index: index, scale: 40.0, ..Default::default() },
                )
                .ok()
            })
            .flatten()
    }
}

impl Default for FontSystem {
    fn default() -> Self {
        Self::new()
    }
}

struct Glyph {
    metrics: fontdue::Metrics,
    coverage: Vec<u8>,
}

/// A font at one pixel size, with a glyph cache.
pub struct TextRenderer {
    font: fontdue::Font,
    pub px: f32,
    pub ascent: f32,
    pub descent: f32,
    pub line_height: f32,
    glyphs: HashMap<char, Glyph>,
    /// Coverage boost for light text (used when the text colour is light).
    lut_light: [u8; 256],
}

/// Perceived brightness 0..1.
pub fn luminance(c: [u8; 3]) -> f32 {
    (0.2126 * c[0] as f32 + 0.7152 * c[1] as f32 + 0.0722 * c[2] as f32) / 255.0
}

impl TextRenderer {
    pub fn new(font: fontdue::Font, px: f32) -> TextRenderer {
        let lm = font.horizontal_line_metrics(px).unwrap_or(fontdue::LineMetrics {
            ascent: px * 0.9,
            descent: -px * 0.25,
            line_gap: 0.0,
            new_line_size: px * 1.15,
        });
        // Light text on a dark background looks thin with linear coverage; boost it a little
        // (similar to the contrast tweak DirectWrite applies to grayscale text).
        let mut lut_light = [0u8; 256];
        for (i, v) in lut_light.iter_mut().enumerate() {
            *v = ((i as f32 / 255.0).powf(1.0 / 1.35) * 255.0).round() as u8;
        }
        TextRenderer {
            font,
            px,
            ascent: lm.ascent.ceil(),
            descent: lm.descent.floor(),
            line_height: (lm.ascent.ceil() - lm.descent.floor() + lm.line_gap).round(),
            glyphs: HashMap::new(),
            lut_light,
        }
    }

    fn glyph(&mut self, c: char) -> &Glyph {
        let font = &self.font;
        let px = self.px;
        self.glyphs.entry(c).or_insert_with(|| {
            let (metrics, coverage) = font.rasterize(c, px);
            Glyph { metrics, coverage }
        })
    }

    fn kern(&self, prev: Option<char>, c: char) -> f32 {
        prev.and_then(|p| self.font.horizontal_kern(p, c, self.px)).unwrap_or(0.0)
    }

    /// Width of a single line of text in pixels.
    pub fn measure(&mut self, text: &str) -> f32 {
        let mut x = 0.0;
        let mut prev = None;
        for c in text.chars() {
            x += self.kern(prev, c);
            x += self.glyph(c).metrics.advance_width;
            prev = Some(c);
        }
        x.ceil()
    }

    /// Draw one line with its baseline at `baseline`. `color` is straight RGB, `alpha` 0..1.
    pub fn draw(&mut self, pm: &mut Pixmap, x: f32, baseline: f32, text: &str, color: [u8; 3], alpha: f32) {
        if alpha <= 0.0 {
            return;
        }
        let a = (alpha.clamp(0.0, 1.0) * 255.0).round() as u32;
        let (pw, ph) = (pm.width() as i32, pm.height() as i32);
        let mut pen = x;
        let mut prev = None;
        let baseline = baseline.round() as i32;
        let lut: [u8; 256] = if luminance(color) > 0.5 {
            self.lut_light
        } else {
            std::array::from_fn(|i| i as u8) // dark text: linear coverage
        };
        for c in text.chars() {
            pen += self.kern(prev, c);
            prev = Some(c);
            let g = self.glyph(c);
            let gx = pen.round() as i32 + g.metrics.xmin;
            let gy = baseline - g.metrics.ymin - g.metrics.height as i32;
            let (gw, gh) = (g.metrics.width as i32, g.metrics.height as i32);
            let advance = g.metrics.advance_width;
            let data = pm.data_mut();
            for row in 0..gh {
                let py = gy + row;
                if py < 0 || py >= ph {
                    continue;
                }
                for col in 0..gw {
                    let px = gx + col;
                    if px < 0 || px >= pw {
                        continue;
                    }
                    let cov = lut[g.coverage[(row * gw + col) as usize] as usize] as u32;
                    if cov == 0 {
                        continue;
                    }
                    let sa = cov * a / 255; // 0..255
                    let i = ((py * pw + px) * 4) as usize;
                    let inv = 255 - sa;
                    for ch in 0..3 {
                        let s = color[ch] as u32 * sa / 255;
                        data[i + ch] = (s + data[i + ch] as u32 * inv / 255).min(255) as u8;
                    }
                    data[i + 3] = (sa + data[i + 3] as u32 * inv / 255).min(255) as u8;
                }
            }
            pen += advance;
        }
    }

    /// Word-wrap `text` to `max_w`. Words that are too long on their own are broken.
    pub fn wrap(&mut self, text: &str, max_w: f32) -> Vec<String> {
        let mut lines: Vec<String> = Vec::new();
        let mut cur = String::new();
        for word in text.split(' ').filter(|w| !w.is_empty()) {
            let candidate = if cur.is_empty() { word.to_string() } else { format!("{cur} {word}") };
            if self.measure(&candidate) <= max_w {
                cur = candidate;
                continue;
            }
            if !cur.is_empty() {
                lines.push(std::mem::take(&mut cur));
            }
            if self.measure(word) <= max_w {
                cur = word.to_string();
            } else {
                // Break a long word across lines.
                for c in word.chars() {
                    let mut next = cur.clone();
                    next.push(c);
                    if !cur.is_empty() && self.measure(&next) > max_w {
                        lines.push(std::mem::take(&mut cur));
                        cur.push(c);
                    } else {
                        cur = next;
                    }
                }
            }
        }
        if !cur.is_empty() || lines.is_empty() {
            lines.push(cur);
        }
        lines
    }

    /// Fit `name` + `suffix` into `max_w`, shortening the name with an ellipsis if needed.
    pub fn truncate(&mut self, name: &str, suffix: &str, max_w: f32) -> String {
        let full = format!("{name}{suffix}");
        if self.measure(&full) <= max_w {
            return full;
        }
        let chars: Vec<char> = name.chars().collect();
        let mut n = chars.len();
        while n > 0 {
            n -= 1;
            let head: String = chars[..n].iter().collect();
            let s = format!("{}…{suffix}", head.trim_end());
            if self.measure(&s) <= max_w {
                return s;
            }
        }
        format!("…{suffix}")
    }
}
