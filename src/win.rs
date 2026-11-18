//! Thin Win32 / GDI helpers.

#![allow(clippy::missing_safety_doc)]

use std::ffi::c_void;
use std::ptr::{null, null_mut};

use windows_sys::Win32::Foundation::*;
use windows_sys::Win32::Graphics::Dwm::DwmSetWindowAttribute;
use windows_sys::Win32::Graphics::Gdi::*;
use windows_sys::Win32::System::DataExchange::*;
use windows_sys::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryW};
use windows_sys::Win32::System::Memory::*;
use windows_sys::Win32::UI::Shell::*;
use windows_sys::Win32::UI::WindowsAndMessaging::*;

pub fn wz(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

pub fn w(s: &str) -> Vec<u16> {
    s.encode_utf16().collect()
}

pub fn rect(l: i32, t: i32, r: i32, b: i32) -> RECT {
    RECT { left: l, top: t, right: r, bottom: b }
}

pub fn xywh(x: i32, y: i32, w: i32, h: i32) -> RECT {
    RECT { left: x, top: y, right: x + w, bottom: y + h }
}

pub fn contains(r: &RECT, x: i32, y: i32) -> bool {
    x >= r.left && x < r.right && y >= r.top && y < r.bottom
}

pub fn rw(r: &RECT) -> i32 {
    r.right - r.left
}

pub fn rh(r: &RECT) -> i32 {
    r.bottom - r.top
}

pub fn create_font(height: i32, weight: i32, face: &str) -> HFONT {
    let f = wz(face);
    unsafe { CreateFontW(-height, 0, 0, 0, weight, 0, 0, 0, DEFAULT_CHARSET as _, OUT_DEFAULT_PRECIS as _, CLIP_DEFAULT_PRECIS as _, CLEARTYPE_QUALITY as _, (DEFAULT_PITCH | FF_DONTCARE) as _, f.as_ptr()) }
}

pub fn font_exists(face: &str) -> bool {
    unsafe extern "system" fn cb(_lf: *const LOGFONTW, _tm: *const TEXTMETRICW, _ft: u32, found: LPARAM) -> i32 {
        *(found as *mut bool) = true;
        0
    }
    let mut lf: LOGFONTW = unsafe { std::mem::zeroed() };
    let name = w(face);
    for (i, c) in name.iter().take(31).enumerate() {
        lf.lfFaceName[i] = *c;
    }
    lf.lfCharSet = DEFAULT_CHARSET as _;
    let mut found = false;
    unsafe {
        let dc = GetDC(null_mut());
        EnumFontFamiliesExW(dc, &lf, Some(cb), &mut found as *mut bool as LPARAM, 0);
        ReleaseDC(null_mut(), dc);
    }
    found
}

pub struct Fonts {
    pub ui: HFONT,
    pub bold: HFONT,
    pub small: HFONT,
    pub mono: HFONT,
    pub icon: HFONT,
    pub icon_small: HFONT,
    pub icon_big: HFONT,
    pub big: HFONT,
    pub cap: HFONT,
    pub mono_small: HFONT,
    pub mono_small_h: i32,
    pub mono_w: i32,
    pub mono_h: i32,
}

impl Fonts {
    pub fn new(scale: f32) -> Fonts {
        let s = |v: f32| (v * scale).round() as i32;
        let icon_face = if font_exists("Segoe Fluent Icons") { "Segoe Fluent Icons" } else { "Segoe MDL2 Assets" };
        let mono = create_font(s(14.0), 400, "Consolas");
        let (mono_w, mono_h) = unsafe {
            let dc = GetDC(null_mut());
            let old = SelectObject(dc, mono as HGDIOBJ);
            let mut tm: TEXTMETRICW = std::mem::zeroed();
            GetTextMetricsW(dc, &mut tm);
            SelectObject(dc, old);
            ReleaseDC(null_mut(), dc);
            (tm.tmAveCharWidth, tm.tmHeight)
        };
        let mono_small = create_font(s(12.0), 400, "Consolas");
        Fonts {
            cap: create_font(s(11.0), 400, "Segoe UI"),
            mono_small,
            mono_small_h: s(17.0),
            ui: create_font(s(13.0), 400, "Segoe UI"),
            bold: create_font(s(11.0), 700, "Segoe UI"),
            small: create_font(s(12.0), 400, "Segoe UI"),
            mono,
            icon: create_font(s(16.0), 400, icon_face),
            icon_small: create_font(s(12.0), 400, icon_face),
            icon_big: create_font(s(22.0), 400, icon_face),
            big: create_font(s(20.0), 600, "Segoe UI"),
            mono_w,
            mono_h,
        }
    }
}

impl Drop for Fonts {
    fn drop(&mut self) {
        unsafe {
            for f in [self.ui, self.bold, self.small, self.mono, self.icon, self.icon_small, self.big, self.cap, self.mono_small, self.icon_big] {
                DeleteObject(f as HGDIOBJ);
            }
        }
    }
}

pub const DT_LINE: u32 = DT_SINGLELINE | DT_VCENTER | DT_NOPREFIX | DT_END_ELLIPSIS;

pub struct Gfx {
    pub dc: HDC,
}

impl Gfx {
    pub fn fill(&self, r: &RECT, color: u32) {
        unsafe {
            SetDCBrushColor(self.dc, color);
            FillRect(self.dc, r, GetStockObject(DC_BRUSH) as HBRUSH);
        }
    }

    pub fn frame(&self, r: &RECT, color: u32) {
        unsafe {
            SetDCBrushColor(self.dc, color);
            FrameRect(self.dc, r, GetStockObject(DC_BRUSH) as HBRUSH);
        }
    }

    pub fn hline(&self, x0: i32, x1: i32, y: i32, color: u32) {
        self.fill(&rect(x0, y, x1, y + 1), color);
    }

    pub fn vline(&self, x: i32, y0: i32, y1: i32, color: u32) {
        self.fill(&rect(x, y0, x + 1, y1), color);
    }

    pub fn text(&self, x: i32, y: i32, s: &str, font: HFONT, color: u32) {
        let ws = wz(s);
        let n = ws.len() - 1;
        unsafe {
            SelectObject(self.dc, font as HGDIOBJ);
            SetTextColor(self.dc, color);
            SetBkMode(self.dc, TRANSPARENT as _);
            TextOutW(self.dc, x, y, ws.as_ptr(), n as i32);
        }
    }

    /// Draws text clipped to `clip`.
    pub fn text_clip(&self, x: i32, y: i32, s: &[u16], font: HFONT, color: u32, clip: &RECT) {
        // GDI may read up to a terminating NUL: always pass a terminated copy
        let mut buf = Vec::with_capacity(s.len() + 1);
        buf.extend_from_slice(s);
        buf.push(0);
        let s = &buf[..buf.len() - 1];
        unsafe {
            SelectObject(self.dc, font as HGDIOBJ);
            SetTextColor(self.dc, color);
            SetBkMode(self.dc, TRANSPARENT as _);
            ExtTextOutW(self.dc, x, y, ETO_CLIPPED, clip, buf.as_ptr(), s.len() as u32, null());
        }
    }

    /// Draws characters on a fixed cell grid (terminal), clipped to `clip`.
    pub fn text_cells(&self, x: i32, y: i32, chars: &[char], cell_w: i32, font: HFONT, color: u32, clip: &RECT) {
        let mut buf: Vec<u16> = Vec::with_capacity(chars.len() * 2 + 1);
        let mut dx: Vec<i32> = Vec::with_capacity(chars.len() * 2 + 1);
        for c in chars {
            let mut b = [0u16; 2];
            let e = c.encode_utf16(&mut b);
            if e.len() == 2 {
                buf.push(e[0]);
                dx.push(0);
                buf.push(e[1]);
                dx.push(cell_w);
            } else {
                buf.push(e[0]);
                dx.push(cell_w);
            }
        }
        let n = buf.len();
        buf.push(0);
        dx.push(0);
        unsafe {
            SelectObject(self.dc, font as HGDIOBJ);
            SetTextColor(self.dc, color);
            SetBkMode(self.dc, TRANSPARENT as _);
            ExtTextOutW(self.dc, x, y, ETO_CLIPPED, clip, buf.as_ptr(), n as u32, dx.as_ptr());
        }
    }

    pub fn text_in(&self, r: &RECT, s: &str, font: HFONT, color: u32, flags: u32) {
        let ws = wz(s);
        let n = (ws.len() - 1) as i32;
        let mut rr = *r;
        unsafe {
            SelectObject(self.dc, font as HGDIOBJ);
            SetTextColor(self.dc, color);
            SetBkMode(self.dc, TRANSPARENT as _);
            DrawTextW(self.dc, ws.as_ptr(), n, &mut rr, flags);
        }
    }

    pub fn text_height(&self, r: &RECT, s: &str, font: HFONT, flags: u32) -> i32 {
        let ws = wz(s);
        let n = (ws.len() - 1) as i32;
        let mut rr = *r;
        unsafe {
            SelectObject(self.dc, font as HGDIOBJ);
            DrawTextW(self.dc, ws.as_ptr(), n, &mut rr, flags | DT_CALCRECT);
        }
        rh(&rr)
    }

    pub fn text_w(&self, s: &str, font: HFONT) -> i32 {
        let ws = wz(s);
        let mut sz = SIZE { cx: 0, cy: 0 };
        unsafe {
            SelectObject(self.dc, font as HGDIOBJ);
            GetTextExtentPoint32W(self.dc, ws.as_ptr(), (ws.len() - 1) as i32, &mut sz);
        }
        sz.cx
    }

    pub fn icon(&self, r: &RECT, glyph: char, font: HFONT, color: u32) {
        let mut b = [0u16; 3];
        let n = glyph.encode_utf16(&mut b[..2]).len();
        let mut rr = *r;
        unsafe {
            SelectObject(self.dc, font as HGDIOBJ);
            SetTextColor(self.dc, color);
            SetBkMode(self.dc, TRANSPARENT as _);
            DrawTextW(self.dc, b.as_ptr(), n as i32, &mut rr, DT_SINGLELINE | DT_VCENTER | DT_CENTER | DT_NOPREFIX);
        }
    }

    fn with_pen<F: FnOnce()>(&self, color: u32, width: i32, f: F) {
        unsafe {
            let pen = CreatePen(PS_SOLID, width.max(1), color);
            let old = SelectObject(self.dc, pen as HGDIOBJ);
            f();
            SelectObject(self.dc, old);
            DeleteObject(pen as HGDIOBJ);
        }
    }

    pub fn line(&self, x0: i32, y0: i32, x1: i32, y1: i32, color: u32, width: i32) {
        self.with_pen(color, width, || unsafe {
            MoveToEx(self.dc, x0, y0, null_mut());
            LineTo(self.dc, x1, y1);
        });
    }

    pub fn bezier(&self, pts: &[POINT], color: u32, width: i32) {
        self.with_pen(color, width, || unsafe {
            PolyBezier(self.dc, pts.as_ptr(), pts.len() as u32);
        });
    }

    pub fn ellipse(&self, cx: i32, cy: i32, r: i32, fill: u32, stroke: u32, sw: i32) {
        self.with_pen(stroke, sw, || unsafe {
            SetDCBrushColor(self.dc, fill);
            let old = SelectObject(self.dc, GetStockObject(DC_BRUSH));
            Ellipse(self.dc, cx - r, cy - r, cx + r + 1, cy + r + 1);
            SelectObject(self.dc, old);
        });
    }

    pub fn ring(&self, cx: i32, cy: i32, r: i32, color: u32, sw: i32) {
        self.with_pen(color, sw, || unsafe {
            let old = SelectObject(self.dc, GetStockObject(NULL_BRUSH));
            Ellipse(self.dc, cx - r, cy - r, cx + r + 1, cy + r + 1);
            SelectObject(self.dc, old);
        });
    }

    pub fn round_rect(&self, r: &RECT, radius: i32, fill: u32, stroke: Option<u32>) {
        unsafe {
            let pen = if let Some(s) = stroke { CreatePen(PS_SOLID, 1, s) } else { GetStockObject(NULL_PEN) as HPEN };
            let oldp = SelectObject(self.dc, pen as HGDIOBJ);
            SetDCBrushColor(self.dc, fill);
            let oldb = SelectObject(self.dc, GetStockObject(DC_BRUSH));
            RoundRect(self.dc, r.left, r.top, r.right + if stroke.is_some() { 0 } else { 1 }, r.bottom + if stroke.is_some() { 0 } else { 1 }, radius, radius);
            SelectObject(self.dc, oldb);
            SelectObject(self.dc, oldp);
            if stroke.is_some() {
                DeleteObject(pen as HGDIOBJ);
            }
        }
    }

    pub fn clip(&self, r: &RECT) -> i32 {
        unsafe {
            let s = SaveDC(self.dc);
            IntersectClipRect(self.dc, r.left, r.top, r.right, r.bottom);
            s
        }
    }

    pub fn unclip(&self, saved: i32) {
        unsafe {
            RestoreDC(self.dc, saved);
        }
    }
}

// ------------------------------------------------------------------- shell

pub fn set_clipboard(hwnd: HWND, text: &str) {
    let data = wz(text);
    unsafe {
        if OpenClipboard(hwnd) == 0 {
            return;
        }
        EmptyClipboard();
        let bytes = data.len() * 2;
        let h = GlobalAlloc(GMEM_MOVEABLE, bytes);
        if !h.is_null() {
            let p = GlobalLock(h) as *mut u16;
            std::ptr::copy_nonoverlapping(data.as_ptr(), p, data.len());
            GlobalUnlock(h);
            SetClipboardData(13 /* CF_UNICODETEXT */, h as HANDLE);
        }
        CloseClipboard();
    }
}

pub fn get_clipboard(hwnd: HWND) -> Option<String> {
    unsafe {
        if OpenClipboard(hwnd) == 0 {
            return None;
        }
        let h = GetClipboardData(13 /* CF_UNICODETEXT */);
        let mut out = None;
        if !h.is_null() {
            let p = GlobalLock(h as HGLOBAL) as *const u16;
            if !p.is_null() {
                let mut n = 0;
                while *p.add(n) != 0 {
                    n += 1;
                }
                out = Some(String::from_utf16_lossy(std::slice::from_raw_parts(p, n)));
                GlobalUnlock(h as HGLOBAL);
            }
        }
        CloseClipboard();
        out
    }
}

pub fn shell_open(path: &str) {
    let p = wz(path);
    let op = wz("open");
    unsafe {
        ShellExecuteW(null_mut(), op.as_ptr(), p.as_ptr(), null(), null(), SW_SHOWNORMAL);
    }
}

pub fn shell_exec(file: &str, params: &str, dir: &str) {
    let f = wz(file);
    let p = wz(params);
    let d = wz(dir);
    let op = wz("open");
    unsafe {
        ShellExecuteW(null_mut(), op.as_ptr(), f.as_ptr(), p.as_ptr(), d.as_ptr(), SW_SHOWNORMAL);
    }
}

pub fn reveal(path: &str) {
    shell_exec("explorer.exe", &format!("/select,\"{}\"", path), "");
}

pub fn pick_folder(hwnd: HWND, title: &str) -> Option<String> {
    let t = wz(title);
    let mut buf = [0u16; 1024];
    unsafe {
        let _ = windows_sys::Win32::System::Com::CoInitializeEx(null(), 2 /* COINIT_APARTMENTTHREADED */);
        let bi = BROWSEINFOW {
            hwndOwner: hwnd,
            pidlRoot: null_mut(),
            pszDisplayName: buf.as_mut_ptr(),
            lpszTitle: t.as_ptr(),
            ulFlags: BIF_RETURNONLYFSDIRS | BIF_NEWDIALOGSTYLE,
            lpfn: None,
            lParam: 0,
            iImage: 0,
        };
        let pidl = SHBrowseForFolderW(&bi);
        if pidl.is_null() {
            return None;
        }
        let mut path = [0u16; 1024];
        let ok = SHGetPathFromIDListW(pidl, path.as_mut_ptr());
        windows_sys::Win32::System::Com::CoTaskMemFree(pidl as *const c_void);
        if ok == 0 {
            return None;
        }
        let len = path.iter().position(|&c| c == 0).unwrap_or(0);
        Some(String::from_utf16_lossy(&path[..len]))
    }
}

// ----------------------------------------------------------------- dark mode

pub fn set_title_colors(hwnd: HWND, dark: bool, caption: u32, text: u32, border: u32) {
    unsafe {
        let v: i32 = dark as i32;
        DwmSetWindowAttribute(hwnd, 20, &v as *const i32 as *const c_void, 4);
        DwmSetWindowAttribute(hwnd, 35, &caption as *const u32 as *const c_void, 4);
        DwmSetWindowAttribute(hwnd, 36, &text as *const u32 as *const c_void, 4);
        DwmSetWindowAttribute(hwnd, 34, &border as *const u32 as *const c_void, 4);
    }
}

/// Uses the (undocumented but stable since 1809) uxtheme ordinals to get dark popup menus.
pub fn set_app_dark_mode(dark: bool) {
    unsafe {
        let name = wz("uxtheme.dll");
        let m = LoadLibraryW(name.as_ptr());
        if m.is_null() {
            return;
        }
        if let Some(f) = GetProcAddress(m, 135usize as *const u8) {
            let set_mode: unsafe extern "system" fn(i32) -> i32 = std::mem::transmute(f);
            set_mode(if dark { 2 } else { 3 });
        }
        if let Some(f) = GetProcAddress(m, 136usize as *const u8) {
            let flush: unsafe extern "system" fn() = std::mem::transmute(f);
            flush();
        }
    }
}

pub fn set_control_theme(hwnd: HWND, dark: bool) {
    let t = wz(if dark { "DarkMode_Explorer" } else { "Explorer" });
    unsafe {
        windows_sys::Win32::UI::Controls::SetWindowTheme(hwnd, t.as_ptr(), null());
    }
}

// ------------------------------------------------------------------ menus

pub enum MenuItem {
    Item { label: String, id: usize, enabled: bool, checked: bool },
    Sub { label: String, items: Vec<MenuItem> },
    Sep,
}

pub fn item(label: &str, id: usize) -> MenuItem {
    MenuItem::Item { label: label.to_string(), id, enabled: true, checked: false }
}

pub fn item_if(label: &str, id: usize, enabled: bool) -> MenuItem {
    MenuItem::Item { label: label.to_string(), id, enabled, checked: false }
}

pub fn item_checked(label: &str, id: usize, checked: bool) -> MenuItem {
    MenuItem::Item { label: label.to_string(), id, enabled: true, checked }
}

unsafe fn build_menu(items: &[MenuItem]) -> HMENU {
    let m = CreatePopupMenu();
    for it in items {
        match it {
            MenuItem::Item { label, id, enabled, checked } => {
                let l = wz(label);
                let mut flags = MF_STRING;
                if !enabled {
                    flags |= MF_GRAYED;
                }
                if *checked {
                    flags |= MF_CHECKED;
                }
                AppendMenuW(m, flags, *id + 1, l.as_ptr());
            }
            MenuItem::Sub { label, items } => {
                let l = wz(label);
                let sub = build_menu(items);
                AppendMenuW(m, MF_POPUP, sub as usize, l.as_ptr());
            }
            MenuItem::Sep => {
                AppendMenuW(m, MF_SEPARATOR, 0, null());
            }
        }
    }
    m
}

/// Shows a native popup menu at screen coordinates; returns the chosen item id.
pub fn popup_menu(hwnd: HWND, sx: i32, sy: i32, items: &[MenuItem]) -> Option<usize> {
    unsafe {
        let m = build_menu(items);
        let r = TrackPopupMenu(m, TPM_RETURNCMD | TPM_RIGHTBUTTON | TPM_NONOTIFY, sx, sy, 0, hwnd, null());
        DestroyMenu(m);
        if r > 0 {
            Some(r as usize - 1)
        } else {
            None
        }
    }
}

pub fn client_to_screen(hwnd: HWND, x: i32, y: i32) -> (i32, i32) {
    let mut p = POINT { x, y };
    unsafe {
        windows_sys::Win32::Graphics::Gdi::ClientToScreen(hwnd, &mut p);
    }
    (p.x, p.y)
}

pub fn get_text(hwnd: HWND) -> String {
    unsafe {
        let n = GetWindowTextLengthW(hwnd);
        let mut buf = vec![0u16; n as usize + 1];
        let got = GetWindowTextW(hwnd, buf.as_mut_ptr(), buf.len() as i32);
        String::from_utf16_lossy(&buf[..got as usize])
    }
}

pub fn set_text(hwnd: HWND, s: &str) {
    let t = wz(&s.replace("\r\n", "\n").replace('\n', "\r\n"));
    unsafe {
        SetWindowTextW(hwnd, t.as_ptr());
    }
}

pub fn fmt_age(ts: i64) -> String {
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0);
    let d = (now - ts).max(0);
    let (v, u) = if d < 60 {
        return "now".into();
    } else if d < 3600 {
        (d / 60, "min")
    } else if d < 86400 {
        (d / 3600, "hr")
    } else if d < 86400 * 7 {
        (d / 86400, "day")
    } else if d < 86400 * 30 {
        (d / (86400 * 7), "wk")
    } else if d < 86400 * 365 {
        (d / (86400 * 30), "mo")
    } else {
        (d / (86400 * 365), "yr")
    };
    format!("{v} {u}{} ago", if v == 1 { "" } else { "s" })
}

pub fn fmt_date(ts: i64) -> String {
    // civil date from unix days (UTC), adequate for display
    let ts = ts + crate::git::local_offset_secs();
    let days = ts.div_euclid(86400);
    let secs = ts.rem_euclid(86400);
    let z = days + 719468;
    let era = z.div_euclid(146097);
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!("{y}-{m:02}-{d:02} {:02}:{:02}", secs / 3600, (secs / 60) % 60)
}
