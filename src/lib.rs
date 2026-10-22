//! PG Pickup Notifier – core (platform-independent) parts.

pub mod logger;

pub mod appicon;
pub mod chatlog;
pub mod config;
pub mod font;
pub mod itemdb;
pub mod notify;
pub mod paths;
pub mod render;
pub mod resolver;

/// Items used for the "Test" button and the settings preview: (name, quantity, icon id), oldest first.
/// "Bottle of Water" and "Trophy Sheep Skin" show off the default hide and highlight rules.
pub const SAMPLE_ITEMS: &[(&str, u32, u32)] = &[
    ("Rubywall Crystal", 2, 5056),
    ("Tonguefish", 1, 6520),
    ("Live-Broiled Steak", 5, 6847),
    ("Bottle of Water", 1, 5010),
    ("Alchemy: Gravel From Hell", 1, 4003),
    ("Trophy Sheep Skin", 1, 5172),
    ("Awesome Deathspike Shield", 1, 14014),
];

/// Sample entries with `cfg`'s hide/highlight rules applied. If no sample matches a highlight
/// rule, an example item for the first rule is added so its colours can be seen.
pub fn sample_entries(
    cfg: &config::Config,
    icon: impl Fn(u32) -> Option<std::sync::Arc<itemdb::Icon>>,
) -> Vec<render::Entry> {
    let mut out: Vec<render::Entry> = SAMPLE_ITEMS
        .iter()
        .filter_map(|(name, qty, id)| render::Entry::classified(cfg, name, *qty, icon(*id)))
        .collect();
    if !out.iter().any(|e| e.highlight.is_some())
        && let Some(rule) = cfg.highlights.iter().find(|h| !h.contains.trim().is_empty())
    {
        let pattern = rule.contains.trim();
        let mut chars = pattern.chars();
        let first = chars.next().map(|c| c.to_uppercase().collect::<String>()).unwrap_or_default();
        let name = format!("{first}{} (example)", chars.as_str());
        if let Some(e) = render::Entry::classified(cfg, &name, 1, None) {
            out.push(e);
        }
    }
    out
}
