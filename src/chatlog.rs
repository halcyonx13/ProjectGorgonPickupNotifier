//! Parsing and tailing of Project Gorgon chat logs.
//!
//! The game writes one file per day (`Chat-YY-MM-DD.log`). Pickup lines look like:
//!
//! ```text
//! 26-10-11 00:21:52	[Status] Live-Broiled Steak x5 added to inventory.
//! ```

use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, mpsc};
use std::time::{Duration, Instant};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pickup {
    pub name: String,
    pub quantity: u32,
}

const STATUS_PREFIX: &str = "[Status] ";
const PICKUP_SUFFIX: &str = " added to inventory.";

/// Parse one chat log line. Returns `Some` only for "added to inventory" status lines.
pub fn parse_pickup(line: &str) -> Option<Pickup> {
    let line = line.trim_end_matches(['\r', '\n']);
    let (_timestamp, rest) = line.split_once('\t')?;
    let body = rest.strip_prefix(STATUS_PREFIX)?.strip_suffix(PICKUP_SUFFIX)?;
    let (name, quantity) = match body.rsplit_once(" x") {
        Some((name, q)) if !q.is_empty() && q.bytes().all(|b| b.is_ascii_digit()) => {
            (name, q.parse::<u32>().unwrap_or(1))
        }
        _ => (body, 1),
    };
    let name = name.trim();
    if name.is_empty() {
        return None;
    }
    Some(Pickup { name: name.to_string(), quantity: quantity.max(1) })
}

/// Is this a daily chat log file name (`Chat-YY-MM-DD.log`)?
pub fn is_chatlog_name(name: &str) -> bool {
    let Some(mid) = name.strip_prefix("Chat-").and_then(|s| s.strip_suffix(".log")) else {
        return false;
    };
    let b = mid.as_bytes();
    b.len() == 8
        && b[2] == b'-'
        && b[5] == b'-'
        && b.iter().enumerate().all(|(i, c)| i == 2 || i == 5 || c.is_ascii_digit())
}

/// The newest daily log in `dir`. The YY-MM-DD names sort chronologically.
pub fn newest_log(dir: &Path) -> Option<PathBuf> {
    let entries = std::fs::read_dir(dir).ok()?;
    entries
        .filter_map(|e| e.ok())
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            is_chatlog_name(&name).then(|| (name, e.path()))
        })
        .max_by(|a, b| a.0.cmp(&b.0))
        .map(|(_, p)| p)
}

struct OpenLog {
    path: PathBuf,
    file: File,
    pos: u64,
}

/// Follows the newest chat log in a directory, switching files when a new day starts.
pub struct Tailer {
    dir: PathBuf,
    current: Option<OpenLog>,
    partial: Vec<u8>,
    started: bool,
    last_scan: Option<Instant>,
}

impl Tailer {
    pub fn new(dir: PathBuf) -> Tailer {
        Tailer { dir, current: None, partial: Vec::new(), started: false, last_scan: None }
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// Read any new lines and return the pickups found in them.
    ///
    /// The first call skips existing content (we only want new pickups). Files that
    /// appear later (a new day) are read from the beginning.
    pub fn poll(&mut self) -> Vec<Pickup> {
        let mut out = Vec::new();
        let rescan = self.current.is_none()
            || self.last_scan.map(|t| t.elapsed() >= Duration::from_secs(2)).unwrap_or(true);
        if rescan {
            self.last_scan = Some(Instant::now());
            let newest = newest_log(&self.dir);
            let first_scan = !self.started;
            self.started = true;
            match (&self.current, newest) {
                (Some(cur), Some(newest)) if cur.path != newest => {
                    // Finish the old file, then start the new one from the top.
                    self.read_new(&mut out);
                    crate::log!("chatlog: switching to {}", newest.display());
                    self.open(newest, false);
                }
                (None, Some(newest)) => {
                    crate::log!("chatlog: following {}", newest.display());
                    self.open(newest, first_scan);
                }
                _ => {}
            }
        }
        self.read_new(&mut out);
        out
    }

    fn open(&mut self, path: PathBuf, at_end: bool) {
        self.partial.clear();
        self.current = match File::open(&path) {
            Ok(file) => {
                let pos = if at_end { file.metadata().map(|m| m.len()).unwrap_or(0) } else { 0 };
                Some(OpenLog { path, file, pos })
            }
            Err(e) => {
                crate::log!("chatlog: cannot open {}: {e}", path.display());
                None
            }
        };
    }

    fn read_new(&mut self, out: &mut Vec<Pickup>) {
        let Some(cur) = self.current.as_mut() else { return };
        let len = match cur.file.metadata() {
            Ok(m) => m.len(),
            Err(_) => {
                self.current = None;
                return;
            }
        };
        if len < cur.pos {
            // File was truncated or replaced; start over.
            cur.pos = 0;
            self.partial.clear();
        }
        if len == cur.pos {
            return;
        }
        if cur.file.seek(SeekFrom::Start(cur.pos)).is_err() {
            return;
        }
        let mut buf = Vec::with_capacity((len - cur.pos).min(4 << 20) as usize);
        let n = match (&mut cur.file).take(len - cur.pos).read_to_end(&mut buf) {
            Ok(n) => n,
            Err(_) => return,
        };
        cur.pos += n as u64;
        self.partial.extend_from_slice(&buf);
        // Only complete lines; keep any trailing partial line for next time.
        if let Some(last_nl) = self.partial.iter().rposition(|&b| b == b'\n') {
            let complete: Vec<u8> = self.partial.drain(..=last_nl).collect();
            for line in complete.split(|&b| b == b'\n') {
                if line.is_empty() {
                    continue;
                }
                let line = String::from_utf8_lossy(line);
                if let Some(p) = parse_pickup(&line) {
                    out.push(p);
                }
            }
        }
    }
}

/// Run the tailer on its own thread. `dir` can be changed at any time by the UI.
pub fn spawn_tailer(dir: Arc<Mutex<PathBuf>>, tx: mpsc::Sender<Pickup>, wake: impl Fn() + Send + 'static) {
    std::thread::Builder::new()
        .name("chatlog-tailer".into())
        .spawn(move || {
            let mut tailer = Tailer::new(dir.lock().unwrap().clone());
            crate::log!("chatlog: watching {}", tailer.dir().display());
            loop {
                let wanted = dir.lock().unwrap().clone();
                if wanted != tailer.dir() {
                    crate::log!("chatlog: watching {}", wanted.display());
                    tailer = Tailer::new(wanted);
                }
                let pickups = tailer.poll();
                if !pickups.is_empty() {
                    for p in pickups {
                        if tx.send(p).is_err() {
                            return;
                        }
                    }
                    wake();
                }
                std::thread::sleep(Duration::from_millis(150));
            }
        })
        .expect("spawn tailer thread");
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn p(name: &str, q: u32) -> Option<Pickup> {
        Some(Pickup { name: name.into(), quantity: q })
    }

    #[test]
    fn parses_sample_lines() {
        assert_eq!(
            parse_pickup("26-10-11 00:21:52\t[Status] Live-Broiled Steak x5 added to inventory."),
            p("Live-Broiled Steak", 5)
        );
        assert_eq!(
            parse_pickup("26-10-11 00:25:20\t[Status] Alchemy: Gravel From Hell added to inventory.\r"),
            p("Alchemy: Gravel From Hell", 1)
        );
        assert_eq!(
            parse_pickup("26-10-11 00:30:05\t[Status] Tonguefish added to inventory."),
            p("Tonguefish", 1)
        );
        assert_eq!(
            parse_pickup(
                "26-10-11 00:41:16\t[Status] Shieldwright: Awesome Deathspike Shield added to inventory."
            ),
            p("Shieldwright: Awesome Deathspike Shield", 1)
        );
    }

    #[test]
    fn ignores_other_lines() {
        assert_eq!(parse_pickup("26-10-11 00:41:16\t[Global] Bob: Tonguefish added to inventory."), None);
        assert_eq!(parse_pickup("26-10-11 00:41:16\t[Status] You earned 5 XP."), None);
        assert_eq!(parse_pickup("garbage"), None);
        // "x" not followed by digits is part of the name
        assert_eq!(parse_pickup("26-10-11 00:41:16\t[Status] Box xyz added to inventory."), p("Box xyz", 1));
    }

    #[test]
    fn chatlog_names() {
        assert!(is_chatlog_name("Chat-26-10-11.log"));
        assert!(!is_chatlog_name("Chat-26-10-11.txt"));
        assert!(!is_chatlog_name("Chat-2026-10-11.log"));
        assert!(!is_chatlog_name("Chat-26-1a-11.log"));
    }

    #[test]
    fn tails_and_switches_days() {
        let dir = std::env::temp_dir().join(format!("pgpn-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let day1 = dir.join("Chat-26-10-10.log");
        let mut f1 = File::create(&day1).unwrap();
        writeln!(f1, "26-10-10 23:00:00\t[Status] Old Item added to inventory.").unwrap();
        f1.flush().unwrap();

        let mut t = Tailer::new(dir.clone());
        assert!(t.poll().is_empty(), "existing content is skipped");

        writeln!(f1, "26-10-10 23:59:59\t[Status] Tonguefish x3 added to inventory.").unwrap();
        write!(f1, "26-10-10 23:59:59\t[Status] Half").unwrap();
        f1.flush().unwrap();
        assert_eq!(t.poll(), vec![Pickup { name: "Tonguefish".into(), quantity: 3 }]);
        writeln!(f1, " Line added to inventory.").unwrap();
        f1.flush().unwrap();
        assert_eq!(t.poll(), vec![Pickup { name: "Half Line".into(), quantity: 1 }]);

        // New day: the new file is read from its beginning.
        writeln!(f1, "26-10-10 23:59:59\t[Status] Last Of Day added to inventory.").unwrap();
        f1.flush().unwrap();
        let mut f2 = File::create(dir.join("Chat-26-10-11.log")).unwrap();
        writeln!(f2, "26-10-11 00:00:01\t[Status] First Of Day added to inventory.").unwrap();
        f2.flush().unwrap();
        t.last_scan = None;
        let got: Vec<String> = t.poll().into_iter().map(|p| p.name).collect();
        assert_eq!(got, vec!["Last Of Day".to_string(), "First Of Day".to_string()]);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
