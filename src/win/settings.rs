//! The settings window (runs as `pg-pickup-notifier.exe --settings`).
//!
//! Every change is saved straight away and the running notifier is told to reload,
//! so changes show up live. "Undo changes" returns to how things were when the window opened.

use super::{IpcMessages, MonitorInfo, SETTINGS_TITLE, ipc_messages};
use eframe::egui;
use pg_pickup_notifier::chatlog::newest_log;
use pg_pickup_notifier::config::*;
use pg_pickup_notifier::font::FontSystem;
use pg_pickup_notifier::itemdb::{Icon, IconCache, ItemDb, fetch_version, newest_cached_version};
use pg_pickup_notifier::notify::preview_image;
use pg_pickup_notifier::render::Painter;
use pg_pickup_notifier::{SAMPLE_ITEMS, log, paths};
use std::collections::HashMap;
use std::sync::{Arc, mpsc};
use std::time::{Duration, Instant};

pub fn run() {
    pg_pickup_notifier::logger::init("settings");
    let icon = egui::IconData { rgba: pg_pickup_notifier::appicon::rgba(64), width: 64, height: 64 };
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title(SETTINGS_TITLE)
            .with_inner_size([1060.0, 740.0])
            .with_min_inner_size([760.0, 480.0])
            .with_icon(Arc::new(icon)),
        ..Default::default()
    };
    if let Err(e) = eframe::run_native(
        SETTINGS_TITLE,
        options,
        Box::new(|cc| Ok(Box::new(SettingsApp::new(&cc.egui_ctx)))),
    ) {
        log!("settings window failed: {e}");
    }
}

struct SettingsApp {
    cfg: Config,
    original: Config,
    saved: Config,
    changed_at: Option<Instant>,
    fonts: Option<Arc<FontSystem>>,
    fonts_rx: Option<mpsc::Receiver<FontSystem>>,
    families: Vec<String>,
    icons: HashMap<u32, Arc<Icon>>,
    icons_rx: Option<mpsc::Receiver<()>>,
    preview: Option<(egui::TextureHandle, [usize; 2])>,
    preview_key: Option<Config>,
    preview_dirty: bool,
    font_used: String,
    bg_hex: String,
    text_hex: String,
    autostart: bool,
    monitors: Vec<MonitorInfo>,
    ipc: IpcMessages,
    status: Option<(String, Instant)>,
    /// Every item name from the cached items.json as (lowercase, original), for rule match counts.
    item_names: Option<Arc<Vec<(String, String)>>>,
    names_rx: Option<mpsc::Receiver<Vec<(String, String)>>>,
    match_cache: HashMap<String, (usize, Vec<String>)>,
}

fn load_sample_icons() -> HashMap<u32, Arc<Icon>> {
    let mut cache = IconCache::new(paths::icon_cache_dir());
    SAMPLE_ITEMS.iter().filter_map(|(_, _, id)| cache.get_local(*id).map(|i| (*id, i))).collect()
}

impl SettingsApp {
    fn new(ctx: &egui::Context) -> SettingsApp {
        let cfg = Config::load(&paths::config_file());

        let (ftx, frx) = mpsc::channel();
        let c = ctx.clone();
        std::thread::spawn(move || {
            let _ = ftx.send(FontSystem::new());
            c.request_repaint();
        });

        let icons = load_sample_icons();
        let missing: Vec<u32> =
            SAMPLE_ITEMS.iter().map(|(_, _, id)| *id).filter(|id| !icons.contains_key(id)).collect();
        let icons_rx = if missing.is_empty() {
            None
        } else {
            let (itx, irx) = mpsc::channel();
            let c = ctx.clone();
            std::thread::spawn(move || {
                let version = fetch_version().ok().or_else(|| newest_cached_version(&paths::cache_dir()));
                if let Some(v) = version {
                    for id in missing {
                        if let Err(e) = IconCache::download(&paths::icon_cache_dir(), v, id) {
                            log!("settings: {e}");
                        }
                    }
                }
                let _ = itx.send(());
                c.request_repaint();
            });
            Some(irx)
        };

        let (ntx, nrx) = mpsc::channel();
        let c = ctx.clone();
        std::thread::spawn(move || {
            if let Some(db) = ItemDb::load_cached(&paths::cache_dir()) {
                let mut names: Vec<(String, String)> =
                    db.names().map(|n| (n.to_lowercase(), n.to_string())).collect();
                names.sort();
                let _ = ntx.send(names);
            }
            c.request_repaint();
        });

        SettingsApp {
            bg_hex: cfg.background_color.clone(),
            text_hex: cfg.text_color.clone(),
            original: cfg.clone(),
            saved: cfg.clone(),
            cfg,
            changed_at: None,
            fonts: None,
            fonts_rx: Some(frx),
            families: Vec::new(),
            icons,
            icons_rx,
            preview: None,
            preview_key: None,
            preview_dirty: true,
            font_used: String::new(),
            autostart: super::autostart_enabled(),
            monitors: super::monitors(),
            ipc: ipc_messages(),
            status: None,
            item_names: None,
            names_rx: Some(nrx),
            match_cache: HashMap::new(),
        }
    }

    fn set_status(&mut self, s: impl Into<String>) {
        self.status = Some((s.into(), Instant::now()));
    }

    /// Save now (if anything changed) and tell the notifier.
    fn save_now(&mut self) {
        if self.cfg == self.saved {
            return;
        }
        // The tray's pause toggle owns `paused`; keep whatever is on disk.
        let disk = Config::load(&paths::config_file());
        self.cfg.paused = disk.paused;
        let cfg = self.cfg.clone().sanitized();
        match cfg.save(&paths::config_file()) {
            Ok(()) => {
                self.saved = self.cfg.clone();
                super::post_to_notifier(self.ipc.reload);
            }
            Err(e) => self.set_status(format!("Could not save settings: {e}")),
        }
        self.changed_at = None;
    }

    fn poll_background(&mut self) {
        if let Some(rx) = &self.fonts_rx
            && let Ok(fonts) = rx.try_recv()
        {
            self.families = fonts.families();
            self.fonts = Some(Arc::new(fonts));
            self.fonts_rx = None;
            self.preview_dirty = true;
        }
        if let Some(rx) = &self.names_rx {
            match rx.try_recv() {
                Ok(names) => {
                    self.item_names = Some(Arc::new(names));
                    self.names_rx = None;
                }
                Err(mpsc::TryRecvError::Disconnected) => self.names_rx = None,
                Err(mpsc::TryRecvError::Empty) => {}
            }
        }
        if let Some(rx) = &self.icons_rx
            && rx.try_recv().is_ok()
        {
            self.icons = load_sample_icons();
            self.icons_rx = None;
            self.preview_dirty = true;
        }
    }

    fn update_preview(&mut self, ctx: &egui::Context) {
        let Some(fonts) = self.fonts.clone() else { return };
        let key = self.cfg.clone().sanitized();
        if !self.preview_dirty && self.preview_key.as_ref() == Some(&key) {
            return;
        }
        self.preview_dirty = false;
        self.preview_key = Some(key.clone());
        let Some(mut painter) = Painter::new(&key, &fonts) else { return };
        self.font_used = painter.font_name.clone();
        let entries = pg_pickup_notifier::sample_entries(&key, |id| self.icons.get(&id).cloned());
        let Some(img) = preview_image(&key, &mut painter, &entries, 360, 200) else { return };
        let size = [img.width() as usize, img.height() as usize];
        let color = egui::ColorImage::from_rgba_unmultiplied(size, img.data());
        let tex = ctx.load_texture("preview", color, egui::TextureOptions::NEAREST);
        self.preview = Some((tex, size));
    }

    // ---------------------------------------------------------------- UI pieces

    fn section(ui: &mut egui::Ui, title: &str, body: impl FnOnce(&mut egui::Ui)) {
        egui::CollapsingHeader::new(egui::RichText::new(title).strong()).default_open(true).show(ui, |ui| {
            egui::Grid::new(title).num_columns(2).spacing([14.0, 8.0]).min_col_width(150.0).show(ui, body);
        });
        ui.add_space(4.0);
    }

    fn settings_ui(&mut self, ui: &mut egui::Ui) {
        let c = &mut self.cfg;

        Self::section(ui, "Notifications", |ui| {
            ui.label("Grouping");
            ui.vertical(|ui| {
                ui.radio_value(&mut c.mode, GroupMode::Combine, "Combine – add items to one box");
                ui.radio_value(&mut c.mode, GroupMode::Stack, "Stack – one box per item");
            });
            ui.end_row();
            if c.mode == GroupMode::Combine {
                ui.label("Combined box leaves");
                ui.vertical(|ui| {
                    ui.radio_value(
                        &mut c.combine_expiry,
                        CombineExpiry::WholeBox,
                        "All at once (each new item resets the timer)",
                    );
                    ui.radio_value(
                        &mut c.combine_expiry,
                        CombineExpiry::EachRow,
                        "One row at a time (each item has its own timer)",
                    );
                });
                ui.end_row();
            }
            ui.label("New items appear");
            ui.horizontal(|ui| {
                ui.radio_value(&mut c.newest_on_top, true, "On top");
                ui.radio_value(&mut c.newest_on_top, false, "At the bottom");
            });
            ui.end_row();
            ui.label("Max notifications");
            ui.add(egui::Slider::new(&mut c.max_notifications, 1..=20));
            ui.end_row();
            ui.label("Show duration");
            ui.add(egui::Slider::new(&mut c.duration_ms, 200..=10000).suffix(" ms").logarithmic(true));
            ui.end_row();
            ui.label("Quantities");
            ui.checkbox(&mut c.show_quantity, "Show quantities picked up (x2, x5 …)");
            ui.end_row();
        });

        let monitors = self.monitors.clone();
        Self::section(ui, "Position", |ui| {
            ui.label("Corner");
            ui.horizontal(|ui| {
                ui.radio_value(&mut c.corner, Corner::BottomLeft, "Bottom left");
                ui.radio_value(&mut c.corner, Corner::BottomRight, "Bottom right");
            });
            ui.end_row();
            ui.label("Monitor");
            let name = |i: u32| -> String {
                if i == 0 {
                    return "Primary monitor".into();
                }
                match monitors.get(i as usize - 1) {
                    Some(m) => format!(
                        "Monitor {i} – {}×{}{}",
                        m.full.right - m.full.left,
                        m.full.bottom - m.full.top,
                        if m.primary { " (primary)" } else { "" }
                    ),
                    None => format!("Monitor {i} (not connected)"),
                }
            };
            egui::ComboBox::from_id_salt("monitor").selected_text(name(c.monitor)).width(260.0).show_ui(
                ui,
                |ui| {
                    for i in 0..=(monitors.len() as u32).max(c.monitor) {
                        ui.selectable_value(&mut c.monitor, i, name(i));
                    }
                },
            );
            ui.end_row();
            ui.label("Distance from side");
            ui.add(egui::Slider::new(&mut c.margin_x, 0..=400).suffix(" px"));
            ui.end_row();
            ui.label("Distance from bottom");
            ui.add(egui::Slider::new(&mut c.margin_y, 0..=400).suffix(" px"));
            ui.end_row();
        });

        Self::section(ui, "Animation", |ui| {
            ui.label("Animate");
            ui.checkbox(&mut c.animate, "Animate notifications");
            ui.end_row();
            ui.label("Style");
            ui.add_enabled_ui(c.animate, |ui| {
                ui.horizontal(|ui| {
                    ui.radio_value(&mut c.animation, AnimationKind::Slide, "Slide");
                    ui.radio_value(&mut c.animation, AnimationKind::Fade, "Fade");
                });
            });
            ui.end_row();
            ui.label("Animation speed");
            ui.add_enabled(c.animate, egui::Slider::new(&mut c.animation_ms, 50..=2000).suffix(" ms"));
            ui.end_row();
        });

        let (bg_hex, text_hex) = (&mut self.bg_hex, &mut self.text_hex);
        Self::section(ui, "Box", |ui| {
            ui.label("Scale");
            let icon = (32.0 * c.scale_percent as f32 / 100.0).round();
            ui.add(
                egui::Slider::new(&mut c.scale_percent, 50..=400)
                    .suffix(format!(" %   (icons {icon}×{icon})")),
            );
            ui.end_row();
            ui.label("Opacity");
            ui.add(egui::Slider::new(&mut c.opacity_percent, 10..=100).suffix(" %"));
            ui.end_row();
            ui.label("Background colour");
            color_row(ui, &mut c.background_color, bg_hex);
            ui.end_row();
            ui.label("Background opacity");
            ui.add(egui::Slider::new(&mut c.background_opacity_percent, 0..=100).suffix(" %"));
            ui.end_row();
            ui.label("Corner radius");
            ui.add(egui::Slider::new(&mut c.corner_radius, 0..=20).suffix(" px"));
            ui.end_row();
            ui.label("Padding");
            ui.add(egui::Slider::new(&mut c.box_padding, 0..=20).suffix(" px"));
            ui.end_row();
        });

        let families = &self.families;
        let fonts_ready = self.fonts.is_some();
        let font_used = self.font_used.clone();
        Self::section(ui, "Text", |ui| {
            ui.label("Font");
            ui.horizontal(|ui| {
                egui::ComboBox::from_id_salt("font")
                    .selected_text(&c.font_family)
                    .width(175.0)
                    .height(400.0)
                    .show_ui(ui, |ui| {
                        if !fonts_ready {
                            ui.label("Loading fonts…");
                        }
                        for f in families {
                            ui.selectable_value(&mut c.font_family, f.clone(), f);
                        }
                    });
                ui.checkbox(&mut c.font_bold, "Bold");
                ui.checkbox(&mut c.font_italic, "Italic");
            });
            ui.end_row();
            let known = !fonts_ready || families.iter().any(|f| f.eq_ignore_ascii_case(c.font_family.trim()));
            if !known {
                ui.label("");
                ui.colored_label(
                    egui::Color32::from_rgb(230, 150, 40),
                    format!("“{}” isn't installed – using {} instead.", c.font_family.trim(), font_used),
                );
                ui.end_row();
            }
            ui.label("Font size");
            ui.add(egui::Slider::new(&mut c.font_size, 8.0..=40.0).step_by(0.5).suffix(" px"));
            ui.end_row();
            ui.label("Text colour");
            color_row(ui, &mut c.text_color, text_hex);
            ui.end_row();
            ui.label("Text shadow");
            ui.checkbox(&mut c.text_shadow, "Draw a soft shadow behind text");
            ui.end_row();
            ui.label("Justify items");
            ui.horizontal(|ui| {
                ui.radio_value(&mut c.justify, Justify::Left, "Left");
                ui.radio_value(&mut c.justify, Justify::Center, "Center");
                ui.radio_value(&mut c.justify, Justify::Right, "Right");
            });
            ui.end_row();
            ui.label("Long names");
            ui.horizontal(|ui| {
                ui.radio_value(&mut c.name_overflow, NameOverflow::FullWidth, "Full width");
                ui.radio_value(&mut c.name_overflow, NameOverflow::Wrap, "Wrap");
                ui.radio_value(&mut c.name_overflow, NameOverflow::Truncate, "Truncate");
            });
            ui.end_row();
            ui.label("Max text width");
            ui.add_enabled(
                c.name_overflow != NameOverflow::FullWidth,
                egui::Slider::new(&mut c.max_text_width, 60..=800).suffix(" px"),
            );
            ui.end_row();
        });

        Self::section(ui, "Icons & spacing", |ui| {
            ui.label("Icons");
            ui.checkbox(&mut c.show_icons, "Show item icons");
            ui.end_row();
            ui.label("Icon position");
            ui.add_enabled_ui(c.show_icons, |ui| {
                ui.horizontal(|ui| {
                    ui.radio_value(&mut c.icon_position, IconPosition::LeftOfText, "Left of text");
                    ui.radio_value(&mut c.icon_position, IconPosition::RightOfText, "Right of text");
                });
            });
            ui.end_row();
            ui.label("Gap between items");
            ui.vertical(|ui| {
                ui.add(
                    egui::Slider::new(&mut c.item_spacing_with_icons, 0..=30)
                        .suffix(" px  when icons are shown"),
                );
                ui.add(
                    egui::Slider::new(&mut c.item_spacing_without_icons, 0..=30)
                        .suffix(" px  when icons are hidden"),
                );
            });
            ui.end_row();
        });

        let names = self.item_names.clone();
        let match_cache = &mut self.match_cache;
        Self::section(ui, "Highlight & hide", |ui| {
            ui.label("Highlight")
                .on_hover_text("Items whose name contains this text (ignoring case) get these colours. The first matching rule wins.");
            ui.vertical(|ui| {
                let mut remove = None;
                for (i, rule) in c.highlights.iter_mut().enumerate() {
                    ui.horizontal(|ui| {
                        ui.add(
                            egui::TextEdit::singleline(&mut rule.contains)
                                .hint_text("name contains…")
                                .desired_width(116.0),
                        );
                        color_swatch(ui, &mut rule.background_color).on_hover_text("Background colour");
                        color_swatch(ui, &mut rule.text_color).on_hover_text("Text colour");
                        if ui.small_button("×").on_hover_text("Remove this rule").clicked() {
                            remove = Some(i);
                        }
                        match_label(ui, names.as_deref(), match_cache, &rule.contains);
                    });
                }
                if let Some(i) = remove {
                    c.highlights.remove(i);
                }
                if ui.button("+ Add highlight").clicked() {
                    c.highlights.push(HighlightRule::default());
                }
            });
            ui.end_row();

            ui.label("Hide")
                .on_hover_text("Items whose name contains this text (ignoring case) aren't shown.");
            ui.vertical(|ui| {
                let mut remove = None;
                for (i, pattern) in c.hidden_items.iter_mut().enumerate() {
                    ui.horizontal(|ui| {
                        ui.add(
                            egui::TextEdit::singleline(pattern)
                                .hint_text("name contains…")
                                .desired_width(116.0),
                        );
                        if ui.small_button("×").on_hover_text("Remove").clicked() {
                            remove = Some(i);
                        }
                        match_label(ui, names.as_deref(), match_cache, pattern);
                    });
                }
                if let Some(i) = remove {
                    c.hidden_items.remove(i);
                }
                if ui.button("+ Add hidden item").clicked() {
                    c.hidden_items.push(String::new());
                }
            });
            ui.end_row();

            ui.label("Hidden items");
            ui.checkbox(&mut c.show_hidden, "Show hidden items anyway (dimmed)").on_hover_text(
                "Handy for checking your rules: hidden items appear faded instead of not at all.",
            );
            ui.end_row();
        });

        let mut autostart = self.autostart;
        let mut autostart_changed = false;
        Self::section(ui, "Chat logs & startup", |ui| {
            ui.label("Chat log folder");
            ui.vertical(|ui| {
                let default = paths::default_chatlog_dir();
                ui.add(
                    egui::TextEdit::singleline(&mut c.chatlog_dir)
                        .hint_text(default.display().to_string())
                        .desired_width(290.0),
                );
                ui.horizontal(|ui| {
                    if ui.button("Browse…").clicked()
                        && let Some(dir) = rfd::FileDialog::new().set_directory(c.chatlog_dir()).pick_folder()
                    {
                        c.chatlog_dir = dir.display().to_string();
                    }
                    if ui.button("Open").clicked() {
                        super::open_folder(&c.chatlog_dir());
                    }
                    if ui.add_enabled(!c.chatlog_dir.is_empty(), egui::Button::new("Use default")).clicked() {
                        c.chatlog_dir.clear();
                    }
                });
                let dir = c.chatlog_dir();
                if !dir.is_dir() {
                    ui.colored_label(egui::Color32::from_rgb(240, 120, 100), "Folder not found.");
                } else if let Some(log) = newest_log(&dir) {
                    ui.weak(format!("Watching {}", log.file_name().unwrap_or_default().to_string_lossy()));
                } else {
                    ui.colored_label(
                        egui::Color32::from_rgb(240, 180, 80),
                        "No chat logs here yet – make sure chat logging is on in the game.",
                    );
                }
            });
            ui.end_row();
            ui.label("Startup");
            if ui.checkbox(&mut autostart, "Start PG Pickup Notifier with Windows").changed() {
                autostart_changed = true;
            }
            ui.end_row();
        });
        if autostart_changed {
            if super::set_autostart(autostart) {
                self.autostart = autostart;
            } else {
                self.set_status("Could not change the startup setting.");
            }
        }
    }

    fn preview_ui(&mut self, ui: &mut egui::Ui) {
        ui.heading("Preview");
        ui.add_space(6.0);
        match &self.preview {
            Some((tex, size)) => {
                let ppp = ui.ctx().pixels_per_point();
                let pts = egui::vec2(size[0] as f32 / ppp, size[1] as f32 / ppp);
                egui::ScrollArea::both()
                    .id_salt("preview_scroll")
                    .max_height(ui.available_height() - 120.0)
                    .show(ui, |ui| {
                        ui.add(egui::Image::from_texture(egui::load::SizedTexture::new(tex.id(), pts)));
                    });
            }
            None => {
                ui.weak("Loading fonts…");
            }
        }
        ui.add_space(10.0);
        ui.horizontal_wrapped(|ui| {
            if ui.button("▶  Show test notification on screen").clicked() {
                self.save_now();
                if !super::post_to_notifier(self.ipc.test) {
                    self.set_status("The notifier isn't running. Click “Start notifier” first.");
                }
            }
            if super::find_notifier_window().is_null()
                && ui.button("Start notifier").clicked()
                && let Ok(exe) = std::env::current_exe()
            {
                let _ = std::process::Command::new(exe).spawn();
                self.set_status("Starting the notifier…");
            }
        });
        ui.add_space(6.0);
        ui.weak("Changes are saved and applied immediately.");
        ui.weak(format!("Settings file: {}", paths::config_file().display()));
    }
}

/// Just a colour swatch for a "#RRGGBB" string.
fn color_swatch(ui: &mut egui::Ui, value: &mut String) -> egui::Response {
    let mut rgb = parse_hex_color(value).unwrap_or([255, 255, 255]);
    let resp = egui::widgets::color_picker::color_edit_button_srgb(ui, &mut rgb);
    if resp.changed() {
        *value = format_hex_color(rgb);
    }
    resp
}

/// "31 items" next to a rule, with some examples on hover.
fn match_label(
    ui: &mut egui::Ui,
    names: Option<&Vec<(String, String)>>,
    cache: &mut HashMap<String, (usize, Vec<String>)>,
    pattern: &str,
) {
    let (Some(names), false) = (names, pattern.trim().is_empty()) else { return };
    let key = pattern.to_lowercase();
    let (count, examples) = cache.entry(key.clone()).or_insert_with(|| {
        let hits: Vec<&String> = names.iter().filter(|(l, _)| l.contains(&key)).map(|(_, n)| n).collect();
        (hits.len(), hits.iter().take(25).map(|s| s.to_string()).collect())
    });
    let text = match *count {
        0 => "no items".to_string(),
        1 => "1 item".to_string(),
        n => format!("{n} items"),
    };
    let resp = ui.weak(text);
    if *count > 0 {
        let more = if *count > examples.len() {
            format!("\n… and {} more", *count - examples.len())
        } else {
            String::new()
        };
        resp.on_hover_text(format!("{}{more}", examples.join("\n")));
    }
}

/// Colour swatch + hex text box, kept in sync with a "#RRGGBB" string.
fn color_row(ui: &mut egui::Ui, value: &mut String, hex: &mut String) {
    ui.horizontal(|ui| {
        let mut rgb = parse_hex_color(value).unwrap_or([255, 255, 255]);
        if egui::widgets::color_picker::color_edit_button_srgb(ui, &mut rgb).changed() {
            *value = format_hex_color(rgb);
            *hex = value.clone();
        }
        let resp = ui.add(egui::TextEdit::singleline(hex).desired_width(80.0).char_limit(7));
        if resp.changed()
            && let Some(c) = parse_hex_color(hex)
        {
            *value = format_hex_color(c);
        }
        if resp.lost_focus() {
            *hex = value.clone();
        }
    });
}

impl eframe::App for SettingsApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.poll_background();

        egui::Panel::bottom("buttons").show(ui, |ui| {
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                if ui.button("Restore defaults").clicked() {
                    let keep_dir = self.cfg.chatlog_dir.clone();
                    self.cfg = Config { chatlog_dir: keep_dir, ..Config::default() };
                    self.bg_hex = self.cfg.background_color.clone();
                    self.text_hex = self.cfg.text_color.clone();
                }
                if ui.add_enabled(self.cfg != self.original, egui::Button::new("Undo changes")).clicked() {
                    self.cfg = self.original.clone();
                    self.bg_hex = self.cfg.background_color.clone();
                    self.text_hex = self.cfg.text_color.clone();
                }
                if let Some((msg, t)) = &self.status
                    && t.elapsed() < Duration::from_secs(6)
                {
                    ui.colored_label(egui::Color32::from_rgb(240, 180, 80), msg);
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button("  Close  ").clicked() {
                        self.save_now();
                        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                    }
                });
            });
            ui.add_space(6.0);
        });

        egui::Panel::left("settings").resizable(false).exact_size(500.0).show(ui, |ui| {
            egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                ui.add_space(6.0);
                self.settings_ui(ui);
                ui.add_space(12.0);
            });
        });

        egui::CentralPanel::default().show(ui, |ui| {
            self.update_preview(&ctx);
            self.preview_ui(ui);
        });

        // Debounced save.
        if self.cfg != self.saved {
            let t = *self.changed_at.get_or_insert_with(Instant::now);
            if t.elapsed() >= Duration::from_millis(200) {
                self.save_now();
            } else {
                ctx.request_repaint_after(Duration::from_millis(60));
            }
        } else {
            self.changed_at = None;
        }
        if self.fonts_rx.is_some() || self.icons_rx.is_some() {
            ctx.request_repaint_after(Duration::from_millis(200));
        }
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        self.save_now();
    }
}
