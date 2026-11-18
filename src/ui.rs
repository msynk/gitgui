//! Layout, painting (GDI, double buffered) and input handling.

use std::ptr::null_mut;

use windows_sys::Win32::Foundation::{HWND, POINT, RECT};
use windows_sys::Win32::Graphics::Gdi::*;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::*;
use windows_sys::Win32::UI::WindowsAndMessaging::*;

use crate::app::*;
use crate::diff::LK;
use crate::git::{self, Group, Kind, RefKind};
use crate::graph::{self, Seg};
use crate::win::{self, contains, rect, rh, rw, xywh, Gfx, MenuItem, DT_LINE};

// Segoe Fluent Icons / MDL2 glyphs
const I_ADD: char = '\u{E710}';
const I_REMOVE: char = '\u{E738}';
const I_UNDO: char = '\u{E7A7}';
const I_REFRESH: char = '\u{E72C}';
const I_CHECK: char = '\u{E73E}';
const I_MORE: char = '\u{E712}';
const I_DOWN: char = '\u{E70D}';
const I_RIGHT: char = '\u{E76C}';
const I_OPEN: char = '\u{E8E5}';
const I_UP_ARROW: char = '\u{E74A}';
const I_DOWN_ARROW: char = '\u{E74B}';
const I_BACK: char = '\u{E72B}';
const I_CLOSE: char = '\u{E711}';
const I_INFO: char = '\u{E946}';
const I_WARN: char = '\u{E7BA}';
const I_ERROR: char = '\u{E783}';
const I_SYNC: char = '\u{E895}';
const I_UPLOAD: char = '\u{E898}';
const I_DOWNLOAD: char = '\u{E896}';
const I_CLOUD: char = '\u{E753}';
const I_DOC: char = '\u{E8A5}';
const I_SC: char = '\u{E8D4}';
const I_SEARCH: char = '\u{E721}';
const I_FOLDER: char = '\u{E8B7}';
const I_TAG: char = '\u{E8EC}';

pub struct Menu {
    pub acts: Vec<Option<Box<dyn FnOnce(&mut App)>>>,
}

impl Menu {
    pub fn new() -> Menu {
        Menu { acts: Vec::new() }
    }
    pub fn it(&mut self, label: &str, f: impl FnOnce(&mut App) + 'static) -> MenuItem {
        self.acts.push(Some(Box::new(f)));
        win::item(label, self.acts.len() - 1)
    }
    pub fn it_if(&mut self, label: &str, enabled: bool, f: impl FnOnce(&mut App) + 'static) -> MenuItem {
        self.acts.push(Some(Box::new(f)));
        win::item_if(label, self.acts.len() - 1, enabled)
    }
    pub fn check(&mut self, label: &str, checked: bool, f: impl FnOnce(&mut App) + 'static) -> MenuItem {
        self.acts.push(Some(Box::new(f)));
        win::item_checked(label, self.acts.len() - 1, checked)
    }
    pub fn cmd(&mut self, label: &str, c: Cmd) -> MenuItem {
        self.it(label, move |a| a.exec(c))
    }
}

fn sub(label: &str, items: Vec<MenuItem>) -> MenuItem {
    MenuItem::Sub { label: label.into(), items }
}

fn expand_tabs(s: &str) -> Vec<u16> {
    let mut out = Vec::with_capacity(s.len());
    let mut col = 0usize;
    for ch in s.trim_end_matches('\r').chars() {
        if ch == '\t' {
            let n = 4 - col % 4;
            for _ in 0..n {
                out.push(b' ' as u16);
            }
            col += n;
        } else {
            let mut b = [0u16; 2];
            out.extend_from_slice(ch.encode_utf16(&mut b));
            col += 1;
        }
    }
    out
}

impl App {
    // ================================================================ layout

    pub fn client(&self) -> RECT {
        let mut r = rect(0, 0, 0, 0);
        unsafe {
            GetClientRect(self.hwnd, &mut r);
        }
        r
    }

    pub fn layout(&mut self) {
        let c = self.client();
        let (w, h) = (rw(&c), rh(&c));
        let sb = self.s(22);
        let ab = self.s(48);
        let sw = self.s(self.st.sidebar_w).clamp(self.s(180), (w - ab - self.s(260)).max(self.s(180)));
        let mut l = Layout::default();
        l.status = rect(0, h - sb, w, h);
        l.activity = rect(0, 0, ab, h - sb);
        l.sidebar = rect(ab, 0, ab + sw, h - sb);
        l.title = rect(ab, 0, ab + sw, self.s(35));
        // sidebar sections
        let avail_top = l.title.bottom;
        let avail_h = l.sidebar.bottom - avail_top;
        let split = avail_top + ((avail_h as f32) * self.st.graph_split) as i32;
        let split = split.clamp(avail_top + self.s(150), (l.sidebar.bottom - self.s(80)).max(avail_top + self.s(150)));
        l.changes_header = rect(ab, avail_top, ab + sw, avail_top + self.s(22));
        let edit_h = self.edit_lines * self.s(18) + self.s(8);
        l.edit = rect(ab + self.s(20), l.changes_header.bottom + self.s(4), ab + sw - self.s(12), l.changes_header.bottom + self.s(4) + edit_h);
        let has_button = self.repo.is_some();
        l.button = if has_button { rect(l.edit.left, l.edit.bottom + self.s(6), l.edit.right, l.edit.bottom + self.s(6) + self.s(28)) } else { rect(l.edit.left, l.edit.bottom, l.edit.right, l.edit.bottom) };
        l.changes = rect(ab, l.button.bottom + self.s(6), ab + sw, split);
        l.graph_header = rect(ab, split, ab + sw, split + self.s(22));
        l.graph = rect(ab, l.graph_header.bottom, ab + sw, l.sidebar.bottom);
        // main
        let mx = l.sidebar.right + 1;
        let out_h = if self.st.output_visible { self.s(self.st.output_h).clamp(self.s(80), (h - sb - self.s(150)).max(self.s(80))) } else { 0 };
        l.main = rect(mx, 0, w, h - sb - out_h);
        l.main_header = rect(mx, 0, w, self.s(35));
        l.main_body = rect(mx, l.main_header.bottom, w, l.main.bottom);
        l.output = rect(mx, l.main.bottom, w, h - sb);
        l.output_body = rect(mx, l.output.top + self.s(35), w, l.output.bottom);
        self.l = l;
        unsafe {
            let show = self.repo.is_some();
            ShowWindow(self.commit_edit, if show { SW_SHOW } else { SW_HIDE });
            let pad_x = self.s(6);
            let pad_y = self.s(4);
            MoveWindow(self.commit_edit, l.edit.left + pad_x, l.edit.top + pad_y, (rw(&l.edit) - pad_x * 2).max(10), (rh(&l.edit) - pad_y * 2).max(10), 1);
        }
        for a in [Area::Changes, Area::Graph, Area::Main, Area::Output] {
            self.clamp_scroll(a);
        }
        self.resize_terminal();
        self.layout_overlay_edit();
        self.invalidate();
    }

    pub fn edit_changed(&mut self) {
        let lines = unsafe { SendMessageW(self.commit_edit, 0x00BA /* EM_GETLINECOUNT */, 0, 0) } as i32;
        let lines = lines.clamp(1, 10);
        if lines != self.edit_lines {
            self.edit_lines = lines;
            self.layout();
        }
        unsafe {
            InvalidateRect(self.commit_edit, std::ptr::null(), 1);
        }
    }

    fn overlay_box(&self) -> RECT {
        let c = self.client();
        let w = self.s(600).min((rw(&c) as f32 * 0.62) as i32).max(self.s(320));
        let x = (rw(&c) - w) / 2;
        xywh(x, self.s(8), w, self.s(42))
    }

    pub fn layout_overlay_edit(&mut self) {
        if self.overlay.is_none() {
            return;
        }
        let b = self.overlay_box();
        unsafe {
            MoveWindow(self.overlay_edit, b.left + self.s(12), b.top + self.s(11), rw(&b) - self.s(24), self.s(20), 1);
        }
    }

    pub fn show_overlay_edit(&mut self, placeholder: &str, value: &str) {
        let p = win::wz(placeholder);
        unsafe {
            SendMessageW(self.overlay_edit, 0x1501 /* EM_SETCUEBANNER */, 1, p.as_ptr() as isize);
        }
        win::set_text(self.overlay_edit, value);
        self.layout_overlay_edit();
        unsafe {
            ShowWindow(self.overlay_edit, SW_SHOW);
            SetFocus(self.overlay_edit);
            SendMessageW(self.overlay_edit, 0x00B1 /* EM_SETSEL */, 0, -1);
        }
        self.invalidate();
    }

    pub fn hide_overlay_edit(&mut self) {
        unsafe {
            ShowWindow(self.overlay_edit, SW_HIDE);
        }
    }

    // ================================================================ scrolling

    fn area_rect(&self, a: Area) -> RECT {
        match a {
            Area::Changes => self.l.changes,
            Area::Graph => self.l.graph,
            Area::Main => self.l.main_body,
            Area::Output => self.l.output_body,
            Area::Pick => {
                let b = self.overlay_box();
                rect(b.left, b.bottom, b.right, b.bottom + self.pick_visible_rows() * self.s(22))
            }
        }
    }

    fn pick_visible_rows(&self) -> i32 {
        if let Some(Overlay { kind: OverlayKind::Pick { filtered, .. } }) = &self.overlay {
            (filtered.len() as i32).min(15)
        } else {
            0
        }
    }

    fn content_h(&self, a: Area) -> i32 {
        let rh22 = self.s(22);
        match a {
            Area::Changes => self.crows.len() as i32 * rh22,
            Area::Graph => (self.commits.len() as i32 + if self.has_more { 1 } else { 0 }) * rh22,
            Area::Main => match &self.view {
                MainView::Diff(d) => d.rows.len() as i32 * self.line_h() + self.s(8),
                MainView::File(f) => (f.lines.len() + f.conflicts.len()) as i32 * self.line_h() + self.s(8),
                MainView::Commit(c) => self.commit_header_h(c) + c.files.len() as i32 * rh22 + self.s(8),
                _ => 0,
            },
            Area::Output => git::log_len() as i32 * self.fonts.mono_small_h + self.s(8),
            Area::Pick => {
                if let Some(Overlay { kind: OverlayKind::Pick { filtered, .. } }) = &self.overlay {
                    filtered.len() as i32 * rh22
                } else {
                    0
                }
            }
        }
    }

    pub fn clamp_scroll(&mut self, a: Area) {
        let view = rh(&self.area_rect(a));
        let max = (self.content_h(a) - view).max(0);
        let s = &mut self.scroll[a as usize];
        *s = (*s).clamp(0, max);
    }

    fn scroll_by(&mut self, a: Area, dy: i32) {
        self.scroll[a as usize] += dy;
        self.clamp_scroll(a);
        self.invalidate();
    }

    fn reveal_row(&mut self, a: Area, row: usize) {
        let rh22 = if a == Area::Main { self.line_h() } else { self.s(22) };
        let top = row as i32 * rh22;
        let view = rh(&self.area_rect(a));
        let s = self.scroll[a as usize];
        if top < s {
            self.scroll[a as usize] = top;
        } else if top + rh22 > s + view {
            self.scroll[a as usize] = top + rh22 - view;
        }
        self.clamp_scroll(a);
    }

    fn draw_scrollbar(&mut self, g: &Gfx, a: Area) {
        let r = self.area_rect(a);
        let content = self.content_h(a);
        let view = rh(&r);
        if content <= view || view <= 0 {
            return;
        }
        let w = self.s(10);
        let th = ((view as f32 * view as f32 / content as f32) as i32).max(self.s(20));
        let ty = r.top + ((view - th) as f32 * self.scroll[a as usize] as f32 / (content - view) as f32) as i32;
        let thumb = rect(r.right - w, ty, r.right, ty + th);
        let hot = self.hover == Hit::Thumb(a) || matches!(self.drag, Some(Drag::Thumb(x, _, _)) if x == a);
        g.fill(&thumb, if hot { self.t.scroll_thumb_hover } else { self.t.scroll_thumb });
        self.hits.push((thumb, Hit::Thumb(a)));
    }

    // ================================================================ painting

    pub fn paint(&mut self, hdc: HDC) {
        let c = self.client();
        let (w, h) = (rw(&c).max(1), rh(&c).max(1));
        unsafe {
            if self.buf.0.is_null() || self.buf.2 != w || self.buf.3 != h {
                if !self.buf.0.is_null() {
                    DeleteObject(self.buf.1 as HGDIOBJ);
                    DeleteDC(self.buf.0);
                }
                let mdc = CreateCompatibleDC(hdc);
                let bmp = CreateCompatibleBitmap(hdc, w, h);
                SelectObject(mdc, bmp as HGDIOBJ);
                self.buf = (mdc, bmp, w, h);
            }
        }
        let g = Gfx { dc: self.buf.0 };
        self.hits.clear();
        g.fill(&c, self.t.editor);
        self.paint_activity(&g);
        self.paint_sidebar(&g);
        self.paint_main(&g);
        if self.st.output_visible {
            self.paint_output(&g);
        }
        self.paint_status(&g);
        self.paint_toasts(&g);
        self.paint_overlay(&g);
        unsafe {
            BitBlt(hdc, 0, 0, w, h, self.buf.0, 0, 0, SRCCOPY);
        }
    }

    pub fn release_buffer(&mut self) {
        unsafe {
            if !self.buf.0.is_null() {
                DeleteObject(self.buf.1 as HGDIOBJ);
                DeleteDC(self.buf.0);
            }
        }
        self.buf = (null_mut(), null_mut(), 0, 0);
    }

    fn icon_button(&mut self, g: &Gfx, r: RECT, glyph: char, hit: Hit, enabled: bool) {
        if self.hover == hit && enabled {
            g.round_rect(&r, self.s(5), self.t.toolbar_hover, None);
        }
        g.icon(&r, glyph, self.fonts.icon, if enabled { self.t.fg } else { self.t.fg_dim });
        if enabled {
            self.hits.push((r, hit));
        }
    }

    fn text_button(&mut self, g: &Gfx, x: i32, y: i32, h: i32, label: &str, hit: Hit) -> i32 {
        let tw = g.text_w(label, self.fonts.small) + self.s(12);
        let r = xywh(x - tw, y, tw, h);
        if self.hover == hit {
            g.round_rect(&r, self.s(5), self.t.toolbar_hover, None);
        }
        g.text_in(&r, label, self.fonts.small, self.t.fg, DT_LINE | DT_CENTER);
        self.hits.push((r, hit));
        r.left
    }

    fn paint_activity(&mut self, g: &Gfx) {
        let r = self.l.activity;
        g.fill(&r, self.t.sidebar);
        g.vline(r.right - 1, r.top, r.bottom, self.t.border);
        // source control item (active)
        let item = xywh(r.left, r.top, rw(&r) - 1, self.s(48));
        g.fill(&rect(item.left, item.top, item.left + self.s(2), item.bottom), self.t.accent);
        draw_branch_icon(g, item.left + rw(&item) / 2, item.top + rh(&item) / 2, self.s(24), self.t.fg_strong, self.scale);
        let count = self.repo.as_ref().map(|r| r.status.files.len()).unwrap_or(0);
        if count > 0 || !self.ops.is_empty() {
            let label = if !self.ops.is_empty() { "\u{2026}".to_string() } else if count > 999 { "999+".into() } else { count.to_string() };
            let tw = g.text_w(&label, self.fonts.cap).max(self.s(8));
            let b = xywh(item.right - self.s(10) - tw - self.s(6), item.top + self.s(26), tw + self.s(8), self.s(16));
            g.round_rect(&b, self.s(16), self.t.accent, None);
            g.text_in(&b, &label, self.fonts.cap, self.t.accent_fg, DT_LINE | DT_CENTER);
        }
        let srch = xywh(r.left, r.top + self.s(48), rw(&r) - 1, self.s(48));
        let hit = Hit::Cmd(Cmd::Palette);
        g.icon(&srch, I_SEARCH, self.fonts.icon_big, if self.hover == hit { self.t.fg_strong } else { self.t.fg_dim });
        self.hits.push((srch, hit));
        let gear = xywh(r.left, r.bottom - self.s(48), rw(&r) - 1, self.s(48));
        let hit = Hit::Cmd(Cmd::ToggleTheme);
        g.icon(&gear, '\u{E790}', self.fonts.icon_big, if self.hover == hit { self.t.fg_strong } else { self.t.fg_dim });
        self.hits.push((gear, hit));
    }

    fn paint_sidebar(&mut self, g: &Gfx) {
        let l = self.l;
        g.fill(&l.sidebar, self.t.sidebar);
        g.vline(l.sidebar.right, l.sidebar.top, l.sidebar.bottom, self.t.border);
        self.hits.push((rect(l.sidebar.right - self.s(2), l.sidebar.top, l.sidebar.right + self.s(3), l.sidebar.bottom), Hit::Sash(Sash::Sidebar)));
        // title
        g.text_in(&rect(l.title.left + self.s(20), l.title.top, l.title.right - self.s(80), l.title.bottom), "SOURCE CONTROL", self.fonts.cap, self.t.fg, DT_LINE);
        let bs = self.s(22);
        let y = l.title.top + (rh(&l.title) - bs) / 2;
        let mut x = l.title.right - self.s(8) - bs;
        if self.repo.is_some() {
            self.icon_button(g, xywh(x, y, bs, bs), I_MORE, Hit::ActionBtn, true);
            x -= bs + self.s(2);
            self.icon_button(g, xywh(x, y, bs, bs), I_REFRESH, Hit::Cmd(Cmd::Refresh), true);
            x -= bs + self.s(2);
            self.icon_button(g, xywh(x, y, bs, bs), I_CHECK, Hit::Cmd(Cmd::Commit), true);
        } else {
            self.icon_button(g, xywh(x, y, bs, bs), I_MORE, Hit::ActionBtn, true);
        }
        // progress bar
        if !self.ops.is_empty() || self.refreshing && self.repo.as_ref().map(|r| !r.loaded).unwrap_or(false) {
            let pw = rw(&l.sidebar) / 5;
            let pos = (self.spin as i32 * self.s(14)) % (rw(&l.sidebar) + pw) - pw;
            g.fill(&rect((l.sidebar.left + pos).max(l.sidebar.left), l.changes_header.top, (l.sidebar.left + pos + pw).min(l.sidebar.right), l.changes_header.top + self.s(2)), self.t.accent);
        }
        // changes header
        let hh = l.changes_header;
        g.icon(&xywh(hh.left + self.s(2), hh.top, self.s(20), rh(&hh)), I_DOWN, self.fonts.icon_small, self.t.fg);
        let name = self.repo.as_ref().map(|r| format!("CHANGES  \u{2022}  {}", r.name.to_uppercase())).unwrap_or_else(|| "CHANGES".into());
        g.text_in(&rect(hh.left + self.s(22), hh.top, hh.right - self.s(8), hh.bottom), &name, self.fonts.bold, self.t.fg, DT_LINE);

        if self.repo.is_none() {
            self.paint_welcome(g);
        } else {
            // commit box frame
            let focused = unsafe { GetFocus() } == self.commit_edit;
            g.fill(&l.edit, self.t.input_bg);
            g.frame(&l.edit, if focused { self.t.focus } else { self.t.input_border });
            self.paint_commit_button(g);
            self.paint_changes(g);
        }
        // graph
        let gh = l.graph_header;
        g.hline(gh.left, gh.right, gh.top, self.t.border);
        self.hits.push((rect(gh.left, gh.top - self.s(2), gh.right, gh.top + self.s(3)), Hit::Sash(Sash::Graph)));
        g.icon(&xywh(gh.left + self.s(2), gh.top, self.s(20), rh(&gh)), I_DOWN, self.fonts.icon_small, self.t.fg);
        g.text_in(&rect(gh.left + self.s(22), gh.top, gh.right - self.s(150), gh.bottom), "GRAPH", self.fonts.bold, self.t.fg, DT_LINE);
        if self.repo.is_some() {
            let bs = self.s(22);
            let mut x = gh.right - self.s(8) - bs;
            let y = gh.top;
            self.icon_button(g, xywh(x, y, bs, bs), I_REFRESH, Hit::Cmd(Cmd::Refresh), true);
            x -= bs;
            self.icon_button(g, xywh(x, y, bs, bs), I_UPLOAD, Hit::Cmd(Cmd::Push), true);
            x -= bs;
            self.icon_button(g, xywh(x, y, bs, bs), I_DOWNLOAD, Hit::Cmd(Cmd::Pull), true);
            x -= bs;
            self.icon_button(g, xywh(x, y, bs, bs), I_CLOUD, Hit::Cmd(Cmd::FetchAll), true);
            let label = if self.st.graph_all { "All" } else { "Auto" };
            self.text_button(g, x - self.s(2), y + self.s(1), bs - self.s(2), label, Hit::Cmd(Cmd::ToggleGraphAll));
            self.paint_graph(g);
        }
    }

    fn paint_welcome(&mut self, g: &Gfx) {
        let l = self.l;
        let x = l.sidebar.left + self.s(20);
        let wdt = rw(&l.sidebar) - self.s(40);
        let mut y = l.changes_header.bottom + self.s(12);
        let msg = if self.git_version.is_none() {
            "Git was not found. Install Git for Windows (https://git-scm.com) and restart."
        } else if self.pending_init.is_some() {
            "The folder currently open doesn't have a git repository. You can initialize a repository which will enable source control features powered by Git."
        } else {
            "In order to use Git features, you can open a folder containing a Git repository or clone from a URL."
        };
        let r = xywh(x, y, wdt, self.s(200));
        let th = g.text_height(&r, msg, self.fonts.ui, DT_WORDBREAK | DT_NOPREFIX);
        g.text_in(&xywh(x, y, wdt, th), msg, self.fonts.ui, self.t.fg, DT_WORDBREAK | DT_NOPREFIX);
        y += th + self.s(14);
        let buttons: Vec<(&str, u8)> = if self.pending_init.is_some() { vec![("Initialize Repository", 2), ("Open Folder", 0), ("Clone Repository", 1)] } else { vec![("Open Folder", 0), ("Clone Repository", 1)] };
        for (label, id) in buttons {
            let b = xywh(x, y, wdt.min(self.s(300)), self.s(28));
            let hit = Hit::Welcome(id);
            g.fill(&b, if self.hover == hit { self.t.accent_hover } else { self.t.accent });
            g.text_in(&b, label, self.fonts.ui, self.t.accent_fg, DT_LINE | DT_CENTER);
            self.hits.push((b, hit));
            y += self.s(36);
        }
        if !self.st.recent.is_empty() {
            y += self.s(8);
            g.text_in(&xywh(x, y, wdt, self.s(20)), "RECENT", self.fonts.bold, self.t.fg_dim, DT_LINE);
            y += self.s(22);
            let recent = self.st.recent.clone();
            for (i, r) in recent.iter().enumerate().take(10) {
                if y + self.s(22) > l.graph_header.top {
                    break;
                }
                let row = xywh(l.sidebar.left, y, rw(&l.sidebar), self.s(22));
                let hit = Hit::Recent(i);
                if self.hover == hit {
                    g.fill(&row, self.t.hover);
                }
                let name = std::path::Path::new(r).file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
                g.icon(&xywh(x - self.s(2), y, self.s(18), self.s(22)), I_FOLDER, self.fonts.icon_small, self.t.fg_dim);
                let nw = g.text_w(&name, self.fonts.ui);
                g.text_in(&rect(x + self.s(20), y, row.right - self.s(8), y + self.s(22)), &name, self.fonts.ui, self.t.link, DT_LINE);
                g.text_in(&rect(x + self.s(28) + nw, y, row.right - self.s(8), y + self.s(22)), r, self.fonts.small, self.t.fg_dim, DT_LINE);
                self.hits.push((row, hit));
                y += self.s(22);
            }
        }
    }

    /// Returns (label, tooltip, enabled, kind) for the primary action button.
    pub fn action_button(&self) -> (String, bool, u8) {
        let Some(s) = self.status() else { return ("Commit".into(), false, 0) };
        if self.ops.iter().any(|o| o == "Committing") {
            return ("Committing...".into(), false, 0);
        }
        if s.rebasing {
            return ("\u{2713} Continue".into(), s.count(Group::Merge) == 0, 0);
        }
        if !s.files.is_empty() || s.merging {
            return ("\u{2713} Commit".into(), s.count(Group::Merge) == 0, 0);
        }
        if self.ops.iter().any(|o| o == "Syncing" || o == "Pushing" || o == "Pulling" || o == "Publishing") {
            return ("Syncing Changes...".into(), false, 1);
        }
        if s.branch.is_some() && s.upstream.is_none() {
            return ("\u{2601} Publish Branch".into(), true, 2);
        }
        if s.upstream.is_some() && (s.ahead > 0 || s.behind > 0) {
            let mut t = "Sync Changes".to_string();
            if s.behind > 0 {
                t += &format!(" {}\u{2193}", s.behind);
            }
            if s.ahead > 0 {
                t += &format!(" {}\u{2191}", s.ahead);
            }
            return (t, true, 1);
        }
        ("\u{2713} Commit".into(), false, 0)
    }

    fn paint_commit_button(&mut self, g: &Gfx) {
        let b = self.l.button;
        let (label, enabled, kind) = self.action_button();
        let drop_w = if kind == 0 { self.s(26) } else { 0 };
        let main = rect(b.left, b.top, b.right - drop_w, b.bottom);
        let base = if enabled { self.t.accent } else { crate::theme::blend(0, 0, 0.0) };
        let _ = base;
        let bg = |hot: bool| if hot && enabled { self.t.accent_hover } else { self.t.accent };
        g.fill(&main, bg(self.hover == Hit::CommitBtn));
        let fg = if enabled { self.t.accent_fg } else { crate::theme::blend_colorref(self.t.accent, self.t.accent_fg, 0.45) };
        g.text_in(&main, &label, self.fonts.ui, fg, DT_LINE | DT_CENTER);
        if enabled {
            self.hits.push((main, Hit::CommitBtn));
        }
        if drop_w > 0 {
            let d = rect(b.right - drop_w, b.top, b.right, b.bottom);
            g.fill(&d, bg(self.hover == Hit::CommitDrop));
            g.vline(d.left, d.top + self.s(4), d.bottom - self.s(4), crate::theme::blend_colorref(self.t.accent, 0xFFFFFF, 0.4));
            g.icon(&d, I_DOWN, self.fonts.icon_small, fg);
            if enabled {
                self.hits.push((d, Hit::CommitDrop));
            }
        }
    }

    fn row_bg(&self, g: &Gfx, r: &RECT, selected: bool, focused: bool, hovered: bool, area_focused: bool) {
        if selected {
            g.fill(r, if area_focused { self.t.sel } else { self.t.sel_inactive });
        } else if hovered {
            g.fill(r, self.t.hover);
        }
        if focused && area_focused {
            g.frame(r, self.t.focus);
        }
    }

    fn git_color(&self, f: &git::FileChange) -> u32 {
        match f.kind {
            Kind::Modified | Kind::TypeChanged => self.t.git_modified,
            Kind::Added => self.t.git_added,
            Kind::Deleted => self.t.git_deleted,
            Kind::Renamed | Kind::Copied | Kind::Untracked => self.t.git_untracked,
            Kind::Conflict => self.t.git_conflict,
        }
    }

    fn main_has_focus(&self) -> bool {
        unsafe { GetFocus() == self.hwnd }
    }

    fn paint_changes(&mut self, g: &Gfx) {
        let r = self.l.changes;
        let rh22 = self.s(22);
        let saved = g.clip(&r);
        let scroll = self.scroll[Area::Changes as usize];
        let first = (scroll / rh22).max(0) as usize;
        let n = (rh(&r) / rh22 + 2) as usize;
        let area_focused = self.main_has_focus() && self.focus == Focus::Changes;
        let rows: Vec<(usize, CRow)> = self.crows.iter().copied().enumerate().skip(first).take(n).collect();
        for (i, row) in rows {
            let y = r.top + i as i32 * rh22 - scroll;
            let rr = rect(r.left, y, r.right, y + rh22);
            match row {
                CRow::Group(gr) => {
                    let hovered = matches!(self.hover, Hit::GroupRow(x) | Hit::GroupAct(x, _) if x == i);
                    self.row_bg(g, &rr, false, self.focus_row == Some(i), hovered, area_focused);
                    let col = self.collapsed[group_id(gr) as usize];
                    g.icon(&xywh(rr.left + self.s(4), y, self.s(16), rh22), if col { I_RIGHT } else { I_DOWN }, self.fonts.icon_small, self.t.fg);
                    g.text_in(&rect(rr.left + self.s(24), y, rr.right - self.s(90), y + rh22), group_label(gr), self.fonts.ui, self.t.fg, DT_LINE);
                    let count = self.status().map(|s| s.count(gr)).unwrap_or(0);
                    let cs = count.to_string();
                    let bw = g.text_w(&cs, self.fonts.cap).max(self.s(6)) + self.s(12);
                    let badge = xywh(rr.right - self.s(12) - bw, y + self.s(2), bw, self.s(18));
                    g.round_rect(&badge, self.s(18), self.t.badge_bg, None);
                    g.text_in(&badge, &cs, self.fonts.cap, self.t.badge_fg, DT_LINE | DT_CENTER);
                    self.hits.push((rr, Hit::GroupRow(i)));
                    if hovered {
                        let acts: Vec<(char, u8)> = match gr {
                            Group::Merge => vec![(I_ADD, 0)],
                            Group::Staged => vec![(I_REMOVE, 1)],
                            Group::Changes => vec![(I_UNDO, 2), (I_ADD, 0)],
                        };
                        let mut x = badge.left - self.s(4) - acts.len() as i32 * self.s(22);
                        for (glyph, a) in acts {
                            self.icon_button(g, xywh(x, y + self.s(1), self.s(20), self.s(20)), glyph, Hit::GroupAct(i, a), true);
                            x += self.s(22);
                        }
                    }
                }
                CRow::File(fi) => {
                    let Some(f) = self.file(fi).cloned() else { continue };
                    let selected = self.sel.contains(&(group_id(f.group), f.path.clone()));
                    let hovered = matches!(self.hover, Hit::FileRow(x) | Hit::FileAct(x, _) if x == i);
                    self.row_bg(g, &rr, selected, self.focus_row == Some(i), hovered, area_focused);
                    let x0 = rr.left + self.s(28);
                    let color = self.git_color(&f);
                    g.icon(&xywh(x0, y, self.s(16), rh22), I_DOC, self.fonts.icon_small, self.t.fg_dim);
                    let name = short_name(&f.path).to_string();
                    let dir = f.path.rsplit_once('/').map(|(d, _)| d.to_string()).unwrap_or_default();
                    let nx = x0 + self.s(22);
                    let letter_w = self.s(28);
                    let acts: Vec<(char, u8)> = if hovered || selected {
                        match f.group {
                            Group::Merge => vec![(I_OPEN, 3), (I_ADD, 0)],
                            Group::Staged => vec![(I_OPEN, 3), (I_REMOVE, 1)],
                            Group::Changes => vec![(I_OPEN, 3), (I_UNDO, 2), (I_ADD, 0)],
                        }
                    } else {
                        vec![]
                    };
                    let acts_w = acts.len() as i32 * self.s(22);
                    let text_right = rr.right - letter_w - acts_w - self.s(4);
                    let nw = g.text_w(&name, self.fonts.ui);
                    g.text_in(&rect(nx, y, text_right, y + rh22), &name, self.fonts.ui, color, DT_LINE);
                    if f.kind == Kind::Deleted {
                        let sw = nw.min(text_right - nx);
                        g.hline(nx, nx + sw, y + rh22 / 2 + self.s(1), color);
                    }
                    if !dir.is_empty() && nx + nw + self.s(8) < text_right {
                        let d = match &f.orig {
                            Some(o) => format!("{dir} \u{2190} {}", short_name(o)),
                            None => dir,
                        };
                        g.text_in(&rect(nx + nw + self.s(6), y, text_right, y + rh22), &d, self.fonts.small, self.t.fg_dim, DT_LINE);
                    }
                    let mut ax = text_right + self.s(2);
                    for (glyph, a) in acts {
                        self.icon_button(g, xywh(ax, y + self.s(1), self.s(20), self.s(20)), glyph, Hit::FileAct(i, a), true);
                        ax += self.s(22);
                    }
                    g.text_in(&rect(rr.right - letter_w, y, rr.right - self.s(12), y + rh22), &f.letter().to_string(), self.fonts.small, color, DT_LINE | DT_CENTER);
                    self.hits.push((rr, Hit::FileRow(i)));
                }
            }
        }
        g.unclip(saved);
        self.draw_scrollbar(g, Area::Changes);
    }

    fn paint_graph(&mut self, g: &Gfx) {
        let r = self.l.graph;
        let rh22 = self.s(22);
        let saved = g.clip(&r);
        let scroll = self.scroll[Area::Graph as usize];
        let first = (scroll / rh22).max(0) as usize;
        let n = (rh(&r) / rh22 + 2) as usize;
        let area_focused = self.main_has_focus() && self.focus == Focus::Graph;
        let lane_w = self.s(11) as f32;
        let (branch, upstream) = self.status().map(|s| (s.branch.clone(), s.upstream.clone())).unwrap_or((None, None));
        for i in first..(first + n).min(self.commits.len()) {
            let y = r.top + i as i32 * rh22 - scroll;
            let rr = rect(r.left, y, r.right, y + rh22);
            let hovered = self.hover == Hit::GraphRow(i);
            self.row_bg(g, &rr, self.graph_sel == Some(i), false, hovered, area_focused);
            let c = &self.commits[i];
            let shape = graph::shape(&self.graph_rows[i], c);
            let gx = rr.left + self.s(6);
            let bg_now = if self.graph_sel == Some(i) { if area_focused { self.t.sel } else { self.t.sel_inactive } } else if hovered { self.t.hover } else { self.t.sidebar };
            for seg in &shape.segs {
                match *seg {
                    Seg::V { x, y0, y1, color } => {
                        let px = gx + (x * lane_w) as i32;
                        g.line(px, y + (y0 * rh22 as f32) as i32, px, y + (y1 * rh22 as f32) as i32, self.t.lane(color), self.s(1).max(1));
                    }
                    Seg::H { x0, x1, y: yy, color } => {
                        let py = y + (yy * rh22 as f32) as i32;
                        g.line(gx + (x0 * lane_w) as i32, py, gx + (x1 * lane_w) as i32, py, self.t.lane(color), self.s(1).max(1));
                    }
                    Seg::C { x0, y0, x1, y1, color } => {
                        let p0 = POINT { x: gx + (x0 * lane_w) as i32, y: y + (y0 * rh22 as f32) as i32 };
                        let p3 = POINT { x: gx + (x1 * lane_w) as i32, y: y + (y1 * rh22 as f32) as i32 };
                        let my = (p0.y + p3.y) / 2;
                        let pts = [p0, POINT { x: p0.x, y: my }, POINT { x: p3.x, y: my }, p3];
                        g.bezier(&pts, self.t.lane(color), self.s(1).max(1));
                    }
                }
            }
            let cx = gx + (shape.circle_x * lane_w) as i32;
            let cy = y + rh22 / 2;
            let col = self.t.lane(shape.circle_color);
            if self.graph_rows[i].current {
                g.ellipse(cx, cy, self.s(6), col, bg_now, self.s(2));
                g.ellipse(cx, cy, self.s(2), bg_now, bg_now, 1);
            } else if shape.merge {
                g.ellipse(cx, cy, self.s(5), col, bg_now, self.s(2));
                g.ellipse(cx, cy, self.s(2), col, bg_now, self.s(1));
            } else {
                g.ellipse(cx, cy, self.s(4), col, bg_now, self.s(1));
            }
            let tx = gx + (shape.width as f32 * lane_w) as i32 + self.s(4);
            // badges (right)
            let mut right = rr.right - self.s(10);
            let mut badges: Vec<(String, u32, bool)> = Vec::new();
            for d in &c.refs {
                let is_cur = d.kind == RefKind::Head && Some(&d.name) == branch.as_ref();
                let is_up = d.kind == RefKind::Remote && Some(&d.name) == upstream.as_ref();
                if !(self.st.graph_all || is_cur || is_up || d.kind == RefKind::Tag) {
                    continue;
                }
                let color = if is_cur { self.t.ref_color } else if is_up { self.t.remote_color } else { col };
                let label = match d.kind {
                    RefKind::Tag => d.name.clone(),
                    RefKind::Remote => format!("\u{2601} {}", d.name),
                    RefKind::Head => format!("{}{}", if is_cur { "\u{25CE} " } else { "" }, d.name),
                };
                badges.push((label, color, false));
            }
            for (label, color, _) in badges.iter().rev() {
                let tw = g.text_w(label, self.fonts.cap).min(self.s(120));
                let b = rect(right - tw - self.s(12), y + self.s(3), right, y + rh22 - self.s(3));
                if b.left < tx + self.s(60) {
                    break;
                }
                g.round_rect(&b, self.s(14), *color, None);
                g.text_in(&rect(b.left + self.s(6), b.top, b.right - self.s(5), b.bottom), label, self.fonts.cap, self.t.label_fg, DT_LINE);
                right = b.left - self.s(4);
            }
            let subject = c.subject().to_string();
            let sw = g.text_w(&subject, self.fonts.ui);
            g.text_in(&rect(tx, y, right - self.s(4), y + rh22), &subject, self.fonts.ui, self.t.fg, DT_LINE);
            if tx + sw + self.s(10) < right - self.s(4) {
                g.text_in(&rect(tx + sw + self.s(8), y, right - self.s(4), y + rh22), &c.author, self.fonts.small, self.t.fg_dim, DT_LINE);
            }
            self.hits.push((rr, Hit::GraphRow(i)));
        }
        if self.has_more {
            let i = self.commits.len();
            let y = r.top + i as i32 * rh22 - scroll;
            let rr = rect(r.left, y, r.right, y + rh22);
            if self.hover == Hit::GraphMore {
                g.fill(&rr, self.t.hover);
            }
            g.text_in(&rect(rr.left + self.s(16), y, rr.right, y + rh22), if self.log_loading { "Loading..." } else { "Load More..." }, self.fonts.ui, self.t.link, DT_LINE);
            self.hits.push((rr, Hit::GraphMore));
        }
        g.unclip(saved);
        self.draw_scrollbar(g, Area::Graph);
    }

    // ------------------------------------------------------------- main

    fn paint_main(&mut self, g: &Gfx) {
        let l = self.l;
        g.fill(&l.main, self.t.editor);
        // header (tab-like)
        let hd = l.main_header;
        g.fill(&hd, self.t.sidebar);
        g.hline(hd.left, hd.right, hd.bottom - 1, self.t.border);
        let (title, desc) = match &self.view {
            MainView::Diff(d) => (d.title.clone(), d.desc.clone()),
            MainView::File(f) => (f.title.clone(), f.path.clone()),
            MainView::Commit(c) => (format!("{} ({})", c.commit.subject(), &c.commit.hash[..8]), if c.stash.is_some() { "Stash".into() } else { "Commit".into() }),
            MainView::Message(t, _) => (t.clone(), String::new()),
            MainView::Loading(t) => (t.clone(), "Loading...".into()),
            MainView::Empty => (String::new(), String::new()),
        };
        if !title.is_empty() {
            let tw = g.text_w(&title, self.fonts.ui) + self.s(44);
            let tab = rect(hd.left, hd.top, (hd.left + tw).min(hd.right - self.s(200)), hd.bottom);
            g.fill(&tab, self.t.editor);
            g.hline(tab.left, tab.right, tab.top, self.t.accent);
            g.vline(tab.right, tab.top, tab.bottom, self.t.border);
            g.icon(&xywh(tab.left + self.s(8), tab.top, self.s(18), rh(&tab)), I_DOC, self.fonts.icon_small, self.t.fg_dim);
            g.text_in(&rect(tab.left + self.s(30), tab.top, tab.right - self.s(6), tab.bottom), &title, self.fonts.ui, self.t.fg_strong, DT_LINE);
            if !desc.is_empty() && tab.right + self.s(12) < hd.right - self.s(220) {
                g.text_in(&rect(tab.right + self.s(12), hd.top, hd.right - self.s(220), hd.bottom), &desc, self.fonts.small, self.t.fg_dim, DT_LINE);
            }
        }
        self.paint_main_actions(g);
        let body = l.main_body;
        let saved = g.clip(&body);
        match &self.view {
            MainView::Empty => self.paint_watermark(g),
            MainView::Loading(_) => g.text_in(&body, "Loading...", self.fonts.ui, self.t.fg_dim, DT_LINE | DT_CENTER),
            MainView::Message(t, m) => {
                let (t, m) = (t.clone(), m.clone());
                let r = rect(body.left + self.s(40), body.top + self.s(60), body.right - self.s(40), body.bottom);
                g.text_in(&xywh(r.left, r.top, rw(&r), self.s(30)), &t, self.fonts.big, self.t.fg, DT_LINE | DT_CENTER);
                g.text_in(&rect(r.left, r.top + self.s(40), r.right, r.bottom), &m, self.fonts.ui, self.t.fg_dim, DT_WORDBREAK | DT_CENTER | DT_NOPREFIX);
            }
            MainView::Diff(_) => self.paint_diff(g),
            MainView::File(_) => self.paint_file(g),
            MainView::Commit(_) => self.paint_commit_view(g),
        }
        g.unclip(saved);
        self.draw_scrollbar(g, Area::Main);
    }

    fn paint_main_actions(&mut self, g: &Gfx) {
        let hd = self.l.main_header;
        let bs = self.s(24);
        let y = hd.top + (rh(&hd) - bs) / 2;
        let mut x = hd.right - self.s(10) - bs;
        let (is_diff, kind, has_back, has_change) = match &self.view {
            MainView::Diff(d) => (true, Some(d.kind), d.back.is_some(), d.change.is_some()),
            MainView::File(f) => (false, None, false, f.change.is_some()),
            _ => (false, None, false, false),
        };
        if is_diff {
            self.icon_button(g, xywh(x, y, bs, bs), I_DOWN_ARROW, Hit::Cmd(Cmd::NextChange), true);
            x -= bs + self.s(2);
            self.icon_button(g, xywh(x, y, bs, bs), I_UP_ARROW, Hit::Cmd(Cmd::PrevChange), true);
            x -= self.s(4);
            let l1 = if self.st.compact_diff { "Show All" } else { "Collapse" };
            x = self.text_button(g, x, y, bs, l1, Hit::Cmd(Cmd::ToggleCompact)) - self.s(2);
            let l2 = if self.st.inline_diff { "Side by Side" } else { "Inline" };
            x = self.text_button(g, x, y, bs, l2, Hit::Cmd(Cmd::ToggleInline)) - self.s(2) - bs;
        }
        if has_change && !has_back {
            match kind {
                Some(DiffKind::WorkingTree) | Some(DiffKind::Untracked) | None => {
                    self.icon_button(g, xywh(x, y, bs, bs), I_ADD, Hit::DiffBlock(usize::MAX, 0), true);
                    x -= bs + self.s(2);
                    if kind.is_some() {
                        self.icon_button(g, xywh(x, y, bs, bs), I_UNDO, Hit::DiffBlock(usize::MAX, 2), true);
                        x -= bs + self.s(2);
                    }
                }
                Some(DiffKind::Index) => {
                    self.icon_button(g, xywh(x, y, bs, bs), I_REMOVE, Hit::DiffBlock(usize::MAX, 1), true);
                    x -= bs + self.s(2);
                }
                _ => {}
            }
            self.icon_button(g, xywh(x, y, bs, bs), I_OPEN, Hit::DiffBlock(usize::MAX, 3), true);
            x -= bs + self.s(2);
        }
        if has_back {
            self.icon_button(g, xywh(x, y, bs, bs), I_BACK, Hit::DiffBack, true);
        }
    }

    fn paint_watermark(&mut self, g: &Gfx) {
        let b = self.l.main_body;
        let cx = b.left + rw(&b) / 2;
        let cy = b.top + rh(&b) / 2 - self.s(80);
        draw_branch_icon(g, cx, cy, self.s(120), crate::theme::blend_colorref(self.t.editor, self.t.fg, 0.08), self.scale * 4.0);
        let lines = [("Show All Commands", "Ctrl+Shift+P"), ("Open Folder", "Ctrl+O"), ("Open Recent", "Ctrl+R"), ("New Window", "Ctrl+Shift+N"), ("Toggle Terminal", "Ctrl+`")];
        let mut y = cy + self.s(100);
        for (label, key) in lines {
            g.text_in(&rect(b.left, y, cx - self.s(10), y + self.s(26)), label, self.fonts.ui, self.t.fg_dim, DT_LINE | DT_RIGHT);
            let kw = g.text_w(key, self.fonts.small) + self.s(10);
            let kr = xywh(cx + self.s(6), y + self.s(3), kw, self.s(20));
            g.round_rect(&kr, self.s(4), crate::theme::blend_colorref(self.t.editor, self.t.fg, 0.08), Some(crate::theme::blend_colorref(self.t.editor, self.t.fg, 0.2)));
            g.text_in(&kr, key, self.fonts.small, self.t.fg_dim, DT_LINE | DT_CENTER);
            y += self.s(30);
        }
    }

    fn paint_diff(&mut self, g: &Gfx) {
        let body = self.l.main_body;
        let lh = self.line_h();
        let scroll = self.scroll[Area::Main as usize];
        let inline = self.st.inline_diff;
        let mw = self.fonts.mono_w;
        let MainView::Diff(d) = &self.view else { return };
        let digits = d.fd.hunks.iter().map(|h| (h.old_start + h.old_len).max(h.new_start + h.new_len)).max().unwrap_or(1).to_string().len() as i32;
        let num_w = digits.max(3) * mw + self.s(14);
        let first = (scroll / lh).max(0) as usize;
        let n = (rh(&body) / lh + 2) as usize;
        let top = body.top + self.s(4);
        let width = rw(&body) - self.s(12);
        let (left_x, right_x, half) = if inline { (body.left, body.left, width) } else { (body.left, body.left + width / 2, width / 2) };
        let sel = d.sel.map(|(a, b)| (a.min(b), a.max(b)));
        let hover_block = match self.hover {
            Hit::DiffRow(r) => d.rows.get(r).and_then(|x| x.block),
            Hit::DiffBlock(b, _) if b != usize::MAX => Some(b),
            _ => None,
        };
        let hs = self.hscroll;
        let mut block_btns: Vec<(usize, i32)> = Vec::new();
        for i in first..(first + n).min(d.rows.len()) {
            let row = &d.rows[i];
            let y = top + i as i32 * lh - scroll;
            let full = rect(body.left, y, body.right, y + lh);
            if let Some(hi) = row.sep {
                let h = &d.fd.hunks[hi];
                g.fill(&full, crate::theme::blend_colorref(self.t.editor, self.t.info, 0.08));
                let t = format!("@@ -{},{} +{},{} @@ {}", h.old_start, h.old_len, h.new_start, h.new_len, h.section);
                g.text_in(&rect(body.left + self.s(10), y, body.right, y + lh), &t, self.fonts.mono, self.t.fg_dim, DT_LINE);
                self.hits.push((full, Hit::DiffRow(i)));
                continue;
            }
            let selected = sel.map(|(a, b)| i >= a && i <= b).unwrap_or(false);
            let draw_side = |id: Option<(usize, usize)>, x: i32, wdt: i32, right_side: bool| {
                let side = rect(x, y, x + wdt, y + lh);
                match id {
                    None => g.fill(&side, self.t.empty_line),
                    Some((hi, li)) => {
                        let line = &d.fd.hunks[hi].lines[li];
                        let (bg, gut, no) = match line.kind {
                            LK::Add => (self.t.ins_line, self.t.ins_text, line.new),
                            LK::Del => (self.t.del_line, self.t.del_text, line.old),
                            _ => (self.t.editor, self.t.editor, if right_side || inline { line.new } else { line.old }),
                        };
                        g.fill(&side, bg);
                        let mut nx = x;
                        if inline {
                            let ow = num_w;
                            let gr = rect(x, y, x + ow * 2, y + lh);
                            if gut != self.t.editor {
                                g.fill(&gr, gut);
                            }
                            if line.old > 0 {
                                g.text_in(&rect(x, y, x + ow - self.s(6), y + lh), &line.old.to_string(), self.fonts.mono, self.t.line_no, DT_LINE | DT_RIGHT);
                            }
                            if line.new > 0 {
                                g.text_in(&rect(x + ow, y, x + ow * 2 - self.s(6), y + lh), &line.new.to_string(), self.fonts.mono, self.t.line_no, DT_LINE | DT_RIGHT);
                            }
                            nx += ow * 2;
                        } else {
                            let gr = rect(x, y, x + num_w, y + lh);
                            if gut != self.t.editor {
                                g.fill(&gr, gut);
                            }
                            if no > 0 {
                                g.text_in(&rect(x, y, x + num_w - self.s(6), y + lh), &no.to_string(), self.fonts.mono, self.t.line_no, DT_LINE | DT_RIGHT);
                            }
                            nx += num_w;
                        }
                        let marker = match line.kind {
                            LK::Add => "+",
                            LK::Del => "-",
                            _ => " ",
                        };
                        g.text(nx + self.s(2), y + self.s(1), marker, self.fonts.mono, self.t.fg_dim);
                        let text = expand_tabs(&line.text);
                        let clip = rect(nx + mw + self.s(4), y, x + wdt, y + lh);
                        g.text_clip(nx + mw + self.s(6) - hs, y + self.s(1), &text, self.fonts.mono, self.t.fg, &clip);
                    }
                }
            };
            if inline {
                draw_side(row.left, left_x, half, false);
            } else {
                draw_side(row.left, left_x, half, false);
                draw_side(row.right, right_x, width - half, true);
                g.vline(right_x - 1, y, y + lh, self.t.border);
            }
            if selected {
                g.fill(&rect(body.left, y, body.left + self.s(3), y + lh), self.t.accent);
                let ov = rect(body.left + self.s(3), y, body.right - self.s(12), y + lh);
                g.frame(&ov, crate::theme::blend_colorref(self.t.editor, self.t.accent, 0.6));
            }
            self.hits.push((rect(body.left, y, body.right - self.s(12), y + lh), Hit::DiffRow(i)));
            if let Some(b) = row.block {
                if Some(b) == hover_block && (i == 0 || d.rows[i - 1].block != Some(b)) {
                    block_btns.push((b, y));
                }
            }
        }
        // block action buttons (on hover)
        let kind = d.kind;
        for (b, y) in block_btns {
            let acts: Vec<(&str, u8)> = match kind {
                DiffKind::WorkingTree => vec![("Stage Block", 0), ("Revert Block", 2)],
                DiffKind::Untracked => vec![("Stage File", 0)],
                DiffKind::Index => vec![("Unstage Block", 1)],
                DiffKind::Commit => vec![],
            };
            let mut x = body.right - self.s(20);
            for (label, a) in acts.iter().rev() {
                let tw = g.text_w(label, self.fonts.small) + self.s(14);
                let r = rect(x - tw, y + self.s(1), x, y + lh - self.s(1));
                let hit = Hit::DiffBlock(b, *a);
                g.round_rect(&r, self.s(4), if self.hover == hit { self.t.accent_hover } else { self.t.accent }, None);
                g.text_in(&r, label, self.fonts.small, self.t.accent_fg, DT_LINE | DT_CENTER);
                self.hits.push((r, hit));
                x = r.left - self.s(4);
            }
        }
    }

    fn paint_file(&mut self, g: &Gfx) {
        let body = self.l.main_body;
        let lh = self.line_h();
        let scroll = self.scroll[Area::Main as usize];
        let mw = self.fonts.mono_w;
        let MainView::File(f) = &self.view else { return };
        // virtual rows: conflict action row before each conflict start
        let mut rows: Vec<(bool, usize)> = Vec::with_capacity(f.lines.len() + f.conflicts.len());
        let starts: std::collections::HashSet<usize> = f.conflicts.iter().map(|c| c.start).collect();
        for i in 0..f.lines.len() {
            if starts.contains(&i) {
                rows.push((true, i));
            }
            rows.push((false, i));
        }
        let num_w = (f.lines.len().to_string().len() as i32).max(3) * mw + self.s(16);
        let first = (scroll / lh).max(0) as usize;
        let n = (rh(&body) / lh + 2) as usize;
        let top = body.top + self.s(4);
        let mut hits = Vec::new();
        for (k, &(is_action, li)) in rows.iter().enumerate().skip(first).take(n) {
            let y = top + k as i32 * lh - scroll;
            if is_action {
                let mut x = body.left + num_w + self.s(8);
                for (label, w) in [("Accept Current Change", 0u8), ("Accept Incoming Change", 1), ("Accept Both Changes", 2)] {
                    let tw = g.text_w(label, self.fonts.small);
                    let r = rect(x, y, x + tw, y + lh);
                    let hit = Hit::Conflict(li, w);
                    g.text_in(&r, label, self.fonts.small, if self.hover == hit { self.t.link } else { crate::theme::blend_colorref(self.t.editor, self.t.fg, 0.6) }, DT_LINE);
                    hits.push((r, hit));
                    x += tw + self.s(6);
                    g.text_in(&rect(x, y, x + self.s(10), y + lh), "|", self.fonts.small, self.t.fg_dim, DT_LINE);
                    x += self.s(10);
                }
                continue;
            }
            let full = rect(body.left, y, body.right, y + lh);
            let mut label = None;
            for c in &f.conflicts {
                if li == c.start {
                    g.fill(&full, self.t.merge_current_header);
                    label = Some(" (Current Change)");
                } else if li > c.start && li < c.base.unwrap_or(c.mid) {
                    g.fill(&full, self.t.merge_current);
                } else if li == c.end {
                    g.fill(&full, self.t.merge_incoming_header);
                    label = Some(" (Incoming Change)");
                } else if li > c.mid && li < c.end {
                    g.fill(&full, self.t.merge_incoming);
                }
            }
            g.text_in(&rect(body.left, y, body.left + num_w - self.s(8), y + lh), &(li + 1).to_string(), self.fonts.mono, self.t.line_no, DT_LINE | DT_RIGHT);
            let text = expand_tabs(&f.lines[li]);
            let clip = rect(body.left + num_w, y, body.right - self.s(12), y + lh);
            g.text_clip(body.left + num_w + self.s(4) - self.hscroll, y + self.s(1), &text, self.fonts.mono, self.t.fg, &clip);
            if let Some(lbl) = label {
                let tw = text.len() as i32 * mw;
                g.text(body.left + num_w + self.s(4) + tw - self.hscroll, y + self.s(1), lbl, self.fonts.mono, self.t.fg_dim);
            }
        }
        self.hits.extend(hits);
    }

    pub fn commit_header_h(&self, c: &CommitView) -> i32 {
        let body_lines = c.commit.message.lines().skip(1).filter(|l| !l.trim().is_empty()).count().min(12) as i32;
        self.s(36) + self.s(24) + body_lines * self.s(18) + self.s(16) + self.s(30)
    }

    fn paint_commit_view(&mut self, g: &Gfx) {
        let body = self.l.main_body;
        let scroll = self.scroll[Area::Main as usize];
        let rh22 = self.s(22);
        let MainView::Commit(cv) = &self.view else { return };
        let hh = self.commit_header_h(cv);
        let x = body.left + self.s(24);
        let mut y = body.top + self.s(12) - scroll;
        g.text_in(&rect(x, y, body.right - self.s(24), y + self.s(30)), cv.commit.subject(), self.fonts.big, self.t.fg_strong, DT_LINE);
        y += self.s(36);
        let meta = format!("{} <{}>  \u{2022}  {} ({})  \u{2022}  {}", cv.commit.author, cv.commit.email, win::fmt_age(cv.commit.date), win::fmt_date(cv.commit.date), &cv.commit.hash);
        g.text_in(&rect(x, y, body.right - self.s(24), y + self.s(20)), &meta, self.fonts.small, self.t.fg_dim, DT_LINE);
        y += self.s(24);
        for l in cv.commit.message.lines().skip(1).filter(|l| !l.trim().is_empty()).take(12) {
            g.text_in(&rect(x, y, body.right - self.s(24), y + self.s(18)), l, self.fonts.ui, self.t.fg, DT_LINE);
            y += self.s(18);
        }
        y += self.s(16);
        g.text_in(&rect(x, y, body.right, y + self.s(22)), &format!("{} CHANGED FILE{}", cv.files.len(), if cv.files.len() == 1 { "" } else { "S" }), self.fonts.bold, self.t.fg_dim, DT_LINE);
        let list_top = body.top + self.s(12) + hh - scroll;
        let mut hits = Vec::new();
        for (i, f) in cv.files.iter().enumerate() {
            let y = list_top + i as i32 * rh22;
            if y + rh22 < body.top || y > body.bottom {
                continue;
            }
            let rr = rect(body.left + self.s(12), y, body.right - self.s(12), y + rh22);
            let hit = Hit::CommitFile(i);
            if self.hover == hit {
                g.fill(&rr, self.t.hover);
            } else if cv.sel == Some(i) {
                g.fill(&rr, self.t.sel_inactive);
            }
            let color = self.git_color(f);
            g.icon(&xywh(rr.left + self.s(8), y, self.s(16), rh22), I_DOC, self.fonts.icon_small, self.t.fg_dim);
            let name = short_name(&f.path);
            let nw = g.text_w(name, self.fonts.ui);
            g.text_in(&rect(rr.left + self.s(30), y, rr.right - self.s(30), y + rh22), name, self.fonts.ui, color, DT_LINE);
            let dir = match (&f.orig, f.path.rsplit_once('/')) {
                (Some(o), _) => format!("\u{2190} {o}"),
                (None, Some((d, _))) => d.to_string(),
                _ => String::new(),
            };
            g.text_in(&rect(rr.left + self.s(36) + nw, y, rr.right - self.s(30), y + rh22), &dir, self.fonts.small, self.t.fg_dim, DT_LINE);
            g.text_in(&rect(rr.right - self.s(28), y, rr.right - self.s(8), y + rh22), &f.letter().to_string(), self.fonts.small, color, DT_LINE | DT_CENTER);
            hits.push((rr, hit));
        }
        self.hits.extend(hits);
    }

    // ------------------------------------------------------------- output

    fn paint_output(&mut self, g: &Gfx) {
        let o = self.l.output;
        g.fill(&o, self.t.sidebar);
        g.hline(o.left, o.right, o.top, self.t.border);
        self.hits.push((rect(o.left, o.top - self.s(2), o.right, o.top + self.s(3)), Hit::Sash(Sash::Output)));
        let term_tab = self.st.panel_terminal;
        // tabs
        let mut x = o.left + self.s(12);
        for (i, label) in ["OUTPUT", "TERMINAL"].iter().enumerate() {
            let tw = g.text_w(label, self.fonts.cap);
            let tr = rect(x, o.top + self.s(4), x + tw + self.s(16), o.top + self.s(31));
            let active = (i == 1) == term_tab;
            let hit = Hit::PanelTab(i as u8);
            g.text_in(&tr, label, self.fonts.cap, if active || self.hover == hit { self.t.fg } else { self.t.fg_dim }, DT_LINE | DT_CENTER);
            if active {
                g.hline(tr.left + self.s(8), tr.right - self.s(8), o.top + self.s(28), self.t.accent);
            }
            self.hits.push((tr, hit));
            x = tr.right + self.s(4);
        }
        let bs = self.s(22);
        let y = o.top + (self.s(35) - bs) / 2;
        let mut rx = o.right - self.s(10) - bs;
        self.icon_button(g, xywh(rx, y, bs, bs), I_CLOSE, Hit::Cmd(if term_tab { Cmd::ToggleTerminal } else { Cmd::ToggleOutput }), true);
        rx -= bs + self.s(2);
        if term_tab {
            self.icon_button(g, xywh(rx, y, bs, bs), '\u{E74D}', Hit::Cmd(Cmd::KillTerminal), true);
            rx -= bs + self.s(2);
            self.icon_button(g, xywh(rx, y, bs, bs), I_ADD, Hit::Cmd(Cmd::NewTerminal), true);
            rx -= self.s(4);
            let name = match &self.term {
                Some(t) => {
                    let title = t.screen.title.trim();
                    // shells often set the title to their own executable path; show the profile name then
                    let is_path = title.to_ascii_lowercase().ends_with(".exe") || title.contains(":\\");
                    if title.is_empty() || is_path { t.shell.name.clone() } else { format!("{} \u{2022} {}", t.shell.name, title) }
                }
                None => "Terminal".into(),
            };
            let name = format!("{} \u{25BE}", name.chars().take(60).collect::<String>());
            let tw = (g.text_w(&name, self.fonts.small) + self.s(14)).min(rw(&o) / 2);
            let r = rect(rx - tw, y, rx, y + bs);
            if self.hover == Hit::TermShellMenu {
                g.round_rect(&r, self.s(5), self.t.toolbar_hover, None);
            }
            g.text_in(&rect(r.left + self.s(6), r.top, r.right - self.s(6), r.bottom), &name, self.fonts.small, self.t.fg, DT_LINE);
            self.hits.push((r, Hit::TermShellMenu));
            self.paint_terminal(g);
            return;
        }
        self.icon_button(g, xywh(rx, y, bs, bs), '\u{E894}', Hit::Cmd(Cmd::ClearOutput), true);
        let body = self.l.output_body;
        let lines = git::log_snapshot();
        let lh = self.fonts.mono_small_h;
        if lines.len() != self.output_seen {
            self.output_seen = lines.len();
            self.scroll[Area::Output as usize] = i32::MAX;
            self.clamp_scroll(Area::Output);
        }
        let scroll = self.scroll[Area::Output as usize];
        let saved = g.clip(&body);
        let first = (scroll / lh).max(0) as usize;
        for (i, line) in lines.iter().enumerate().skip(first).take((rh(&body) / lh + 2) as usize) {
            let y = body.top + i as i32 * lh - scroll;
            let is_err = !line.contains(" > git ");
            let ws = win::w(line);
            g.text_clip(body.left + self.s(20), y, &ws, self.fonts.mono_small, if is_err { self.t.fg_dim } else { self.t.fg }, &body);
        }
        g.unclip(saved);
        self.draw_scrollbar(g, Area::Output);
    }

    // ------------------------------------------------------------- status bar

    fn paint_status(&mut self, g: &Gfx) {
        let r = self.l.status;
        g.fill(&r, self.t.sidebar);
        g.hline(r.left, r.right, r.top, self.t.border);
        let remote = rect(r.left, r.top + 1, r.left + self.s(36), r.bottom);
        g.fill(&remote, self.t.accent);
        g.icon(&remote, '\u{E8AB}', self.fonts.icon_small, self.t.accent_fg);
        let mut x = remote.right + self.s(6);
        let item = |app: &mut App, g: &Gfx, x: &mut i32, text: &str, hit: Option<Hit>| {
            let tw = g.text_w(text, app.fonts.small) + app.s(10);
            let rr = rect(*x, r.top + 1, *x + tw, r.bottom);
            if let Some(h) = hit {
                if app.hover == h {
                    g.fill(&rr, app.t.toolbar_hover);
                }
                app.hits.push((rr, h));
            }
            g.text_in(&rr, text, app.fonts.small, app.t.fg, DT_LINE | DT_CENTER);
            *x += tw;
        };
        if let Some(s) = self.status().cloned() {
            let dirty = if s.count(Group::Changes) > 0 { "*" } else { "" };
            let staged = if s.count(Group::Staged) > 0 { "+" } else { "" };
            let conflict = if s.count(Group::Merge) > 0 { "!" } else { "" };
            let head = match &s.branch {
                Some(b) => b.clone(),
                None => s.head.as_deref().map(|h| h[..h.len().min(8)].to_string()).unwrap_or_else(|| "(no commits)".into()),
            };
            let label = format!("\u{2387} {head}{dirty}{staged}{conflict}");
            item(self, g, &mut x, &label, Some(Hit::StatusBranch));
            if s.upstream.is_some() {
                let spin = if self.ops.iter().any(|o| o == "Syncing" || o == "Pushing" || o == "Pulling" || o == "Fetching") { ["\u{25D0}", "\u{25D3}", "\u{25D1}", "\u{25D2}"][(self.spin % 4) as usize] } else { "\u{21BB}" };
                item(self, g, &mut x, &format!("{spin} {}\u{2193} {}\u{2191}", s.behind, s.ahead), Some(Hit::StatusSync));
            } else if s.branch.is_some() {
                item(self, g, &mut x, "\u{2601} Publish", Some(Hit::Cmd(Cmd::Publish)));
            }
            if s.merging || s.rebasing || s.cherry_picking {
                let what = if s.rebasing { "Rebase" } else if s.cherry_picking { "Cherry Pick" } else { "Merge" };
                item(self, g, &mut x, &format!("\u{26A0} {what} in progress"), Some(Hit::Cmd(if s.rebasing { Cmd::RebaseAbort } else { Cmd::MergeAbort })));
            }
            if let Some(r) = &self.repo {
                if !r.stashes.is_empty() {
                    item(self, g, &mut x, &format!("\u{2630} {} stash{}", r.stashes.len(), if r.stashes.len() == 1 { "" } else { "es" }), Some(Hit::Cmd(Cmd::StashView)));
                }
            }
        }
        if !self.ops.is_empty() {
            let dots = ".".repeat((self.spin % 4) as usize);
            let t = format!("{}{}", self.ops.last().unwrap(), dots);
            item(self, g, &mut x, &t, None);
        }
        // right side
        let mut rx = r.right - self.s(8);
        let right_items: Vec<(String, Hit)> = vec![("Output".into(), Hit::Cmd(Cmd::ToggleOutput)), (if self.t.dark { "\u{263D} Dark".into() } else { "\u{2600} Light".into() }, Hit::Cmd(Cmd::ToggleTheme))];
        for (text, hit) in right_items {
            let tw = g.text_w(&text, self.fonts.small) + self.s(12);
            let rr = rect(rx - tw, r.top + 1, rx, r.bottom);
            if self.hover == hit {
                g.fill(&rr, self.t.toolbar_hover);
            }
            g.text_in(&rr, &text, self.fonts.small, self.t.fg, DT_LINE | DT_CENTER);
            self.hits.push((rr, hit));
            rx = rr.left;
        }
        if let Some(rp) = &self.repo {
            let p = rp.root.display().to_string();
            g.text_in(&rect(x + self.s(20), r.top + 1, rx - self.s(10), r.bottom), &p, self.fonts.small, self.t.fg_dim, DT_LINE | DT_RIGHT | DT_PATH_ELLIPSIS);
        }
    }

    // ------------------------------------------------------------- toasts

    fn paint_toasts(&mut self, g: &Gfx) {
        let c = self.client();
        let w = self.s(420).min(rw(&c) - self.s(20));
        let mut y = self.l.status.top - self.s(10);
        let toasts: Vec<(ToastKind, String)> = self.toasts.iter().map(|t| (t.kind, t.message.clone())).collect();
        for (i, (kind, msg)) in toasts.iter().enumerate().rev() {
            let x = rw(&c) - w - self.s(10);
            let tr = rect(x + self.s(36), 0, x + w - self.s(36), 1000);
            let th = g.text_height(&tr, msg, self.fonts.ui, DT_WORDBREAK | DT_NOPREFIX).max(self.s(20));
            let extra = if *kind == ToastKind::Error { self.s(26) } else { 0 };
            let hgt = th + self.s(22) + extra;
            let box_ = rect(x, y - hgt, x + w, y);
            g.round_rect(&box_, self.s(6), self.t.widget_bg, Some(self.t.widget_border));
            let (glyph, color) = match kind {
                ToastKind::Info => (I_INFO, self.t.info),
                ToastKind::Warning => (I_WARN, self.t.warning),
                ToastKind::Error => (I_ERROR, self.t.error),
            };
            g.icon(&xywh(x + self.s(8), box_.top + self.s(10), self.s(20), self.s(20)), glyph, self.fonts.icon, color);
            g.text_in(&rect(x + self.s(36), box_.top + self.s(11), x + w - self.s(36), box_.top + self.s(11) + th), msg, self.fonts.ui, self.t.fg, DT_WORDBREAK | DT_NOPREFIX);
            self.icon_button(g, xywh(x + w - self.s(30), box_.top + self.s(8), self.s(22), self.s(22)), I_CLOSE, Hit::ToastClose(i), true);
            if *kind == ToastKind::Error {
                let lbl = "Open Git Log";
                let tw = g.text_w(lbl, self.fonts.small) + self.s(16);
                let b = rect(x + w - self.s(10) - tw, box_.bottom - self.s(30), x + w - self.s(10), box_.bottom - self.s(8));
                g.fill(&b, if self.hover == Hit::ToastOutput { self.t.button2_hover } else { self.t.button2 });
                g.text_in(&b, lbl, self.fonts.small, self.t.fg, DT_LINE | DT_CENTER);
                self.hits.push((b, Hit::ToastOutput));
            }
            y = box_.top - self.s(8);
        }
    }

    // ------------------------------------------------------------- overlay

    fn paint_overlay(&mut self, g: &Gfx) {
        if self.overlay.is_none() {
            return;
        }
        let c = self.client();
        self.hits.push((c, Hit::Backdrop));
        let b = self.overlay_box();
        let rh22 = self.s(22);
        let Some(ov) = &self.overlay else { return };
        match &ov.kind {
            OverlayKind::Pick { items, filtered, sel, .. } => {
                let vis = (filtered.len() as i32).min(15);
                let total = rect(b.left, b.top, b.right, b.bottom + vis * rh22 + self.s(6));
                let shadow = rect(total.left - 1, total.top - 1, total.right + 1, total.bottom + 1);
                g.round_rect(&shadow, self.s(8), crate::theme::blend_colorref(self.t.editor, 0, 0.5), None);
                g.round_rect(&total, self.s(6), self.t.widget_bg, Some(self.t.widget_border));
                let er = rect(b.left + self.s(6), b.top + self.s(7), b.right - self.s(6), b.top + self.s(35));
                g.fill(&er, self.t.input_bg);
                g.frame(&er, self.t.focus);
                let lr = rect(b.left, b.bottom, b.right, b.bottom + vis * rh22);
                let saved = g.clip(&lr);
                let scroll = self.scroll[Area::Pick as usize];
                let mut hits = Vec::new();
                for (k, &ix) in filtered.iter().enumerate() {
                    let y = lr.top + k as i32 * rh22 - scroll;
                    if y + rh22 < lr.top || y > lr.bottom {
                        continue;
                    }
                    let it = &items[ix];
                    let row = rect(b.left + self.s(6), y, b.right - self.s(6), y + rh22);
                    if k == *sel {
                        g.round_rect(&row, self.s(4), self.t.sel, None);
                    } else if self.hover == Hit::PickRow(k) {
                        g.round_rect(&row, self.s(4), self.t.hover, None);
                    }
                    let mut x = row.left + self.s(10);
                    if it.icon != '\0' {
                        if it.icon == 'B' {
                            draw_branch_icon(g, x + self.s(7), y + rh22 / 2, self.s(14), self.t.fg, self.scale);
                        } else {
                            g.icon(&xywh(x, y, self.s(16), rh22), it.icon, self.fonts.icon_small, self.t.fg);
                        }
                        x += self.s(22);
                    }
                    let right_w = if it.sep.is_empty() { 0 } else { g.text_w(&it.sep, self.fonts.small) + self.s(12) };
                    let lw = g.text_w(&it.label, self.fonts.ui);
                    g.text_in(&rect(x, y, row.right - right_w, y + rh22), &it.label, self.fonts.ui, if k == *sel { self.t.fg_strong } else { self.t.fg }, DT_LINE);
                    if !it.desc.is_empty() && x + lw + self.s(10) < row.right - right_w {
                        g.text_in(&rect(x + lw + self.s(8), y, row.right - right_w, y + rh22), &it.desc, self.fonts.small, self.t.fg_dim, DT_LINE);
                    }
                    if !it.sep.is_empty() {
                        g.text_in(&rect(row.right - right_w, y, row.right - self.s(8), y + rh22), &it.sep, self.fonts.small, self.t.link, DT_LINE | DT_RIGHT);
                    }
                    hits.push((row, Hit::PickRow(k)));
                }
                if filtered.is_empty() {
                    g.text_in(&rect(b.left + self.s(16), b.bottom, b.right, b.bottom + rh22), "No matching results", self.fonts.ui, self.t.fg_dim, DT_LINE);
                }
                g.unclip(saved);
                self.hits.push((total, Hit::None));
                self.hits.extend(hits);
                if filtered.len() > 15 {
                    self.draw_scrollbar(g, Area::Pick);
                }
            }
            OverlayKind::Input { prompt, .. } => {
                let total = rect(b.left, b.top, b.right, b.bottom + self.s(28));
                g.round_rect(&total, self.s(6), self.t.widget_bg, Some(self.t.widget_border));
                let er = rect(b.left + self.s(6), b.top + self.s(7), b.right - self.s(6), b.top + self.s(35));
                g.fill(&er, self.t.input_bg);
                g.frame(&er, self.t.focus);
                g.text_in(&rect(b.left + self.s(10), b.bottom, b.right - self.s(10), b.bottom + self.s(24)), prompt, self.fonts.small, self.t.fg, DT_LINE);
                self.hits.push((total, Hit::None));
            }
            OverlayKind::Confirm { message, detail, buttons, sel, .. } => {
                let w = self.s(500).min(rw(&c) - self.s(40));
                let x = (rw(&c) - w) / 2;
                let tr = rect(x + self.s(76), 0, x + w - self.s(24), 2000);
                let mh = g.text_height(&tr, message, self.fonts.ui, DT_WORDBREAK | DT_NOPREFIX);
                let dh = if detail.is_empty() { 0 } else { g.text_height(&tr, detail, self.fonts.ui, DT_WORDBREAK | DT_NOPREFIX) + self.s(10) };
                let hgt = self.s(30) + mh + dh + self.s(70);
                let y = (rh(&c) - hgt) / 3;
                let box_ = rect(x, y, x + w, y + hgt);
                g.round_rect(&rect(box_.left - 2, box_.top - 2, box_.right + 2, box_.bottom + 2), self.s(8), crate::theme::blend_colorref(self.t.editor, 0, 0.5), None);
                g.round_rect(&box_, self.s(6), self.t.widget_bg, Some(self.t.widget_border));
                g.icon(&xywh(x + self.s(20), y + self.s(26), self.s(40), self.s(40)), I_WARN, self.fonts.icon_big, self.t.warning);
                g.text_in(&rect(tr.left, y + self.s(28), tr.right, y + self.s(28) + mh), message, self.fonts.ui, self.t.fg_strong, DT_WORDBREAK | DT_NOPREFIX);
                if !detail.is_empty() {
                    g.text_in(&rect(tr.left, y + self.s(38) + mh, tr.right, y + self.s(38) + mh + dh), detail, self.fonts.ui, self.t.fg, DT_WORDBREAK | DT_NOPREFIX);
                }
                let mut bx = box_.right - self.s(20);
                let by = box_.bottom - self.s(46);
                let mut hits = Vec::new();
                let n = buttons.len();
                for (i, label) in buttons.iter().enumerate().rev() {
                    let tw = (g.text_w(label, self.fonts.ui) + self.s(24)).max(self.s(80));
                    let br = rect(bx - tw, by, bx, by + self.s(28));
                    let primary = i == 0;
                    let hot = self.hover == Hit::DialogBtn(i);
                    let bg = if primary { if hot { self.t.accent_hover } else { self.t.accent } } else if hot { self.t.button2_hover } else { self.t.button2 };
                    g.fill(&br, bg);
                    if i == *sel {
                        g.frame(&rect(br.left - 2, br.top - 2, br.right + 2, br.bottom + 2), self.t.focus);
                    }
                    g.text_in(&br, label, self.fonts.ui, if primary { self.t.accent_fg } else { self.t.fg }, DT_LINE | DT_CENTER);
                    hits.push((br, Hit::DialogBtn(i)));
                    bx = br.left - self.s(8);
                }
                let _ = n;
                self.hits.push((box_, Hit::None));
                self.hits.extend(hits);
            }
        }
    }

    // ================================================================ input

    pub fn hit_test(&self, x: i32, y: i32) -> Hit {
        for (r, h) in self.hits.iter().rev() {
            if contains(r, x, y) {
                return *h;
            }
        }
        Hit::None
    }

    pub fn on_mouse_move(&mut self, x: i32, y: i32) {
        self.mouse = (x, y);
        if let Some(d) = &self.drag {
            match *d {
                Drag::Sash(s, start, orig) => {
                    let delta = match s {
                        Sash::Sidebar => x - start,
                        _ => y - start,
                    };
                    match s {
                        Sash::Sidebar => self.st.sidebar_w = ((orig + delta) as f32 / self.scale) as i32,
                        Sash::Graph => {
                            let top = self.l.title.bottom;
                            let h = (self.l.sidebar.bottom - top).max(1);
                            self.st.graph_split = (((orig + delta) - top) as f32 / h as f32).clamp(0.15, 0.9);
                        }
                        Sash::Output => self.st.output_h = ((orig - delta) as f32 / self.scale) as i32,
                    }
                    self.layout();
                }
                Drag::Thumb(a, start, orig) => {
                    let r = self.area_rect(a);
                    let content = self.content_h(a);
                    let view = rh(&r).max(1);
                    let ratio = content as f32 / view as f32;
                    self.scroll[a as usize] = orig + ((y - start) as f32 * ratio) as i32;
                    self.clamp_scroll(a);
                    self.invalidate();
                }
                Drag::TermSelect => {
                    if let Some(p) = self.term_pos_at(x, y) {
                        if let Some((a, _)) = self.term_sel {
                            self.term_sel = Some((a, p));
                        }
                        if y < self.term_rect().top {
                            self.term_scroll_by(1);
                        } else if y > self.term_rect().bottom {
                            self.term_scroll_by(-1);
                        }
                        self.invalidate();
                    }
                }
                Drag::DiffSelect => {
                    if let Hit::DiffRow(r) = self.hit_test(x, y) {
                        if let MainView::Diff(d) = &mut self.view {
                            if let Some((a, _)) = d.sel {
                                d.sel = Some((a, r));
                            }
                        }
                        self.invalidate();
                    } else if y > self.l.main_body.bottom - self.s(10) {
                        self.scroll_by(Area::Main, self.line_h());
                    } else if y < self.l.main_body.top + self.s(10) {
                        self.scroll_by(Area::Main, -self.line_h());
                    }
                }
            }
            return;
        }
        let h = self.hit_test(x, y);
        if h != self.hover {
            self.hover = h;
            self.invalidate();
        }
    }

    pub fn cursor_for(&self, x: i32, y: i32) -> Option<windows_sys::core::PCWSTR> {
        match self.hit_test(x, y) {
            Hit::Sash(Sash::Sidebar) => Some(IDC_SIZEWE),
            Hit::Sash(_) => Some(IDC_SIZENS),
            Hit::TermBody => Some(IDC_IBEAM),
            _ => None,
        }
    }

    fn area_at(&self, x: i32, y: i32) -> Option<Area> {
        if self.overlay.is_some() {
            return Some(Area::Pick);
        }
        for a in [Area::Changes, Area::Graph, Area::Main, Area::Output] {
            if a == Area::Output && !self.st.output_visible {
                continue;
            }
            if contains(&self.area_rect(a), x, y) {
                return Some(a);
            }
        }
        None
    }

    pub fn on_wheel(&mut self, x: i32, y: i32, delta: i32, horizontal: bool) {
        let Some(a) = self.area_at(x, y) else { return };
        if a == Area::Output && self.st.panel_terminal {
            self.term_scroll_by((delta as f32 / 120.0 * 3.0) as i32);
            return;
        }
        let step = if a == Area::Main { self.line_h() } else { self.s(22) };
        let amount = -(delta as f32 / 120.0 * 3.0 * step as f32) as i32;
        if horizontal && a == Area::Main {
            self.hscroll = (self.hscroll + amount).max(0);
            self.invalidate();
            return;
        }
        self.scroll_by(a, amount);
        if a == Area::Graph && self.has_more && !self.log_loading {
            let r = self.area_rect(a);
            if self.scroll[a as usize] + rh(&r) >= self.content_h(a) - self.s(22) * 5 {
                self.load_log(true);
            }
        }
        let (mx, my) = self.mouse;
        self.on_mouse_move(mx, my);
    }

    fn select_file_row(&mut self, i: usize, ctrl: bool, shift: bool) {
        let Some(CRow::File(fi)) = self.crows.get(i).copied() else { return };
        let f = self.file(fi).cloned().unwrap();
        let key = (group_id(f.group), f.path.clone());
        if ctrl {
            if !self.sel.remove(&key) {
                self.sel.insert(key);
            }
            self.anchor_row = Some(i);
        } else if shift {
            let a = self.anchor_row.unwrap_or(i);
            let (lo, hi) = (a.min(i), a.max(i));
            self.sel.clear();
            for r in lo..=hi {
                if let Some(CRow::File(x)) = self.crows.get(r) {
                    let ff = self.file(*x).unwrap();
                    self.sel.insert((group_id(ff.group), ff.path.clone()));
                }
            }
        } else {
            self.sel.clear();
            self.sel.insert(key);
            self.anchor_row = Some(i);
        }
        self.focus_row = Some(i);
        self.focus = Focus::Changes;
    }

    fn open_row(&mut self, i: usize) {
        if let Some(CRow::File(fi)) = self.crows.get(i).copied() {
            let f = self.file(fi).cloned().unwrap();
            self.load_change_view(f, false);
        }
    }

    fn file_action(&mut self, files: Vec<git::FileChange>, a: u8) {
        match a {
            0 => self.stage(files),
            1 => self.unstage(files),
            2 => self.discard(files),
            3 => {
                if let Some(r) = self.root() {
                    for f in files {
                        win::shell_open(&r.join(&f.path).display().to_string());
                    }
                }
            }
            _ => {}
        }
    }

    pub fn on_lbutton_down(&mut self, x: i32, y: i32, ctrl: bool, shift: bool) {
        let hit = self.hit_test(x, y);
        if self.focus == Focus::Terminal && !matches!(hit, Hit::TermBody | Hit::TermShellMenu | Hit::PanelTab(_) | Hit::Cmd(_) | Hit::Sash(_)) {
            self.focus = Focus::Changes;
        }
        unsafe {
            if GetFocus() != self.hwnd && hit != Hit::Backdrop && !matches!(hit, Hit::None) && self.overlay.is_none() {
                SetFocus(self.hwnd);
            }
            SetCapture(self.hwnd);
        }
        match hit {
            Hit::None => {
                if self.overlay.is_none() {
                    unsafe {
                        SetFocus(self.hwnd);
                    }
                }
            }
            Hit::Backdrop => self.close_overlay(),
            Hit::Sash(s) => {
                let orig = match s {
                    Sash::Sidebar => rw(&self.l.sidebar),
                    Sash::Graph => self.l.graph_header.top,
                    Sash::Output => rh(&self.l.output),
                };
                self.drag = Some(Drag::Sash(s, if s == Sash::Sidebar { x } else { y }, orig));
            }
            Hit::Thumb(a) => self.drag = Some(Drag::Thumb(a, y, self.scroll[a as usize])),
            Hit::Cmd(c) => self.exec(c),
            Hit::ActionBtn => self.more_menu(x, y),
            Hit::GroupRow(i) => {
                if let Some(CRow::Group(g)) = self.crows.get(i).copied() {
                    let id = group_id(g) as usize;
                    self.collapsed[id] = !self.collapsed[id];
                    self.focus_row = Some(i);
                    self.focus = Focus::Changes;
                    self.rebuild_changes();
                    self.clamp_scroll(Area::Changes);
                }
            }
            Hit::GroupAct(i, a) => {
                if let Some(CRow::Group(g)) = self.crows.get(i).copied() {
                    match (g, a) {
                        (Group::Merge, 0) => {
                            let files = self.status().map(|s| s.files.iter().filter(|f| f.group == Group::Merge).cloned().collect()).unwrap_or_default();
                            self.stage(files);
                        }
                        (Group::Staged, 1) => self.unstage_all(),
                        (Group::Changes, 0) => self.stage_all(),
                        (Group::Changes, 2) => self.discard_all(),
                        _ => {}
                    }
                }
            }
            Hit::FileRow(i) => {
                self.select_file_row(i, ctrl, shift);
                if !ctrl && !shift {
                    self.open_row(i);
                }
            }
            Hit::FileAct(i, a) => {
                let files = self.selected_files(Some(i));
                self.file_action(files, a);
            }
            Hit::CommitBtn => {
                let (_, _, kind) = self.action_button();
                match kind {
                    1 => self.sync(),
                    2 => self.publish(),
                    _ => self.exec(Cmd::Commit),
                }
            }
            Hit::CommitDrop => {
                let mut m = Menu::new();
                let items = vec![m.cmd("Commit", Cmd::Commit), m.cmd("Commit (Amend)", Cmd::CommitAmend), m.cmd("Commit All", Cmd::CommitAll), MenuItem::Sep, m.cmd("Commit & Push", Cmd::CommitPush), m.cmd("Commit & Sync", Cmd::CommitSync)];
                let b = self.l.button;
                self.show_menu(b.left, b.bottom, items, m);
            }
            Hit::GraphRow(i) => {
                self.graph_sel = Some(i);
                self.focus = Focus::Graph;
                if let Some(c) = self.commits.get(i).cloned() {
                    self.open_commit(c, None);
                }
            }
            Hit::GraphMore => self.load_log(true),
            Hit::DiffRow(r) => {
                self.focus = Focus::Main;
                if let MainView::Diff(d) = &mut self.view {
                    if shift {
                        let a = d.sel.map(|s| s.0).unwrap_or(r);
                        d.sel = Some((a, r));
                    } else {
                        d.sel = Some((r, r));
                    }
                }
                self.drag = Some(Drag::DiffSelect);
            }
            Hit::DiffBlock(b, a) => {
                if b == usize::MAX {
                    let change = match &self.view {
                        MainView::Diff(d) => d.change.clone(),
                        MainView::File(f) => f.change.clone(),
                        _ => None,
                    };
                    if let Some(c) = change {
                        self.file_action(vec![c], a);
                    }
                } else {
                    self.apply_lines(Some(b), a);
                }
            }
            Hit::DiffBack => self.go_back(),
            Hit::CommitFile(i) => self.open_commit_file(i),
            Hit::Conflict(line, w) => self.resolve_conflict(line, w),
            Hit::PickRow(k) => self.accept_overlay(Some(k)),
            Hit::DialogBtn(i) => {
                let last = matches!(&self.overlay, Some(Overlay { kind: OverlayKind::Confirm { buttons, .. } }) if i + 1 == buttons.len());
                if last {
                    self.close_overlay();
                } else {
                    self.accept_overlay(Some(i));
                }
            }
            Hit::ToastClose(i) => {
                if i < self.toasts.len() {
                    self.toasts.remove(i);
                }
            }
            Hit::ToastOutput => {
                self.toasts.clear();
                if !self.st.output_visible {
                    self.exec(Cmd::ToggleOutput);
                }
            }
            Hit::StatusBranch => self.exec(Cmd::Checkout),
            Hit::StatusSync => self.exec(Cmd::Sync),
            Hit::Recent(i) => {
                if let Some(p) = self.st.recent.get(i).cloned() {
                    self.open_repo(std::path::Path::new(&p));
                    self.layout();
                }
            }
            Hit::PanelTab(i) => {
                if i == 1 {
                    self.show_terminal(true);
                } else {
                    self.st.panel_terminal = false;
                    self.st.save();
                    self.focus = Focus::Changes;
                    self.layout();
                }
            }
            Hit::TermBody => {
                self.focus = Focus::Terminal;
                if self.term.is_none() {
                    self.start_terminal(None);
                }
                if let Some(p) = self.term_pos_at(x, y) {
                    self.term_sel = Some((p, p));
                    self.drag = Some(Drag::TermSelect);
                }
            }
            Hit::TermShellMenu => {
                let mut m = Menu::new();
                let current = self.term.as_ref().map(|t| t.shell.name.clone()).unwrap_or_default();
                let mut items: Vec<MenuItem> = self
                    .shells()
                    .into_iter()
                    .map(|sh| {
                        let label = format!("New {} Terminal", sh.name);
                        let checked = sh.name == current;
                        m.check(&label, checked, move |a| {
                            a.show_terminal(true);
                            a.start_terminal(Some(sh));
                        })
                    })
                    .collect();
                items.push(MenuItem::Sep);
                items.push(m.cmd("Select Default Profile...", Cmd::SelectShell));
                items.push(m.cmd("Clear", Cmd::ClearTerminal));
                items.push(m.cmd("Kill Terminal", Cmd::KillTerminal));
                items.push(m.cmd("Open External Terminal", Cmd::OpenTerminal));
                self.show_menu(x, y + self.s(12), items, m);
            }
            Hit::Welcome(k) => match k {
                0 => self.exec(Cmd::OpenFolder),
                1 => self.exec(Cmd::Clone),
                _ => self.exec(Cmd::Init),
            },
        }
        self.invalidate();
    }

    pub fn on_lbutton_up(&mut self) {
        unsafe {
            ReleaseCapture();
        }
        if let Some(Drag::Sash(..)) = self.drag {
            self.st.save();
        }
        if let Some(Drag::TermSelect) = self.drag {
            if let Some((a, b)) = self.term_sel {
                if a == b {
                    self.term_sel = None;
                }
            }
        }
        self.drag = None;
        self.invalidate();
    }

    pub fn on_dblclick(&mut self, x: i32, y: i32) {
        match self.hit_test(x, y) {
            Hit::FileRow(i) => {
                let files = self.selected_files(Some(i));
                self.file_action(files, 3);
            }
            Hit::CommitFile(i) => self.open_commit_file(i),
            Hit::TermBody => self.term_select_word(x, y),
            _ => {}
        }
    }

    pub fn show_menu(&mut self, x: i32, y: i32, items: Vec<MenuItem>, mut m: Menu) {
        let (sx, sy) = win::client_to_screen(self.hwnd, x, y);
        if let Some(id) = win::popup_menu(self.hwnd, sx, sy, &items) {
            if let Some(Some(f)) = m.acts.get_mut(id).map(|x| x.take()) {
                f(self);
            }
        }
        self.invalidate();
    }

    pub fn more_menu(&mut self, x: i32, y: i32) {
        let mut m = Menu::new();
        let has = self.repo.is_some();
        let s = self.status().cloned().unwrap_or_default();
        let items = vec![
            m.it_if("Pull", has, |a| a.exec(Cmd::Pull)),
            m.it_if("Push", has, |a| a.exec(Cmd::Push)),
            m.cmd("Clone", Cmd::Clone),
            m.it_if("Checkout to...", has, |a| a.exec(Cmd::Checkout)),
            m.it_if("Fetch", has, |a| a.exec(Cmd::Fetch)),
            MenuItem::Sep,
            sub("Commit", vec![
                m.cmd("Commit", Cmd::Commit),
                m.cmd("Commit All", Cmd::CommitAll),
                m.cmd("Undo Last Commit", Cmd::UndoCommit),
                m.it_if("Abort Rebase", s.rebasing, |a| a.exec(Cmd::RebaseAbort)),
                m.it_if("Abort Merge", s.merging, |a| a.exec(Cmd::MergeAbort)),
                MenuItem::Sep,
                m.cmd("Commit (Amend)", Cmd::CommitAmend),
                m.cmd("Commit & Push", Cmd::CommitPush),
                m.cmd("Commit & Sync", Cmd::CommitSync),
            ]),
            sub("Changes", vec![m.cmd("Stage All Changes", Cmd::StageAll), m.cmd("Unstage All Changes", Cmd::UnstageAll), m.cmd("Discard All Changes", Cmd::DiscardAll)]),
            sub("Pull, Push", vec![
                m.cmd("Sync", Cmd::Sync),
                MenuItem::Sep,
                m.cmd("Pull", Cmd::Pull),
                m.cmd("Pull (Rebase)", Cmd::PullRebase),
                MenuItem::Sep,
                m.cmd("Push", Cmd::Push),
                m.cmd("Push (Force With Lease)", Cmd::PushForce),
                m.cmd("Push Tags", Cmd::PushTags),
                MenuItem::Sep,
                m.cmd("Fetch", Cmd::Fetch),
                m.cmd("Fetch (Prune)", Cmd::FetchPrune),
                m.cmd("Fetch From All Remotes", Cmd::FetchAll),
            ]),
            sub("Branch", vec![
                m.cmd("Merge...", Cmd::Merge),
                m.cmd("Rebase Branch...", Cmd::Rebase),
                MenuItem::Sep,
                m.cmd("Create Branch...", Cmd::Branch),
                m.cmd("Create Branch From...", Cmd::BranchFrom),
                MenuItem::Sep,
                m.cmd("Rename Branch...", Cmd::RenameBranch),
                m.cmd("Delete Branch...", Cmd::DeleteBranch),
                MenuItem::Sep,
                m.cmd("Publish Branch...", Cmd::Publish),
            ]),
            sub("Remote", vec![m.cmd("Add Remote...", Cmd::AddRemote), m.cmd("Remove Remote", Cmd::RemoveRemote)]),
            sub("Stash", vec![
                m.cmd("Stash", Cmd::Stash),
                m.cmd("Stash (Include Untracked)", Cmd::StashUntracked),
                m.cmd("Stash Staged", Cmd::StashStaged),
                MenuItem::Sep,
                m.cmd("Apply Stash...", Cmd::StashApply),
                m.cmd("Pop Latest Stash", Cmd::StashPop),
                m.cmd("Pop Stash...", Cmd::StashPopPick),
                MenuItem::Sep,
                m.cmd("Drop Stash...", Cmd::StashDrop),
                m.cmd("View Stash...", Cmd::StashView),
            ]),
            sub("Tags", vec![m.cmd("Create Tag", Cmd::TagCreate), m.cmd("Delete Tag", Cmd::TagDelete)]),
            MenuItem::Sep,
            m.cmd("Open Folder...", Cmd::OpenFolder),
            m.cmd("Open Recent...", Cmd::OpenRecent),
            m.cmd("New Window", Cmd::NewWindow),
            m.it_if("Open in File Explorer", has, |a| a.exec(Cmd::OpenExplorer)),
            m.it_if("Open Terminal", has, |a| a.exec(Cmd::OpenTerminal)),
            MenuItem::Sep,
            m.check("Show Output", self.st.output_visible, |a| a.exec(Cmd::ToggleOutput)),
            m.check("Dark Theme", self.st.dark, |a| a.exec(Cmd::ToggleTheme)),
            m.cmd("Show All Commands...", Cmd::Palette),
        ];
        let _ = y;
        self.show_menu(x, self.l.title.bottom, items, m);
    }

    pub fn on_rbutton_up(&mut self, x: i32, y: i32) {
        let hit = self.hit_test(x, y);
        if hit == Hit::TermBody {
            // Windows Terminal behavior: copy the selection, otherwise paste
            self.focus = Focus::Terminal;
            if !self.term_copy() {
                self.term_paste();
            }
            return;
        }
        let mut m = Menu::new();
        let items: Vec<MenuItem> = match hit {
            Hit::FileRow(i) | Hit::FileAct(i, _) => {
                if let Some(CRow::File(fi)) = self.crows.get(i).copied() {
                    let f = self.file(fi).cloned().unwrap();
                    let key = (group_id(f.group), f.path.clone());
                    if !self.sel.contains(&key) {
                        self.select_file_row(i, false, false);
                    }
                }
                let files = self.selected_files(Some(i));
                if files.is_empty() {
                    return;
                }
                let root = self.root().unwrap();
                let first = files[0].clone();
                let group = first.group;
                let abs = root.join(&first.path).display().to_string();
                let rel = first.path.clone();
                let (f1, f2, f3, f4, f5) = (files.clone(), files.clone(), files.clone(), files.clone(), files.clone());
                let mut v = vec![
                    m.it_if("Open Changes", files.len() == 1, move |a| a.load_change_view(first, false)),
                    m.it("Open File", move |a| a.file_action(f1, 3)),
                    MenuItem::Sep,
                ];
                match group {
                    Group::Staged => v.push(m.it("Unstage Changes", move |a| a.unstage(f2))),
                    Group::Merge => v.push(m.it("Stage Changes (Mark Resolved)", move |a| a.stage(f2))),
                    Group::Changes => {
                        v.push(m.it("Stage Changes", move |a| a.stage(f2)));
                        v.push(m.it("Discard Changes", move |a| a.discard(f3)));
                    }
                }
                if group == Group::Merge {
                    let (o1, o2) = (f4.clone(), f4.clone());
                    v.push(m.it("Accept All Current (Ours)", move |a| a.checkout_side(o1, "--ours")));
                    v.push(m.it("Accept All Incoming (Theirs)", move |a| a.checkout_side(o2, "--theirs")));
                }
                v.push(MenuItem::Sep);
                if files.iter().any(|f| f.kind == Kind::Untracked) {
                    v.push(m.it("Add to .gitignore", move |a| a.add_to_gitignore(f5)));
                }
                let (abs2, abs3) = (abs.clone(), abs.clone());
                v.push(m.it("Reveal in File Explorer", move |_| win::reveal(&abs2)));
                v.push(m.it("Copy Path", move |a| win::set_clipboard(a.hwnd, &abs3)));
                v.push(m.it("Copy Relative Path", move |a| win::set_clipboard(a.hwnd, &rel)));
                v
            }
            Hit::GroupRow(i) | Hit::GroupAct(i, _) => match self.crows.get(i) {
                Some(CRow::Group(Group::Staged)) => vec![m.cmd("Unstage All Changes", Cmd::UnstageAll), m.cmd("Commit Staged", Cmd::Commit)],
                Some(CRow::Group(Group::Changes)) => vec![m.cmd("Stage All Changes", Cmd::StageAll), m.cmd("Discard All Changes", Cmd::DiscardAll), MenuItem::Sep, m.cmd("Stash All Changes", Cmd::StashUntracked)],
                Some(CRow::Group(Group::Merge)) => vec![m.it("Stage All Merge Changes", |a| {
                    let files = a.status().map(|s| s.files.iter().filter(|f| f.group == Group::Merge).cloned().collect()).unwrap_or_default();
                    a.stage(files);
                }), m.cmd("Abort Merge", Cmd::MergeAbort)],
                _ => return,
            },
            Hit::GraphRow(i) => {
                let Some(c) = self.commits.get(i).cloned() else { return };
                self.graph_sel = Some(i);
                let head = self.status().and_then(|s| s.head.clone());
                let branch = self.status().and_then(|s| s.branch.clone());
                let (h1, h2, h3, h4, h5, h6) = (c.hash.clone(), c.hash.clone(), c.hash.clone(), c.hash.clone(), c.hash.clone(), c.hash.clone());
                let msg = c.message.clone();
                let c2 = c.clone();
                let mut v = vec![m.it("Open Changes", move |a| a.open_commit(c2, None)), MenuItem::Sep];
                for d in c.refs.iter().filter(|d| d.kind == RefKind::Head && Some(&d.name) != branch.as_ref()) {
                    let n = d.name.clone();
                    v.push(m.it(&format!("Checkout {}", d.name), move |a| a.checkout(format!("H:{n}"))));
                }
                v.push(m.it("Checkout (Detached)", move |a| a.checkout(format!("D:{h1}"))));
                v.push(MenuItem::Sep);
                v.push(m.it("Create Branch...", move |a| a.create_branch(Some(h2))));
                v.push(m.it("Create Tag...", move |a| a.create_tag(Some(h3))));
                for d in c.refs.iter().filter(|d| d.kind == RefKind::Tag) {
                    let n = d.name.clone();
                    v.push(m.it(&format!("Delete Tag {}", d.name), move |a| a.git_op("Delete Tag", vec!["tag".into(), "-d".into(), n])));
                }
                for d in c.refs.iter().filter(|d| d.kind == RefKind::Head && Some(&d.name) != branch.as_ref()) {
                    let (n1, n2) = (d.name.clone(), d.name.clone());
                    v.push(m.it(&format!("Merge {} into Current Branch", d.name), move |a| a.git_op("Merge", vec!["merge".into(), n1])));
                    v.push(m.it(&format!("Delete Branch {}", d.name), move |a| a.delete_branch(Some(n2))));
                }
                v.push(MenuItem::Sep);
                v.push(m.it_if("Cherry Pick", Some(&c.hash) != head.as_ref(), move |a| a.git_op("Cherry Pick", vec!["cherry-pick".into(), h4])));
                v.push(m.it("Revert Commit", move |a| a.git_op("Revert", vec!["revert".into(), "--no-edit".into(), h5])));
                v.push(MenuItem::Sep);
                v.push(m.it("Copy Commit ID", move |a| win::set_clipboard(a.hwnd, &h6)));
                v.push(m.it("Copy Commit Message", move |a| win::set_clipboard(a.hwnd, &msg)));
                v
            }
            Hit::DiffRow(_) | Hit::DiffBlock(..) => {
                let MainView::Diff(d) = &self.view else { return };
                let kind = d.kind;
                let has_sel = d.sel.is_some();
                let mut v = Vec::new();
                match kind {
                    DiffKind::WorkingTree | DiffKind::Untracked => {
                        v.push(m.it_if("Stage Selected Lines", has_sel, |a| a.apply_lines(None, 0)));
                        if kind == DiffKind::WorkingTree {
                            v.push(m.it_if("Revert Selected Lines", has_sel, |a| a.apply_lines(None, 2)));
                        }
                    }
                    DiffKind::Index => v.push(m.it_if("Unstage Selected Lines", has_sel, |a| a.apply_lines(None, 1))),
                    DiffKind::Commit => {}
                }
                v.push(MenuItem::Sep);
                v.push(m.it_if("Copy", has_sel, |a| a.copy_diff_selection()));
                v.push(m.check("Inline View", self.st.inline_diff, |a| a.exec(Cmd::ToggleInline)));
                v.push(m.check("Collapse Unchanged Regions", self.st.compact_diff, |a| a.exec(Cmd::ToggleCompact)));
                v
            }
            _ => return,
        };
        self.show_menu(x, y, items, m);
    }

    pub fn checkout_side(&mut self, files: Vec<git::FileChange>, side: &'static str) {
        let paths: Vec<String> = files.iter().map(|f| f.path.clone()).collect();
        self.op("Resolve", move |root| {
            for p in &paths {
                git::exec(root, &["checkout", side, "--", p])?;
                git::exec(root, &["add", "--", p])?;
            }
            Ok(String::new())
        }, None, None);
    }

    pub fn copy_diff_selection(&mut self) {
        let MainView::Diff(d) = &self.view else { return };
        let Some((a, b)) = d.sel else { return };
        let mut out = String::new();
        for r in d.rows.iter().take(a.max(b) + 1).skip(a.min(b)) {
            if let Some((hi, li)) = r.right.or(r.left) {
                out.push_str(d.fd.hunks[hi].lines[li].text.trim_end_matches('\r'));
                out.push_str("\r\n");
            }
        }
        win::set_clipboard(self.hwnd, &out);
    }

    // ------------------------------------------------------------- keyboard

    /// Global shortcuts (handled for the main window and the edit controls).
    pub fn global_key(&mut self, vk: u16, ctrl: bool, shift: bool) -> bool {
        let c = match (vk, ctrl, shift) {
            (0x50, true, true) | (0x70, false, false) => Some(Cmd::Palette), // Ctrl+Shift+P, F1
            (0x74, false, false) => Some(Cmd::Refresh),                      // F5
            (0x4A, true, false) => Some(Cmd::ToggleOutput),                  // Ctrl+J
            (0x4F, true, false) => Some(Cmd::OpenFolder),                    // Ctrl+O
            (0x52, true, false) => Some(Cmd::OpenRecent),                    // Ctrl+R
            (0x4E, true, true) => Some(Cmd::NewWindow),                      // Ctrl+Shift+N
            (0x47, true, true) => Some(Cmd::FocusCommit),                    // Ctrl+Shift+G
            (0x43, true, true) => Some(Cmd::OpenTerminal),                   // Ctrl+Shift+C
            (0xC0, true, false) => Some(Cmd::ToggleTerminal),                // Ctrl+`
            (0xC0, true, true) => Some(Cmd::NewTerminal),                    // Ctrl+Shift+`
            (0x76, false, false) => Some(Cmd::NextChange),                   // F7
            (0x76, false, true) => Some(Cmd::PrevChange),
            (0x0D, true, _) => Some(if shift { Cmd::CommitAll } else { Cmd::Commit }),
            _ => None,
        };
        if let Some(c) = c {
            self.exec(c);
            return true;
        }
        false
    }

    pub fn overlay_key(&mut self, vk: u16) -> bool {
        let Some(ov) = &mut self.overlay else { return false };
        match &mut ov.kind {
            OverlayKind::Pick { filtered, sel, .. } => {
                let n = filtered.len();
                match vk {
                    0x26 => {
                        if n > 0 {
                            *sel = (*sel + n - 1) % n;
                        }
                    }
                    0x28 => {
                        if n > 0 {
                            *sel = (*sel + 1) % n;
                        }
                    }
                    0x21 => *sel = sel.saturating_sub(10),
                    0x22 => *sel = (*sel + 10).min(n.saturating_sub(1)),
                    0x0D => {
                        self.accept_overlay(None);
                        return true;
                    }
                    0x1B => {
                        self.close_overlay();
                        return true;
                    }
                    _ => return false,
                }
                let s = *sel;
                self.reveal_row(Area::Pick, s);
                self.invalidate();
                true
            }
            OverlayKind::Input { .. } => match vk {
                0x0D => {
                    self.accept_overlay(None);
                    true
                }
                0x1B => {
                    self.close_overlay();
                    true
                }
                _ => false,
            },
            OverlayKind::Confirm { buttons, sel, .. } => {
                let n = buttons.len();
                match vk {
                    0x25 | 0x26 => *sel = (*sel + n - 1) % n,
                    0x27 | 0x28 | 0x09 => *sel = (*sel + 1) % n,
                    0x0D | 0x20 => {
                        if *sel + 1 == n {
                            self.close_overlay();
                        } else {
                            self.accept_overlay(None);
                        }
                        return true;
                    }
                    0x1B => {
                        self.close_overlay();
                        return true;
                    }
                    _ => return false,
                }
                self.invalidate();
                true
            }
        }
    }

    pub fn on_key(&mut self, vk: u16, ctrl: bool, shift: bool) {
        if self.overlay.is_some() {
            self.overlay_key(vk);
            return;
        }
        if self.focus == Focus::Terminal && self.terminal_visible() {
            let alt = unsafe { (GetKeyState(0x12) as u16 & 0x8000) != 0 };
            self.term_key(vk, ctrl, shift, alt);
            return;
        }
        if self.global_key(vk, ctrl, shift) {
            return;
        }
        match vk {
            0x09 => {
                self.focus = match self.focus {
                    Focus::Changes => Focus::Graph,
                    Focus::Graph => Focus::Main,
                    Focus::Main => Focus::Changes,
                    Focus::Terminal => Focus::Changes,
                };
            }
            _ => match self.focus {
                Focus::Changes => self.changes_key(vk, ctrl, shift),
                Focus::Graph => self.graph_key(vk),
                Focus::Main => self.main_key(vk, ctrl),
                Focus::Terminal => {}
            },
        }
        self.invalidate();
    }

    fn changes_key(&mut self, vk: u16, ctrl: bool, shift: bool) {
        let n = self.crows.len();
        if n == 0 {
            return;
        }
        let cur = self.focus_row.unwrap_or(0);
        let next = match vk {
            0x26 => Some(cur.saturating_sub(1)),
            0x28 => Some((cur + 1).min(n - 1)),
            0x24 => Some(0),
            0x23 => Some(n - 1),
            0x21 => Some(cur.saturating_sub(10)),
            0x22 => Some((cur + 10).min(n - 1)),
            _ => None,
        };
        if let Some(i) = next {
            self.focus_row = Some(i);
            if matches!(self.crows[i], CRow::File(_)) {
                self.select_file_row(i, false, shift);
                if !shift {
                    self.open_row(i);
                }
            }
            self.reveal_row(Area::Changes, i);
            return;
        }
        match (vk, ctrl) {
            (0x0D, false) | (0x20, false) => match self.crows.get(cur).copied() {
                Some(CRow::Group(g)) => {
                    let id = group_id(g) as usize;
                    self.collapsed[id] = !self.collapsed[id];
                    self.rebuild_changes();
                }
                Some(CRow::File(_)) => self.open_row(cur),
                None => {}
            },
            (0x41, true) => {
                // Ctrl+A select all files
                if let Some(s) = self.status() {
                    let keys: Vec<(u8, String)> = s.files.iter().map(|f| (group_id(f.group), f.path.clone())).collect();
                    self.sel.extend(keys);
                }
            }
            (0x2E, false) => {
                let files = self.selected_files(self.focus_row);
                self.discard(files);
            }
            _ => {}
        }
    }

    fn graph_key(&mut self, vk: u16) {
        let n = self.commits.len();
        if n == 0 {
            return;
        }
        let cur = self.graph_sel.unwrap_or(0);
        let next = match vk {
            0x26 => cur.saturating_sub(1),
            0x28 => (cur + 1).min(n - 1),
            0x24 => 0,
            0x23 => n - 1,
            0x0D => {
                if let Some(c) = self.commits.get(cur).cloned() {
                    self.open_commit(c, None);
                }
                return;
            }
            _ => return,
        };
        self.graph_sel = Some(next);
        self.reveal_row(Area::Graph, next);
        if next + 5 >= n && self.has_more && !self.log_loading {
            self.load_log(true);
        }
    }

    fn main_key(&mut self, vk: u16, ctrl: bool) {
        if let MainView::Commit(cv) = &mut self.view {
            let n = cv.files.len();
            if n > 0 && matches!(vk, 0x26 | 0x28 | 0x0D) {
                let cur = cv.sel.unwrap_or(0);
                match vk {
                    0x26 => cv.sel = Some(cur.saturating_sub(1)),
                    0x28 => cv.sel = Some(if cv.sel.is_none() { 0 } else { (cur + 1).min(n - 1) }),
                    _ => {
                        self.open_commit_file(cur);
                        return;
                    }
                }
                return;
            }
        }
        let lh = self.line_h();
        let page = rh(&self.l.main_body) - lh;
        match (vk, ctrl) {
            (0x26, _) => self.scroll_by(Area::Main, -lh),
            (0x28, _) => self.scroll_by(Area::Main, lh),
            (0x21, _) => self.scroll_by(Area::Main, -page),
            (0x22, _) => self.scroll_by(Area::Main, page),
            (0x24, _) => self.scroll_by(Area::Main, -i32::MAX / 2),
            (0x23, _) => self.scroll_by(Area::Main, i32::MAX / 2),
            (0x25, _) => {
                self.hscroll = (self.hscroll - self.fonts.mono_w * 8).max(0);
            }
            (0x27, _) => self.hscroll += self.fonts.mono_w * 8,
            (0x43, true) => self.copy_diff_selection(),
            (0x08, _) => self.go_back(),
            (0x1B, _) => {
                if let MainView::Diff(d) = &mut self.view {
                    d.sel = None;
                }
            }
            (0x53, false) => self.apply_lines(None, 0),
            (0x55, false) => self.apply_lines(None, 1),
            _ => {}
        }
    }
}

/// Draws a small "source control" branch glyph (two commits + branch).
pub fn draw_branch_icon(g: &Gfx, cx: i32, cy: i32, size: i32, color: u32, scale: f32) {
    let s = size as f32;
    let pw = ((s / 12.0).round() as i32).max((scale * 1.5).round() as i32).max(1);
    let r = (s * 0.13).round() as i32;
    let (x1, y1) = (cx - (s * 0.22) as i32, cy - (s * 0.30) as i32);
    let (x2, y2) = (cx - (s * 0.22) as i32, cy + (s * 0.30) as i32);
    let (x3, y3) = (cx + (s * 0.22) as i32, cy - (s * 0.16) as i32);
    g.line(x1, y1 + r, x2, y2 - r, color, pw);
    let pts = [POINT { x: x3, y: y3 + r }, POINT { x: x3, y: cy + (s * 0.05) as i32 }, POINT { x: x1, y: cy }, POINT { x: x1, y: cy + (s * 0.15) as i32 }];
    g.bezier(&pts, color, pw);
    for (x, y) in [(x1, y1), (x2, y2), (x3, y3)] {
        g.ring(x, y, r, color, pw);
    }
}

pub fn hwnd_null() -> HWND {
    null_mut()
}
