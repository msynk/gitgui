//! GitGui — a native, ultra-light Windows Git client modeled on the VS Code
//! Source Control experience. Pure Win32 + GDI: no runtime, no web view.

#![windows_subsystem = "windows"]
#![allow(dead_code)]

mod app;
mod branches;
mod cli;
mod diff;
mod explorer;
mod git;
mod graph;
mod settings;
mod tabs;
mod term;
mod term_ui;
mod theme;
mod ui;
mod watch;
#[cfg(test)]
mod tests;
mod win;
#[cfg(feature = "guard-alloc")]
mod guardalloc;
#[cfg(feature = "guard-alloc")]
#[global_allocator]
static GLOBAL: guardalloc::GuardAlloc = guardalloc::GuardAlloc;

use std::cell::Cell;
use std::ptr::{null, null_mut};

use windows_sys::Win32::Foundation::*;
use windows_sys::Win32::Graphics::Gdi::*;
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::HiDpi::{GetDpiForWindow, SetProcessDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::*;
use windows_sys::Win32::UI::Shell::{DefSubclassProc, SetWindowSubclass};
use windows_sys::Win32::UI::WindowsAndMessaging::*;

use app::App;

const DEFERRED_EDIT: u32 = WM_APP + 3;
const WM_MOUSELEAVE: u32 = 0x02A3;

thread_local! {
    static BUSY: Cell<bool> = const { Cell::new(false) };
    static PENDING: Cell<u32> = const { Cell::new(0) };
}
const P_PAINT: u32 = 1;
const P_JOB: u32 = 2;
const P_FS: u32 = 4;
const P_TERM: u32 = 8;

fn app_ptr(hwnd: HWND) -> *mut App {
    unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut App }
}

fn lo(l: LPARAM) -> i32 {
    (l & 0xFFFF) as i16 as i32
}

fn hi(l: LPARAM) -> i32 {
    ((l >> 16) & 0xFFFF) as i16 as i32
}

fn key_down(vk: u16) -> bool {
    unsafe { (GetKeyState(vk as i32) as u16 & 0x8000) != 0 }
}

/// Runs `f` with exclusive access to the App; messages arriving re-entrantly
/// (e.g. while a popup menu's modal loop runs) are deferred.
fn with_app<R>(hwnd: HWND, f: impl FnOnce(&mut App) -> R) -> Option<R> {
    let p = app_ptr(hwnd);
    if p.is_null() || BUSY.with(|b| b.get()) {
        return None;
    }
    BUSY.with(|b| b.set(true));
    let r = f(unsafe { &mut *p });
    BUSY.with(|b| b.set(false));
    // process what was deferred while busy
    loop {
        let pend = PENDING.with(|p| p.replace(0));
        if pend == 0 {
            break;
        }
        BUSY.with(|b| b.set(true));
        let app = unsafe { &mut *p };
        if pend & P_JOB != 0 {
            app.on_jobs();
        }
        if pend & P_TERM != 0 {
            app.on_term_data();
        }
        if pend & P_FS != 0 {
            unsafe {
                SetTimer(hwnd, app::TIMER_FS, 300, None);
            }
        }
        if pend & P_PAINT != 0 {
            app.invalidate();
        }
        BUSY.with(|b| b.set(false));
    }
    Some(r)
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    if app_ptr(hwnd).is_null() {
        return DefWindowProcW(hwnd, msg, wp, lp);
    }
    if BUSY.with(|b| b.get()) {
        // re-entrant message: defer
        match msg {
            WM_PAINT => {
                let mut ps: PAINTSTRUCT = std::mem::zeroed();
                BeginPaint(hwnd, &mut ps);
                EndPaint(hwnd, &ps);
                PENDING.with(|p| p.set(p.get() | P_PAINT));
                return 0;
            }
            app::WM_JOB => {
                PENDING.with(|p| p.set(p.get() | P_JOB));
                return 0;
            }
            app::WM_FS => {
                PENDING.with(|p| p.set(p.get() | P_FS));
                return 0;
            }
            app::WM_TERM => {
                PENDING.with(|p| p.set(p.get() | P_TERM));
                return 0;
            }
            WM_COMMAND => {
                PostMessageW(hwnd, DEFERRED_EDIT, wp, lp);
                return 0;
            }
            WM_CTLCOLOREDIT => {}
            _ => return DefWindowProcW(hwnd, msg, wp, lp),
        }
    }
    match msg {
        WM_PAINT => {
            let mut ps: PAINTSTRUCT = std::mem::zeroed();
            let hdc = BeginPaint(hwnd, &mut ps);
            with_app(hwnd, |a| a.paint(hdc));
            EndPaint(hwnd, &ps);
            0
        }
        WM_ERASEBKGND => 1,
        WM_SIZE => {
            with_app(hwnd, |a| {
                if wp as u32 == SIZE_MINIMIZED {
                    a.release_buffer();
                } else {
                    a.layout();
                }
            });
            0
        }
        WM_DPICHANGED => {
            let dpi = (wp & 0xFFFF) as u32;
            let r = &*(lp as *const RECT);
            with_app(hwnd, |a| {
                a.dpi = dpi;
                a.scale = dpi as f32 / 96.0;
                a.fonts = win::Fonts::new(a.scale);
                set_edit_fonts(a);
                a.release_buffer();
            });
            SetWindowPos(hwnd, null_mut(), r.left, r.top, r.right - r.left, r.bottom - r.top, SWP_NOZORDER | SWP_NOACTIVATE);
            0
        }
        WM_GETMINMAXINFO => {
            let mmi = &mut *(lp as *mut MINMAXINFO);
            mmi.ptMinTrackSize = POINT { x: 640, y: 400 };
            0
        }
        app::WM_JOB => {
            with_app(hwnd, |a| a.on_jobs());
            0
        }
        app::WM_FS => {
            SetTimer(hwnd, app::TIMER_FS, 300, None);
            0
        }
        app::WM_TERM => {
            with_app(hwnd, |a| a.on_term_data());
            0
        }
        WM_CHAR | WM_SYSCHAR => {
            let alt = msg == WM_SYSCHAR;
            let handled = with_app(hwnd, |a| {
                if a.focus == app::Focus::Terminal && a.terminal_visible() && a.overlay.is_none() {
                    a.term_char(wp as u16, alt);
                    true
                } else {
                    false
                }
            })
            .unwrap_or(false);
            if handled {
                0
            } else {
                DefWindowProcW(hwnd, msg, wp, lp)
            }
        }
        WM_TIMER => {
            let id = wp;
            with_app(hwnd, |a| match id {
                app::TIMER_FS => unsafe {
                    KillTimer(hwnd, app::TIMER_FS);
                    a.refresh();
                },
                app::TIMER_TOAST => a.tick_toasts(),
                app::TIMER_SPIN => {
                    a.spin = a.spin.wrapping_add(1);
                    a.invalidate();
                }
                _ => {}
            });
            0
        }
        WM_ACTIVATE => {
            if (wp & 0xFFFF) != 0 {
                with_app(hwnd, |a| {
                    if a.repo.is_some() {
                        unsafe {
                            SetTimer(hwnd, app::TIMER_FS, 150, None);
                        }
                    }
                    a.invalidate();
                });
            }
            DefWindowProcW(hwnd, msg, wp, lp)
        }
        WM_SETFOCUS | WM_KILLFOCUS => {
            InvalidateRect(hwnd, null(), 0);
            0
        }
        WM_MOUSEMOVE => {
            let mut tme = TRACKMOUSEEVENT { cbSize: std::mem::size_of::<TRACKMOUSEEVENT>() as u32, dwFlags: TME_LEAVE, hwndTrack: hwnd, dwHoverTime: 0 };
            TrackMouseEvent(&mut tme);
            with_app(hwnd, |a| a.on_mouse_move(lo(lp), hi(lp)));
            0
        }
        WM_MOUSELEAVE => {
            with_app(hwnd, |a| {
                if a.hover != app::Hit::None {
                    a.hover = app::Hit::None;
                    a.invalidate();
                }
            });
            0
        }
        WM_SETCURSOR => {
            if (lp & 0xFFFF) as u32 == HTCLIENT {
                let mut p = POINT { x: 0, y: 0 };
                GetCursorPos(&mut p);
                ScreenToClient(hwnd, &mut p);
                let cur = with_app(hwnd, |a| a.cursor_for(p.x, p.y)).flatten();
                if let Some(c) = cur {
                    SetCursor(LoadCursorW(null_mut(), c));
                    return 1;
                }
            }
            DefWindowProcW(hwnd, msg, wp, lp)
        }
        WM_LBUTTONDOWN => {
            let (ctrl, shift) = ((wp & 0x0008usize) != 0, (wp & 0x0004usize) != 0);
            with_app(hwnd, |a| a.on_lbutton_down(lo(lp), hi(lp), ctrl, shift));
            0
        }
        WM_LBUTTONUP => {
            with_app(hwnd, |a| a.on_lbutton_up());
            0
        }
        WM_LBUTTONDBLCLK => {
            with_app(hwnd, |a| a.on_dblclick(lo(lp), hi(lp)));
            0
        }
        WM_MBUTTONUP => {
            with_app(hwnd, |a| a.on_mbutton_up(lo(lp), hi(lp)));
            0
        }
        WM_RBUTTONUP => {
            with_app(hwnd, |a| a.on_rbutton_up(lo(lp), hi(lp)));
            0
        }
        WM_MOUSEWHEEL | WM_MOUSEHWHEEL => {
            let delta = ((wp >> 16) & 0xFFFF) as i16 as i32;
            let mut p = POINT { x: lo(lp), y: hi(lp) };
            ScreenToClient(hwnd, &mut p);
            let horizontal = msg == WM_MOUSEHWHEEL || (wp & 0x0004usize) != 0;
            let delta = if msg == WM_MOUSEHWHEEL { -delta } else { delta };
            with_app(hwnd, |a| a.on_wheel(p.x, p.y, delta, horizontal));
            0
        }
        WM_KEYDOWN | WM_SYSKEYDOWN => {
            let vk = wp as u16;
            let (ctrl, shift) = (key_down(VK_CONTROL), key_down(VK_SHIFT));
            if msg == WM_SYSKEYDOWN {
                let term_focus = with_app(hwnd, |a| a.focus == app::Focus::Terminal && a.terminal_visible()).unwrap_or(false);
                if vk == VK_F4 || !(term_focus || vk == VK_F10) {
                    return DefWindowProcW(hwnd, msg, wp, lp);
                }
            }
            with_app(hwnd, |a| a.on_key(vk, ctrl, shift));
            0
        }
        WM_COMMAND | DEFERRED_EDIT => {
            let code = ((wp >> 16) & 0xFFFF) as u32;
            let ctl = lp as HWND;
            if code == EN_CHANGE {
                with_app(hwnd, |a| {
                    if ctl == a.commit_edit {
                        a.edit_changed();
                    } else if ctl == a.overlay_edit {
                        a.filter_pick();
                    }
                });
            } else if code == EN_SETFOCUS || code == EN_KILLFOCUS {
                InvalidateRect(hwnd, null(), 0);
            }
            0
        }
        WM_CTLCOLOREDIT => {
            let p = app_ptr(hwnd);
            let a = &*p;
            let hdc = wp as HDC;
            SetTextColor(hdc, a.t.fg);
            SetBkColor(hdc, a.t.input_bg);
            a.edit_brush as LRESULT
        }
        WM_CLOSE => {
            with_app(hwnd, |a| {
                let mut wp: WINDOWPLACEMENT = unsafe { std::mem::zeroed() };
                wp.length = std::mem::size_of::<WINDOWPLACEMENT>() as u32;
                unsafe {
                    GetWindowPlacement(hwnd, &mut wp);
                }
                let r = wp.rcNormalPosition;
                a.st.window = Some((r.left, r.top, r.right - r.left, r.bottom - r.top, wp.showCmd == SW_SHOWMAXIMIZED as u32));
                a.st.save();
            });
            DestroyWindow(hwnd);
            0
        }
        WM_DESTROY => {
            PostQuitMessage(0);
            0
        }
        _ => DefWindowProcW(hwnd, msg, wp, lp),
    }
}

/// Subclass for the commit message box (multi-line EDIT).
unsafe extern "system" fn commit_edit_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM, _id: usize, main: usize) -> LRESULT {
    let main = main as HWND;
    match msg {
        WM_KEYDOWN => {
            let vk = wp as u16;
            let (ctrl, shift) = (key_down(VK_CONTROL), key_down(VK_SHIFT));
            if vk == VK_ESCAPE {
                SetFocus(main);
                return 0;
            }
            if ctrl && vk == 0x41 {
                SendMessageW(hwnd, 0x00B1, 0, -1);
                return 0;
            }
            if (ctrl || vk >= VK_F1 && vk <= VK_F12) && with_app(main, |a| a.global_key(vk, ctrl, shift)).unwrap_or(false) {
                return 0;
            }
        }
        WM_CHAR => {
            // swallow Ctrl+Enter (LF) and Ctrl+A / Ctrl+Backspace control chars
            if (wp == 0x0A) || (wp == 0x01) || (wp == 0x7F) {
                return 0;
            }
        }
        WM_PAINT => {
            let r = DefSubclassProc(hwnd, msg, wp, lp);
            if GetWindowTextLengthW(hwnd) == 0 {
                let p = app_ptr(main);
                if !p.is_null() {
                    let a = &*p;
                    let placeholder = match a.status() {
                        Some(s) => match &s.branch {
                            Some(b) => format!("Message (Ctrl+Enter to commit on \"{b}\")"),
                            None => "Message (Ctrl+Enter to commit)".to_string(),
                        },
                        None => String::new(),
                    };
                    let dc = GetDC(hwnd);
                    let g = win::Gfx { dc };
                    let mut rc = win::rect(0, 0, 0, 0);
                    GetClientRect(hwnd, &mut rc);
                    let m = (SendMessageW(hwnd, 0x00D4, 0, 0) & 0xFFFF) as i32;
                    g.text_in(&win::rect(rc.left + m, rc.top, rc.right, rc.top + a.s(18)), &placeholder, a.fonts.ui, crate::theme::blend_colorref(a.t.input_bg, a.t.fg, 0.55), win::DT_LINE);
                    ReleaseDC(hwnd, dc);
                }
            }
            return r;
        }
        _ => {}
    }
    DefSubclassProc(hwnd, msg, wp, lp)
}

/// Subclass for the quick pick / input box filter (single-line EDIT).
unsafe extern "system" fn overlay_edit_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM, _id: usize, main: usize) -> LRESULT {
    let main = main as HWND;
    match msg {
        WM_KEYDOWN => {
            let vk = wp as u16;
            if matches!(vk, 0x26 | 0x28 | 0x21 | 0x22 | 0x0D | 0x1B) && with_app(main, |a| a.overlay_key(vk)).unwrap_or(false) {
                return 0;
            }
            if key_down(VK_CONTROL) && vk == 0x41 {
                SendMessageW(hwnd, 0x00B1, 0, -1);
                return 0;
            }
        }
        WM_CHAR => {
            if wp == 0x0D || wp == 0x1B || wp == 0x01 || wp == 0x7F {
                return 0;
            }
        }
        WM_KILLFOCUS => {
            // clicking elsewhere closes the overlay (unless it moved to the main window for list clicks)
        }
        _ => {}
    }
    DefSubclassProc(hwnd, msg, wp, lp)
}

/// Keeps a saved window rectangle on screen: moves it into the work area of the nearest
/// monitor and shrinks it to fit, so a position saved on a monitor that is no longer
/// connected (or at a different scale) doesn't open the window off-screen.
fn fit_to_monitor(x: i32, y: i32, w: i32, h: i32) -> (i32, i32, i32, i32) {
    unsafe {
        let r = RECT { left: x, top: y, right: x + w.max(1), bottom: y + h.max(1) };
        let mon = MonitorFromRect(&r, MONITOR_DEFAULTTONEAREST);
        let mut mi: MONITORINFO = std::mem::zeroed();
        mi.cbSize = std::mem::size_of::<MONITORINFO>() as u32;
        if mon.is_null() || GetMonitorInfoW(mon, &mut mi) == 0 {
            return (CW_USEDEFAULT, CW_USEDEFAULT, 1280, 820);
        }
        let wa = mi.rcWork;
        let w = w.clamp(400, (wa.right - wa.left).max(400));
        let h = h.clamp(300, (wa.bottom - wa.top).max(300));
        let x = x.clamp(wa.left, (wa.right - w).max(wa.left));
        let y = y.clamp(wa.top, (wa.bottom - h).max(wa.top));
        (x, y, w, h)
    }
}

fn set_edit_fonts(a: &App) {
    unsafe {
        SendMessageW(a.commit_edit, WM_SETFONT, a.fonts.ui as WPARAM, 1);
        SendMessageW(a.overlay_edit, WM_SETFONT, a.fonts.ui as WPARAM, 1);
        let m = a.s(2) as isize;
        SendMessageW(a.commit_edit, 0x00D3, 3, m | (m << 16));
    }
}

fn main() {
    // ---- command line (code.exe style)
    let cli = cli::parse(std::env::args().skip(1));
    if cli.help {
        cli::output("GitGui", cli::USAGE);
        return;
    }
    if cli.version {
        cli::output("GitGui", &format!("{}
{}
", env!("CARGO_PKG_VERSION"), git::version().map(|v| format!("git {v}")).unwrap_or_else(|| "git not found".into())));
        return;
    }
    if cli.install_menu {
        match cli::install_context_menu() {
            Ok(()) => cli::output("GitGui", "Added \"Open with GitGui\" to the Explorer folder context menu.
"),
            Err(e) => cli::output("GitGui", &format!("Failed: {e}
")),
        }
        return;
    }
    if cli.uninstall_menu {
        cli::uninstall_context_menu();
        cli::output("GitGui", "Removed the Explorer context menu entry.
");
        return;
    }
    let cwd = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
    let mut targets = Vec::new();
    let mut errors: Vec<String> = cli.unknown.iter().map(|u| format!("Ignoring unknown option '{u}'. Run with --help for usage.")).collect();
    for p in &cli.paths {
        match cli::resolve(p, &cwd) {
            Ok(t) => targets.push(t),
            Err(e) => errors.push(e),
        }
    }
    // every additional path gets its own window (process)
    if targets.len() > 1 {
        if let Ok(exe) = std::env::current_exe() {
            for t in targets.drain(1..) {
                let arg = t.file.as_ref().unwrap_or(&t.folder).display().to_string();
                let _ = std::process::Command::new(&exe).arg(arg).spawn();
            }
        }
    }
    let target = targets.into_iter().next();

    unsafe {
        SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        let hinst = GetModuleHandleW(null());
        let class = win::wz("GitGuiWindow");
        let wc = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            style: CS_DBLCLKS,
            lpfnWndProc: Some(wndproc),
            cbClsExtra: 0,
            cbWndExtra: 0,
            hInstance: hinst,
            hIcon: LoadIconW(null_mut(), IDI_APPLICATION),
            hCursor: LoadCursorW(null_mut(), IDC_ARROW),
            hbrBackground: null_mut(),
            lpszMenuName: null(),
            lpszClassName: class.as_ptr(),
            hIconSm: null_mut(),
        };
        RegisterClassExW(&wc);
        let st = settings::Settings::load();
        let (x, y, w, h, max) = match st.window {
            Some((x, y, w, h, max)) => {
                let (x, y, w, h) = fit_to_monitor(x, y, w, h);
                (x, y, w, h, max)
            }
            None => (CW_USEDEFAULT, CW_USEDEFAULT, 1280, 820, false),
        };
        let title = win::wz("GitGui");
        let hwnd = CreateWindowExW(0, class.as_ptr(), title.as_ptr(), WS_OVERLAPPEDWINDOW | WS_CLIPCHILDREN, x, y, w, h, null_mut(), null_mut(), hinst, null());
        if hwnd.is_null() {
            return;
        }
        let dpi = GetDpiForWindow(hwnd);
        let mut app = Box::new(App::new(hwnd, dpi, st));
        app.git_version = git::version();

        let edit_class = win::wz("EDIT");
        let empty = win::wz("");
        app.commit_edit = CreateWindowExW(0, edit_class.as_ptr(), empty.as_ptr(), WS_CHILD | WS_VISIBLE | ES_MULTILINE as u32 | ES_AUTOVSCROLL as u32 | ES_WANTRETURN as u32, 0, 0, 10, 10, hwnd, 100 as HMENU, hinst, null());
        app.overlay_edit = CreateWindowExW(0, edit_class.as_ptr(), empty.as_ptr(), WS_CHILD | ES_AUTOHSCROLL as u32, 0, 0, 10, 10, hwnd, 101 as HMENU, hinst, null());
        let main_ptr = hwnd as usize;
        SetWindowSubclass(app.commit_edit, Some(commit_edit_proc), 1, main_ptr);
        SetWindowSubclass(app.overlay_edit, Some(overlay_edit_proc), 2, main_ptr);
        set_edit_fonts(&app);
        app.apply_theme();

        let raw = Box::into_raw(app);
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, raw as isize);

        // open folder from the command line, or the last used one
        let had_args = !cli.paths.is_empty();
        with_app(hwnd, |a| {
            if a.git_version.is_none() {
                a.error("Git not found. Install Git for Windows (https://git-scm.com) and make sure it is on PATH.");
            }
            for e in &errors {
                a.error(e.clone());
            }
            match target {
                Some(t) => a.open_target(t),
                None if had_args => {}
                None => {
                    if let Some(last) = a.st.recent.first().cloned() {
                        if std::path::Path::new(&last).is_dir() && a.git_version.is_some() {
                            a.open_repo(std::path::Path::new(&last));
                        }
                    }
                }
            }
            a.layout();
            // restore the terminal like VS Code does when it was open last time
            if a.terminal_visible() {
                a.start_terminal(None);
            }
        });

        ShowWindow(hwnd, if max { SW_SHOWMAXIMIZED } else { SW_SHOW });
        UpdateWindow(hwnd);

        let mut msg: MSG = std::mem::zeroed();
        while GetMessageW(&mut msg, null_mut(), 0, 0) > 0 {
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
        let _ = Box::from_raw(raw);
    }
}
