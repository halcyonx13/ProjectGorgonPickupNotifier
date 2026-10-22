//! Windows-specific parts: overlay window, tray icon, message loop and settings window.

pub mod app;
pub mod overlay;
pub mod settings;
pub mod tray;

use windows_sys::Win32::Foundation::*;
use windows_sys::Win32::Graphics::Gdi::*;
use windows_sys::Win32::System::Registry::*;
use windows_sys::Win32::UI::WindowsAndMessaging::*;
use windows_sys::core::BOOL;

/// Window class of the notifier's hidden message window (used to find the running instance).
pub const MSG_WINDOW_CLASS: &str = "PGPickupNotifier.MessageWindow";
pub const SETTINGS_TITLE: &str = "PG Pickup Notifier Settings";

pub fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// Messages the settings window sends to the running notifier.
pub struct IpcMessages {
    pub reload: u32,
    pub test: u32,
    pub open_settings: u32,
}

pub fn ipc_messages() -> IpcMessages {
    unsafe {
        IpcMessages {
            reload: RegisterWindowMessageW(wide("PGPickupNotifier.Reload").as_ptr()),
            test: RegisterWindowMessageW(wide("PGPickupNotifier.Test").as_ptr()),
            open_settings: RegisterWindowMessageW(wide("PGPickupNotifier.OpenSettings").as_ptr()),
        }
    }
}

pub fn find_notifier_window() -> HWND {
    unsafe { FindWindowW(wide(MSG_WINDOW_CLASS).as_ptr(), std::ptr::null()) }
}

/// Send a registered message to the running notifier. Returns false if it isn't running.
pub fn post_to_notifier(msg: u32) -> bool {
    let hwnd = find_notifier_window();
    if hwnd.is_null() {
        return false;
    }
    unsafe { PostMessageW(hwnd, msg, 0, 0) != 0 }
}

#[derive(Clone, Copy)]
pub struct MonitorInfo {
    pub work: RECT,
    pub full: RECT,
    pub primary: bool,
}

unsafe extern "system" fn monitor_enum(hmon: HMONITOR, _hdc: HDC, _rc: *mut RECT, data: LPARAM) -> BOOL {
    let list = unsafe { &mut *(data as *mut Vec<MonitorInfo>) };
    let mut mi: MONITORINFO = unsafe { std::mem::zeroed() };
    mi.cbSize = std::mem::size_of::<MONITORINFO>() as u32;
    if unsafe { GetMonitorInfoW(hmon, &mut mi) } != 0 {
        list.push(MonitorInfo {
            work: mi.rcWork,
            full: mi.rcMonitor,
            primary: mi.dwFlags & MONITORINFOF_PRIMARY != 0,
        });
    }
    1
}

pub fn monitors() -> Vec<MonitorInfo> {
    let mut list: Vec<MonitorInfo> = Vec::new();
    unsafe {
        EnumDisplayMonitors(
            std::ptr::null_mut(),
            std::ptr::null(),
            Some(monitor_enum),
            &mut list as *mut Vec<MonitorInfo> as LPARAM,
        );
    }
    list
}

/// Work area (screen minus taskbar) of monitor `index` (0 = primary, 1.. = monitor number).
pub fn work_area(index: u32) -> RECT {
    let list = monitors();
    let chosen = if index == 0 { None } else { list.get(index as usize - 1) };
    if let Some(m) = chosen.or_else(|| list.iter().find(|m| m.primary)).or(list.first()) {
        return m.work;
    }
    let mut r = RECT { left: 0, top: 0, right: 1920, bottom: 1080 };
    unsafe {
        SystemParametersInfoW(SPI_GETWORKAREA, 0, &mut r as *mut RECT as *mut _, 0);
    }
    r
}

/// An HICON built from straight RGBA pixels.
pub fn icon_from_rgba(rgba: &[u8], size: u32) -> HICON {
    unsafe {
        let mut bmi: BITMAPINFO = std::mem::zeroed();
        bmi.bmiHeader.biSize = std::mem::size_of::<BITMAPINFOHEADER>() as u32;
        bmi.bmiHeader.biWidth = size as i32;
        bmi.bmiHeader.biHeight = -(size as i32);
        bmi.bmiHeader.biPlanes = 1;
        bmi.bmiHeader.biBitCount = 32;
        bmi.bmiHeader.biCompression = BI_RGB;
        let mut bits: *mut core::ffi::c_void = std::ptr::null_mut();
        let color =
            CreateDIBSection(std::ptr::null_mut(), &bmi, DIB_RGB_COLORS, &mut bits, std::ptr::null_mut(), 0);
        if color.is_null() || bits.is_null() {
            return std::ptr::null_mut();
        }
        let dst = std::slice::from_raw_parts_mut(bits as *mut u8, (size * size * 4) as usize);
        for (d, s) in dst.chunks_exact_mut(4).zip(rgba.chunks_exact(4)) {
            d[0] = s[2];
            d[1] = s[1];
            d[2] = s[0];
            d[3] = s[3];
        }
        let mask_bits = vec![0u8; ((size as usize).div_ceil(16) * 2) * size as usize];
        let mask = CreateBitmap(size as i32, size as i32, 1, 1, mask_bits.as_ptr() as *const _);
        let info = ICONINFO { fIcon: 1, xHotspot: 0, yHotspot: 0, hbmMask: mask, hbmColor: color };
        let icon = CreateIconIndirect(&info);
        DeleteObject(color);
        DeleteObject(mask);
        icon
    }
}

const RUN_KEY: &str = "Software\\Microsoft\\Windows\\CurrentVersion\\Run";
const RUN_VALUE: &str = "PG Pickup Notifier";

pub fn autostart_enabled() -> bool {
    let mut size: u32 = 0;
    unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            wide(RUN_KEY).as_ptr(),
            wide(RUN_VALUE).as_ptr(),
            RRF_RT_REG_SZ,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            &mut size,
        ) == ERROR_SUCCESS
    }
}

pub fn set_autostart(on: bool) -> bool {
    unsafe {
        if on {
            let Ok(exe) = std::env::current_exe() else { return false };
            let cmd = wide(&format!("\"{}\"", exe.display()));
            RegSetKeyValueW(
                HKEY_CURRENT_USER,
                wide(RUN_KEY).as_ptr(),
                wide(RUN_VALUE).as_ptr(),
                REG_SZ,
                cmd.as_ptr() as *const _,
                (cmd.len() * 2) as u32,
            ) == ERROR_SUCCESS
        } else {
            let r = RegDeleteKeyValueW(HKEY_CURRENT_USER, wide(RUN_KEY).as_ptr(), wide(RUN_VALUE).as_ptr());
            r == ERROR_SUCCESS || r == ERROR_FILE_NOT_FOUND
        }
    }
}

/// Open a folder in Explorer.
pub fn open_folder(path: &std::path::Path) {
    let _ = std::fs::create_dir_all(path);
    let _ = std::process::Command::new("explorer.exe").arg(path).spawn();
}
