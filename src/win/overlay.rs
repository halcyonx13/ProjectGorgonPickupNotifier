//! A click-through, always-on-top, per-pixel-alpha window that shows the notification scene.

use super::wide;
use pg_pickup_notifier::config::Corner;
use pg_pickup_notifier::notify::Scene;
use windows_sys::Win32::Foundation::*;
use windows_sys::Win32::Graphics::Gdi::*;
use windows_sys::Win32::UI::WindowsAndMessaging::*;

const CLASS: &str = "PGPickupNotifier.Overlay";

pub struct Overlay {
    hwnd: HWND,
    mem_dc: HDC,
    dib: HBITMAP,
    old: HGDIOBJ,
    bits: *mut u8,
    dib_w: i32,
    dib_h: i32,
    visible: bool,
}

unsafe extern "system" fn overlay_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match msg {
        WM_NCHITTEST => HTTRANSPARENT as LRESULT,
        WM_MOUSEACTIVATE => MA_NOACTIVATE as LRESULT,
        _ => unsafe { DefWindowProcW(hwnd, msg, wp, lp) },
    }
}

impl Overlay {
    pub fn new(hinst: HINSTANCE) -> Overlay {
        unsafe {
            let class = wide(CLASS);
            let wc = WNDCLASSEXW {
                cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
                style: 0,
                lpfnWndProc: Some(overlay_proc),
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
            let hwnd = CreateWindowExW(
                WS_EX_LAYERED | WS_EX_TRANSPARENT | WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
                class.as_ptr(),
                wide("PG Pickup Notifier").as_ptr(),
                WS_POPUP,
                0,
                0,
                1,
                1,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                hinst,
                std::ptr::null(),
            );
            let mem_dc = CreateCompatibleDC(std::ptr::null_mut());
            Overlay {
                hwnd,
                mem_dc,
                dib: std::ptr::null_mut(),
                old: std::ptr::null_mut(),
                bits: std::ptr::null_mut(),
                dib_w: 0,
                dib_h: 0,
                visible: false,
            }
        }
    }

    /// Make sure the backing bitmap is at least w×h (it only ever grows).
    fn ensure_dib(&mut self, w: i32, h: i32) -> bool {
        if w <= self.dib_w && h <= self.dib_h && !self.bits.is_null() {
            return true;
        }
        let nw = w.max(self.dib_w);
        let nh = h.max(self.dib_h);
        unsafe {
            let mut bmi: BITMAPINFO = std::mem::zeroed();
            bmi.bmiHeader.biSize = std::mem::size_of::<BITMAPINFOHEADER>() as u32;
            bmi.bmiHeader.biWidth = nw;
            bmi.bmiHeader.biHeight = -nh; // top-down
            bmi.bmiHeader.biPlanes = 1;
            bmi.bmiHeader.biBitCount = 32;
            bmi.bmiHeader.biCompression = BI_RGB;
            let mut bits: *mut core::ffi::c_void = std::ptr::null_mut();
            let dib = CreateDIBSection(self.mem_dc, &bmi, DIB_RGB_COLORS, &mut bits, std::ptr::null_mut(), 0);
            if dib.is_null() || bits.is_null() {
                return false;
            }
            let prev = SelectObject(self.mem_dc, dib);
            if self.dib.is_null() {
                self.old = prev;
            } else {
                DeleteObject(self.dib);
            }
            self.dib = dib;
            self.bits = bits as *mut u8;
            self.dib_w = nw;
            self.dib_h = nh;
        }
        true
    }

    /// Show `scene` in the bottom corner of `work` (a monitor's work area).
    pub fn show(&mut self, scene: &Scene, work: RECT) {
        let w = scene.pixmap.width() as i32;
        let h = scene.pixmap.height() as i32;
        if w <= 0 || h <= 0 || !self.ensure_dib(w, h) {
            return;
        }
        // RGBA (premultiplied) → BGRA (premultiplied), into the top-left of the DIB.
        unsafe {
            let src = scene.pixmap.data();
            let stride = self.dib_w as usize * 4;
            for y in 0..h as usize {
                let d = std::slice::from_raw_parts_mut(self.bits.add(y * stride), w as usize * 4);
                let s = &src[y * w as usize * 4..(y + 1) * w as usize * 4];
                for (dp, sp) in d.chunks_exact_mut(4).zip(s.chunks_exact(4)) {
                    dp[0] = sp[2];
                    dp[1] = sp[1];
                    dp[2] = sp[0];
                    dp[3] = sp[3];
                }
            }
        }
        let x = match scene.corner {
            Corner::BottomRight => work.right - w,
            Corner::BottomLeft => work.left,
        };
        let y = work.bottom - scene.margin_y - h;
        let dst = POINT { x, y };
        let size = SIZE { cx: w, cy: h };
        let src = POINT { x: 0, y: 0 };
        let blend = BLENDFUNCTION {
            BlendOp: AC_SRC_OVER as u8,
            BlendFlags: 0,
            SourceConstantAlpha: 255,
            AlphaFormat: AC_SRC_ALPHA as u8,
        };
        unsafe {
            UpdateLayeredWindow(
                self.hwnd,
                std::ptr::null_mut(),
                &dst,
                &size,
                self.mem_dc,
                &src,
                0,
                &blend,
                ULW_ALPHA,
            );
            if !self.visible {
                ShowWindow(self.hwnd, SW_SHOWNOACTIVATE);
                self.visible = true;
                self.raise();
            }
        }
    }

    /// Re-assert always-on-top (games sometimes push themselves above us).
    pub fn raise(&self) {
        unsafe {
            SetWindowPos(
                self.hwnd,
                HWND_TOPMOST,
                0,
                0,
                0,
                0,
                SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | SWP_NOOWNERZORDER,
            );
        }
    }

    pub fn hide(&mut self) {
        if self.visible {
            unsafe {
                ShowWindow(self.hwnd, SW_HIDE);
            }
            self.visible = false;
        }
    }
}

impl Drop for Overlay {
    fn drop(&mut self) {
        unsafe {
            if !self.old.is_null() {
                SelectObject(self.mem_dc, self.old);
            }
            if !self.dib.is_null() {
                DeleteObject(self.dib);
            }
            DeleteDC(self.mem_dc);
            DestroyWindow(self.hwnd);
        }
    }
}
