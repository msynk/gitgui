//! Integrated terminal: Windows ConPTY backend + a compact VT/xterm emulator.
//!
//! The shell runs attached to a pseudo console. A reader thread blocks in
//! `ReadFile` on the output pipe (zero CPU when idle) and hands bytes to the UI
//! thread, which feeds them through [`Screen::feed`].

use std::collections::VecDeque;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Receiver};
use std::sync::Arc;

use windows_sys::Win32::Foundation::{CloseHandle, HANDLE, HWND};
use windows_sys::Win32::Storage::FileSystem::{ReadFile, WriteFile};
use windows_sys::Win32::System::Console::{ClosePseudoConsole, CreatePseudoConsole, ResizePseudoConsole, COORD, HPCON};
use windows_sys::Win32::System::Pipes::CreatePipe;
use windows_sys::Win32::System::Threading::*;
use windows_sys::Win32::UI::WindowsAndMessaging::PostMessageW;

// ================================================================== backend

pub enum TermEvent {
    Data(Vec<u8>),
    Exited,
}

pub struct Pty {
    hpc: HPCON,
    input: HANDLE,
    process: HANDLE,
    pub pid: u32,
}

unsafe impl Send for Pty {}

impl Pty {
    /// Starts `cmdline` attached to a new pseudo console of `cols` x `rows`.
    pub fn spawn(cmdline: &str, cwd: &Path, cols: u16, rows: u16, hwnd: HWND, msg: u32, pending: Arc<AtomicBool>) -> Result<(Pty, Receiver<TermEvent>), String> {
        unsafe {
            let (mut in_read, mut in_write, mut out_read, mut out_write): (HANDLE, HANDLE, HANDLE, HANDLE) = (std::ptr::null_mut(), std::ptr::null_mut(), std::ptr::null_mut(), std::ptr::null_mut());
            if CreatePipe(&mut in_read, &mut in_write, std::ptr::null(), 0) == 0 || CreatePipe(&mut out_read, &mut out_write, std::ptr::null(), 0) == 0 {
                return Err("Unable to create pipes".into());
            }
            let mut hpc: HPCON = 0;
            let size = COORD { X: cols.max(2) as i16, Y: rows.max(1) as i16 };
            let hr = CreatePseudoConsole(size, in_read, out_write, 0, &mut hpc);
            // the pseudo console owns duplicates of these now
            CloseHandle(in_read);
            CloseHandle(out_write);
            if hr < 0 {
                CloseHandle(in_write);
                CloseHandle(out_read);
                return Err(format!("CreatePseudoConsole failed (0x{hr:08X}). Windows 10 1809 or later is required."));
            }

            let mut attr_size = 0usize;
            InitializeProcThreadAttributeList(std::ptr::null_mut(), 1, 0, &mut attr_size);
            let mut attr_buf = vec![0u8; attr_size];
            let attrs = attr_buf.as_mut_ptr() as LPPROC_THREAD_ATTRIBUTE_LIST;
            if InitializeProcThreadAttributeList(attrs, 1, 0, &mut attr_size) == 0
                || UpdateProcThreadAttribute(attrs, 0, PROC_THREAD_ATTRIBUTE_PSEUDOCONSOLE as usize, hpc as *const _, std::mem::size_of::<HPCON>(), std::ptr::null_mut(), std::ptr::null())
                    == 0
            {
                ClosePseudoConsole(hpc);
                CloseHandle(in_write);
                CloseHandle(out_read);
                return Err("Unable to set up the pseudo console attribute".into());
            }
            let mut si: STARTUPINFOEXW = std::mem::zeroed();
            si.StartupInfo.cb = std::mem::size_of::<STARTUPINFOEXW>() as u32;
            // do not let the child inherit our own (possibly redirected) std handles
            si.StartupInfo.dwFlags = STARTF_USESTDHANDLES;
            si.lpAttributeList = attrs;
            let mut pi: PROCESS_INFORMATION = std::mem::zeroed();
            let mut cmd: Vec<u16> = cmdline.encode_utf16().chain(std::iter::once(0)).collect();
            let dir: Vec<u16> = cwd.as_os_str().to_string_lossy().encode_utf16().chain(std::iter::once(0)).collect();
            let ok = CreateProcessW(
                std::ptr::null(),
                cmd.as_mut_ptr(),
                std::ptr::null(),
                std::ptr::null(),
                0,
                EXTENDED_STARTUPINFO_PRESENT | CREATE_UNICODE_ENVIRONMENT,
                std::ptr::null(),
                if cwd.is_dir() { dir.as_ptr() } else { std::ptr::null() },
                &si.StartupInfo,
                &mut pi,
            );
            DeleteProcThreadAttributeList(attrs);
            if ok == 0 {
                let err = std::io::Error::last_os_error();
                ClosePseudoConsole(hpc);
                CloseHandle(in_write);
                CloseHandle(out_read);
                return Err(format!("Unable to start '{cmdline}': {err}"));
            }
            CloseHandle(pi.hThread);

            let (tx, rx) = channel();
            let out = out_read as isize;
            let hwnd_v = hwnd as isize;
            std::thread::Builder::new()
                .name("pty-reader".into())
                .stack_size(128 * 1024)
                .spawn(move || {
                    let mut buf = vec![0u8; 32 * 1024];
                    loop {
                        let mut n = 0u32;
                        let ok = ReadFile(out as HANDLE, buf.as_mut_ptr(), buf.len() as u32, &mut n, std::ptr::null_mut());
                        if ok == 0 || n == 0 {
                            break;
                        }
                        if tx.send(TermEvent::Data(buf[..n as usize].to_vec())).is_err() {
                            break;
                        }
                        if !pending.swap(true, Ordering::AcqRel) {
                            PostMessageW(hwnd_v as HWND, msg, 0, 0);
                        }
                    }
                    let _ = tx.send(TermEvent::Exited);
                    PostMessageW(hwnd_v as HWND, msg, 0, 0);
                    CloseHandle(out as HANDLE);
                })
                .map_err(|e| e.to_string())?;
            Ok((Pty { hpc, input: in_write, process: pi.hProcess, pid: pi.dwProcessId }, rx))
        }
    }

    pub fn write(&self, data: &[u8]) {
        let mut off = 0;
        while off < data.len() {
            let mut n = 0u32;
            let ok = unsafe { WriteFile(self.input, data[off..].as_ptr(), (data.len() - off) as u32, &mut n, std::ptr::null_mut()) };
            if ok == 0 || n == 0 {
                break;
            }
            off += n as usize;
        }
    }

    pub fn resize(&self, cols: u16, rows: u16) {
        unsafe {
            ResizePseudoConsole(self.hpc, COORD { X: cols.max(2) as i16, Y: rows.max(1) as i16 });
        }
    }

    pub fn exit_code(&self) -> Option<u32> {
        let mut code = 0u32;
        unsafe {
            if GetExitCodeProcess(self.process, &mut code) != 0 && code != 259 {
                Some(code)
            } else {
                None
            }
        }
    }
}

impl Drop for Pty {
    fn drop(&mut self) {
        // ClosePseudoConsole may wait for the output pipe to drain; the reader
        // thread keeps draining, but do it off the UI thread to be safe.
        let (hpc, input, process) = (self.hpc, self.input as isize, self.process as isize);
        std::thread::spawn(move || unsafe {
            ClosePseudoConsole(hpc);
            CloseHandle(input as HANDLE);
            let mut code = 0u32;
            if GetExitCodeProcess(process as HANDLE, &mut code) != 0 && code == 259 {
                TerminateProcess(process as HANDLE, 1);
            }
            CloseHandle(process as HANDLE);
        });
    }
}

// ------------------------------------------------------------------- shells

#[derive(Clone, Debug, PartialEq)]
pub struct Shell {
    pub name: String,
    pub cmd: String,
}

fn which(exe: &str) -> Option<std::path::PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path).map(|d| d.join(exe)).find(|p| p.is_file())
}

/// Shells available on this machine, default first (like VS Code's profiles).
pub fn detect_shells() -> Vec<Shell> {
    let mut v = Vec::new();
    if let Some(p) = which("pwsh.exe") {
        v.push(Shell { name: "PowerShell 7".into(), cmd: format!("\"{}\" -NoLogo", p.display()) });
    }
    let sysroot = std::env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".into());
    let ps = format!(r"{sysroot}\System32\WindowsPowerShell\v1.0\powershell.exe");
    if Path::new(&ps).is_file() {
        v.push(Shell { name: "PowerShell".into(), cmd: format!("\"{ps}\" -NoLogo") });
    }
    let comspec = std::env::var("ComSpec").unwrap_or_else(|_| format!(r"{sysroot}\System32\cmd.exe"));
    v.push(Shell { name: "Command Prompt".into(), cmd: format!("\"{comspec}\"") });
    // Git Bash next to git.exe (…\Git\cmd\git.exe -> …\Git\bin\bash.exe)
    // git.exe may live in Git\cmd, Git\bin or Git\mingw64\bin: walk up to the install root
    let mut bash = None;
    if let Some(git) = which("git.exe") {
        for dir in git.ancestors().skip(1).take(4) {
            let b = dir.join("bin").join("bash.exe");
            if b.is_file() && dir.join("git-bash.exe").is_file() || b.is_file() && dir.join("usr").is_dir() {
                bash = Some(b);
                break;
            }
        }
    }
    if bash.is_none() {
        for base in [std::env::var("ProgramFiles").ok(), std::env::var("LOCALAPPDATA").ok().map(|l| format!(r"{l}\Programs"))].into_iter().flatten() {
            let b = Path::new(&base).join("Git").join("bin").join("bash.exe");
            if b.is_file() {
                bash = Some(b);
                break;
            }
        }
    }
    if let Some(b) = bash {
        v.push(Shell { name: "Git Bash".into(), cmd: format!("\"{}\" --login -i", b.display()) });
    }
    if let Some(wsl) = which("wsl.exe") {
        v.push(Shell { name: "WSL".into(), cmd: format!("\"{}\"", wsl.display()) });
    }
    v
}

// ================================================================ emulator

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Color {
    Default,
    Idx(u8),
    Rgb(u8, u8, u8),
}

pub const BOLD: u8 = 1;
pub const DIM: u8 = 2;
pub const UNDERLINE: u8 = 4;
pub const INVERSE: u8 = 8;
pub const ITALIC: u8 = 16;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Cell {
    pub ch: char,
    pub fg: Color,
    pub bg: Color,
    pub flags: u8,
}

impl Default for Cell {
    fn default() -> Self {
        Cell { ch: ' ', fg: Color::Default, bg: Color::Default, flags: 0 }
    }
}

#[derive(Clone, Copy)]
struct Pen {
    fg: Color,
    bg: Color,
    flags: u8,
}

impl Default for Pen {
    fn default() -> Self {
        Pen { fg: Color::Default, bg: Color::Default, flags: 0 }
    }
}

enum State {
    Ground,
    Esc,
    EscInter,
    Csi,
    Osc,
    OscEsc,
}

pub type Line = Vec<Cell>;

pub struct Screen {
    pub cols: usize,
    pub rows: usize,
    pub lines: Vec<Line>,
    alt_saved: Option<(Vec<Line>, (usize, usize))>,
    pub scrollback: VecDeque<Line>,
    pub max_scrollback: usize,
    pub cx: usize,
    pub cy: usize,
    wrap_pending: bool,
    pen: Pen,
    saved: (usize, usize, Pen),
    top: usize,
    bottom: usize,
    pub cursor_visible: bool,
    pub app_cursor: bool,
    pub bracketed_paste: bool,
    autowrap: bool,
    state: State,
    params: String,
    osc: String,
    utf8: Vec<u8>,
    pub title: String,
    /// Responses the emulator must send back to the application (e.g. DSR).
    pub replies: Vec<u8>,
    /// Incremented whenever lines are pushed to scrollback (used to keep selections stable).
    pub scrolled_total: u64,
}

impl Screen {
    pub fn new(cols: usize, rows: usize) -> Screen {
        let cols = cols.max(2);
        let rows = rows.max(1);
        Screen {
            cols,
            rows,
            lines: vec![vec![Cell::default(); cols]; rows],
            alt_saved: None,
            scrollback: VecDeque::new(),
            max_scrollback: 3000,
            cx: 0,
            cy: 0,
            wrap_pending: false,
            pen: Pen::default(),
            saved: (0, 0, Pen::default()),
            top: 0,
            bottom: rows - 1,
            cursor_visible: true,
            app_cursor: false,
            bracketed_paste: false,
            autowrap: true,
            state: State::Ground,
            params: String::new(),
            osc: String::new(),
            utf8: Vec::new(),
            title: String::new(),
            replies: Vec::new(),
            scrolled_total: 0,
        }
    }

    pub fn in_alt_screen(&self) -> bool {
        self.alt_saved.is_some()
    }

    fn blank(&self) -> Cell {
        Cell { ch: ' ', fg: Color::Default, bg: self.pen.bg, flags: 0 }
    }

    pub fn resize(&mut self, cols: usize, rows: usize) {
        let cols = cols.max(2);
        let rows = rows.max(1);
        if cols == self.cols && rows == self.rows {
            return;
        }
        for l in self.lines.iter_mut() {
            l.resize(cols, Cell::default());
        }
        // shrinking: move the top lines into scrollback so the cursor stays visible
        while self.lines.len() > rows {
            if self.cy > 0 {
                let l = self.lines.remove(0);
                self.push_scrollback(l);
                self.cy -= 1;
            } else {
                self.lines.pop();
            }
        }
        while self.lines.len() < rows {
            self.lines.push(vec![Cell::default(); cols]);
        }
        self.cols = cols;
        self.rows = rows;
        self.top = 0;
        self.bottom = rows - 1;
        self.cx = self.cx.min(cols - 1);
        self.cy = self.cy.min(rows - 1);
        self.wrap_pending = false;
        if let Some((saved, _)) = self.alt_saved.as_mut() {
            for l in saved.iter_mut() {
                l.resize(cols, Cell::default());
            }
            saved.resize(rows, vec![Cell::default(); cols]);
        }
    }

    fn push_scrollback(&mut self, mut l: Line) {
        if self.alt_saved.is_some() {
            return;
        }
        // store history lines without their blank tail (most lines are short)
        let keep = l.iter().rposition(|c| *c != Cell::default()).map(|i| i + 1).unwrap_or(0);
        l.truncate(keep);
        l.shrink_to_fit();
        self.scrollback.push_back(l);
        self.scrolled_total += 1;
        while self.scrollback.len() > self.max_scrollback {
            self.scrollback.pop_front();
        }
    }

    fn scroll_up(&mut self, n: usize) {
        for _ in 0..n {
            let removed = self.lines.remove(self.top);
            if self.top == 0 {
                self.push_scrollback(removed);
            }
            let b = self.blank();
            self.lines.insert(self.bottom, vec![b; self.cols]);
        }
    }

    fn scroll_down(&mut self, n: usize) {
        for _ in 0..n {
            self.lines.remove(self.bottom);
            let b = self.blank();
            self.lines.insert(self.top, vec![b; self.cols]);
        }
    }

    fn line_feed(&mut self) {
        if self.cy == self.bottom {
            self.scroll_up(1);
        } else if self.cy + 1 < self.rows {
            self.cy += 1;
        }
    }

    fn put(&mut self, ch: char) {
        if self.wrap_pending {
            self.wrap_pending = false;
            if self.autowrap {
                self.cx = 0;
                self.line_feed();
            }
        }
        let cell = Cell { ch, fg: self.pen.fg, bg: self.pen.bg, flags: self.pen.flags };
        self.lines[self.cy][self.cx] = cell;
        if self.cx + 1 >= self.cols {
            self.wrap_pending = true;
        } else {
            self.cx += 1;
        }
    }

    pub fn feed(&mut self, data: &[u8]) {
        // UTF-8 decode (keeping incomplete sequences between reads)
        let mut bytes = std::mem::take(&mut self.utf8);
        bytes.extend_from_slice(data);
        let mut i = 0;
        while i < bytes.len() {
            let b = bytes[i];
            let len = if b < 0x80 {
                1
            } else if b >> 5 == 0b110 {
                2
            } else if b >> 4 == 0b1110 {
                3
            } else if b >> 3 == 0b11110 {
                4
            } else {
                i += 1;
                continue;
            };
            if i + len > bytes.len() {
                self.utf8 = bytes[i..].to_vec();
                break;
            }
            let ch = std::str::from_utf8(&bytes[i..i + len]).ok().and_then(|s| s.chars().next()).unwrap_or('\u{FFFD}');
            i += len;
            self.handle(ch);
        }
    }

    fn handle(&mut self, ch: char) {
        match self.state {
            State::Ground => match ch {
                '\x1b' => self.state = State::Esc,
                '\r' => {
                    self.cx = 0;
                    self.wrap_pending = false;
                }
                '\n' | '\x0b' | '\x0c' => {
                    self.wrap_pending = false;
                    self.line_feed();
                }
                '\x08' => {
                    self.wrap_pending = false;
                    self.cx = self.cx.saturating_sub(1);
                }
                '\t' => {
                    let next = ((self.cx / 8) + 1) * 8;
                    self.cx = next.min(self.cols - 1);
                }
                '\x07' | '\0' | '\x0e' | '\x0f' => {}
                c if (c as u32) < 0x20 || c == '\x7f' => {}
                c => self.put(c),
            },
            State::Esc => {
                self.state = State::Ground;
                match ch {
                    '[' => {
                        self.params.clear();
                        self.state = State::Csi;
                    }
                    ']' => {
                        self.osc.clear();
                        self.state = State::Osc;
                    }
                    '7' => self.saved = (self.cx, self.cy, self.pen),
                    '8' => {
                        self.cx = self.saved.0.min(self.cols - 1);
                        self.cy = self.saved.1.min(self.rows - 1);
                        self.pen = self.saved.2;
                    }
                    'M' => {
                        if self.cy == self.top {
                            self.scroll_down(1);
                        } else {
                            self.cy = self.cy.saturating_sub(1);
                        }
                    }
                    'D' => self.line_feed(),
                    'E' => {
                        self.cx = 0;
                        self.line_feed();
                    }
                    'c' => {
                        let (c, r) = (self.cols, self.rows);
                        let sb = std::mem::take(&mut self.scrollback);
                        *self = Screen::new(c, r);
                        self.scrollback = sb;
                    }
                    '(' | ')' | '*' | '+' | '#' | '%' => self.state = State::EscInter,
                    _ => {}
                }
            }
            State::EscInter => self.state = State::Ground,
            State::Csi => {
                if ('\x40'..='\x7e').contains(&ch) {
                    let params = std::mem::take(&mut self.params);
                    self.state = State::Ground;
                    self.csi(&params, ch);
                } else if self.params.len() < 64 {
                    self.params.push(ch);
                }
            }
            State::Osc => match ch {
                '\x07' => {
                    self.osc_done();
                    self.state = State::Ground;
                }
                '\x1b' => self.state = State::OscEsc,
                c => {
                    if self.osc.len() < 2048 {
                        self.osc.push(c)
                    }
                }
            },
            State::OscEsc => {
                self.osc_done();
                self.state = if ch == '\\' { State::Ground } else { State::Ground };
            }
        }
    }

    fn osc_done(&mut self) {
        let osc = std::mem::take(&mut self.osc);
        if let Some((code, text)) = osc.split_once(';') {
            if code == "0" || code == "2" {
                self.title = text.chars().take(200).collect();
            }
        }
    }

    fn csi(&mut self, raw: &str, f: char) {
        let private = raw.starts_with('?') || raw.starts_with('>') || raw.starts_with('=');
        let body = raw.trim_start_matches(['?', '>', '=']);
        let body: String = body.chars().filter(|c| c.is_ascii_digit() || *c == ';' || *c == ':').collect();
        let ps: Vec<usize> = if body.is_empty() { vec![] } else { body.split(';').map(|p| p.split(':').next().unwrap_or("").parse().unwrap_or(0)).collect() };
        let p = |i: usize, d: usize| -> usize { ps.get(i).copied().filter(|&v| v != 0).unwrap_or(d) };
        if f != 'm' {
            self.wrap_pending = false;
        }
        match f {
            'A' => self.cy = self.cy.saturating_sub(p(0, 1)).max(if self.cy >= self.top { self.top } else { 0 }),
            'B' | 'e' => self.cy = (self.cy + p(0, 1)).min(if self.cy <= self.bottom { self.bottom } else { self.rows - 1 }),
            'C' | 'a' => self.cx = (self.cx + p(0, 1)).min(self.cols - 1),
            'D' => self.cx = self.cx.saturating_sub(p(0, 1)),
            'E' => {
                self.cy = (self.cy + p(0, 1)).min(self.rows - 1);
                self.cx = 0;
            }
            'F' => {
                self.cy = self.cy.saturating_sub(p(0, 1));
                self.cx = 0;
            }
            'G' | '`' => self.cx = (p(0, 1) - 1).min(self.cols - 1),
            'd' => self.cy = (p(0, 1) - 1).min(self.rows - 1),
            'H' | 'f' => {
                self.cy = (p(0, 1) - 1).min(self.rows - 1);
                self.cx = (p(1, 1) - 1).min(self.cols - 1);
            }
            'J' => {
                let b = self.blank();
                match ps.first().copied().unwrap_or(0) {
                    0 => {
                        for c in self.cx..self.cols {
                            self.lines[self.cy][c] = b;
                        }
                        for r in self.cy + 1..self.rows {
                            self.lines[r] = vec![b; self.cols];
                        }
                    }
                    1 => {
                        for r in 0..self.cy {
                            self.lines[r] = vec![b; self.cols];
                        }
                        for c in 0..=self.cx.min(self.cols - 1) {
                            self.lines[self.cy][c] = b;
                        }
                    }
                    2 => {
                        for r in 0..self.rows {
                            self.lines[r] = vec![b; self.cols];
                        }
                    }
                    3 => self.scrollback.clear(),
                    _ => {}
                }
            }
            'K' => {
                let b = self.blank();
                let (s, e) = match ps.first().copied().unwrap_or(0) {
                    0 => (self.cx, self.cols),
                    1 => (0, self.cx + 1),
                    _ => (0, self.cols),
                };
                for c in s..e.min(self.cols) {
                    self.lines[self.cy][c] = b;
                }
            }
            'X' => {
                let b = self.blank();
                let n = p(0, 1);
                for c in self.cx..(self.cx + n).min(self.cols) {
                    self.lines[self.cy][c] = b;
                }
            }
            'P' => {
                let n = p(0, 1).min(self.cols - self.cx);
                let b = self.blank();
                let line = &mut self.lines[self.cy];
                line.drain(self.cx..self.cx + n);
                line.extend(std::iter::repeat(b).take(n));
            }
            '@' => {
                let n = p(0, 1).min(self.cols - self.cx);
                let b = self.blank();
                let cols = self.cols;
                let line = &mut self.lines[self.cy];
                for _ in 0..n {
                    line.insert(self.cx, b);
                }
                line.truncate(cols);
            }
            'L' => {
                if self.cy >= self.top && self.cy <= self.bottom {
                    let n = p(0, 1).min(self.bottom - self.cy + 1);
                    for _ in 0..n {
                        self.lines.remove(self.bottom);
                        let b = self.blank();
                        self.lines.insert(self.cy, vec![b; self.cols]);
                    }
                }
            }
            'M' => {
                if self.cy >= self.top && self.cy <= self.bottom {
                    let n = p(0, 1).min(self.bottom - self.cy + 1);
                    for _ in 0..n {
                        self.lines.remove(self.cy);
                        let b = self.blank();
                        self.lines.insert(self.bottom, vec![b; self.cols]);
                    }
                }
            }
            'S' => self.scroll_up(p(0, 1)),
            'T' if !private => self.scroll_down(p(0, 1)),
            'r' => {
                let t = p(0, 1) - 1;
                let b = p(1, self.rows) - 1;
                if t < b && b < self.rows {
                    self.top = t;
                    self.bottom = b;
                } else {
                    self.top = 0;
                    self.bottom = self.rows - 1;
                }
                self.cx = 0;
                self.cy = 0;
            }
            's' => self.saved = (self.cx, self.cy, self.pen),
            'u' => {
                self.cx = self.saved.0.min(self.cols - 1);
                self.cy = self.saved.1.min(self.rows - 1);
                self.pen = self.saved.2;
            }
            'm' => self.sgr(&ps),
            'h' | 'l' => {
                let on = f == 'h';
                if private {
                    for &m in &ps {
                        match m {
                            1 => self.app_cursor = on,
                            7 => self.autowrap = on,
                            25 => self.cursor_visible = on,
                            2004 => self.bracketed_paste = on,
                            47 | 1047 | 1049 => self.set_alt(on),
                            _ => {}
                        }
                    }
                }
            }
            'n' => {
                if ps.first() == Some(&6) {
                    self.replies.extend_from_slice(format!("\x1b[{};{}R", self.cy + 1, self.cx + 1).as_bytes());
                } else if ps.first() == Some(&5) {
                    self.replies.extend_from_slice(b"\x1b[0n");
                }
            }
            'c' if !private => self.replies.extend_from_slice(b"\x1b[?1;0c"),
            _ => {}
        }
    }

    fn set_alt(&mut self, on: bool) {
        if on && self.alt_saved.is_none() {
            let main = std::mem::replace(&mut self.lines, vec![vec![Cell::default(); self.cols]; self.rows]);
            self.alt_saved = Some((main, (self.cx, self.cy)));
        } else if !on {
            if let Some((main, (x, y))) = self.alt_saved.take() {
                self.lines = main;
                self.cx = x.min(self.cols - 1);
                self.cy = y.min(self.rows - 1);
            }
        }
    }

    fn sgr(&mut self, ps: &[usize]) {
        if ps.is_empty() {
            self.pen = Pen::default();
            return;
        }
        let mut i = 0;
        while i < ps.len() {
            let v = ps[i];
            match v {
                0 => self.pen = Pen::default(),
                1 => self.pen.flags |= BOLD,
                2 => self.pen.flags |= DIM,
                3 => self.pen.flags |= ITALIC,
                4 => self.pen.flags |= UNDERLINE,
                7 => self.pen.flags |= INVERSE,
                22 => self.pen.flags &= !(BOLD | DIM),
                23 => self.pen.flags &= !ITALIC,
                24 => self.pen.flags &= !UNDERLINE,
                27 => self.pen.flags &= !INVERSE,
                30..=37 => self.pen.fg = Color::Idx((v - 30) as u8),
                39 => self.pen.fg = Color::Default,
                40..=47 => self.pen.bg = Color::Idx((v - 40) as u8),
                49 => self.pen.bg = Color::Default,
                90..=97 => self.pen.fg = Color::Idx((v - 90 + 8) as u8),
                100..=107 => self.pen.bg = Color::Idx((v - 100 + 8) as u8),
                38 | 48 => {
                    let c = match ps.get(i + 1) {
                        Some(5) => {
                            i += 2;
                            ps.get(i).map(|&n| Color::Idx(n.min(255) as u8))
                        }
                        Some(2) => {
                            i += 4;
                            match (ps.get(i - 2), ps.get(i - 1), ps.get(i)) {
                                (Some(&r), Some(&g), Some(&b)) => Some(Color::Rgb(r.min(255) as u8, g.min(255) as u8, b.min(255) as u8)),
                                _ => None,
                            }
                        }
                        _ => None,
                    };
                    if let Some(c) = c {
                        if v == 38 {
                            self.pen.fg = c;
                        } else {
                            self.pen.bg = c;
                        }
                    }
                }
                _ => {}
            }
            i += 1;
        }
    }

    /// Total number of lines (scrollback + screen).
    pub fn total_lines(&self) -> usize {
        self.scrollback.len() + self.rows
    }

    /// Line by absolute index (0 = oldest scrollback line).
    pub fn line(&self, i: usize) -> Option<&Line> {
        if i < self.scrollback.len() {
            self.scrollback.get(i)
        } else {
            self.lines.get(i - self.scrollback.len())
        }
    }

    pub fn line_text(&self, i: usize, from: usize, to: usize) -> String {
        match self.line(i) {
            Some(l) => l[from.min(l.len())..to.min(l.len())].iter().map(|c| c.ch).collect::<String>(),
            None => String::new(),
        }
    }
}

// ---------------------------------------------------------------- colors

pub fn ansi_color(idx: u8, dark: bool) -> u32 {
    const DARK: [u32; 16] = [0x000000, 0xCD3131, 0x0DBC79, 0xE5E510, 0x2472C8, 0xBC3FBC, 0x11A8CD, 0xE5E5E5, 0x666666, 0xF14C4C, 0x23D18B, 0xF5F543, 0x3B8EEA, 0xD670D6, 0x29B8DB, 0xE5E5E5];
    const LIGHT: [u32; 16] = [0x000000, 0xCD3131, 0x00BC00, 0x949800, 0x0451A5, 0xBC05BC, 0x0598BC, 0x555555, 0x666666, 0xCD3131, 0x14CE14, 0xB5BA00, 0x0451A5, 0xBC05BC, 0x0598BC, 0xA5A5A5];
    let rgb = if idx < 16 {
        if dark { DARK[idx as usize] } else { LIGHT[idx as usize] }
    } else if idx < 232 {
        let i = idx as u32 - 16;
        let level = |v: u32| if v == 0 { 0 } else { 55 + v * 40 };
        (level(i / 36) << 16) | (level((i / 6) % 6) << 8) | level(i % 6)
    } else {
        let g = 8 + (idx as u32 - 232) * 10;
        (g << 16) | (g << 8) | g
    };
    crate::theme::hex(rgb)
}

// ----------------------------------------------------------------- input

/// Translates a non-character key (WM_KEYDOWN) into the VT input sequence.
pub fn key_sequence(vk: u16, ctrl: bool, shift: bool, alt: bool, app_cursor: bool) -> Option<Vec<u8>> {
    let modc = 1 + (shift as u8) + 2 * (alt as u8) + 4 * (ctrl as u8);
    let csi_mod = |base: &str, letter: char| -> Vec<u8> {
        if modc > 1 {
            format!("\x1b[1;{modc}{letter}").into_bytes()
        } else {
            format!("{base}{letter}").into_bytes()
        }
    };
    let arrow = |letter: char| -> Vec<u8> {
        if modc > 1 {
            format!("\x1b[1;{modc}{letter}").into_bytes()
        } else if app_cursor {
            format!("\x1bO{letter}").into_bytes()
        } else {
            format!("\x1b[{letter}").into_bytes()
        }
    };
    let tilde = |n: u8| -> Vec<u8> {
        if modc > 1 {
            format!("\x1b[{n};{modc}~").into_bytes()
        } else {
            format!("\x1b[{n}~").into_bytes()
        }
    };
    Some(match vk {
        0x26 => arrow('A'),
        0x28 => arrow('B'),
        0x27 => arrow('C'),
        0x25 => arrow('D'),
        0x24 => csi_mod("\x1b[", 'H'),
        0x23 => csi_mod("\x1b[", 'F'),
        0x2D => tilde(2),
        0x2E => tilde(3),
        0x21 => tilde(5),
        0x22 => tilde(6),
        0x70 => csi_mod("\x1bO", 'P'),
        0x71 => csi_mod("\x1bO", 'Q'),
        0x72 => csi_mod("\x1bO", 'R'),
        0x73 => csi_mod("\x1bO", 'S'),
        0x74 => tilde(15),
        0x75 => tilde(17),
        0x76 => tilde(18),
        0x77 => tilde(19),
        0x78 => tilde(20),
        0x79 => tilde(21),
        0x7A => tilde(23),
        0x7B => tilde(24),
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(s: &Screen, row: usize) -> String {
        s.lines[row].iter().map(|c| c.ch).collect::<String>().trim_end().to_string()
    }

    #[test]
    fn prints_and_wraps() {
        let mut s = Screen::new(5, 3);
        s.feed(b"hello world");
        assert_eq!(text(&s, 0), "hello");
        assert_eq!(text(&s, 1), " worl");
        assert_eq!(text(&s, 2), "d");
    }

    #[test]
    fn scrolls_into_history() {
        let mut s = Screen::new(10, 2);
        s.feed(b"a\r\nb\r\nc\r\nd");
        assert_eq!(s.scrollback.len(), 2);
        assert_eq!(text(&s, 0), "c");
        assert_eq!(text(&s, 1), "d");
        assert_eq!(s.line_text(0, 0, 1), "a");
    }

    #[test]
    fn cursor_erase_and_colors() {
        let mut s = Screen::new(10, 3);
        s.feed(b"\x1b[2;3Hxy\x1b[1;31mZ\x1b[0m\x1b[1;1Habcdef\x1b[1;3H\x1b[K");
        assert_eq!(text(&s, 0), "ab");
        assert_eq!(text(&s, 1), "  xyZ");
        assert_eq!(s.lines[1][4].fg, Color::Idx(1));
        assert_eq!(s.lines[1][4].flags & BOLD, BOLD);
        s.feed(b"\x1b[38;2;10;20;30mT\x1b[48;5;200mU");
        assert_eq!(s.lines[0][2].fg, Color::Rgb(10, 20, 30));
        assert_eq!(s.lines[0][3].bg, Color::Idx(200));
    }

    #[test]
    fn title_utf8_and_alt_screen() {
        let mut s = Screen::new(10, 2);
        s.feed("\x1b]0;my title\x07h\u{e9}".as_bytes()[..14].as_ref());
        s.feed(&"\x1b]0;my title\x07h\u{e9}".as_bytes()[14..]);
        assert_eq!(s.title, "my title");
        assert_eq!(text(&s, 0), "h\u{e9}");
        s.feed(b"\x1b[?1049h\x1b[HALT");
        assert!(s.in_alt_screen());
        assert_eq!(text(&s, 0), "ALT");
        s.feed(b"\x1b[?1049l");
        assert_eq!(text(&s, 0), "h\u{e9}");
    }

    #[test]
    fn dsr_reply_and_keys() {
        let mut s = Screen::new(10, 5);
        s.feed(b"\x1b[3;4H\x1b[6n");
        assert_eq!(s.replies, b"\x1b[3;4R");
        assert_eq!(key_sequence(0x26, false, false, false, false).unwrap(), b"\x1b[A");
        assert_eq!(key_sequence(0x26, false, false, false, true).unwrap(), b"\x1bOA");
        assert_eq!(key_sequence(0x25, true, false, false, false).unwrap(), b"\x1b[1;5D");
        assert_eq!(key_sequence(0x2E, false, false, false, false).unwrap(), b"\x1b[3~");
    }
}

#[cfg(test)]
mod replay {
    #[test]
    fn replay_capture() {
        let Ok(path) = std::env::var("REPLAY") else { return };
        let data = std::fs::read(path).unwrap();
        let mut s = super::Screen::new(107, 15);
        let i = data.windows(14).position(|w| w == b"git log --stat").unwrap_or(0);
        s.feed(&data[..i]);
        s.resize(122, 15);
        s.feed(&data[i..]);
        for (n, l) in s.lines.iter().enumerate() {
            println!("{n:2}|{}", l.iter().map(|c| c.ch).collect::<String>().trim_end());
        }
        println!("cursor {},{} scrollback {}", s.cx, s.cy, s.scrollback.len());
    }
}
