//! The notifier process: tray icon, message loop, chat log watching and the overlay.

use super::overlay::Overlay;
use super::tray::{self, Tray};
use super::{IpcMessages, MSG_WINDOW_CLASS, SETTINGS_TITLE, ipc_messages, wide};
use pg_pickup_notifier::chatlog::{Pickup, spawn_tailer};
use pg_pickup_notifier::config::Config;
use pg_pickup_notifier::font::FontSystem;
use pg_pickup_notifier::notify::Notifier;
use pg_pickup_notifier::render::{Entry, Painter};
use pg_pickup_notifier::resolver::{DbSlot, Resolved, spawn_db_loader, spawn_resolver};
use pg_pickup_notifier::{log, paths};
use std::cell::RefCell;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, mpsc};
use std::time::{Duration, Instant, SystemTime};
use windows_sys::Win32::Foundation::*;
use windows_sys::Win32::Graphics::Dwm::DwmFlush;
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::System::Threading::CreateMutexW;
use windows_sys::Win32::UI::WindowsAndMessaging::*;

const WM_RESOLVED: u32 = WM_APP + 1;
const WM_FRAME: u32 = WM_APP + 2;
const TIMER_WAKE: usize = 1;
const TIMER_CONFIG: usize = 2;

/// Wakes the UI thread about once per display refresh while something is animating.
struct Pacer {
    active: Arc<(Mutex<bool>, Condvar)>,
    pending: Arc<AtomicBool>,
}

impl Pacer {
    fn new(hwnd: HWND) -> Pacer {
        let active = Arc::new((Mutex::new(false), Condvar::new()));
        let pending = Arc::new(AtomicBool::new(false));
        let (a, p) = (active.clone(), pending.clone());
        let hwnd = hwnd as usize;
        std::thread::Builder::new()
            .name("frame-pacer".into())
            .spawn(move || {
                let (lock, cv) = &*a;
                loop {
                    {
                        let mut on = lock.lock().unwrap();
                        while !*on {
                            on = cv.wait(on).unwrap();
                        }
                    }
                    let t0 = Instant::now();
                    // Waits for the next desktop composition (vsync). Fall back to a sleep if
                    // composition isn't available or returns immediately.
                    unsafe {
                        DwmFlush();
                    }
                    let spent = t0.elapsed();
                    if spent < Duration::from_millis(4) {
                        std::thread::sleep(Duration::from_millis(16) - spent);
                    }
                    if !p.swap(true, Ordering::SeqCst) {
                        unsafe {
                            PostMessageW(hwnd as HWND, WM_FRAME, 0, 0);
                        }
                    }
                }
            })
            .expect("spawn pacer");
        Pacer { active, pending }
    }

    fn set_active(&self, on: bool) {
        let (lock, cv) = &*self.active;
        let mut cur = lock.lock().unwrap();
        if *cur != on {
            *cur = on;
            cv.notify_all();
        }
    }
}

struct App {
    hwnd: HWND,
    cfg: Config,
    cfg_mtime: Option<SystemTime>,
    fonts: FontSystem,
    painter: Option<Painter>,
    notifier: Notifier,
    overlay: Overlay,
    tray: Tray,
    pacer: Pacer,
    resolved_rx: mpsc::Receiver<Resolved>,
    pickup_tx: mpsc::Sender<Pickup>,
    chat_dir: Arc<Mutex<PathBuf>>,
    start: Instant,
    ipc: IpcMessages,
    taskbar_created: u32,
    settings_child: Option<std::process::Child>,
    force_show_until: f64,
}

thread_local! {
    static APP: RefCell<Option<App>> = const { RefCell::new(None) };
}

fn with_app<R>(f: impl FnOnce(&mut App) -> R) -> Option<R> {
    APP.with(|cell| match cell.try_borrow_mut() {
        Ok(mut guard) => guard.as_mut().map(f),
        Err(_) => None,
    })
}

fn config_mtime() -> Option<SystemTime> {
    std::fs::metadata(paths::config_file()).and_then(|m| m.modified()).ok()
}

impl App {
    fn now(&self) -> f64 {
        self.start.elapsed().as_secs_f64()
    }

    fn rebuild_painter(&mut self) {
        self.painter = Painter::new(&self.cfg, &self.fonts);
        match &self.painter {
            Some(p) => log!("font: using {:?} at {}px", p.font_name, p.text.px),
            None => log!("font: no usable font found!"),
        }
        self.notifier.invalidate_layouts();
    }

    fn reload_config(&mut self) {
        let new = Config::load(&paths::config_file());
        self.cfg_mtime = config_mtime();
        if new == self.cfg {
            return;
        }
        log!("config: reloaded");
        if new.mode != self.cfg.mode {
            self.notifier.clear();
        }
        self.cfg = new;
        *self.chat_dir.lock().unwrap() = self.cfg.chatlog_dir();
        self.tray.update(self.cfg.paused);
        self.rebuild_painter();
        self.frame();
    }

    fn on_resolved(&mut self) {
        let now = self.now();
        let mut added = 0;
        while let Ok(r) = self.resolved_rx.try_recv() {
            if self.cfg.paused && now > self.force_show_until {
                continue;
            }
            // Hide/highlight rules are applied here so rule changes take effect at once.
            let Some(entry) = Entry::classified(&self.cfg, &r.name, r.quantity, r.icon) else {
                log!("hidden by rule: {}", r.name);
                continue;
            };
            self.notifier.push(&self.cfg, now, entry);
            added += 1;
        }
        if added > 0 {
            self.overlay.raise();
            self.frame();
        }
    }

    /// Advance the animation, redraw, and decide when we next need to wake up.
    fn frame(&mut self) {
        self.pacer.pending.store(false, Ordering::SeqCst);
        let now = self.now();
        let Some(painter) = self.painter.as_mut() else { return };
        self.notifier.tick(&self.cfg, painter, now);
        match self.notifier.compose(&self.cfg, painter) {
            Some(scene) => {
                let work = super::work_area(self.cfg.monitor);
                self.overlay.show(&scene, work);
            }
            None => self.overlay.hide(),
        }
        unsafe {
            if self.notifier.is_animating() {
                KillTimer(self.hwnd, TIMER_WAKE);
                self.pacer.set_active(true);
            } else {
                self.pacer.set_active(false);
                match self.notifier.next_deadline(&self.cfg) {
                    Some(t) => {
                        let ms = ((t - now).max(0.0) * 1000.0).ceil() as u32 + 1;
                        SetTimer(self.hwnd, TIMER_WAKE, ms.max(10), None);
                    }
                    None => {
                        KillTimer(self.hwnd, TIMER_WAKE);
                    }
                }
            }
        }
    }

    fn test(&mut self) {
        self.force_show_until = self.now() + 5.0;
        let tx = self.pickup_tx.clone();
        std::thread::spawn(move || {
            for (name, qty, _) in pg_pickup_notifier::SAMPLE_ITEMS.iter().skip(1) {
                let _ = tx.send(Pickup { name: name.to_string(), quantity: *qty });
                std::thread::sleep(Duration::from_millis(180));
            }
        });
    }

    fn open_settings(&mut self) {
        if let Some(child) = self.settings_child.as_mut()
            && matches!(child.try_wait(), Ok(None))
        {
            let hwnd = unsafe { FindWindowW(std::ptr::null(), wide(SETTINGS_TITLE).as_ptr()) };
            if !hwnd.is_null() {
                unsafe {
                    ShowWindow(hwnd, SW_RESTORE);
                    SetForegroundWindow(hwnd);
                }
            }
            return;
        }
        match std::env::current_exe()
            .and_then(|exe| std::process::Command::new(exe).arg("--settings").spawn())
        {
            Ok(child) => self.settings_child = Some(child),
            Err(e) => log!("could not open settings: {e}"),
        }
    }

    fn toggle_pause(&mut self) {
        self.cfg.paused = !self.cfg.paused;
        // Save only the pause flag on top of what's on disk.
        let mut disk = Config::load(&paths::config_file());
        disk.paused = self.cfg.paused;
        if let Err(e) = disk.save(&paths::config_file()) {
            log!("config: save failed: {e}");
        }
        self.cfg_mtime = config_mtime();
        self.tray.update(self.cfg.paused);
        if self.cfg.paused {
            self.notifier.clear();
            self.frame();
        }
    }
}

enum Action {
    None,
    Menu(bool),
    Command(usize),
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match msg {
        WM_RESOLVED => {
            with_app(|a| a.on_resolved());
            0
        }
        WM_FRAME => {
            if with_app(|a| a.frame()).is_none() {
                // Busy (e.g. inside the tray menu); try again shortly.
                unsafe { SetTimer(hwnd, TIMER_WAKE, 16, None) };
            }
            0
        }
        WM_TIMER => {
            match wp {
                TIMER_WAKE => {
                    unsafe { KillTimer(hwnd, TIMER_WAKE) };
                    with_app(|a| a.frame());
                }
                TIMER_CONFIG => {
                    with_app(|a| {
                        if config_mtime() != a.cfg_mtime {
                            a.reload_config();
                        }
                        // Reap a closed settings window.
                        if let Some(c) = a.settings_child.as_mut()
                            && !matches!(c.try_wait(), Ok(None))
                        {
                            a.settings_child = None;
                        }
                    });
                }
                _ => {}
            }
            0
        }
        tray::WM_TRAY => {
            let action = match (lp as u32) & 0xFFFF {
                WM_LBUTTONUP | WM_LBUTTONDBLCLK => Action::Command(tray::ID_SETTINGS),
                WM_RBUTTONUP | WM_CONTEXTMENU => {
                    with_app(|a| Action::Menu(a.cfg.paused)).unwrap_or(Action::None)
                }
                _ => Action::None,
            };
            let cmd = match action {
                Action::Menu(paused) => Tray::show_menu(hwnd, paused),
                Action::Command(c) => c,
                Action::None => 0,
            };
            if cmd != 0 {
                unsafe { PostMessageW(hwnd, WM_COMMAND, cmd, 0) };
            }
            0
        }
        WM_COMMAND => {
            match wp & 0xFFFF {
                tray::ID_SETTINGS => {
                    with_app(|a| a.open_settings());
                }
                tray::ID_TEST => {
                    with_app(|a| a.test());
                }
                tray::ID_PAUSE => {
                    with_app(|a| a.toggle_pause());
                }
                tray::ID_OPEN_LOGS => {
                    if let Some(dir) = with_app(|a| a.cfg.chatlog_dir()) {
                        super::open_folder(&dir);
                    }
                }
                tray::ID_EXIT => unsafe {
                    DestroyWindow(hwnd);
                },
                _ => {}
            }
            0
        }
        WM_DESTROY => {
            with_app(|a| {
                a.tray.remove();
                a.overlay.hide();
            });
            unsafe { PostQuitMessage(0) };
            0
        }
        WM_DISPLAYCHANGE => {
            with_app(|a| a.frame());
            0
        }
        _ => {
            // Registered (runtime-numbered) messages.
            let handled = with_app(|a| {
                if msg == a.ipc.reload {
                    a.reload_config();
                    true
                } else if msg == a.ipc.test {
                    a.reload_config();
                    a.test();
                    true
                } else if msg == a.ipc.open_settings {
                    a.open_settings();
                    true
                } else if msg == a.taskbar_created && msg != 0 {
                    a.tray.add(a.cfg.paused);
                    true
                } else {
                    false
                }
            });
            if handled == Some(true) { 0 } else { unsafe { DefWindowProcW(hwnd, msg, wp, lp) } }
        }
    }
}

pub fn run() {
    pg_pickup_notifier::logger::init("notifier");
    let ipc = ipc_messages();

    // One instance only. A second launch opens the settings of the running one.
    unsafe {
        let name = wide("Local\\PGPickupNotifier.Instance");
        let mutex = CreateMutexW(std::ptr::null(), 1, name.as_ptr());
        if !mutex.is_null() && GetLastError() == ERROR_ALREADY_EXISTS {
            log!("already running; asking it to open settings");
            super::post_to_notifier(ipc.open_settings);
            return;
        }
        // (The handle is intentionally kept open for the life of the process.)
    }

    let cfg = Config::load(&paths::config_file());
    if !paths::config_file().exists()
        && let Err(e) = cfg.save(&paths::config_file())
    {
        log!("config: could not write defaults: {e}");
    }
    log!("config: {}", paths::config_file().display());
    log!("cache: {}", paths::cache_dir().display());

    unsafe {
        let hinst = GetModuleHandleW(std::ptr::null());
        let class = wide(MSG_WINDOW_CLASS);
        let wc = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            style: 0,
            lpfnWndProc: Some(wndproc),
            cbClsExtra: 0,
            cbWndExtra: 0,
            hInstance: hinst,
            hIcon: std::ptr::null_mut(),
            hCursor: std::ptr::null_mut(),
            hbrBackground: std::ptr::null_mut(),
            lpszMenuName: std::ptr::null(),
            lpszClassName: class.as_ptr(),
            hIconSm: std::ptr::null_mut(),
        };
        RegisterClassExW(&wc);
        // A hidden top-level window (not message-only, so it receives the TaskbarCreated broadcast).
        let hwnd = CreateWindowExW(
            WS_EX_TOOLWINDOW,
            class.as_ptr(),
            wide("PG Pickup Notifier").as_ptr(),
            WS_POPUP,
            0,
            0,
            0,
            0,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            hinst,
            std::ptr::null(),
        );
        if hwnd.is_null() {
            log!("could not create message window");
            return;
        }

        // Background work: chat log → resolver → UI.
        let (pickup_tx, pickup_rx) = mpsc::channel::<Pickup>();
        let (resolved_tx, resolved_rx) = mpsc::channel::<Resolved>();
        let (poke_tx, poke_rx) = mpsc::channel::<()>();
        let slot = DbSlot::default();
        spawn_db_loader(paths::cache_dir(), slot.clone(), poke_rx);
        let hw = hwnd as usize;
        spawn_resolver(paths::icon_cache_dir(), slot, pickup_rx, resolved_tx, poke_tx, move || {
            PostMessageW(hw as HWND, WM_RESOLVED, 0, 0);
        });
        let chat_dir = Arc::new(Mutex::new(cfg.chatlog_dir()));
        spawn_tailer(chat_dir.clone(), pickup_tx.clone(), || {});

        let fonts = FontSystem::new();
        let mut tray = Tray::new(hwnd);
        tray.add(cfg.paused);
        let app = App {
            hwnd,
            cfg_mtime: config_mtime(),
            painter: Painter::new(&cfg, &fonts),
            cfg,
            fonts,
            notifier: Notifier::new(),
            overlay: Overlay::new(hinst),
            tray,
            pacer: Pacer::new(hwnd),
            resolved_rx,
            pickup_tx,
            chat_dir,
            start: Instant::now(),
            ipc,
            taskbar_created: RegisterWindowMessageW(wide("TaskbarCreated").as_ptr()),
            settings_child: None,
            force_show_until: -1.0,
        };
        if let Some(p) = &app.painter {
            log!("font: using {:?} at {}px", p.font_name, p.text.px);
        }
        APP.with(|cell| *cell.borrow_mut() = Some(app));
        SetTimer(hwnd, TIMER_CONFIG, 1000, None);

        let mut msg: MSG = std::mem::zeroed();
        while GetMessageW(&mut msg, std::ptr::null_mut(), 0, 0) > 0 {
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
        APP.with(|cell| cell.borrow_mut().take());
        log!("exit");
    }
}
