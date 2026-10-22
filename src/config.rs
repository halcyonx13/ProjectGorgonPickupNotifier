//! User-configurable settings, persisted as TOML.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Corner {
    BottomLeft,
    BottomRight,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GroupMode {
    /// Every pickup gets its own notification box.
    Stack,
    /// Pickups are added as rows to one shared notification box.
    Combine,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CombineExpiry {
    /// Each new item resets the timer for the whole box; the box leaves all at once.
    WholeBox,
    /// Each row expires on its own and the box shrinks.
    EachRow,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AnimationKind {
    Slide,
    Fade,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Justify {
    Left,
    Center,
    Right,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IconPosition {
    LeftOfText,
    RightOfText,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NameOverflow {
    /// Long names wrap onto extra lines at the maximum text width.
    Wrap,
    /// Long names are cut off with an ellipsis at the maximum text width.
    Truncate,
    /// Names are never shortened; the box grows to fit.
    FullWidth,
}

/// Items whose name contains `contains` (case-insensitive) get their own colours.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct HighlightRule {
    pub contains: String,
    pub background_color: String,
    pub text_color: String,
}

impl Default for HighlightRule {
    fn default() -> Self {
        HighlightRule {
            contains: String::new(),
            background_color: "#FFD23F".into(),
            text_color: "#1C212E".into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    // ---- Placement & behaviour ----
    pub corner: Corner,
    /// 0 = primary monitor, 1.. = monitor number.
    pub monitor: u32,
    /// Distance from the screen edge, in pixels at 100% scale.
    pub margin_x: u32,
    /// Distance from the bottom of the work area (above the taskbar), in pixels at 100% scale.
    pub margin_y: u32,
    pub mode: GroupMode,
    pub combine_expiry: CombineExpiry,
    /// New items appear above older ones (true) or below them (false).
    pub newest_on_top: bool,
    pub max_notifications: u32,
    pub duration_ms: u32,

    // ---- Animation ----
    pub animate: bool,
    pub animation: AnimationKind,
    pub animation_ms: u32,

    // ---- Box appearance ----
    pub scale_percent: u32,
    pub opacity_percent: u32,
    pub background_color: String,
    pub background_opacity_percent: u32,
    /// Corner radius, in pixels at 100% scale.
    pub corner_radius: u32,
    /// Inner padding of the box, in pixels at 100% scale.
    pub box_padding: u32,

    // ---- Text ----
    pub font_family: String,
    /// Font size in pixels at 100% scale.
    pub font_size: f32,
    pub font_bold: bool,
    pub font_italic: bool,
    pub text_color: String,
    pub text_shadow: bool,
    pub justify: Justify,
    pub name_overflow: NameOverflow,
    /// Maximum text width (pixels at 100% scale) used by Wrap and Truncate.
    pub max_text_width: u32,
    pub show_quantity: bool,

    // ---- Icons ----
    pub show_icons: bool,
    pub icon_position: IconPosition,

    // ---- Spacing ----
    /// Vertical gap between items when icons are shown (pixels at 100% scale).
    pub item_spacing_with_icons: u32,
    /// Vertical gap between items when icons are hidden (pixels at 100% scale).
    pub item_spacing_without_icons: u32,

    // ---- Source ----
    /// Empty = the default Project Gorgon ChatLogs folder.
    pub chatlog_dir: String,
    pub paused: bool,

    // ---- Filtering ----
    /// Items whose name contains any of these (case-insensitive) are not shown.
    pub hidden_items: Vec<String>,
    /// Show hidden items anyway (dimmed), e.g. to check the rules.
    pub show_hidden: bool,
    /// First matching rule wins. (Kept last: TOML tables must follow plain values.)
    pub highlights: Vec<HighlightRule>,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            corner: Corner::BottomRight,
            monitor: 0,
            margin_x: 16,
            margin_y: 16,
            mode: GroupMode::Combine,
            combine_expiry: CombineExpiry::WholeBox,
            newest_on_top: true,
            max_notifications: 5,
            duration_ms: 1200,

            animate: true,
            animation: AnimationKind::Slide,
            animation_ms: 300,

            scale_percent: 100,
            opacity_percent: 100,
            background_color: "#1C212E".into(),
            background_opacity_percent: 100,
            corner_radius: 4,
            box_padding: 4,

            font_family: "Segoe UI".into(),
            font_size: 15.0,
            font_bold: false,
            font_italic: false,
            text_color: "#FFFFFF".into(),
            text_shadow: false,
            justify: Justify::Right,
            name_overflow: NameOverflow::FullWidth,
            max_text_width: 220,
            show_quantity: false,

            show_icons: true,
            icon_position: IconPosition::LeftOfText,

            item_spacing_with_icons: 2,
            item_spacing_without_icons: 1,

            chatlog_dir: String::new(),
            paused: false,

            hidden_items: vec!["bottle of".into()],
            show_hidden: false,
            highlights: vec![HighlightRule { contains: "trophy".into(), ..HighlightRule::default() }],
        }
    }
}

impl Config {
    pub fn load(path: &Path) -> Config {
        match std::fs::read_to_string(path) {
            Ok(text) => match toml::from_str::<Config>(&text) {
                Ok(c) => c.sanitized(),
                Err(e) => {
                    crate::log!("config: could not parse {}: {e}; using defaults", path.display());
                    Config::default()
                }
            },
            Err(_) => Config::default(),
        }
    }

    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let text = toml::to_string_pretty(self).map_err(std::io::Error::other)?;
        let tmp = path.with_extension("toml.tmp");
        std::fs::write(&tmp, text)?;
        std::fs::rename(&tmp, path)
    }

    /// Clamp values into usable ranges.
    pub fn sanitized(mut self) -> Config {
        self.scale_percent = self.scale_percent.clamp(25, 400);
        self.opacity_percent = self.opacity_percent.clamp(5, 100);
        self.background_opacity_percent = self.background_opacity_percent.min(100);
        self.max_notifications = self.max_notifications.clamp(1, 50);
        self.duration_ms = self.duration_ms.clamp(100, 600_000);
        self.animation_ms = self.animation_ms.clamp(10, 5000);
        self.font_size = self.font_size.clamp(6.0, 96.0);
        self.max_text_width = self.max_text_width.clamp(40, 4000);
        self.corner_radius = self.corner_radius.min(64);
        self.box_padding = self.box_padding.min(64);
        self.margin_x = self.margin_x.min(2000);
        self.margin_y = self.margin_y.min(2000);
        self.item_spacing_with_icons = self.item_spacing_with_icons.min(100);
        self.item_spacing_without_icons = self.item_spacing_without_icons.min(100);
        if parse_hex_color(&self.background_color).is_none() {
            self.background_color = Config::default().background_color;
        }
        if parse_hex_color(&self.text_color).is_none() {
            self.text_color = Config::default().text_color;
        }
        if self.font_family.trim().is_empty() {
            self.font_family = Config::default().font_family;
        }
        for h in &mut self.highlights {
            if parse_hex_color(&h.background_color).is_none() {
                h.background_color = HighlightRule::default().background_color;
            }
            if parse_hex_color(&h.text_color).is_none() {
                h.text_color = HighlightRule::default().text_color;
            }
        }
        self
    }

    /// Is this item hidden by one of the `hidden_items` rules?
    pub fn is_hidden(&self, item_name: &str) -> bool {
        let name = item_name.to_lowercase();
        self.hidden_items.iter().any(|p| matches_lower(&name, p))
    }

    /// The first highlight rule that matches this item, if any.
    pub fn highlight_for(&self, item_name: &str) -> Option<&HighlightRule> {
        let name = item_name.to_lowercase();
        self.highlights.iter().find(|h| matches_lower(&name, &h.contains))
    }

    pub fn scale(&self) -> f32 {
        self.scale_percent as f32 / 100.0
    }

    pub fn chatlog_dir(&self) -> PathBuf {
        if self.chatlog_dir.trim().is_empty() {
            crate::paths::default_chatlog_dir()
        } else {
            PathBuf::from(self.chatlog_dir.trim())
        }
    }

    pub fn item_spacing(&self) -> u32 {
        if self.show_icons { self.item_spacing_with_icons } else { self.item_spacing_without_icons }
    }
}

/// Case-insensitive "contains". `name_lower` must already be lowercase. Blank patterns never match.
pub fn matches_lower(name_lower: &str, pattern: &str) -> bool {
    !pattern.trim().is_empty() && name_lower.contains(&pattern.to_lowercase())
}

/// Parse "#RRGGBB", "RRGGBB" or "#RGB" into RGB bytes.
pub fn parse_hex_color(s: &str) -> Option<[u8; 3]> {
    let s = s.trim().trim_start_matches('#');
    let hex = |a: &str| u8::from_str_radix(a, 16).ok();
    match s.len() {
        6 => Some([hex(&s[0..2])?, hex(&s[2..4])?, hex(&s[4..6])?]),
        3 => {
            let r = hex(&s[0..1])?;
            let g = hex(&s[1..2])?;
            let b = hex(&s[2..3])?;
            Some([r * 17, g * 17, b * 17])
        }
        _ => None,
    }
}

pub fn format_hex_color(c: [u8; 3]) -> String {
    format!("#{:02X}{:02X}{:02X}", c[0], c[1], c[2])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_round_trip() {
        let c = Config::default();
        let text = toml::to_string_pretty(&c).unwrap();
        let back: Config = toml::from_str(&text).unwrap();
        assert_eq!(c, back);
    }

    #[test]
    fn partial_file_fills_defaults() {
        let c: Config = toml::from_str("corner = \"bottom_left\"\nfont_size = 20.0\n").unwrap();
        assert_eq!(c.corner, Corner::BottomLeft);
        assert_eq!(c.font_size, 20.0);
        assert_eq!(c.duration_ms, 1200);
        assert_eq!(c.mode, GroupMode::Combine);
    }

    #[test]
    fn hide_and_highlight_rules() {
        let c = Config::default();
        assert!(c.is_hidden("Bottle of Water"));
        assert!(c.is_hidden("Bottle Of Soy Milk"));
        assert!(!c.is_hidden("Empty Bottle"));
        assert!(c.highlight_for("Trophy Rat Skin").is_some());
        assert!(c.highlight_for("Work Order for Trophy Bear Skins (Skinning)").is_some());
        assert!(c.highlight_for("Rat Skin").is_none());

        let c = Config {
            hidden_items: vec!["  ".into(), "".into()],
            highlights: vec![
                HighlightRule { contains: "".into(), ..HighlightRule::default() },
                HighlightRule {
                    contains: "SKIN".into(),
                    background_color: "#FF0000".into(),
                    ..HighlightRule::default()
                },
                HighlightRule {
                    contains: "rat".into(),
                    background_color: "#00FF00".into(),
                    ..HighlightRule::default()
                },
            ],
            ..Config::default()
        };
        assert!(!c.is_hidden("Anything"), "blank patterns never match");
        assert_eq!(
            c.highlight_for("Trophy Rat Skin").unwrap().background_color,
            "#FF0000",
            "first rule wins"
        );
        assert_eq!(c.highlight_for("Pirate Hat").unwrap().background_color, "#00FF00");
    }

    #[test]
    fn rules_round_trip_through_toml() {
        let mut c = Config::default();
        c.highlights.push(HighlightRule { contains: "Mushroom".into(), ..HighlightRule::default() });
        c.hidden_items.push("Rotten".into());
        let text = toml::to_string_pretty(&c).unwrap();
        let back: Config = toml::from_str(&text).unwrap();
        assert_eq!(c, back);
        // Old config files without the new keys get the defaults.
        let old: Config = toml::from_str("corner = \"bottom_left\"\n").unwrap();
        assert_eq!(old.hidden_items, vec!["bottle of".to_string()]);
        assert_eq!(old.highlights[0].contains, "trophy");
    }

    #[test]
    fn hex_colors() {
        assert_eq!(parse_hex_color("#1C212E"), Some([0x1c, 0x21, 0x2e]));
        assert_eq!(parse_hex_color("fff"), Some([255, 255, 255]));
        assert_eq!(parse_hex_color("#12345"), None);
        assert_eq!(format_hex_color([0x1c, 0x21, 0x2e]), "#1C212E");
    }
}
