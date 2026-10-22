//! The app's icon, drawn in code so it is crisp at every size.

use crate::render::rounded_rect;
use tiny_skia::{Color, FillRule, Paint, PathBuilder, Pixmap, Stroke, Transform};

/// Straight (non-premultiplied) RGBA pixels of the app icon at `size`×`size`.
pub fn rgba(size: u32) -> Vec<u8> {
    let s = size as f32;
    let mut pm = Pixmap::new(size, size).unwrap();
    let mut paint = Paint { anti_alias: true, ..Default::default() };

    // Dark rounded tile.
    paint.set_color(Color::from_rgba8(0x1c, 0x21, 0x2e, 255));
    if let Some(p) = rounded_rect(0.0, 0.0, s, s, s * 0.22) {
        pm.fill_path(&p, &paint, FillRule::Winding, Transform::identity(), None);
    }

    // A gold coin, lower left.
    let (cx, cy, r) = (s * 0.42, s * 0.58, s * 0.27);
    paint.set_color(Color::from_rgba8(0xE8, 0xB6, 0x4A, 255));
    if let Some(p) = PathBuilder::from_circle(cx, cy, r) {
        pm.fill_path(&p, &paint, FillRule::Winding, Transform::identity(), None);
    }
    if size >= 24 {
        paint.set_color(Color::from_rgba8(0xB9, 0x84, 0x22, 255));
        if let Some(p) = PathBuilder::from_circle(cx, cy, r * 0.62) {
            let stroke = Stroke { width: (s * 0.05).max(1.0), ..Default::default() };
            pm.stroke_path(&p, &paint, &stroke, Transform::identity(), None);
        }
    }

    // A white "+" badge, upper right.
    let (bx, by, arm, w) = (s * 0.73, s * 0.27, s * 0.15, (s * 0.09).max(1.5));
    paint.set_color(Color::from_rgba8(255, 255, 255, 255));
    let mut pb = PathBuilder::new();
    pb.move_to(bx - arm, by);
    pb.line_to(bx + arm, by);
    pb.move_to(bx, by - arm);
    pb.line_to(bx, by + arm);
    if let Some(p) = pb.finish() {
        let stroke = Stroke { width: w, line_cap: tiny_skia::LineCap::Round, ..Default::default() };
        pm.stroke_path(&p, &paint, &stroke, Transform::identity(), None);
    }

    // Demultiply.
    let mut out = pm.take();
    for px in out.chunks_exact_mut(4) {
        let a = px[3] as u32;
        if a > 0 && a < 255 {
            for c in &mut px[..3] {
                *c = ((*c as u32 * 255 + a / 2) / a).min(255) as u8;
            }
        }
    }
    out
}

/// A Windows .ico file containing PNG images at the given sizes.
pub fn ico_file(sizes: &[u32]) -> Vec<u8> {
    let pngs: Vec<Vec<u8>> = sizes
        .iter()
        .map(|&sz| {
            let mut buf = Vec::new();
            let img = image::RgbaImage::from_raw(sz, sz, rgba(sz)).unwrap();
            img.write_to(&mut std::io::Cursor::new(&mut buf), image::ImageFormat::Png).unwrap();
            buf
        })
        .collect();
    let mut out = Vec::new();
    out.extend_from_slice(&[0, 0, 1, 0]);
    out.extend_from_slice(&(sizes.len() as u16).to_le_bytes());
    let mut offset = 6 + 16 * sizes.len() as u32;
    for (sz, png) in sizes.iter().zip(&pngs) {
        let d = if *sz >= 256 { 0 } else { *sz as u8 };
        out.extend_from_slice(&[d, d, 0, 0]);
        out.extend_from_slice(&1u16.to_le_bytes()); // planes
        out.extend_from_slice(&32u16.to_le_bytes()); // bpp
        out.extend_from_slice(&(png.len() as u32).to_le_bytes());
        out.extend_from_slice(&offset.to_le_bytes());
        offset += png.len() as u32;
    }
    for png in pngs {
        out.extend_from_slice(&png);
    }
    out
}
