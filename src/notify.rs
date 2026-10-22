//! The notification model: stacking/combining, timers and animation state,
//! and composing everything into one image for the overlay window.

use crate::config::{AnimationKind, CombineExpiry, Config, Corner, GroupMode};
use crate::render::{Entry, Painter, RowDraw, RowLayout, RowMotion, ease, stack_positions};
use tiny_skia::{Pixmap, PixmapPaint, Transform};

/// A value that moves linearly toward 0 or 1.
#[derive(Debug, Clone, Copy)]
struct Anim {
    v: f32,
    target: f32,
}

impl Anim {
    fn hidden_to_shown() -> Anim {
        Anim { v: 0.0, target: 1.0 }
    }
    fn shown() -> Anim {
        Anim { v: 1.0, target: 1.0 }
    }
    fn step(&mut self, dt: f32, dur: f32) {
        if dur <= 0.0 {
            self.v = self.target;
            return;
        }
        let d = dt / dur;
        if self.v < self.target {
            self.v = (self.v + d).min(self.target);
        } else if self.v > self.target {
            self.v = (self.v - d).max(self.target);
        }
    }
    fn settled(&self) -> bool {
        self.v == self.target
    }
}

struct Row {
    entry: Entry,
    vis: Anim,
    h: Anim,
    expires: f64,
    exiting: bool,
    layout: Option<RowLayout>,
}

impl Row {
    fn start_exit(&mut self) {
        self.exiting = true;
        self.vis.target = 0.0;
    }
}

struct Card {
    rows: Vec<Row>,
    vis: Anim,
    h: Anim,
    expires: f64,
    exiting: bool,
    width: Option<f32>,
    width_target: f32,
}

impl Card {
    fn start_exit(&mut self) {
        self.exiting = true;
        self.vis.target = 0.0;
    }
    fn live_rows(&self) -> usize {
        self.rows.iter().filter(|r| !r.exiting).count()
    }
}

/// What the overlay window should show.
pub struct Scene {
    /// Premultiplied RGBA.
    pub pixmap: Pixmap,
    pub corner: Corner,
    /// Gap between the bottom of the work area and the bottom of the scene.
    pub margin_y: i32,
}

#[derive(Default)]
pub struct Notifier {
    cards: Vec<Card>,
    last: Option<f64>,
    mode: Option<GroupMode>,
    /// Was anything moving after the previous tick? If not, no time has "passed" for animations.
    was_animating: bool,
}

fn anim_secs(cfg: &Config) -> f32 {
    if cfg.animate { cfg.animation_ms as f32 / 1000.0 } else { 0.0 }
}

impl Notifier {
    pub fn new() -> Notifier {
        Notifier::default()
    }

    pub fn is_empty(&self) -> bool {
        self.cards.is_empty()
    }

    pub fn clear(&mut self) {
        self.cards.clear();
    }

    /// Call after the config (and so the Painter) changed.
    pub fn invalidate_layouts(&mut self) {
        for c in &mut self.cards {
            c.width = None;
            for r in &mut c.rows {
                r.layout = None;
            }
        }
    }

    /// Add a newly picked-up item.
    pub fn push(&mut self, cfg: &Config, now: f64, entry: Entry) {
        if self.mode != Some(cfg.mode) {
            self.cards.clear();
            self.mode = Some(cfg.mode);
        }
        let anim = anim_secs(cfg) as f64;
        let dur = cfg.duration_ms as f64 / 1000.0;
        let expires = now + anim + dur;
        let max = cfg.max_notifications.max(1) as usize;
        match cfg.mode {
            GroupMode::Stack => {
                self.cards.push(Card {
                    rows: vec![Row {
                        entry,
                        vis: Anim::shown(),
                        h: Anim::shown(),
                        expires,
                        exiting: false,
                        layout: None,
                    }],
                    vis: Anim::hidden_to_shown(),
                    h: Anim::hidden_to_shown(),
                    expires,
                    exiting: false,
                    width: None,
                    width_target: 0.0,
                });
                // Too many: the oldest ones leave.
                let mut live = self.cards.iter().filter(|c| !c.exiting).count();
                for c in self.cards.iter_mut() {
                    if live <= max {
                        break;
                    }
                    if !c.exiting {
                        c.start_exit();
                        live -= 1;
                    }
                }
            }
            GroupMode::Combine => {
                if let Some(card) = self.cards.last_mut() {
                    if card.exiting {
                        // Bring the box back instead of letting it leave.
                        card.exiting = false;
                        card.vis.target = 1.0;
                        card.h.target = 1.0;
                    }
                    card.expires = expires;
                    card.rows.push(Row {
                        entry,
                        vis: Anim::hidden_to_shown(),
                        h: Anim::hidden_to_shown(),
                        expires,
                        exiting: false,
                        layout: None,
                    });
                    let mut live = card.live_rows();
                    for r in card.rows.iter_mut() {
                        if live <= max {
                            break;
                        }
                        if !r.exiting {
                            r.start_exit();
                            live -= 1;
                        }
                    }
                } else {
                    self.cards.push(Card {
                        rows: vec![Row {
                            entry,
                            vis: Anim::shown(),
                            h: Anim::shown(),
                            expires,
                            exiting: false,
                            layout: None,
                        }],
                        vis: Anim::hidden_to_shown(),
                        h: Anim::shown(),
                        expires,
                        exiting: false,
                        width: None,
                        width_target: 0.0,
                    });
                }
            }
        }
    }

    /// Advance timers and animations to `now` (seconds).
    pub fn tick(&mut self, cfg: &Config, painter: &mut Painter, now: f64) {
        // After an idle period (nothing moving, or a new item just arrived) start animations from
        // zero rather than jumping ahead by the idle time.
        let dt = if self.was_animating {
            self.last.map(|l| (now - l).max(0.0) as f32).unwrap_or(0.0).min(0.1)
        } else {
            0.0
        };
        self.last = Some(now);
        let dur = anim_secs(cfg);

        for card in &mut self.cards {
            if !card.exiting {
                match (cfg.mode, cfg.combine_expiry) {
                    (GroupMode::Combine, CombineExpiry::EachRow) => {
                        let mut live = card.live_rows();
                        let mut exit_card = false;
                        for r in card.rows.iter_mut() {
                            if !r.exiting && now >= r.expires {
                                if live <= 1 {
                                    exit_card = true;
                                    break;
                                }
                                r.start_exit();
                                live -= 1;
                            }
                        }
                        if exit_card {
                            card.start_exit();
                        }
                    }
                    _ => {
                        if now >= card.expires {
                            card.start_exit();
                        }
                    }
                }
            }
            card.vis.step(dt, dur);
            if card.exiting && card.vis.v <= 0.0 {
                card.h.target = 0.0;
            }
            card.h.step(dt, dur);
            for r in &mut card.rows {
                r.vis.step(dt, dur);
                if r.exiting && r.vis.v <= 0.0 {
                    r.h.target = 0.0;
                }
                r.h.step(dt, dur);
            }
            card.rows.retain(|r| !(r.exiting && r.vis.v <= 0.0 && r.h.v <= 0.0));
        }
        let n = self.cards.len();
        self.cards
            .retain(|c| !c.rows.is_empty() && !(c.exiting && c.vis.v <= 0.0 && (c.h.v <= 0.0 || n == 1)));

        // Layouts and widths.
        for card in &mut self.cards {
            for r in &mut card.rows {
                if r.layout.is_none() {
                    r.layout = Some(painter.layout_row(&r.entry));
                }
            }
            let live: Vec<&Row> = card.rows.iter().filter(|r| !r.exiting).collect();
            let src: Vec<&Row> = if live.is_empty() { card.rows.iter().collect() } else { live };
            let content =
                src.iter().filter_map(|r| r.layout.as_ref()).map(|l| l.content_w).fold(0.0, f32::max);
            card.width_target = painter.box_width(content);
        }
        if cfg.mode == GroupMode::Stack {
            // All boxes in a stack share the width of the widest one.
            let shared = self.cards.iter().filter(|c| !c.exiting).map(|c| c.width_target).fold(0.0, f32::max);
            for c in &mut self.cards {
                if !c.exiting {
                    c.width_target = shared;
                }
            }
        }
        let k = if dur > 0.0 { 6.0 / dur } else { 0.0 };
        for c in &mut self.cards {
            match c.width {
                None => c.width = Some(c.width_target),
                Some(w) if c.exiting => c.width = Some(w), // keep the size it had when leaving
                Some(w) => {
                    let mut nw = if k > 0.0 {
                        w + (c.width_target - w) * (1.0 - (-k * dt).exp())
                    } else {
                        c.width_target
                    };
                    if (nw - c.width_target).abs() < 0.5 {
                        nw = c.width_target;
                    }
                    c.width = Some(nw);
                }
            }
        }
        self.was_animating = self.is_animating();
    }

    /// Is anything moving (so we need to redraw every frame)?
    pub fn is_animating(&self) -> bool {
        self.cards.iter().any(|c| {
            !c.vis.settled()
                || !c.h.settled()
                || (!c.exiting && c.width.map(|w| w != c.width_target).unwrap_or(true))
                || c.rows.iter().any(|r| !r.vis.settled() || !r.h.settled())
        })
    }

    /// The next time something will expire (when nothing is animating).
    pub fn next_deadline(&self, cfg: &Config) -> Option<f64> {
        let mut t: Option<f64> = None;
        let mut consider = |v: f64| t = Some(t.map_or(v, |x| x.min(v)));
        for c in &self.cards {
            if c.exiting {
                continue;
            }
            match (cfg.mode, cfg.combine_expiry) {
                (GroupMode::Combine, CombineExpiry::EachRow) => {
                    for r in c.rows.iter().filter(|r| !r.exiting) {
                        consider(r.expires);
                    }
                }
                _ => consider(c.expires),
            }
        }
        t
    }

    /// Draw the current state. `None` when there is nothing to show.
    pub fn compose(&self, cfg: &Config, painter: &mut Painter) -> Option<Scene> {
        if self.cards.is_empty() {
            return None;
        }
        let st = painter.style.clone();
        let dir = if cfg.corner == Corner::BottomRight { 1.0 } else { -1.0 };
        let row_motion = match (cfg.animate, cfg.animation) {
            (false, _) => RowMotion::None,
            (true, AnimationKind::Slide) => RowMotion::Slide(dir),
            (true, AnimationKind::Fade) => RowMotion::Fade,
        };

        // Display order: top → bottom.
        let order: Vec<usize> = if cfg.newest_on_top {
            (0..self.cards.len()).rev().collect()
        } else {
            (0..self.cards.len()).collect()
        };
        let mut boxes: Vec<(usize, Pixmap)> = Vec::new();
        for &i in &order {
            let c = &self.cards[i];
            let rows: Vec<RowDraw> = c
                .rows
                .iter()
                .filter_map(|r| {
                    Some(RowDraw { entry: &r.entry, layout: r.layout.as_ref()?, vis: r.vis.v, h: r.h.v })
                })
                .collect();
            let w = c.width.unwrap_or(c.width_target);
            if let Some(pm) = painter.draw_box(w, &rows, row_motion, cfg.newest_on_top) {
                boxes.push((i, pm));
            }
        }
        let items: Vec<(f32, f32)> =
            boxes.iter().map(|(i, pm)| (pm.height() as f32, self.cards[*i].h.v)).collect();
        let (mut tops, total) = stack_positions(&items, st.item_spacing);
        // A box whose slot is still opening sticks out above it; make room so it isn't clipped.
        let min_top = tops.iter().cloned().fold(0.0, f32::min);
        for t in &mut tops {
            *t -= min_top;
        }
        let total = total - min_top;
        let max_w = boxes.iter().map(|(_, pm)| pm.width()).max()? as f32;
        let scene_w = (max_w + st.margin_x).ceil().max(1.0) as u32;
        let scene_h = total.ceil() as u32;
        if scene_h == 0 {
            return None;
        }
        let mut scene = Pixmap::new(scene_w, scene_h)?;

        // Moving boxes first, so settled boxes sit on top of them while they slide past.
        let mut draw_order: Vec<usize> = (0..boxes.len()).collect();
        draw_order.sort_by_key(|&n| {
            if self.cards[boxes[n].0].vis.settled() && !self.cards[boxes[n].0].exiting { 1 } else { 0 }
        });
        for n in draw_order {
            let (i, pm) = &boxes[n];
            let c = &self.cards[*i];
            let bw = pm.width() as f32;
            let x_rest = if cfg.corner == Corner::BottomRight {
                scene_w as f32 - st.margin_x - bw
            } else {
                st.margin_x
            };
            let (dx, alpha) = match row_motion {
                RowMotion::Slide(d) => ((1.0 - ease(c.vis.v)) * (bw + st.margin_x) * d, st.opacity),
                RowMotion::Fade => (0.0, st.opacity * ease(c.vis.v)),
                RowMotion::None => (0.0, st.opacity),
            };
            if alpha <= 0.0 || c.vis.v <= 0.0 {
                continue;
            }
            let paint = PixmapPaint { opacity: alpha.min(1.0), ..Default::default() };
            scene.draw_pixmap(
                (x_rest + dx).round() as i32,
                tops[n].round() as i32,
                pm.as_ref(),
                &paint,
                Transform::identity(),
                None,
            );
        }
        Some(Scene { pixmap: scene, corner: cfg.corner, margin_y: st.margin_y as i32 })
    }
}

/// A soft, game-like backdrop so transparency is visible in previews.
pub fn backdrop(w: u32, h: u32) -> Option<Pixmap> {
    let mut pm = Pixmap::new(w.max(1), h.max(1))?;
    let (w, h) = (pm.width(), pm.height());
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
    Some(pm)
}

/// The preview scene placed in the configured corner of a backdrop, like on screen.
pub fn preview_image(
    cfg: &Config,
    painter: &mut Painter,
    entries: &[Entry],
    min_w: u32,
    min_h: u32,
) -> Option<Pixmap> {
    let scene = preview(cfg, painter, entries)?;
    let pad_top = (16.0 * cfg.scale()).round() as u32;
    let w = (scene.pixmap.width() + (24.0 * cfg.scale()) as u32).max(min_w);
    let h = (scene.pixmap.height() + scene.margin_y.max(0) as u32 + pad_top).max(min_h);
    let mut canvas = backdrop(w, h)?;
    let x = match scene.corner {
        Corner::BottomRight => w as i32 - scene.pixmap.width() as i32,
        Corner::BottomLeft => 0,
    };
    let y = h as i32 - scene.margin_y - scene.pixmap.height() as i32;
    canvas.draw_pixmap(x, y, scene.pixmap.as_ref(), &PixmapPaint::default(), Transform::identity(), None);
    Some(canvas)
}

/// Render a still image of how notifications look with `cfg` (used by the settings preview).
pub fn preview(cfg: &Config, painter: &mut Painter, entries: &[Entry]) -> Option<Scene> {
    // Keep everything on screen regardless of the configured duration.
    let mut long = cfg.clone();
    long.duration_ms = 600_000;
    let cfg = &long;
    let mut n = Notifier::new();
    for e in entries {
        n.push(cfg, 0.0, e.clone());
    }
    n.tick(cfg, painter, 0.0);
    let settle = anim_secs(cfg) as f64 + 0.001;
    // Step in small increments so width easing settles too.
    let mut t = 0.0;
    while t < settle * 3.0 {
        t += 0.05f64.min(settle.max(0.001));
        n.tick(cfg, painter, t.min(settle * 3.0));
    }
    n.compose(cfg, painter)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::font::FontSystem;

    fn painter(cfg: &Config) -> Option<Painter> {
        // Needs at least one font on the machine running the tests.
        Painter::new(cfg, &FontSystem::new())
    }

    fn e(name: &str) -> Entry {
        Entry::new(name, 1, None)
    }

    /// Tick at 60 fps from `from` to `to`.
    fn run(n: &mut Notifier, cfg: &Config, p: &mut Painter, from: f64, to: f64) {
        let mut t = from;
        while t < to {
            t = (t + 1.0 / 60.0).min(to);
            n.tick(cfg, p, t);
        }
    }

    #[test]
    fn stack_respects_max_and_shares_width() {
        let cfg = Config { mode: GroupMode::Stack, max_notifications: 3, ..Config::default() };
        let Some(mut p) = painter(&cfg) else { return };
        let mut n = Notifier::new();
        for (i, name) in ["A", "Bb", "Ccc", "A much longer item name", "E"].iter().enumerate() {
            n.push(&cfg, i as f64 * 0.01, e(name));
        }
        n.tick(&cfg, &mut p, 0.05);
        assert_eq!(n.cards.iter().filter(|c| !c.exiting).count(), 3);
        run(&mut n, &cfg, &mut p, 0.05, 1.0);
        assert_eq!(n.cards.len(), 3, "overflowed boxes are gone");
        let widths: Vec<f32> = n.cards.iter().map(|c| c.width.unwrap()).collect();
        assert!(widths.windows(2).all(|w| w[0] == w[1]), "same width: {widths:?}");
    }

    #[test]
    fn combine_whole_box_timer_resets() {
        let cfg = Config::default(); // combine, whole box, 1200 ms, 300 ms animation
        let Some(mut p) = painter(&cfg) else { return };
        let mut n = Notifier::new();
        n.push(&cfg, 0.0, e("A"));
        n.tick(&cfg, &mut p, 0.0);
        run(&mut n, &cfg, &mut p, 0.0, 1.0);
        n.push(&cfg, 1.0, e("B"));
        run(&mut n, &cfg, &mut p, 1.0, 2.45);
        assert_eq!(n.cards.len(), 1);
        assert!(!n.cards[0].exiting, "timer was reset by B");
        assert_eq!(n.cards[0].rows.len(), 2);
        run(&mut n, &cfg, &mut p, 2.45, 3.2);
        assert!(n.is_empty(), "whole box left at once");
    }

    #[test]
    fn combine_each_row_expires_alone() {
        let cfg = Config { combine_expiry: CombineExpiry::EachRow, ..Config::default() };
        let Some(mut p) = painter(&cfg) else { return };
        let mut n = Notifier::new();
        n.push(&cfg, 0.0, e("A"));
        n.tick(&cfg, &mut p, 0.0);
        run(&mut n, &cfg, &mut p, 0.0, 1.0);
        n.push(&cfg, 1.0, e("B"));
        run(&mut n, &cfg, &mut p, 1.0, 2.3);
        assert_eq!(n.cards[0].rows.len(), 1, "A left on its own");
        assert_eq!(n.cards[0].rows[0].entry.name, "B");
        assert!(!n.cards[0].exiting);
        run(&mut n, &cfg, &mut p, 2.3, 3.4);
        assert!(n.is_empty());
    }

    #[test]
    fn exit_animation_starts_from_the_beginning_after_idle() {
        let cfg = Config { mode: GroupMode::Stack, ..Config::default() };
        let Some(mut p) = painter(&cfg) else { return };
        let mut n = Notifier::new();
        n.push(&cfg, 0.0, e("A"));
        n.tick(&cfg, &mut p, 0.0);
        run(&mut n, &cfg, &mut p, 0.0, 0.5);
        assert!(!n.is_animating());
        // The UI sleeps until the deadline, then ticks once.
        let deadline = n.next_deadline(&cfg).unwrap();
        assert!((deadline - 1.5).abs() < 1e-6);
        n.tick(&cfg, &mut p, deadline + 0.001);
        assert!(n.cards[0].exiting);
        assert!(n.cards[0].vis.v > 0.99, "no jump: {}", n.cards[0].vis.v);
        n.tick(&cfg, &mut p, deadline + 0.017);
        assert!(n.cards[0].vis.v < 0.99);
    }

    #[test]
    fn no_animation_is_instant() {
        let cfg = Config { animate: false, ..Config::default() };
        let Some(mut p) = painter(&cfg) else { return };
        let mut n = Notifier::new();
        n.push(&cfg, 0.0, e("A"));
        n.tick(&cfg, &mut p, 0.0);
        assert!(!n.is_animating());
        assert!(n.compose(&cfg, &mut p).is_some());
        n.tick(&cfg, &mut p, 1.21);
        assert!(n.is_empty());
    }
}
