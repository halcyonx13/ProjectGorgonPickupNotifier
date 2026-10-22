//! Turns raw pickups into displayable entries (name + icon), off the UI thread.

use crate::chatlog::Pickup;
use crate::itemdb::{Icon, IconCache, ItemDb, fetch_version, newest_cached_version};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, RwLock, mpsc};
use std::time::{Duration, Instant};

#[derive(Debug, Clone)]
pub struct Resolved {
    pub name: String,
    pub quantity: u32,
    pub icon: Option<Arc<Icon>>,
}

/// Shared, hot-swappable item database.
#[derive(Clone, Default)]
pub struct DbSlot {
    db: Arc<RwLock<Option<Arc<ItemDb>>>>,
    loading: Arc<AtomicBool>,
}

impl DbSlot {
    pub fn get(&self) -> Option<Arc<ItemDb>> {
        self.db.read().unwrap().clone()
    }
    fn set(&self, db: ItemDb) {
        crate::log!("itemdb: v{} ready ({} item names)", db.version, db.len());
        *self.db.write().unwrap() = Some(Arc::new(db));
    }
    pub fn is_loading(&self) -> bool {
        self.loading.load(Ordering::SeqCst)
    }
}

/// Keeps items.json current: loads the cached copy at once, then checks the server
/// version at startup, every 30 minutes, and when an unknown item name shows up.
pub fn spawn_db_loader(cache_dir: PathBuf, slot: DbSlot, poke: mpsc::Receiver<()>) {
    slot.loading.store(true, Ordering::SeqCst);
    std::thread::Builder::new()
        .name("itemdb".into())
        .spawn(move || {
            if let Some(v) = newest_cached_version(&cache_dir) {
                match ItemDb::load_or_download(&cache_dir, v) {
                    Ok(db) => slot.set(db),
                    Err(e) => crate::log!("itemdb: {e}"),
                }
            }
            let mut last_check: Instant;
            loop {
                slot.loading.store(true, Ordering::SeqCst);
                last_check = Instant::now();
                match fetch_version() {
                    Ok(v) => {
                        let have = slot.get().map(|d| d.version);
                        if have != Some(v) {
                            match ItemDb::load_or_download(&cache_dir, v) {
                                Ok(db) => slot.set(db),
                                Err(e) => crate::log!("itemdb: {e}"),
                            }
                        }
                    }
                    Err(e) => crate::log!("itemdb: version check failed: {e}"),
                }
                slot.loading.store(false, Ordering::SeqCst);

                // Sleep until the next scheduled check, or until poked (rate-limited).
                let interval =
                    if slot.get().is_some() { Duration::from_secs(30 * 60) } else { Duration::from_secs(60) };
                let deadline = Instant::now() + interval;
                loop {
                    let wait = deadline.saturating_duration_since(Instant::now());
                    if wait.is_zero() {
                        break;
                    }
                    match poke.recv_timeout(wait) {
                        Ok(()) => {
                            let recent = last_check.elapsed() < Duration::from_secs(300);
                            if !recent {
                                break;
                            }
                        }
                        Err(mpsc::RecvTimeoutError::Timeout) => break,
                        Err(mpsc::RecvTimeoutError::Disconnected) => return,
                    }
                }
            }
        })
        .expect("spawn itemdb thread");
}

/// Resolve pickups in batches so that many simultaneous pickups arrive together.
pub fn spawn_resolver(
    icon_dir: PathBuf,
    slot: DbSlot,
    rx: mpsc::Receiver<Pickup>,
    tx: mpsc::Sender<Resolved>,
    poke_db: mpsc::Sender<()>,
    wake: impl Fn() + Send + 'static,
) {
    std::thread::Builder::new()
        .name("resolver".into())
        .spawn(move || {
            let mut icons = IconCache::new(icon_dir);
            while let Ok(first) = rx.recv() {
                let mut batch = vec![first];
                // Lines written together are usually picked up in one poll; give stragglers a moment.
                std::thread::sleep(Duration::from_millis(5));
                while let Ok(p) = rx.try_recv() {
                    batch.push(p);
                }
                // On a very first run the item list may still be downloading.
                let start = Instant::now();
                while slot.get().is_none() && slot.is_loading() && start.elapsed() < Duration::from_secs(8) {
                    std::thread::sleep(Duration::from_millis(100));
                }
                let db = slot.get();
                let ids: Vec<Option<u32>> =
                    batch.iter().map(|p| db.as_ref().and_then(|d| d.icon_for(&p.name))).collect();
                if db.is_some() && ids.iter().any(|i| i.is_none()) {
                    for (p, id) in batch.iter().zip(&ids) {
                        if id.is_none() {
                            crate::log!("resolver: unknown item {:?}", p.name);
                        }
                    }
                    let _ = poke_db.send(());
                }
                let wanted: Vec<u32> = ids.iter().flatten().copied().collect();
                let got = icons.get_many(db.as_ref().map(|d| d.version), &wanted);
                for (p, id) in batch.into_iter().zip(ids) {
                    let icon = id.and_then(|id| got.get(&id).cloned());
                    if tx.send(Resolved { name: p.name, quantity: p.quantity, icon }).is_err() {
                        return;
                    }
                }
                wake();
            }
        })
        .expect("spawn resolver thread");
}
