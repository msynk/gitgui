//! Integrated terminal panel: session lifecycle, input, selection and painting.

use std::sync::atomic::Ordering;
use std::sync::mpsc::Receiver;

use windows_sys::Win32::Foundation::RECT;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::SetFocus;

use crate::app::*;
use crate::term::{self, Color, Pty, Screen, Shell, TermEvent, BOLD, DIM, INVERSE, UNDERLINE};
use crate::win::{self, rect, rh, rw, Gfx};

pub struct TermSession {
    pub pty: Option<Pty>,
    pub rx: Receiver<TermEvent>,
    pub screen: Screen,
    pub shell: Shell,
    pub exited: Option<String>,
}

impl App {
    pub fn terminal_visible(&self) -> bool {
        self.st.output_visible && self.st.panel_terminal
    }

    pub fn term_cell(&self) -> (i32, i32) {
        (self.fonts.mono_w.max(1), self.fonts.mono_h.max(1))
    }

    /// Terminal grid area inside the panel body.
    pub fn term_rect(&self) -> RECT {
        let b = self.l.output_body;
        rect(b.left + self.s(20), b.top + self.s(2), b.right - self.s(14), b.bottom - self.s(2))
    }

    pub fn term_grid_size(&self) -> (usize, usize) {
        let r = self.term_rect();
        let (cw, ch) = self.term_cell();
        (((rw(&r) / cw).max(20)) as usize, ((rh(&r) / ch).max(3)) as usize)
    }

    pub fn shells(&mut self) -> Vec<Shell> {
        if self.shells.is_empty() {
            self.shells = term::detect_shells();
        }
        self.shells.clone()
    }

    fn default_shell(&mut self) -> Shell {
        let shells = self.shells();
        shells.iter().find(|s| s.name == self.st.terminal_shell).or_else(|| shells.first()).cloned().unwrap_or(Shell { name: "Command Prompt".into(), cmd: "cmd.exe".into() })
    }

    pub fn start_terminal(&mut self, shell: Option<Shell>) {
        let shell = shell.unwrap_or_else(|| self.default_shell());
        self.term = None; // drops (closes) the previous pseudo console
        let cwd = self.root().or_else(|| self.pending_init.clone()).or_else(|| std::env::var_os("USERPROFILE").map(Into::into)).unwrap_or_else(|| ".".into());
        let (cols, rows) = self.term_grid_size();
        self.term_pending.store(false, Ordering::SeqCst);
        match Pty::spawn(&shell.cmd, &cwd, cols as u16, rows as u16, self.hwnd, WM_TERM, self.term_pending.clone()) {
            Ok((pty, rx)) => {
                self.term = Some(TermSession { pty: Some(pty), rx, screen: Screen::new(cols, rows), shell, exited: None });
            }
            Err(e) => {
                let mut screen = Screen::new(cols, rows);
                screen.feed(format!("\x1b[31m{e}\x1b[0m\r\n").as_bytes());
                let (_, rx) = std::sync::mpsc::channel();
                self.term = Some(TermSession { pty: None, rx, screen, shell, exited: Some(e.clone()) });
                self.error(e);
            }
        }
        self.term_scroll = 0;
        self.term_sel = None;
        self.invalidate();
    }

    pub fn show_terminal(&mut self, focus: bool) {
        self.st.output_visible = true;
        self.st.panel_terminal = true;
        self.st.save();
        self.layout();
        if self.term.is_none() {
            self.start_terminal(None);
        }
        if focus {
            self.focus = Focus::Terminal;
            unsafe {
                SetFocus(self.hwnd);
            }
        }
        self.invalidate();
    }

    pub fn toggle_terminal(&mut self) {
        if self.terminal_visible() && self.focus == Focus::Terminal {
            self.st.output_visible = false;
            self.st.save();
            self.focus = Focus::Changes;
            self.layout();
        } else {
            self.show_terminal(true);
        }
    }

    pub fn kill_terminal(&mut self) {
        self.term = None;
        if self.terminal_visible() {
            self.st.output_visible = false;
            self.st.save();
            self.focus = Focus::Changes;
            self.layout();
        }
        self.invalidate();
    }

    pub fn select_shell(&mut self) {
        let current = self.default_shell().name;
        let items = self
            .shells()
            .into_iter()
            .map(|s| {
                let mut it = pi(s.name.clone(), s.cmd.clone(), '\u{E756}', s.name.clone());
                if s.name == current {
                    it.sep = "default".into();
                }
                it
            })
            .collect();
        self.pick("Select your default terminal profile", items, |app, it| {
            app.st.terminal_shell = it.data.clone();
            app.st.save();
            let shell = app.shells().into_iter().find(|s| s.name == it.data);
            app.show_terminal(true);
            app.start_terminal(shell);
        });
    }

    /// Resizes the emulator and the pseudo console to the panel.
    pub fn resize_terminal(&mut self) {
        if !self.terminal_visible() {
            return;
        }
        let (cols, rows) = self.term_grid_size();
        if let Some(t) = &mut self.term {
            if t.screen.cols != cols || t.screen.rows != rows {
                t.screen.resize(cols, rows);
                if let Some(p) = &t.pty {
                    p.resize(cols as u16, rows as u16);
                }
            }
        }
    }

    pub fn on_term_data(&mut self) {
        self.term_pending.store(false, Ordering::SeqCst);
        let Some(t) = &mut self.term else { return };
        let before = t.screen.scrolled_total;
        let mut exited = false;
        while let Ok(ev) = t.rx.try_recv() {
            match ev {
                TermEvent::Data(d) => {
                    if let Some(path) = std::env::var_os("GITGUI_TERM_LOG") {
                        use std::io::Write;
                        if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(path) {
                            let _ = f.write_all(&d);
                        }
                    }
                    t.screen.feed(&d)
                }
                TermEvent::Exited => exited = true,
            }
        }
        if !t.screen.replies.is_empty() {
            let r = std::mem::take(&mut t.screen.replies);
            if let Some(p) = &t.pty {
                p.write(&r);
            }
        }
        if exited && t.exited.is_none() {
            let code = t.pty.as_ref().and_then(|p| p.exit_code());
            let msg = match code {
                Some(c) => format!("The terminal process \"{}\" terminated with exit code: {c}.", t.shell.name),
                None => format!("The terminal process \"{}\" terminated.", t.shell.name),
            };
            t.screen.feed(format!("\r\n\x1b[7m * {msg} Press any key to restart the terminal. \x1b[0m\r\n").as_bytes());
            t.exited = Some(msg);
        }
        // keep the viewport stable when scrolled up into history
        let added = (t.screen.scrolled_total - before) as usize;
        if self.term_scroll > 0 {
            self.term_scroll = (self.term_scroll + added).min(t.screen.scrollback.len());
        }
        if self.terminal_visible() {
            self.invalidate();
        }
    }

    pub fn term_write(&mut self, data: &[u8]) {
        if let Some(t) = &self.term {
            if t.exited.is_some() {
                let shell = t.shell.clone();
                self.start_terminal(Some(shell));
                return;
            }
            if let Some(p) = &t.pty {
                p.write(data);
            }
        }
        self.term_scroll = 0;
        self.term_sel = None;
        self.invalidate();
    }

    pub fn term_paste(&mut self) {
        let Some(text) = win::get_clipboard(self.hwnd) else { return };
        let text = text.replace("\r\n", "\r").replace('\n', "\r");
        let bracketed = self.term.as_ref().map(|t| t.screen.bracketed_paste).unwrap_or(false);
        let data = if bracketed { format!("\x1b[200~{text}\x1b[201~") } else { text };
        self.term_write(data.as_bytes());
    }

    pub fn term_selection_text(&self) -> Option<String> {
        let t = self.term.as_ref()?;
        let (a, b) = self.term_sel?;
        let (s, e) = if a <= b { (a, b) } else { (b, a) };
        if s == e {
            return None;
        }
        let mut out = Vec::new();
        for line in s.0..=e.0 {
            let from = if line == s.0 { s.1 } else { 0 };
            let to = if line == e.0 { e.1 } else { t.screen.cols };
            out.push(t.screen.line_text(line, from, to).trim_end().to_string());
        }
        Some(out.join("\r\n"))
    }

    pub fn term_copy(&mut self) -> bool {
        match self.term_selection_text() {
            Some(text) => {
                win::set_clipboard(self.hwnd, &text);
                self.term_sel = None;
                self.invalidate();
                true
            }
            None => false,
        }
    }

    /// Keyboard input while the terminal has focus. Returns true when handled.
    pub fn term_key(&mut self, vk: u16, ctrl: bool, shift: bool, alt: bool) -> bool {
        // workbench shortcuts that win over the shell (like VS Code's defaults)
        match (vk, ctrl, shift) {
            (0x50, true, true) | (0x70, false, false) => {
                self.exec(Cmd::Palette);
                return true;
            }
            (0xC0, true, false) => {
                self.toggle_terminal();
                return true;
            }
            (0xC0, true, true) => {
                self.start_terminal(None);
                return true;
            }
            (0x4A, true, false) => {
                self.exec(Cmd::ToggleOutput);
                return true;
            }
            (0x47, true, true) => {
                self.exec(Cmd::FocusCommit);
                return true;
            }
            _ => {}
        }
        if ctrl && vk == 0x43 && (shift || self.term_sel.is_some()) {
            // Ctrl+C copies when there is a selection, Ctrl+Shift+C always copies
            if self.term_copy() || shift {
                self.term_skip_char = true;
                return true;
            }
        }
        if (ctrl && vk == 0x56) || (shift && vk == 0x2D) {
            self.term_paste();
            self.term_skip_char = ctrl;
            return true;
        }
        if shift && (vk == 0x21 || vk == 0x22) {
            let page = self.term.as_ref().map(|t| t.screen.rows).unwrap_or(10);
            self.term_scroll_by(if vk == 0x21 { page as i32 } else { -(page as i32) });
            return true;
        }
        let app_cursor = self.term.as_ref().map(|t| t.screen.app_cursor).unwrap_or(false);
        if let Some(seq) = term::key_sequence(vk, ctrl, shift, alt, app_cursor) {
            self.term_write(&seq);
            return true;
        }
        false
    }

    pub fn term_char(&mut self, ch: u16, alt: bool) {
        if self.term_skip_char {
            self.term_skip_char = false;
            return;
        }
        let s: String = if (0xD800..0xDC00).contains(&ch) {
            self.term_surrogate = ch;
            return;
        } else if (0xDC00..0xE000).contains(&ch) {
            let hi = std::mem::take(&mut self.term_surrogate);
            String::from_utf16_lossy(&[hi, ch])
        } else if ch == 0x08 {
            "\x7f".into()
        } else {
            String::from_utf16_lossy(&[ch])
        };
        let mut data = Vec::new();
        if alt {
            data.push(0x1b);
        }
        data.extend_from_slice(s.as_bytes());
        self.term_write(&data);
    }

    pub fn term_scroll_by(&mut self, lines: i32) {
        let Some(t) = &self.term else { return };
        if t.screen.in_alt_screen() {
            // full-screen apps (less, vim...) get arrow keys instead
            let seq: &[u8] = if lines > 0 { b"\x1b[A" } else { b"\x1b[B" };
            let n = lines.unsigned_abs().min(10);
            let data: Vec<u8> = seq.repeat(n as usize);
            if let Some(p) = &t.pty {
                p.write(&data);
            }
            return;
        }
        let max = t.screen.scrollback.len() as i32;
        self.term_scroll = (self.term_scroll as i32 + lines).clamp(0, max) as usize;
        self.invalidate();
    }

    /// Absolute (line, col) under a client point.
    pub fn term_pos_at(&self, x: i32, y: i32) -> Option<(usize, usize)> {
        let t = self.term.as_ref()?;
        let r = self.term_rect();
        let (cw, ch) = self.term_cell();
        let col = ((x - r.left).max(0) / cw).min(t.screen.cols as i32) as usize;
        let row = ((y - r.top).max(0) / ch).min(t.screen.rows as i32 - 1) as usize;
        let top = t.screen.total_lines() - t.screen.rows - self.term_scroll;
        Some((top + row, col))
    }

    pub fn term_select_word(&mut self, x: i32, y: i32) {
        let Some((line, col)) = self.term_pos_at(x, y) else { return };
        let Some(t) = &self.term else { return };
        let Some(l) = t.screen.line(line) else { return };
        let is_word = |c: char| !c.is_whitespace() && !"\"'()[]{}<>|,;`".contains(c);
        if col >= l.len() || !is_word(l[col].ch) {
            return;
        }
        let mut s = col;
        while s > 0 && is_word(l[s - 1].ch) {
            s -= 1;
        }
        let mut e = col;
        while e < l.len() && is_word(l[e].ch) {
            e += 1;
        }
        self.term_sel = Some(((line, s), (line, e)));
        self.invalidate();
    }

    // ------------------------------------------------------------- painting

    fn term_color(&self, c: Color, fg: bool, flags: u8) -> u32 {
        match c {
            Color::Default => {
                if fg {
                    self.t.fg
                } else {
                    self.t.sidebar
                }
            }
            Color::Idx(i) => {
                let i = if fg && flags & BOLD != 0 && i < 8 { i + 8 } else { i };
                term::ansi_color(i, self.t.dark)
            }
            Color::Rgb(r, g, b) => crate::theme::hex(((r as u32) << 16) | ((g as u32) << 8) | b as u32),
        }
    }

    pub fn paint_terminal(&mut self, g: &Gfx) {
        let area = self.l.output_body;
        let r = self.term_rect();
        self.hits.push((area, Hit::TermBody));
        let Some(t) = &self.term else {
            g.text_in(&r, "Starting terminal...", self.fonts.ui, self.t.fg_dim, win::DT_LINE);
            return;
        };
        let (cw, chh) = self.term_cell();
        let sc = &t.screen;
        let top = sc.total_lines() - sc.rows - self.term_scroll.min(sc.scrollback.len());
        let sel = self.term_sel.map(|(a, b)| if a <= b { (a, b) } else { (b, a) });
        let focused = self.focus == Focus::Terminal && unsafe { windows_sys::Win32::UI::Input::KeyboardAndMouse::GetFocus() } == self.hwnd;
        let saved = g.clip(&area);
        let sel_bg = if self.t.dark { crate::theme::hex(0x264F78) } else { crate::theme::hex(0xADD6FF) };
        let mut run: Vec<char> = Vec::with_capacity(sc.cols);
        for row in 0..sc.rows {
            let li = top + row;
            let Some(line) = sc.line(li) else { continue };
            let y = r.top + row as i32 * chh;
            let mut c = 0usize;
            while c < sc.cols {
                let cell = line.get(c).copied().unwrap_or_default();
                let in_sel = |col: usize| sel.map(|(s, e)| (li, col) >= s && (li, col) < e && s != e).unwrap_or(false);
                let selected = in_sel(c);
                // collect a run of cells with identical attributes
                run.clear();
                let start = c;
                while c < sc.cols {
                    let cc = line.get(c).copied().unwrap_or_default();
                    if cc.fg != cell.fg || cc.bg != cell.bg || cc.flags != cell.flags || in_sel(c) != selected {
                        break;
                    }
                    run.push(cc.ch);
                    c += 1;
                }
                let (mut fg, mut bg) = (self.term_color(cell.fg, true, cell.flags), self.term_color(cell.bg, false, cell.flags));
                if cell.flags & INVERSE != 0 {
                    std::mem::swap(&mut fg, &mut bg);
                }
                if cell.flags & DIM != 0 {
                    fg = crate::theme::blend_colorref(bg, fg, 0.6);
                }
                if selected {
                    bg = sel_bg;
                }
                let x = r.left + start as i32 * cw;
                let run_rect = rect(x, y, x + run.len() as i32 * cw, y + chh);
                if bg != self.t.sidebar {
                    g.fill(&run_rect, bg);
                }
                if run.iter().any(|ch| *ch != ' ') {
                    g.text_cells(x, y, &run, cw, self.fonts.mono, fg, &run_rect);
                }
                if cell.flags & UNDERLINE != 0 {
                    g.hline(run_rect.left, run_rect.right, y + chh - 2, fg);
                }
            }
        }
        // cursor
        if sc.cursor_visible && self.term_scroll == 0 && t.exited.is_none() {
            let x = r.left + sc.cx as i32 * cw;
            let y = r.top + sc.cy as i32 * chh;
            let cr = rect(x, y, x + cw, y + chh);
            if focused {
                let cell = sc.lines[sc.cy].get(sc.cx).copied().unwrap_or_default();
                g.fill(&cr, self.t.fg);
                if cell.ch != ' ' {
                    g.text_cells(x, y, &[cell.ch], cw, self.fonts.mono, self.t.sidebar, &cr);
                }
            } else {
                g.frame(&cr, self.t.fg);
            }
        }
        g.unclip(saved);
        // scrollbar for history
        let total = sc.total_lines() as i32;
        let rows = sc.rows as i32;
        if total > rows {
            let h = rh(&area);
            let th = ((h as f32 * rows as f32 / total as f32) as i32).max(self.s(20));
            let pos = total - rows - self.term_scroll as i32;
            let ty = area.top + ((h - th) as f32 * pos as f32 / (total - rows) as f32) as i32;
            g.fill(&rect(area.right - self.s(10), ty, area.right, ty + th), self.t.scroll_thumb);
        }
    }
}
