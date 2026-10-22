//! Notification-area (tray) icon and its menu.

use super::wide;
use windows_sys::Win32::Foundation::*;
use windows_sys::Win32::UI::Shell::*;
use windows_sys::Win32::UI::WindowsAndMessaging::*;

pub const WM_TRAY: u32 = WM_APP + 10;

pub const ID_SETTINGS: usize = 1001;
pub const ID_TEST: usize = 1002;
pub const ID_PAUSE: usize = 1003;
pub const ID_OPEN_LOGS: usize = 1004;
pub const ID_EXIT: usize = 1005;

pub struct Tray {
    hwnd: HWND,
    icon: HICON,
    added: bool,
}

fn copy_tip(dst: &mut [u16], s: &str) {
    let w: Vec<u16> = s.encode_utf16().take(dst.len() - 1).collect();
    dst[..w.len()].copy_from_slice(&w);
    dst[w.len()] = 0;
}

impl Tray {
    pub fn new(hwnd: HWND) -> Tray {
        let size = unsafe { GetSystemMetrics(SM_CXSMICON) }.clamp(16, 64) as u32;
        let icon = super::icon_from_rgba(&pg_pickup_notifier::appicon::rgba(size), size);
        Tray { hwnd, icon, added: false }
    }

    fn data(&self, paused: bool) -> NOTIFYICONDATAW {
        let mut nid: NOTIFYICONDATAW = unsafe { std::mem::zeroed() };
        nid.cbSize = std::mem::size_of::<NOTIFYICONDATAW>() as u32;
        nid.hWnd = self.hwnd;
        nid.uID = 1;
        nid.uFlags = NIF_ICON | NIF_MESSAGE | NIF_TIP;
        nid.uCallbackMessage = WM_TRAY;
        nid.hIcon = self.icon;
        let tip = if paused { "PG Pickup Notifier (paused)" } else { "PG Pickup Notifier" };
        copy_tip(&mut nid.szTip, tip);
        nid
    }

    /// Add the icon (again – also used after Explorer restarts).
    pub fn add(&mut self, paused: bool) {
        let nid = self.data(paused);
        unsafe {
            Shell_NotifyIconW(NIM_DELETE, &nid);
            self.added = Shell_NotifyIconW(NIM_ADD, &nid) != 0;
        }
    }

    pub fn update(&mut self, paused: bool) {
        let nid = self.data(paused);
        unsafe {
            if Shell_NotifyIconW(NIM_MODIFY, &nid) == 0 {
                self.added = Shell_NotifyIconW(NIM_ADD, &nid) != 0;
            }
        }
    }

    pub fn remove(&mut self) {
        if self.added {
            let nid = self.data(false);
            unsafe {
                Shell_NotifyIconW(NIM_DELETE, &nid);
            }
            self.added = false;
        }
    }

    /// Show the context menu at the cursor and return the chosen command (0 = none).
    pub fn show_menu(hwnd: HWND, paused: bool) -> usize {
        unsafe {
            let menu = CreatePopupMenu();
            let add = |id: usize, text: &str, flags: u32| {
                AppendMenuW(menu, MF_STRING | flags, id, wide(text).as_ptr());
            };
            add(ID_SETTINGS, "&Settings…", 0);
            add(ID_TEST, "Show &test notification", 0);
            add(ID_PAUSE, "&Pause notifications", if paused { MF_CHECKED } else { MF_UNCHECKED });
            AppendMenuW(menu, MF_SEPARATOR, 0, std::ptr::null());
            add(ID_OPEN_LOGS, "Open &chat log folder", 0);
            AppendMenuW(menu, MF_SEPARATOR, 0, std::ptr::null());
            add(ID_EXIT, "E&xit", 0);
            SetMenuDefaultItem(menu, ID_SETTINGS as u32, 0);

            let mut pt = POINT { x: 0, y: 0 };
            GetCursorPos(&mut pt);
            // Required so the menu closes when clicking elsewhere.
            SetForegroundWindow(hwnd);
            let cmd = TrackPopupMenu(
                menu,
                TPM_RETURNCMD | TPM_NONOTIFY | TPM_RIGHTBUTTON | TPM_BOTTOMALIGN,
                pt.x,
                pt.y,
                0,
                hwnd,
                std::ptr::null(),
            );
            PostMessageW(hwnd, WM_NULL, 0, 0);
            DestroyMenu(menu);
            cmd as usize
        }
    }
}

impl Drop for Tray {
    fn drop(&mut self) {
        self.remove();
        if !self.icon.is_null() {
            unsafe {
                DestroyIcon(self.icon);
            }
        }
    }
}
