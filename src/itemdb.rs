//! Item data (items.json) and item icons from the Project Gorgon CDN, with a disk cache.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

pub const VERSION_URL: &str = "https://client.projectgorgon.com/fileversion.txt";

pub fn items_url(version: u32) -> String {
    format!("https://cdn.projectgorgon.com/v{version}/data/items.json")
}

pub fn icon_url(version: u32, icon_id: u32) -> String {
    format!("https://cdn.projectgorgon.com/v{version}/icons/icon_{icon_id}.png")
}

/// An item icon as straight (non-premultiplied) RGBA8.
#[derive(Debug)]
pub struct Icon {
    pub id: u32,
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

impl Icon {
    pub fn decode_png(id: u32, bytes: &[u8]) -> Option<Icon> {
        let img = image::load_from_memory_with_format(bytes, image::ImageFormat::Png).ok()?;
        let rgba = img.to_rgba8();
        Some(Icon { id, width: rgba.width(), height: rgba.height(), rgba: rgba.into_raw() })
    }
}

fn agent() -> ureq::Agent {
    use ureq::tls::{RootCerts, TlsConfig};
    ureq::Agent::config_builder()
        // Trust the OS certificate store (works behind antivirus/corporate TLS inspection too).
        .tls_config(TlsConfig::builder().root_certs(RootCerts::PlatformVerifier).build())
        .timeout_global(Some(Duration::from_secs(60)))
        .timeout_connect(Some(Duration::from_secs(10)))
        .user_agent(concat!("pg-pickup-notifier/", env!("CARGO_PKG_VERSION")))
        .build()
        .new_agent()
}

pub fn http_get(url: &str, limit: u64) -> Result<Vec<u8>, String> {
    let mut resp = agent().get(url).call().map_err(|e| format!("{url}: {e}"))?;
    resp.body_mut().with_config().limit(limit).read_to_vec().map_err(|e| format!("{url}: {e}"))
}

/// Ask the game's server for the current data version.
pub fn fetch_version() -> Result<u32, String> {
    let bytes = http_get(VERSION_URL, 64 * 1024)?;
    let text = String::from_utf8_lossy(&bytes);
    text.trim().parse::<u32>().map_err(|_| format!("unexpected version text {:?}", text.trim()))
}

/// The newest `items_v<N>.json` in the cache directory, if any.
pub fn newest_cached_version(cache_dir: &Path) -> Option<u32> {
    std::fs::read_dir(cache_dir)
        .ok()?
        .filter_map(|e| e.ok())
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            name.strip_prefix("items_v")?.strip_suffix(".json")?.parse::<u32>().ok()
        })
        .max()
}

fn items_cache_path(cache_dir: &Path, version: u32) -> PathBuf {
    cache_dir.join(format!("items_v{version}.json"))
}

fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("part");
    std::fs::write(&tmp, bytes)?;
    std::fs::rename(&tmp, path)
}

#[derive(serde::Deserialize)]
struct RawItem {
    #[serde(rename = "Name")]
    name: Option<String>,
    #[serde(rename = "IconId")]
    icon_id: Option<u32>,
}

/// Item name → icon id lookup.
pub struct ItemDb {
    pub version: u32,
    by_name: HashMap<String, u32>,
    by_lower: HashMap<String, u32>,
}

impl ItemDb {
    pub fn parse(version: u32, json: &[u8]) -> Result<ItemDb, String> {
        let raw: HashMap<String, RawItem> =
            serde_json::from_slice(json).map_err(|e| format!("items.json: {e}"))?;
        // Sort by numeric id so duplicate names resolve the same way every time.
        let mut items: Vec<(u64, RawItem)> = raw
            .into_iter()
            .map(|(k, v)| (k.trim_start_matches("item_").parse::<u64>().unwrap_or(u64::MAX), v))
            .collect();
        items.sort_by_key(|(id, _)| *id);
        let mut by_name = HashMap::with_capacity(items.len());
        let mut by_lower = HashMap::with_capacity(items.len());
        for (_, item) in items {
            if let (Some(name), Some(icon)) = (item.name, item.icon_id) {
                by_lower.entry(name.to_lowercase()).or_insert(icon);
                by_name.entry(name).or_insert(icon);
            }
        }
        Ok(ItemDb { version, by_name, by_lower })
    }

    pub fn len(&self) -> usize {
        self.by_name.len()
    }

    pub fn is_empty(&self) -> bool {
        self.by_name.is_empty()
    }

    /// All distinct item names.
    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.by_name.keys().map(|s| s.as_str())
    }

    /// The newest cached items.json, without touching the network.
    pub fn load_cached(cache_dir: &Path) -> Option<ItemDb> {
        let v = newest_cached_version(cache_dir)?;
        let bytes = std::fs::read(items_cache_path(cache_dir, v)).ok()?;
        ItemDb::parse(v, &bytes).ok()
    }

    pub fn icon_for(&self, name: &str) -> Option<u32> {
        self.by_name.get(name).or_else(|| self.by_lower.get(&name.to_lowercase())).copied()
    }

    /// Load from the cache, downloading first if this version isn't cached yet.
    pub fn load_or_download(cache_dir: &Path, version: u32) -> Result<ItemDb, String> {
        let path = items_cache_path(cache_dir, version);
        if let Ok(bytes) = std::fs::read(&path) {
            match ItemDb::parse(version, &bytes) {
                Ok(db) => return Ok(db),
                Err(e) => crate::log!("itemdb: cached file is bad ({e}); downloading again"),
            }
        }
        crate::log!("itemdb: downloading items.json v{version}");
        let bytes = http_get(&items_url(version), 512 * 1024 * 1024)?;
        let db = ItemDb::parse(version, &bytes)?;
        write_atomic(&path, &bytes).map_err(|e| format!("write {}: {e}", path.display()))?;
        // Remove older versions.
        if let Ok(rd) = std::fs::read_dir(cache_dir) {
            for e in rd.flatten() {
                let name = e.file_name().to_string_lossy().into_owned();
                if name.starts_with("items_v") && name.ends_with(".json") && e.path() != path {
                    let _ = std::fs::remove_file(e.path());
                }
            }
        }
        Ok(db)
    }

    /// Best effort: current version from the server, otherwise the newest cached copy.
    pub fn load_current(cache_dir: &Path) -> Result<ItemDb, String> {
        match fetch_version() {
            Ok(v) => match ItemDb::load_or_download(cache_dir, v) {
                Ok(db) => return Ok(db),
                Err(e) => crate::log!("itemdb: {e}"),
            },
            Err(e) => crate::log!("itemdb: version check failed: {e}"),
        }
        let v = newest_cached_version(cache_dir).ok_or("no network and no cached items.json")?;
        crate::log!("itemdb: using cached items.json v{v}");
        ItemDb::load_or_download(cache_dir, v)
    }
}

/// Icons on disk and in memory.
pub struct IconCache {
    dir: PathBuf,
    mem: HashMap<u32, Arc<Icon>>,
    failed: HashMap<u32, Instant>,
}

impl IconCache {
    pub fn new(dir: PathBuf) -> IconCache {
        IconCache { dir, mem: HashMap::new(), failed: HashMap::new() }
    }

    pub fn path_for(dir: &Path, id: u32) -> PathBuf {
        dir.join(format!("icon_{id}.png"))
    }

    /// Already in memory or on disk (no network).
    pub fn get_local(&mut self, id: u32) -> Option<Arc<Icon>> {
        if let Some(icon) = self.mem.get(&id) {
            return Some(icon.clone());
        }
        let bytes = std::fs::read(Self::path_for(&self.dir, id)).ok()?;
        let icon = Arc::new(Icon::decode_png(id, &bytes)?);
        self.mem.insert(id, icon.clone());
        Some(icon)
    }

    /// Should we try downloading this icon (not recently failed)?
    pub fn should_fetch(&self, id: u32) -> bool {
        self.failed.get(&id).map(|t| t.elapsed() > Duration::from_secs(300)).unwrap_or(true)
    }

    /// Download one icon to disk (no shared state – safe to call from many threads).
    pub fn download(dir: &Path, version: u32, id: u32) -> Result<(), String> {
        let bytes = http_get(&icon_url(version, id), 8 * 1024 * 1024)?;
        if Icon::decode_png(id, &bytes).is_none() {
            return Err(format!("icon {id}: not a PNG"));
        }
        write_atomic(&Self::path_for(dir, id), &bytes).map_err(|e| e.to_string())
    }

    pub fn mark_failed(&mut self, id: u32) {
        self.failed.insert(id, Instant::now());
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// Get icons for `ids`, downloading any that are missing in parallel.
    pub fn get_many(&mut self, version: Option<u32>, ids: &[u32]) -> HashMap<u32, Arc<Icon>> {
        let mut out = HashMap::new();
        let mut missing = Vec::new();
        for &id in ids {
            match self.get_local(id) {
                Some(icon) => {
                    out.insert(id, icon);
                }
                None => {
                    if version.is_some() && self.should_fetch(id) && !missing.contains(&id) {
                        missing.push(id);
                    }
                }
            }
        }
        if let (Some(version), false) = (version, missing.is_empty()) {
            let dir = self.dir.clone();
            let results: Vec<(u32, Result<(), String>)> = std::thread::scope(|s| {
                let handles: Vec<_> = missing
                    .iter()
                    .map(|&id| {
                        let dir = &dir;
                        s.spawn(move || (id, IconCache::download(dir, version, id)))
                    })
                    .collect();
                handles.into_iter().filter_map(|h| h.join().ok()).collect()
            });
            for (id, r) in results {
                match r {
                    Ok(()) => {
                        if let Some(icon) = self.get_local(id) {
                            out.insert(id, icon);
                        }
                    }
                    Err(e) => {
                        crate::log!("icons: {e}");
                        self.mark_failed(id);
                    }
                }
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_items_and_prefers_lowest_id() {
        let json = br#"{
            "item_200": {"Name": "Daisy", "IconId": 2, "Value": 1},
            "item_1": {"Name": "Empty Bottle", "IconId": 5009, "Keywords": ["X"]},
            "item_100": {"Name": "Daisy", "IconId": 1},
            "item_5": {"Description": "no name"}
        }"#;
        let db = ItemDb::parse(486, json).unwrap();
        assert_eq!(db.len(), 2);
        assert_eq!(db.icon_for("Empty Bottle"), Some(5009));
        assert_eq!(db.icon_for("empty bottle"), Some(5009));
        assert_eq!(db.icon_for("Daisy"), Some(1));
        assert_eq!(db.icon_for("Nope"), None);
    }

    #[test]
    fn real_items_file_if_present() {
        // Optional: PGPN_ITEMS_JSON=/path/to/items.json cargo test
        let Ok(path) = std::env::var("PGPN_ITEMS_JSON") else { return };
        let db = ItemDb::parse(486, &std::fs::read(path).unwrap()).unwrap();
        assert_eq!(db.icon_for("Live-Broiled Steak"), Some(6847));
        assert_eq!(db.icon_for("Alchemy: Gravel From Hell"), Some(4003));
        assert_eq!(db.icon_for("Tonguefish"), Some(6520));
        assert_eq!(db.icon_for("Shieldwright: Awesome Deathspike Shield"), Some(4003));
    }
}
