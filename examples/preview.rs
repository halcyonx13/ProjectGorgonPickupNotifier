//! Offline renderer used during development: draws notification previews to PNG files.
//!
//! cargo run --example preview -- <out_dir> [icon_cache_dir]

use pg_pickup_notifier::config::*;
use pg_pickup_notifier::font::FontSystem;
use pg_pickup_notifier::itemdb::IconCache;
use pg_pickup_notifier::notify::{Notifier, preview};
use pg_pickup_notifier::render::{Entry, Painter};
use std::path::PathBuf;
use tiny_skia::{Color, Pixmap, PixmapPaint, Transform};

fn backdrop(w: u32, h: u32) -> Pixmap {
    let mut pm = Pixmap::new(w, h).unwrap();
    let data = pm.data_mut();
    for y in 0..h {
        for x in 0..w {
            let t = y as f32 / h as f32;
            let s = x as f32 / w as f32;
            let i = ((y * w + x) * 4) as usize;
            data[i] = (70.0 + 60.0 * t + 20.0 * s) as u8;
            data[i + 1] = (95.0 + 40.0 * t) as u8;
            data[i + 2] = (60.0 + 30.0 * (1.0 - t)) as u8;
            data[i + 3] = 255;
        }
    }
    pm
}

fn put(scene: &tiny_skia::Pixmap, canvas: &mut Pixmap, corner: Corner, margin_y: i32) {
    let x = match corner {
        Corner::BottomRight => canvas.width() as i32 - scene.width() as i32,
        Corner::BottomLeft => 0,
    };
    let y = canvas.height() as i32 - margin_y - scene.height() as i32;
    canvas.draw_pixmap(x, y, scene.as_ref(), &PixmapPaint::default(), Transform::identity(), None);
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let out = PathBuf::from(args.get(1).cloned().unwrap_or_else(|| "preview-out".into()));
    std::fs::create_dir_all(&out).unwrap();
    let icon_dir = args.get(2).map(PathBuf::from);
    let font_family = std::env::var("PGPN_FONT").unwrap_or_else(|_| "Segoe UI".into());

    let fonts = FontSystem::new();
    let mut cache = icon_dir.map(IconCache::new);
    let icons: std::collections::HashMap<u32, std::sync::Arc<pg_pickup_notifier::itemdb::Icon>> =
        pg_pickup_notifier::SAMPLE_ITEMS
            .iter()
            .filter_map(|(_, _, id)| Some((*id, cache.as_mut()?.get_local(*id)?)))
            .collect();
    let entries_for = |cfg: &Config| {
        let mut v = pg_pickup_notifier::sample_entries(cfg, |id| icons.get(&id).cloned());
        v.push(Entry::new("Shieldwright: Awesome Deathspike Shield", 1, None));
        v
    };
    let entries = entries_for(&Config::default());

    let base = Config { font_family: font_family.clone(), ..Config::default() };
    let variants: Vec<(&str, Config)> = vec![
        ("01-default-combine", base.clone()),
        ("02-stack", Config { mode: GroupMode::Stack, ..base.clone() }),
        (
            "03-stack-qty-left",
            Config {
                mode: GroupMode::Stack,
                show_quantity: true,
                corner: Corner::BottomLeft,
                justify: Justify::Left,
                ..base.clone()
            },
        ),
        ("04-no-icons", Config { show_icons: false, show_quantity: true, ..base.clone() }),
        (
            "05-icon-right-center",
            Config { icon_position: IconPosition::RightOfText, justify: Justify::Center, ..base.clone() },
        ),
        ("06-wrap", Config { name_overflow: NameOverflow::Wrap, max_text_width: 160, ..base.clone() }),
        (
            "07-truncate-qty",
            Config {
                name_overflow: NameOverflow::Truncate,
                max_text_width: 160,
                show_quantity: true,
                ..base.clone()
            },
        ),
        ("08-scale200", Config { scale_percent: 200, max_notifications: 3, ..base.clone() }),
        (
            "09-translucent-shadow",
            Config {
                background_opacity_percent: 45,
                text_shadow: true,
                opacity_percent: 90,
                corner_radius: 8,
                text_color: "#FFE9A8".into(),
                font_bold: true,
                ..base.clone()
            },
        ),
        (
            "10-stack-wrap-nooicons",
            Config {
                mode: GroupMode::Stack,
                show_icons: false,
                name_overflow: NameOverflow::Wrap,
                max_text_width: 150,
                ..base.clone()
            },
        ),
        ("11-show-hidden", Config { show_hidden: true, max_notifications: 7, ..base.clone() }),
        (
            "12-stack-highlight-left",
            Config { mode: GroupMode::Stack, corner: Corner::BottomLeft, ..base.clone() },
        ),
        (
            "13-highlight-translucent",
            Config { background_opacity_percent: 55, corner_radius: 8, max_notifications: 7, ..base.clone() },
        ),
        (
            "14-three-rules-no-icons",
            Config {
                show_icons: false,
                max_notifications: 7,
                highlights: vec![
                    HighlightRule { contains: "trophy".into(), ..HighlightRule::default() },
                    HighlightRule {
                        contains: "fish".into(),
                        background_color: "#3B82F6".into(),
                        text_color: "#FFFFFF".into(),
                    },
                    HighlightRule {
                        contains: "awesome".into(),
                        background_color: "#7C3AED".into(),
                        text_color: "#FFFFFF".into(),
                    },
                ],
                ..base.clone()
            },
        ),
        (
            "15-custom-rule-example",
            Config {
                highlights: vec![HighlightRule {
                    contains: "mushroom".into(),
                    background_color: "#E05252".into(),
                    text_color: "#FFFFFF".into(),
                }],
                ..base.clone()
            },
        ),
    ];
    for (name, cfg) in &variants {
        let mut painter = Painter::new(cfg, &fonts).expect("font");
        let scene = preview(cfg, &mut painter, &entries_for(cfg)).expect("scene");
        let mut canvas = backdrop(scene.pixmap.width() + 60, scene.pixmap.height() + 40);
        put(&scene.pixmap, &mut canvas, scene.corner, scene.margin_y);
        canvas.save_png(out.join(format!("{name}.png"))).unwrap();
        println!(
            "{name}: font={} scene={}x{}",
            painter.font_name,
            scene.pixmap.width(),
            scene.pixmap.height()
        );
    }

    // Animation filmstrips: frames every 50 ms.
    for (name, cfg) in [
        ("anim-combine-slide", Config { combine_expiry: CombineExpiry::EachRow, ..base.clone() }),
        ("anim-stack-slide", Config { mode: GroupMode::Stack, ..base.clone() }),
        (
            "anim-stack-fade",
            Config { mode: GroupMode::Stack, animation: AnimationKind::Fade, ..base.clone() },
        ),
        (
            "anim-combine-fade",
            Config { combine_expiry: CombineExpiry::EachRow, animation: AnimationKind::Fade, ..base.clone() },
        ),
    ] {
        let mut painter = Painter::new(&cfg, &fonts).unwrap();
        let mut n = Notifier::new();
        let mut frames: Vec<Pixmap> = Vec::new();
        let (fw, fh) = (380u32, 200u32);
        let mut t = 0.0f64;
        let pushes = [(0.0, 0usize), (0.15, 4), (0.6, 2), (0.62, 3)];
        let mut next = 0;
        while t <= 3.6 {
            while next < pushes.len() && pushes[next].0 <= t + 1e-9 {
                n.push(&cfg, t, entries[pushes[next].1].clone());
                next += 1;
            }
            n.tick(&cfg, &mut painter, t);
            let mut frame = backdrop(fw, fh);
            if let Some(scene) = n.compose(&cfg, &mut painter) {
                put(&scene.pixmap, &mut frame, scene.corner, scene.margin_y);
            }
            frames.push(frame);
            t += 0.05;
        }
        let cols = 8u32;
        let rows = (frames.len() as u32).div_ceil(cols);
        let mut sheet = Pixmap::new(cols * (fw + 4), rows * (fh + 4)).unwrap();
        sheet.fill(Color::from_rgba8(0, 0, 0, 255));
        for (i, f) in frames.iter().enumerate() {
            let (c, r) = (i as u32 % cols, i as u32 / cols);
            sheet.draw_pixmap(
                (c * (fw + 4)) as i32,
                (r * (fh + 4)) as i32,
                f.as_ref(),
                &PixmapPaint::default(),
                Transform::identity(),
                None,
            );
        }
        sheet.save_png(out.join(format!("{name}.png"))).unwrap();
        println!("{name}: {} frames", frames.len());
    }
}
