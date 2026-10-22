//! Drawing notification boxes into pixmaps.

use crate::config::{Config, IconPosition, Justify, NameOverflow, parse_hex_color};
use crate::font::{FontSystem, TextRenderer, luminance};
use crate::itemdb::Icon;
use std::collections::HashMap;
use std::sync::Arc;
use tiny_skia::{BlendMode, Color, FillRule, Mask, Paint, PathBuilder, Pixmap, PixmapPaint, Transform};

/// Colours for a highlighted row.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RowColors {
    pub background: [u8; 3],
    pub text: [u8; 3],
}

/// How much a hidden-but-shown item is faded.
pub const DIMMED_ALPHA: f32 = 0.4;

/// One item to show.
#[derive(Debug, Clone)]
pub struct Entry {
    pub name: String,
    pub quantity: u32,
    pub icon: Option<Arc<Icon>>,
    /// Special colours from a highlight rule.
    pub highlight: Option<RowColors>,
    /// Hidden by a rule but shown anyway ("show hidden items").
    pub dimmed: bool,
}

impl Entry {
    pub fn new(name: &str, quantity: u32, icon: Option<Arc<Icon>>) -> Entry {
        Entry { name: name.to_string(), quantity, icon, highlight: None, dimmed: false }
    }

    /// Apply the hide/highlight rules from `cfg`. `None` means the item is hidden.
    pub fn classified(cfg: &Config, name: &str, quantity: u32, icon: Option<Arc<Icon>>) -> Option<Entry> {
        let hidden = cfg.is_hidden(name);
        if hidden && !cfg.show_hidden {
            return None;
        }
        let highlight = cfg.highlight_for(name).map(|h| RowColors {
            background: parse_hex_color(&h.background_color).unwrap_or([0xff, 0xd2, 0x3f]),
            text: parse_hex_color(&h.text_color).unwrap_or([0x1c, 0x21, 0x2e]),
        });
        Some(Entry { name: name.to_string(), quantity, icon, highlight, dimmed: hidden })
    }

    fn suffix(&self, show_quantity: bool) -> String {
        if show_quantity && self.quantity > 1 { format!(" x{}", self.quantity) } else { String::new() }
    }
}

/// Pixel measurements derived from the config and scale.
#[derive(Debug, Clone)]
pub struct Style {
    pub scale: f32,
    pub icon_size: u32,
    pub show_icons: bool,
    pub icon_pos: IconPosition,
    pub justify: Justify,
    pub overflow: NameOverflow,
    pub max_text_w: f32,
    pub pad: f32,
    pub text_pad_extra: f32,
    pub icon_gap: f32,
    pub item_spacing: f32,
    pub radius: f32,
    pub bg: [u8; 3],
    pub bg_alpha: f32,
    pub text_color: [u8; 3],
    pub shadow: bool,
    pub show_quantity: bool,
    pub opacity: f32,
    pub margin_x: f32,
    pub margin_y: f32,
}

impl Style {
    pub fn from_config(c: &Config) -> Style {
        let s = c.scale();
        Style {
            scale: s,
            icon_size: (32.0 * s).round().max(4.0) as u32,
            show_icons: c.show_icons,
            icon_pos: c.icon_position,
            justify: c.justify,
            overflow: c.name_overflow,
            max_text_w: (c.max_text_width as f32 * s).round(),
            pad: (c.box_padding as f32 * s).round(),
            text_pad_extra: (4.0 * s).round(),
            icon_gap: (6.0 * s).round(),
            item_spacing: (c.item_spacing() as f32 * s).round(),
            radius: c.corner_radius as f32 * s,
            bg: parse_hex_color(&c.background_color).unwrap_or([0x1c, 0x21, 0x2e]),
            bg_alpha: c.background_opacity_percent as f32 / 100.0,
            text_color: parse_hex_color(&c.text_color).unwrap_or([255, 255, 255]),
            shadow: c.text_shadow,
            show_quantity: c.show_quantity,
            opacity: c.opacity_percent as f32 / 100.0,
            margin_x: (c.margin_x as f32 * s).round(),
            margin_y: (c.margin_y as f32 * s).round(),
        }
    }

    pub fn pad_left(&self) -> f32 {
        if self.show_icons && self.icon_pos == IconPosition::LeftOfText {
            self.pad
        } else {
            self.pad + self.text_pad_extra
        }
    }

    pub fn pad_right(&self) -> f32 {
        if self.show_icons && self.icon_pos == IconPosition::RightOfText {
            self.pad
        } else {
            self.pad + self.text_pad_extra
        }
    }
}

/// Measured text for one row.
#[derive(Debug, Clone)]
pub struct RowLayout {
    pub lines: Vec<String>,
    pub line_widths: Vec<f32>,
    pub text_w: f32,
    pub text_h: f32,
    /// Icon + gap + text.
    pub content_w: f32,
    pub height: f32,
}

/// What to draw for one row of a box.
pub struct RowDraw<'a> {
    pub entry: &'a Entry,
    pub layout: &'a RowLayout,
    /// 0 = hidden, 1 = fully shown (drives slide/fade of the row inside its box).
    pub vis: f32,
    /// 0 = collapsed, 1 = full height.
    pub h: f32,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum RowMotion {
    /// Slide horizontally toward +1 (right) or -1 (left) as `vis` drops.
    Slide(f32),
    Fade,
    None,
}

pub struct Painter {
    pub style: Style,
    pub text: TextRenderer,
    pub font_name: String,
    icons: HashMap<(u32, u32), Pixmap>,
}

impl Painter {
    pub fn new(cfg: &Config, fonts: &FontSystem) -> Option<Painter> {
        let style = Style::from_config(cfg);
        let px = (cfg.font_size * cfg.scale()).max(4.0);
        let (font, font_name) = fonts.load(&cfg.font_family, cfg.font_bold, cfg.font_italic)?;
        let text = TextRenderer::new(font, px);
        Some(Painter { style, text, font_name, icons: HashMap::new() })
    }

    pub fn layout_row(&mut self, e: &Entry) -> RowLayout {
        let st = &self.style;
        let suffix = e.suffix(st.show_quantity);
        let lines = match st.overflow {
            NameOverflow::FullWidth => vec![format!("{}{suffix}", e.name)],
            NameOverflow::Truncate => vec![self.text.truncate(&e.name, &suffix, st.max_text_w)],
            NameOverflow::Wrap => self.text.wrap(&format!("{}{suffix}", e.name), st.max_text_w),
        };
        let line_widths: Vec<f32> = lines.iter().map(|l| self.text.measure(l)).collect();
        let st = &self.style;
        let text_w = line_widths.iter().cloned().fold(0.0, f32::max);
        let text_h = self.text.line_height * lines.len() as f32;
        let (content_w, height) = if st.show_icons {
            (st.icon_size as f32 + st.icon_gap + text_w, text_h.max(st.icon_size as f32))
        } else {
            (text_w, text_h)
        };
        RowLayout { lines, line_widths, text_w, text_h, content_w, height }
    }

    /// Box width needed for rows with these layouts.
    pub fn box_width(&self, max_content_w: f32) -> f32 {
        (max_content_w + self.style.pad_left() + self.style.pad_right()).ceil()
    }

    /// Height of a box with these rows (taking row collapse into account).
    pub fn box_height(&self, rows: &[RowDraw]) -> f32 {
        let items: Vec<(f32, f32)> = rows.iter().map(|r| (r.layout.height, r.h)).collect();
        let (_, total) = stack_positions(&items, self.style.item_spacing);
        (total + 2.0 * self.style.pad).round()
    }

    fn icon_pixmap(&mut self, icon: &Icon) -> Option<&Pixmap> {
        let size = self.style.icon_size;
        let key = (icon.id, size);
        if let std::collections::hash_map::Entry::Vacant(e) = self.icons.entry(key) {
            let img = image::RgbaImage::from_raw(icon.width, icon.height, icon.rgba.clone())?;
            let img = if img.width() == size && img.height() == size {
                img
            } else {
                let filter = if size > img.width() {
                    image::imageops::FilterType::CatmullRom
                } else {
                    image::imageops::FilterType::Lanczos3
                };
                image::imageops::resize(&img, size, size, filter)
            };
            let mut data = img.into_raw();
            for px in data.chunks_exact_mut(4) {
                let a = px[3] as u32;
                for c in &mut px[..3] {
                    *c = (*c as u32 * a / 255) as u8;
                }
            }
            let pm = Pixmap::from_vec(data, tiny_skia::IntSize::from_wh(size, size)?)?;
            e.insert(pm);
        }
        self.icons.get(&key)
    }

    fn draw_placeholder_icon(&mut self, pm: &mut Pixmap, x: f32, y: f32, alpha: f32, tc: [u8; 3]) {
        let size = self.style.icon_size as f32;
        let r = (size * 0.15).max(2.0);
        if let Some(path) = rounded_rect(x, y, size, size, r) {
            let mut paint = Paint::default();
            paint.set_color(Color::from_rgba8(tc[0], tc[1], tc[2], (28.0 * alpha) as u8));
            paint.anti_alias = true;
            pm.fill_path(&path, &paint, FillRule::Winding, Transform::identity(), None);
        }
        let qw = self.text.measure("?");
        let baseline = y + (size - self.text.line_height) / 2.0 + self.text.ascent;
        self.text.draw(pm, x + (size - qw) / 2.0, baseline, "?", tc, 0.55 * alpha);
    }

    /// Draw a whole box (background + rows) at full opacity into a new pixmap.
    pub fn draw_box(
        &mut self,
        width: f32,
        rows: &[RowDraw],
        motion: RowMotion,
        newest_on_top: bool,
    ) -> Option<Pixmap> {
        let height = self.box_height(rows);
        let w = width.ceil().max(1.0) as u32;
        let h = height.ceil().max(1.0) as u32;
        let mut pm = Pixmap::new(w, h)?;
        let st = self.style.clone();

        // Background
        if st.bg_alpha > 0.0
            && let Some(path) = rounded_rect(0.0, 0.0, w as f32, h as f32, st.radius)
        {
            let mut paint = Paint::default();
            paint.set_color(Color::from_rgba8(
                st.bg[0],
                st.bg[1],
                st.bg[2],
                (st.bg_alpha * 255.0).round() as u8,
            ));
            paint.anti_alias = true;
            pm.fill_path(&path, &paint, FillRule::Winding, Transform::identity(), None);
        }

        // Rows are given oldest → newest. Lay them out top → bottom.
        let order: Vec<usize> =
            if newest_on_top { (0..rows.len()).rev().collect() } else { (0..rows.len()).collect() };
        let items: Vec<(f32, f32)> = order.iter().map(|&i| (rows[i].layout.height, rows[i].h)).collect();
        let (tops, _) = stack_positions(&items, st.item_spacing);
        let inner_l = st.pad_left();
        let inner_r = w as f32 - st.pad_right();
        let motion_of = |r: &RowDraw| match motion {
            RowMotion::Slide(dir) => ((1.0 - ease(r.vis)) * w as f32 * dir, 1.0),
            RowMotion::Fade => (0.0, ease(r.vis)),
            RowMotion::None => (0.0, 1.0),
        };

        // Highlighted rows: their part of the box takes the highlight colour. Each band reaches
        // halfway into the gaps around the row, and to the box edge for the first/last row.
        if st.bg_alpha > 0.0 && rows.iter().any(|r| r.entry.highlight.is_some()) {
            let mut clip = Mask::new(w, h)?;
            if let Some(path) = rounded_rect(0.0, 0.0, w as f32, h as f32, st.radius) {
                clip.fill_path(&path, FillRule::Winding, true, Transform::identity());
            }
            let last = order.len().saturating_sub(1);
            for (n, &i) in order.iter().enumerate() {
                let r = &rows[i];
                let Some(hl) = r.entry.highlight else { continue };
                if r.vis <= 0.0 || r.h <= 0.0 {
                    continue;
                }
                let top = st.pad + tops[n];
                let bottom = top + r.layout.height;
                let band_top = if n == 0 {
                    0.0
                } else {
                    (st.pad + tops[n - 1] + rows[order[n - 1]].layout.height + top) / 2.0
                };
                let band_bottom = if n == last { h as f32 } else { (bottom + st.pad + tops[n + 1]) / 2.0 };
                let (dx, alpha) = motion_of(r);
                // Fade from the normal background to the highlight colour (keeps background opacity).
                let mix = |a: u8, b: u8| (a as f32 + (b as f32 - a as f32) * alpha).round() as u8;
                let c = [
                    mix(st.bg[0], hl.background[0]),
                    mix(st.bg[1], hl.background[1]),
                    mix(st.bg[2], hl.background[2]),
                ];
                let mut paint = Paint::default();
                paint.set_color(Color::from_rgba8(c[0], c[1], c[2], (st.bg_alpha * 255.0).round() as u8));
                paint.blend_mode = BlendMode::Source;
                if let Some(rect) = tiny_skia::Rect::from_ltrb(
                    dx,
                    band_top.max(0.0),
                    w as f32 + dx,
                    band_bottom.min(h as f32),
                ) {
                    pm.fill_rect(rect, &paint, Transform::identity(), Some(&clip));
                }
            }
        }

        for (n, &i) in order.iter().enumerate() {
            let r = &rows[i];
            let y0 = (st.pad + tops[n]).round();
            if r.vis <= 0.0 || r.h <= 0.0 {
                continue;
            }
            let (dx, alpha) = motion_of(r);
            self.draw_row(&mut pm, inner_l + dx, inner_r + dx, y0, r, alpha);
        }
        Some(pm)
    }

    fn draw_row(&mut self, pm: &mut Pixmap, inner_l: f32, inner_r: f32, y0: f32, r: &RowDraw, alpha: f32) {
        let st = self.style.clone();
        let lay = r.layout;
        let text_color = r.entry.highlight.map(|h| h.text).unwrap_or(st.text_color);
        let alpha = if r.entry.dimmed { alpha * DIMMED_ALPHA } else { alpha };
        // A dark shadow only helps light text.
        let shadow = st.shadow && luminance(text_color) > 0.5;
        let inner_w = inner_r - inner_l;
        let bx = match st.justify {
            Justify::Left => inner_l,
            Justify::Center => inner_l + ((inner_w - lay.content_w) / 2.0).max(0.0),
            Justify::Right => inner_r - lay.content_w,
        }
        .round();
        let icon_size = st.icon_size as f32;
        let (icon_x, text_x) = match (st.show_icons, st.icon_pos) {
            (false, _) => (0.0, bx),
            (true, IconPosition::LeftOfText) => (bx, bx + icon_size + st.icon_gap),
            (true, IconPosition::RightOfText) => (bx + lay.text_w + st.icon_gap, bx),
        };
        if st.show_icons {
            let iy = (y0 + (lay.height - icon_size) / 2.0).round();
            let drawn = match &r.entry.icon {
                Some(icon) => {
                    let icon = icon.clone();
                    if let Some(ipm) = self.icon_pixmap(&icon) {
                        let paint = PixmapPaint { opacity: alpha, ..Default::default() };
                        pm.draw_pixmap(
                            icon_x.round() as i32,
                            iy as i32,
                            ipm.as_ref(),
                            &paint,
                            Transform::identity(),
                            None,
                        );
                        true
                    } else {
                        false
                    }
                }
                None => false,
            };
            if !drawn {
                self.draw_placeholder_icon(pm, icon_x.round(), iy, alpha, text_color);
            }
        }
        let ty = (y0 + (lay.height - lay.text_h) / 2.0).round();
        let shadow_off = (st.scale).round().max(1.0);
        for (k, line) in lay.lines.iter().enumerate() {
            let lw = lay.line_widths[k];
            let lx = match st.justify {
                Justify::Left => text_x,
                Justify::Center => text_x + (lay.text_w - lw) / 2.0,
                Justify::Right => text_x + lay.text_w - lw,
            }
            .round();
            let baseline = ty + self.text.ascent + k as f32 * self.text.line_height;
            if shadow {
                self.text.draw(pm, lx + shadow_off, baseline + shadow_off, line, [0, 0, 0], 0.75 * alpha);
            }
            self.text.draw(pm, lx, baseline, line, text_color, alpha);
        }
    }
}

/// Lay out items top → bottom. Each item is `(full_height, h)` where `h` (0..1) is how much
/// of its slot is open. The gap between neighbours opens and closes with the smaller of the
/// two, so items appear and disappear smoothly. Returns each item's top `y` (anchored to the
/// bottom of its slot) and the total height.
pub fn stack_positions(items: &[(f32, f32)], spacing: f32) -> (Vec<f32>, f32) {
    let mut tops = Vec::with_capacity(items.len());
    let mut cursor = 0.0;
    for (n, &(height, h)) in items.iter().enumerate() {
        if n > 0 {
            cursor += spacing * h.min(items[n - 1].1);
        }
        let slot = height * h;
        tops.push(cursor + slot - height);
        cursor += slot;
    }
    (tops, cursor)
}

/// Ease-out cubic.
pub fn ease(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    1.0 - (1.0 - t).powi(3)
}

pub fn rounded_rect(x: f32, y: f32, w: f32, h: f32, r: f32) -> Option<tiny_skia::Path> {
    let r = r.min(w / 2.0).min(h / 2.0).max(0.0);
    let mut pb = PathBuilder::new();
    if r < 0.5 {
        pb.push_rect(tiny_skia::Rect::from_xywh(x, y, w, h)?);
        return pb.finish();
    }
    let k = 0.552_284_8 * r; // circle approximation
    pb.move_to(x + r, y);
    pb.line_to(x + w - r, y);
    pb.cubic_to(x + w - r + k, y, x + w, y + r - k, x + w, y + r);
    pb.line_to(x + w, y + h - r);
    pb.cubic_to(x + w, y + h - r + k, x + w - r + k, y + h, x + w - r, y + h);
    pb.line_to(x + r, y + h);
    pb.cubic_to(x + r - k, y + h, x, y + h - r + k, x, y + h - r);
    pb.line_to(x, y + r);
    pb.cubic_to(x, y + r - k, x + r - k, y, x + r, y);
    pb.close();
    pb.finish()
}
