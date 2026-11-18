//! Application state, background jobs and git commands.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Receiver, Sender};

use windows_sys::Win32::Foundation::{HWND, RECT};
use windows_sys::Win32::Graphics::Gdi::{HBRUSH, HDC, HBITMAP};
use windows_sys::Win32::UI::WindowsAndMessaging::{PostMessageW, SetTimer, KillTimer, WM_APP};

use crate::diff::{self, FileDiff, LineId, Row, Block};
use crate::git::{self, Commit, FileChange, GitError, GitResult, Group, Kind, RefInfo, RefKind, Stash, Status};
use crate::graph::{self, GraphRow};
use crate::settings::Settings;
use crate::theme::Theme;
use crate::watch::Watcher;
use crate::win::{self, Fonts};

pub const WM_JOB: u32 = WM_APP + 1;
pub const WM_FS: u32 = WM_APP + 2;
pub const WM_TERM: u32 = WM_APP + 4;
pub const TIMER_FS: usize = 1;
pub const TIMER_TOAST: usize = 2;
pub const TIMER_SPIN: usize = 3;

pub type Then = Box<dyn FnOnce(&mut App) + Send>;
pub type OnErr = Box<dyn FnOnce(&mut App, &GitError) -> bool + Send>;

pub enum JobResult {
    Refresh { gen: u64, status: GitResult<Status>, refs: Vec<RefInfo>, stashes: Vec<Stash> },
    Log { gen: u64, commits: Vec<Commit>, more: bool, append: bool },
    View { gen: u64, view: MainView, keep_scroll: bool },
    Op { title: String, result: GitResult<String>, then: Option<Then>, on_err: Option<OnErr> },
}

pub struct Repo {
    pub root: PathBuf,
    pub name: String,
    pub status: Status,
    pub refs: Vec<RefInfo>,
    pub stashes: Vec<Stash>,
    pub loaded: bool,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum CRow {
    Group(Group),
    File(usize),
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum DiffKind {
    WorkingTree,
    Index,
    Untracked,
    Commit,
}

pub struct DiffView {
    pub title: String,
    pub desc: String,
    pub path: String,
    pub kind: DiffKind,
    pub change: Option<FileChange>,
    pub fd: FileDiff,
    pub rows: Vec<Row>,
    pub blocks: Vec<Block>,
    pub sel: Option<(usize, usize)>,
    pub back: Option<Box<MainView>>,
    pub max_cols: usize,
}

pub struct FileView {
    pub title: String,
    pub path: String,
    pub lines: Vec<String>,
    pub conflicts: Vec<diff::Conflict>,
    pub change: Option<FileChange>,
}

pub struct CommitView {
    pub commit: Commit,
    pub files: Vec<FileChange>,
    pub parent: Option<String>,
    pub sel: Option<usize>,
    pub stash: Option<usize>,
}

pub enum MainView {
    Empty,
    Loading(String),
    Message(String, String),
    Diff(DiffView),
    File(FileView),
    Commit(CommitView),
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Area {
    Changes = 0,
    Graph = 1,
    Main = 2,
    Output = 3,
    Pick = 4,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Focus {
    Changes,
    Graph,
    Main,
    Terminal,
}

pub struct PickItem {
    pub label: String,
    pub desc: String,
    pub icon: char,
    pub data: String,
    pub sep: String,
}

pub fn pi(label: impl Into<String>, desc: impl Into<String>, icon: char, data: impl Into<String>) -> PickItem {
    PickItem { label: label.into(), desc: desc.into(), icon, data: data.into(), sep: String::new() }
}

pub enum OverlayKind {
    Pick { items: Vec<PickItem>, filtered: Vec<usize>, sel: usize, on_pick: Option<Box<dyn FnOnce(&mut App, &PickItem)>> },
    Input { prompt: String, on_ok: Option<Box<dyn FnOnce(&mut App, String)>> },
    Confirm { message: String, detail: String, buttons: Vec<String>, sel: usize, on_choice: Option<Box<dyn FnOnce(&mut App, usize)>> },
}

pub struct Overlay {
    pub kind: OverlayKind,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ToastKind {
    Info,
    Warning,
    Error,
}

pub struct Toast {
    pub kind: ToastKind,
    pub message: String,
    pub expires: std::time::Instant,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Sash {
    Sidebar,
    Graph,
    Output,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Hit {
    None,
    Backdrop,
    Sash(Sash),
    Cmd(Cmd),
    GroupRow(usize),
    GroupAct(usize, u8),
    FileRow(usize),
    FileAct(usize, u8),
    CommitBtn,
    CommitDrop,
    ActionBtn,
    GraphRow(usize),
    GraphMore,
    DiffRow(usize),
    DiffBlock(usize, u8),
    DiffBack,
    CommitFile(usize),
    Conflict(usize, u8),
    Thumb(Area),
    PickRow(usize),
    DialogBtn(usize),
    ToastClose(usize),
    ToastOutput,
    StatusBranch,
    StatusSync,
    Recent(usize),
    Welcome(u8),
    PanelTab(u8),
    TermBody,
    TermShellMenu,
}

pub enum Drag {
    Sash(Sash, i32, i32),
    Thumb(Area, i32, i32),
    DiffSelect,
    TermSelect,
}

#[derive(Clone, Copy)]
pub struct Layout {
    pub activity: RECT,
    pub sidebar: RECT,
    pub title: RECT,
    pub changes_header: RECT,
    pub edit: RECT,
    pub button: RECT,
    pub changes: RECT,
    pub graph_header: RECT,
    pub graph: RECT,
    pub main: RECT,
    pub main_header: RECT,
    pub main_body: RECT,
    pub output: RECT,
    pub output_body: RECT,
    pub status: RECT,
}

impl Default for Layout {
    fn default() -> Self {
        let z = RECT { left: 0, top: 0, right: 0, bottom: 0 };
        Layout { activity: z, sidebar: z, title: z, changes_header: z, edit: z, button: z, changes: z, graph_header: z, graph: z, main: z, main_header: z, main_body: z, output: z, output_body: z, status: z }
    }
}

macro_rules! cmds {
    ($($name:ident => $title:expr, $key:expr;)*) => {
        #[derive(Clone, Copy, PartialEq, Eq, Debug)]
        pub enum Cmd { $($name),* }
        pub const COMMANDS: &[(Cmd, &str, &str)] = &[$((Cmd::$name, $title, $key)),*];
    };
}

cmds! {
    Palette => "Show All Commands", "Ctrl+Shift+P";
    Refresh => "Git: Refresh", "F5";
    Commit => "Git: Commit", "Ctrl+Enter";
    CommitAmend => "Git: Commit (Amend)", "";
    CommitAll => "Git: Commit All", "";
    CommitPush => "Git: Commit & Push", "";
    CommitSync => "Git: Commit & Sync", "";
    UndoCommit => "Git: Undo Last Commit", "";
    StageAll => "Git: Stage All Changes", "";
    UnstageAll => "Git: Unstage All Changes", "";
    DiscardAll => "Git: Discard All Changes", "";
    Checkout => "Git: Checkout to...", "";
    Branch => "Git: Create Branch...", "";
    BranchFrom => "Git: Create Branch From...", "";
    RenameBranch => "Git: Rename Branch...", "";
    DeleteBranch => "Git: Delete Branch...", "";
    Merge => "Git: Merge...", "";
    Rebase => "Git: Rebase Branch...", "";
    MergeAbort => "Git: Abort Merge", "";
    RebaseAbort => "Git: Abort Rebase", "";
    RebaseContinue => "Git: Continue Rebase", "";
    Fetch => "Git: Fetch", "";
    FetchPrune => "Git: Fetch (Prune)", "";
    FetchAll => "Git: Fetch From All Remotes", "";
    Pull => "Git: Pull", "";
    PullRebase => "Git: Pull (Rebase)", "";
    Push => "Git: Push", "";
    PushForce => "Git: Push (Force With Lease)", "";
    PushTags => "Git: Push Tags", "";
    Sync => "Git: Sync", "";
    Publish => "Git: Publish Branch...", "";
    AddRemote => "Git: Add Remote...", "";
    RemoveRemote => "Git: Remove Remote", "";
    Stash => "Git: Stash", "";
    StashUntracked => "Git: Stash (Include Untracked)", "";
    StashStaged => "Git: Stash Staged", "";
    StashPop => "Git: Pop Latest Stash", "";
    StashPopPick => "Git: Pop Stash...", "";
    StashApply => "Git: Apply Stash...", "";
    StashDrop => "Git: Drop Stash...", "";
    StashView => "Git: View Stash...", "";
    TagCreate => "Git: Create Tag", "";
    TagDelete => "Git: Delete Tag", "";
    CherryPick => "Git: Cherry Pick...", "";
    ConfigureUser => "Git: Configure User Name and Email", "";
    OpenFolder => "File: Open Folder...", "Ctrl+O";
    OpenRecent => "File: Open Recent...", "Ctrl+R";
    NewWindow => "File: New Window", "Ctrl+Shift+N";
    Clone => "Git: Clone", "";
    Init => "Git: Initialize Repository", "";
    OpenExplorer => "File: Reveal Repository in File Explorer", "";
    OpenTerminal => "Terminal: Open External Terminal", "Ctrl+Shift+C";
    ToggleTerminal => "View: Toggle Terminal", "Ctrl+`";
    NewTerminal => "Terminal: Create New Terminal", "Ctrl+Shift+`";
    KillTerminal => "Terminal: Kill the Active Terminal Instance", "";
    SelectShell => "Terminal: Select Default Profile", "";
    ClearTerminal => "Terminal: Clear", "";
    ToggleOutput => "View: Toggle Output", "Ctrl+J";
    ToggleTheme => "Preferences: Toggle Light/Dark Theme", "";
    ToggleInline => "Diff: Toggle Inline View", "";
    ToggleCompact => "Diff: Toggle Collapse Unchanged Regions", "";
    ToggleGraphAll => "Graph: Toggle Show All Branches", "";
    NextChange => "Diff: Go to Next Change", "F7";
    PrevChange => "Diff: Go to Previous Change", "Shift+F7";
    FocusCommit => "Source Control: Focus Commit Message", "Ctrl+Shift+G";
    ClearOutput => "Output: Clear", "";
    About => "Help: About", "";
}

pub struct App {
    pub hwnd: HWND,
    pub dpi: u32,
    pub scale: f32,
    pub fonts: Fonts,
    pub t: Theme,
    pub st: Settings,
    pub tx: Sender<JobResult>,
    pub rx: Receiver<JobResult>,
    pub op_tx: Sender<Box<dyn FnOnce() + Send>>,
    pub repo: Option<Repo>,
    pub watcher: Option<Watcher>,
    pub refresh_gen: u64,
    pub refreshing: bool,
    pub refresh_pending: bool,
    pub log_gen: u64,
    pub commits: Vec<Commit>,
    pub graph_rows: Vec<GraphRow>,
    pub has_more: bool,
    pub log_loading: bool,
    pub log_key: String,
    pub crows: Vec<CRow>,
    pub collapsed: [bool; 3],
    pub sel: HashSet<(u8, String)>,
    pub focus_row: Option<usize>,
    pub anchor_row: Option<usize>,
    pub scroll: [i32; 5],
    pub hscroll: i32,
    pub view: MainView,
    pub view_gen: u64,
    pub ops: Vec<String>,
    pub spin: u32,
    pub overlay: Option<Overlay>,
    pub toasts: Vec<Toast>,
    pub hits: Vec<(RECT, Hit)>,
    pub hover: Hit,
    pub drag: Option<Drag>,
    pub focus: Focus,
    pub commit_edit: HWND,
    pub overlay_edit: HWND,
    pub edit_brush: HBRUSH,
    pub edit_lines: i32,
    pub l: Layout,
    pub graph_sel: Option<usize>,
    pub git_version: Option<String>,
    pub buf: (HDC, HBITMAP, i32, i32),
    pub output_seen: usize,
    pub mouse: (i32, i32),
    pub pending_init: Option<PathBuf>,
    pub pending_file: Option<PathBuf>,
    pub term: Option<crate::term_ui::TermSession>,
    pub term_pending: std::sync::Arc<std::sync::atomic::AtomicBool>,
    pub term_scroll: usize,
    pub term_sel: Option<((usize, usize), (usize, usize))>,
    pub term_skip_char: bool,
    pub term_surrogate: u16,
    pub shells: Vec<crate::term::Shell>,
}

pub fn group_id(g: Group) -> u8 {
    match g {
        Group::Merge => 0,
        Group::Staged => 1,
        Group::Changes => 2,
    }
}

pub fn group_label(g: Group) -> &'static str {
    match g {
        Group::Merge => "Merge Changes",
        Group::Staged => "Staged Changes",
        Group::Changes => "Changes",
    }
}

fn post(hwnd: isize) {
    unsafe {
        PostMessageW(hwnd as HWND, WM_JOB, 0, 0);
    }
}

impl App {
    pub fn new(hwnd: HWND, dpi: u32, st: Settings) -> App {
        let (tx, rx) = channel();
        let (op_tx, op_rx) = channel::<Box<dyn FnOnce() + Send>>();
        std::thread::Builder::new()
            .name("git-ops".into())
            .stack_size(512 * 1024)
            .spawn(move || {
                for job in op_rx {
                    job();
                }
            })
            .unwrap();
        let scale = dpi as f32 / 96.0;
        let t = if st.dark { Theme::dark() } else { Theme::light() };
        App {
            hwnd,
            dpi,
            scale,
            fonts: Fonts::new(scale),
            t,
            st,
            tx,
            rx,
            op_tx,
            repo: None,
            watcher: None,
            refresh_gen: 0,
            refreshing: false,
            refresh_pending: false,
            log_gen: 0,
            commits: Vec::new(),
            graph_rows: Vec::new(),
            has_more: false,
            log_loading: false,
            log_key: String::new(),
            crows: Vec::new(),
            collapsed: [false; 3],
            sel: HashSet::new(),
            focus_row: None,
            anchor_row: None,
            scroll: [0; 5],
            hscroll: 0,
            view: MainView::Empty,
            view_gen: 0,
            ops: Vec::new(),
            spin: 0,
            overlay: None,
            toasts: Vec::new(),
            hits: Vec::new(),
            hover: Hit::None,
            drag: None,
            focus: Focus::Changes,
            commit_edit: std::ptr::null_mut(),
            overlay_edit: std::ptr::null_mut(),
            edit_brush: std::ptr::null_mut(),
            edit_lines: 1,
            l: Layout::default(),
            graph_sel: None,
            git_version: None,
            buf: (std::ptr::null_mut(), std::ptr::null_mut(), 0, 0),
            output_seen: 0,
            mouse: (0, 0),
            pending_init: None,
            pending_file: None,
            term: None,
            term_pending: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
            term_scroll: 0,
            term_sel: None,
            term_skip_char: false,
            term_surrogate: 0,
            shells: Vec::new(),
        }
    }

    pub fn s(&self, v: i32) -> i32 {
        (v as f32 * self.scale).round() as i32
    }

    pub fn invalidate(&self) {
        unsafe {
            windows_sys::Win32::Graphics::Gdi::InvalidateRect(self.hwnd, std::ptr::null(), 0);
        }
    }

    // ------------------------------------------------------------- toasts

    pub fn toast(&mut self, kind: ToastKind, msg: impl Into<String>) {
        let secs = if kind == ToastKind::Error { 12 } else { 6 };
        self.toasts.push(Toast { kind, message: msg.into(), expires: std::time::Instant::now() + std::time::Duration::from_secs(secs) });
        if self.toasts.len() > 3 {
            self.toasts.remove(0);
        }
        unsafe {
            SetTimer(self.hwnd, TIMER_TOAST, 500, None);
        }
        self.invalidate();
    }

    pub fn info(&mut self, msg: impl Into<String>) {
        self.toast(ToastKind::Info, msg);
    }

    pub fn warn(&mut self, msg: impl Into<String>) {
        self.toast(ToastKind::Warning, msg);
    }

    pub fn error(&mut self, msg: impl Into<String>) {
        self.toast(ToastKind::Error, msg);
    }

    pub fn tick_toasts(&mut self) {
        let now = std::time::Instant::now();
        let n = self.toasts.len();
        self.toasts.retain(|t| t.expires > now);
        if self.toasts.is_empty() {
            unsafe {
                KillTimer(self.hwnd, TIMER_TOAST);
            }
        }
        if n != self.toasts.len() {
            self.invalidate();
        }
    }

    // --------------------------------------------------------------- repo

    pub fn open_repo(&mut self, folder: &Path) {
        let Some(root) = git::toplevel(folder) else {
            let f = folder.display().to_string();
            self.repo = None;
            self.watcher = None;
            self.view = MainView::Message("Not a Git repository".into(), format!("The folder '{f}' doesn't have a git repository. Use 'Git: Initialize Repository' to create one."));
            self.st.add_recent(&f);
            self.st.save();
            self.pending_init = Some(folder.to_path_buf());
            self.update_title();
            self.invalidate();
            return;
        };
        self.pending_init = None;
        let name = root.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| root.display().to_string());
        self.st.add_recent(&root.display().to_string());
        self.st.save();
        self.watcher = None;
        self.watcher = Watcher::start(&root, self.hwnd, WM_FS);
        self.repo = Some(Repo { root, name, status: Status::default(), refs: vec![], stashes: vec![], loaded: false });
        self.commits.clear();
        self.graph_rows.clear();
        self.log_key.clear();
        self.sel.clear();
        self.crows.clear();
        self.scroll = [0; 5];
        self.view = MainView::Empty;
        win::set_text(self.commit_edit, "");
        self.refresh();
        self.update_title();
    }

    /// Opens a command line target (folder or file), like `code <path>`.
    pub fn open_target(&mut self, t: crate::cli::Target) {
        self.pending_file = t.file;
        self.open_repo(&t.folder);
    }

    /// Shows the changes of a file given on the command line.
    pub fn open_file_target(&mut self, file: &Path) {
        let Some(root) = self.root() else { return };
        let canon_root = std::fs::canonicalize(&root).ok();
        let canon_file = std::fs::canonicalize(file).ok();
        let rel = match (canon_root, canon_file) {
            (Some(r), Some(f)) => f.strip_prefix(&r).ok().map(|p| p.to_string_lossy().replace('\\', "/")),
            _ => None,
        };
        let Some(rel) = rel else { return };
        let found = self.repo.as_ref().and_then(|r| {
            let order = [Group::Merge, Group::Changes, Group::Staged];
            order.iter().find_map(|g| r.status.files.iter().find(|f| f.group == *g && f.path.eq_ignore_ascii_case(&rel)).cloned())
        });
        match found {
            Some(f) => {
                if let Some(i) = self.crows.iter().position(|r| matches!(r, CRow::File(x) if self.repo.as_ref().map(|rp| rp.status.files[*x].path == f.path && rp.status.files[*x].group == f.group).unwrap_or(false))) {
                    self.focus_row = Some(i);
                    self.sel.clear();
                    self.sel.insert((group_id(f.group), f.path.clone()));
                }
                self.load_change_view(f, false);
            }
            None => {
                self.view = MainView::Message(short_name(&rel).to_string(), format!("'{rel}' has no changes."));
                self.invalidate();
            }
        }
    }

    pub fn root(&self) -> Option<PathBuf> {
        self.repo.as_ref().map(|r| r.root.clone())
    }

    pub fn refresh(&mut self) {
        let Some(root) = self.root() else { return };
        if self.refreshing {
            self.refresh_pending = true;
            return;
        }
        self.refreshing = true;
        self.refresh_gen += 1;
        let gen = self.refresh_gen;
        let tx = self.tx.clone();
        let hwnd = self.hwnd as isize;
        std::thread::spawn(move || {
            let (status, (refs, stashes)) = std::thread::scope(|sc| {
                let a = sc.spawn(|| git::refs(&root));
                let b = sc.spawn(|| git::stashes(&root));
                let s = git::status(&root);
                (s, (a.join().unwrap_or_default(), b.join().unwrap_or_default()))
            });
            let _ = tx.send(JobResult::Refresh { gen, status, refs, stashes });
            post(hwnd);
        });
    }

    pub fn on_jobs(&mut self) {
        while let Ok(r) = self.rx.try_recv() {
            match r {
                JobResult::Refresh { gen, status, refs, stashes } => {
                    if gen != self.refresh_gen {
                        continue;
                    }
                    self.refreshing = false;
                    match status {
                        Ok(s) => self.apply_status(s, refs, stashes),
                        Err(e) => self.error(e.message),
                    }
                    if self.refresh_pending {
                        self.refresh_pending = false;
                        self.refresh();
                    }
                }
                JobResult::Log { gen, commits, more, append } => {
                    if gen != self.log_gen {
                        continue;
                    }
                    self.log_loading = false;
                    if append {
                        self.commits.extend(commits);
                    } else {
                        self.commits = commits;
                    }
                    self.has_more = more;
                    self.rebuild_graph();
                }
                JobResult::View { gen, view, keep_scroll } => {
                    if gen != self.view_gen {
                        continue;
                    }
                    if !keep_scroll {
                        self.scroll[Area::Main as usize] = 0;
                        self.hscroll = 0;
                    } else if let (MainView::Diff(old), MainView::Diff(_)) = (&self.view, &view) {
                        let _ = old;
                    }
                    let prev_sel = if let MainView::Diff(d) = &self.view { d.sel } else { None };
                    self.view = view;
                    if keep_scroll {
                        if let MainView::Diff(d) = &mut self.view {
                            if let Some((a, b)) = prev_sel {
                                if a < d.rows.len() && b < d.rows.len() {
                                    d.sel = Some((a, b));
                                }
                            }
                        }
                    }
                }
                JobResult::Op { title, result, then, on_err } => {
                    if let Some(i) = self.ops.iter().position(|t| *t == title) {
                        self.ops.remove(i);
                    }
                    match result {
                        Ok(_) => {
                            if let Some(t) = then {
                                t(self);
                            }
                        }
                        Err(e) => {
                            let handled = match on_err {
                                Some(f) => f(self, &e),
                                None => false,
                            };
                            if !handled {
                                self.error(e.message.clone());
                            }
                        }
                    }
                    if self.ops.is_empty() {
                        unsafe {
                            KillTimer(self.hwnd, TIMER_SPIN);
                        }
                    }
                    self.refresh();
                }
            }
        }
        self.invalidate();
    }

    fn apply_status(&mut self, s: Status, refs: Vec<RefInfo>, stashes: Vec<Stash>) {
        let first = self.repo.as_ref().map(|r| !r.loaded).unwrap_or(false);
        let Some(repo) = self.repo.as_mut() else { return };
        // keep merge message in the commit box
        if let Some(m) = &s.merge_msg {
            if win::get_text(self.commit_edit).trim().is_empty() && !m.is_empty() {
                win::set_text(self.commit_edit, m);
            }
        }
        repo.status = s;
        unsafe {
            windows_sys::Win32::Graphics::Gdi::InvalidateRect(self.commit_edit, std::ptr::null(), 1);
        }
        repo.refs = refs;
        repo.stashes = stashes;
        repo.loaded = true;
        self.rebuild_changes();
        let r = self.repo.as_ref().unwrap();
        let mut key = format!("{:?}|{:?}|{:?}|{}", r.status.head, r.status.branch, r.status.upstream, self.st.graph_all);
        for rf in &r.refs {
            key.push_str(&rf.name);
            key.push_str(&rf.sha[..rf.sha.len().min(8)]);
        }
        if key != self.log_key {
            self.log_key = key;
            self.load_log(false);
        }
        self.update_title();
        if first {
            if let Some(i) = self.crows.iter().position(|r| matches!(r, CRow::File(_))) {
                self.focus_row = Some(i);
            }
            if let Some(file) = self.pending_file.take() {
                self.open_file_target(&file);
                return;
            }
        }
        // live-update the open change diff
        let current = match &self.view {
            MainView::Diff(d) if d.kind != DiffKind::Commit && d.back.is_none() => d.change.clone(),
            MainView::File(f) => f.change.clone(),
            _ => None,
        };
        if let Some(c) = current {
            let still = self.repo.as_ref().unwrap().status.files.iter().find(|f| f.path == c.path && f.group == c.group).cloned();
            if let Some(f) = still {
                self.load_change_view(f, true);
            }
        }
    }

    pub fn rebuild_changes(&mut self) {
        let Some(repo) = &self.repo else {
            self.crows.clear();
            return;
        };
        let mut rows = Vec::new();
        for g in [Group::Merge, Group::Staged, Group::Changes] {
            let idx: Vec<usize> = repo.status.files.iter().enumerate().filter(|(_, f)| f.group == g).map(|(i, _)| i).collect();
            if idx.is_empty() && g != Group::Changes {
                continue;
            }
            rows.push(CRow::Group(g));
            if !self.collapsed[group_id(g) as usize] {
                rows.extend(idx.into_iter().map(CRow::File));
            }
        }
        let keys: HashSet<(u8, String)> = repo.status.files.iter().map(|f| (group_id(f.group), f.path.clone())).collect();
        self.sel.retain(|k| keys.contains(k));
        self.crows = rows;
        if let Some(f) = self.focus_row {
            if f >= self.crows.len() {
                self.focus_row = if self.crows.is_empty() { None } else { Some(self.crows.len() - 1) };
            }
        }
    }

    pub fn file(&self, idx: usize) -> Option<&FileChange> {
        self.repo.as_ref().and_then(|r| r.status.files.get(idx))
    }

    pub fn update_title(&self) {
        let title = match &self.repo {
            Some(r) => {
                let b = r.status.branch.clone().unwrap_or_else(|| r.status.head.as_deref().map(|h| h[..h.len().min(8)].to_string()).unwrap_or_default());
                format!("{} ({}) - GitGui", r.name, b)
            }
            None => "GitGui".to_string(),
        };
        let t = win::wz(&title);
        unsafe {
            windows_sys::Win32::UI::WindowsAndMessaging::SetWindowTextW(self.hwnd, t.as_ptr());
        }
    }

    // --------------------------------------------------------------- graph

    pub fn load_log(&mut self, append: bool) {
        let Some(repo) = &self.repo else { return };
        if repo.status.head.is_none() {
            self.commits.clear();
            self.graph_rows.clear();
            self.has_more = false;
            return;
        }
        let root = repo.root.clone();
        let refs: Vec<String> = if self.st.graph_all {
            vec!["--branches".into(), "--remotes".into(), "--tags".into(), "HEAD".into()]
        } else {
            let mut v = vec!["HEAD".to_string()];
            if let Some(u) = &repo.status.upstream {
                v.push(format!("refs/remotes/{u}"));
            }
            v
        };
        let skip = if append { self.commits.len() } else { 0 };
        let limit = if append { 100 } else { self.commits.len().clamp(100, 1000) };
        self.log_gen += 1;
        let gen = self.log_gen;
        self.log_loading = true;
        let tx = self.tx.clone();
        let hwnd = self.hwnd as isize;
        std::thread::spawn(move || {
            let (commits, more) = git::log(&root, &refs, skip, limit);
            let _ = tx.send(JobResult::Log { gen, commits, more, append });
            post(hwnd);
        });
    }

    pub fn rebuild_graph(&mut self) {
        let (b, u, h) = match &self.repo {
            Some(r) => (r.status.branch.clone(), r.status.upstream.clone(), r.status.head.clone()),
            None => (None, None, None),
        };
        self.graph_rows = graph::compute(&self.commits, b.as_deref(), u.as_deref(), h.as_deref());
    }

    // ---------------------------------------------------------------- ops

    pub fn op<F>(&mut self, title: &str, f: F, then: Option<Then>, on_err: Option<OnErr>)
    where
        F: FnOnce(&Path) -> GitResult<String> + Send + 'static,
    {
        let Some(root) = self.root() else {
            self.info("There are no available repositories.");
            return;
        };
        let tx = self.tx.clone();
        let hwnd = self.hwnd as isize;
        let t = title.to_string();
        self.ops.push(t.clone());
        unsafe {
            SetTimer(self.hwnd, TIMER_SPIN, 120, None);
        }
        let _ = self.op_tx.send(Box::new(move || {
            let result = f(&root);
            let _ = tx.send(JobResult::Op { title: t, result, then, on_err });
            post(hwnd);
        }));
        self.invalidate();
    }

    /// Simple op: runs one git command.
    pub fn git_op(&mut self, title: &str, args: Vec<String>) {
        self.op(title, move |root| {
            let a: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
            git::exec(root, &a)
        }, None, None);
    }

    pub fn status(&self) -> Option<&Status> {
        self.repo.as_ref().map(|r| &r.status)
    }

    // ---------------------------------------------------------- overlays

    pub fn pick(&mut self, placeholder: &str, items: Vec<PickItem>, on_pick: impl FnOnce(&mut App, &PickItem) + 'static) {
        let filtered = (0..items.len()).collect();
        self.overlay = Some(Overlay { kind: OverlayKind::Pick { items, filtered, sel: 0, on_pick: Some(Box::new(on_pick)) } });
        self.scroll[Area::Pick as usize] = 0;
        self.show_overlay_edit(placeholder, "");
    }

    pub fn input(&mut self, prompt: &str, value: &str, on_ok: impl FnOnce(&mut App, String) + 'static) {
        self.overlay = Some(Overlay { kind: OverlayKind::Input { prompt: format!("{prompt} (Press 'Enter' to confirm or 'Escape' to cancel)"), on_ok: Some(Box::new(on_ok)) } });
        self.show_overlay_edit(prompt, value);
    }

    pub fn confirm(&mut self, message: &str, detail: &str, buttons: &[&str], on_choice: impl FnOnce(&mut App, usize) + 'static) {
        let mut b: Vec<String> = buttons.iter().map(|s| s.to_string()).collect();
        b.push("Cancel".into());
        self.overlay = Some(Overlay { kind: OverlayKind::Confirm { message: message.into(), detail: detail.into(), buttons: b, sel: 0, on_choice: Some(Box::new(on_choice)) } });
        self.hide_overlay_edit();
        unsafe {
            windows_sys::Win32::UI::Input::KeyboardAndMouse::SetFocus(self.hwnd);
        }
        self.invalidate();
    }

    pub fn close_overlay(&mut self) {
        self.overlay = None;
        self.hide_overlay_edit();
        unsafe {
            windows_sys::Win32::UI::Input::KeyboardAndMouse::SetFocus(self.hwnd);
        }
        self.invalidate();
    }

    pub fn filter_pick(&mut self) {
        let q = win::get_text(self.overlay_edit).to_lowercase();
        if let Some(Overlay { kind: OverlayKind::Pick { items, filtered, sel, .. } }) = &mut self.overlay {
            let words: Vec<&str> = q.split_whitespace().collect();
            *filtered = (0..items.len())
                .filter(|&i| {
                    let hay = format!("{} {}", items[i].label, items[i].desc).to_lowercase();
                    words.iter().all(|w| hay.contains(w))
                })
                .collect();
            *sel = 0;
        }
        self.scroll[Area::Pick as usize] = 0;
        self.invalidate();
    }

    pub fn accept_overlay(&mut self, index: Option<usize>) {
        let Some(ov) = self.overlay.take() else { return };
        let text = win::get_text(self.overlay_edit);
        self.hide_overlay_edit();
        unsafe {
            windows_sys::Win32::UI::Input::KeyboardAndMouse::SetFocus(self.hwnd);
        }
        match ov.kind {
            OverlayKind::Pick { items, filtered, sel, mut on_pick } => {
                let i = index.unwrap_or(sel);
                if let Some(&ix) = filtered.get(i) {
                    if let Some(f) = on_pick.take() {
                        f(self, &items[ix]);
                    }
                }
            }
            OverlayKind::Input { mut on_ok, .. } => {
                if let Some(f) = on_ok.take() {
                    f(self, text);
                }
            }
            OverlayKind::Confirm { buttons, sel, mut on_choice, .. } => {
                let i = index.unwrap_or(sel);
                if i + 1 < buttons.len() {
                    if let Some(f) = on_choice.take() {
                        f(self, i);
                    }
                }
            }
        }
        self.invalidate();
    }

    // ------------------------------------------------------------ views

    pub fn diff_context(&self) -> String {
        format!("-U{}", if self.st.compact_diff { 3 } else { 100000 })
    }

    pub fn load_change_view(&mut self, f: FileChange, keep_scroll: bool) {
        let Some(root) = self.root() else { return };
        self.view_gen += 1;
        let gen = self.view_gen;
        if !keep_scroll {
            self.view = MainView::Loading(f.path.clone());
        }
        let ctx = self.diff_context();
        let inline = self.st.inline_diff;
        let tx = self.tx.clone();
        let hwnd = self.hwnd as isize;
        std::thread::spawn(move || {
            let view = build_change_view(&root, &f, &ctx, inline);
            let _ = tx.send(JobResult::View { gen, view, keep_scroll });
            post(hwnd);
        });
        self.invalidate();
    }

    pub fn open_commit(&mut self, commit: Commit, stash: Option<usize>) {
        let Some(root) = self.root() else { return };
        self.view_gen += 1;
        let gen = self.view_gen;
        self.view = MainView::Loading(commit.subject().to_string());
        let tx = self.tx.clone();
        let hwnd = self.hwnd as isize;
        std::thread::spawn(move || {
            let (files, parent) = git::commit_files(&root, &commit.hash);
            let view = MainView::Commit(CommitView { commit, files, parent, sel: None, stash });
            let _ = tx.send(JobResult::View { gen, view, keep_scroll: false });
            post(hwnd);
        });
        self.invalidate();
    }

    pub fn open_commit_file(&mut self, index: usize) {
        let Some(root) = self.root() else { return };
        let MainView::Commit(cv) = std::mem::replace(&mut self.view, MainView::Empty) else { return };
        let Some(file) = cv.files.get(index).cloned() else {
            self.view = MainView::Commit(cv);
            return;
        };
        let sha = cv.commit.hash.clone();
        let parent = cv.parent.clone();
        let mut cv = cv;
        cv.sel = Some(index);
        self.view_gen += 1;
        let gen = self.view_gen;
        let ctx = self.diff_context();
        let inline = self.st.inline_diff;
        let tx = self.tx.clone();
        let hwnd = self.hwnd as isize;
        let back = Box::new(MainView::Commit(cv));
        self.view = MainView::Loading(file.path.clone());
        // keep `back` alive across the thread by sending it along
        std::thread::spawn(move || {
            let mut args: Vec<String> = match &parent {
                Some(p) => vec!["diff".into(), "--no-color".into(), "--no-ext-diff".into(), "-M".into(), ctx.clone(), p.clone(), sha.clone()],
                None => vec!["show".into(), "--format=".into(), "--no-color".into(), "--no-ext-diff".into(), ctx.clone(), sha.clone()],
            };
            args.push("--".into());
            if let Some(o) = &file.orig {
                args.push(o.clone());
            }
            args.push(file.path.clone());
            let a: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
            let out = git::run(&root, &a, None, false);
            let text = out.text();
            let mut view = diff_view_from(&text, &file, DiffKind::Commit, inline, &format!("{} ({})", short_name(&file.path), &sha[..8]));
            if let MainView::Diff(d) = &mut view {
                d.back = Some(back);
            } else {
                view = *back;
            }
            let _ = tx.send(JobResult::View { gen, view, keep_scroll: false });
            post(hwnd);
        });
        self.invalidate();
    }

    pub fn go_back(&mut self) {
        if let MainView::Diff(d) = &mut self.view {
            if let Some(b) = d.back.take() {
                self.view = *b;
                self.scroll[Area::Main as usize] = 0;
                self.invalidate();
            }
        }
    }

    pub fn rebuild_rows(&mut self) {
        let inline = self.st.inline_diff;
        if let MainView::Diff(d) = &mut self.view {
            let (rows, blocks) = diff::build_rows(&d.fd, inline);
            d.rows = rows;
            d.blocks = blocks;
            d.sel = None;
        }
    }

    // ------------------------------------------------------------ actions on files

    pub fn selected_files(&self, row: Option<usize>) -> Vec<FileChange> {
        let Some(repo) = &self.repo else { return vec![] };
        let in_sel = |f: &FileChange| self.sel.contains(&(group_id(f.group), f.path.clone()));
        if let Some(r) = row {
            if let Some(CRow::File(i)) = self.crows.get(r) {
                let f = &repo.status.files[*i];
                if !in_sel(f) {
                    return vec![f.clone()];
                }
                return repo.status.files.iter().filter(|x| x.group == f.group && in_sel(x)).cloned().collect();
            }
            return vec![];
        }
        repo.status.files.iter().filter(|f| in_sel(f)).cloned().collect()
    }

    pub fn stage(&mut self, files: Vec<FileChange>) {
        if files.is_empty() {
            return;
        }
        let conflicted: Vec<String> = files.iter().filter(|f| f.group == Group::Merge).map(|f| f.path.clone()).collect();
        let root = self.root().unwrap();
        let still_marked: Vec<String> = conflicted.iter().filter(|p| std::fs::read_to_string(root.join(p)).map(|t| t.lines().any(|l| l.starts_with("<<<<<<<"))).unwrap_or(false)).cloned().collect();
        let paths: Vec<String> = files.iter().map(|f| f.path.clone()).collect();
        let run = move |app: &mut App| {
            app.op("Stage", move |root| {
                for chunk in paths.chunks(200) {
                    let mut a = vec!["add", "-A", "--"];
                    a.extend(chunk.iter().map(|s| s.as_str()));
                    git::exec(root, &a)?;
                }
                Ok(String::new())
            }, None, None);
        };
        if !still_marked.is_empty() {
            let name = short_name(&still_marked[0]).to_string();
            self.confirm(&format!("Are you sure you want to stage {name} with merge conflicts?"), "", &["Yes"], move |app, _| run(app));
        } else {
            run(self);
        }
    }

    pub fn unstage(&mut self, files: Vec<FileChange>) {
        let mut paths: Vec<String> = Vec::new();
        for f in files.iter().filter(|f| f.group == Group::Staged) {
            paths.push(f.path.clone());
            if let Some(o) = &f.orig {
                paths.push(o.clone());
            }
        }
        if paths.is_empty() {
            return;
        }
        let has_head = self.status().map(|s| s.head.is_some()).unwrap_or(false);
        self.op("Unstage", move |root| {
            for chunk in paths.chunks(200) {
                let mut a = if has_head { vec!["reset", "-q", "HEAD", "--"] } else { vec!["rm", "--cached", "-r", "-q", "--"] };
                a.extend(chunk.iter().map(|s| s.as_str()));
                git::exec(root, &a)?;
            }
            Ok(String::new())
        }, None, None);
    }

    pub fn discard(&mut self, files: Vec<FileChange>) {
        let files: Vec<FileChange> = files.into_iter().filter(|f| f.group == Group::Changes).collect();
        if files.is_empty() {
            return;
        }
        let untracked: Vec<String> = files.iter().filter(|f| f.kind == Kind::Untracked).map(|f| f.path.clone()).collect();
        let tracked: Vec<String> = files.iter().filter(|f| f.kind != Kind::Untracked).map(|f| f.path.clone()).collect();
        let (msg, btn) = if untracked.is_empty() {
            if tracked.len() == 1 {
                (format!("Are you sure you want to discard changes in '{}'?", short_name(&tracked[0])), "Discard File".to_string())
            } else {
                (format!("Are you sure you want to discard changes in {} files?", tracked.len()), format!("Discard {} Files", tracked.len()))
            }
        } else if tracked.is_empty() {
            if untracked.len() == 1 {
                (format!("Are you sure you want to DELETE the following untracked file: '{}'?", short_name(&untracked[0])), "Delete File".to_string())
            } else {
                (format!("Are you sure you want to DELETE {} untracked files?", untracked.len()), format!("Delete {} Files", untracked.len()))
            }
        } else {
            (format!("Are you sure you want to discard changes in {} files?\n\nThis will DELETE {} untracked files!", files.len(), untracked.len()), format!("Discard {} Files", files.len()))
        };
        let detail = if untracked.is_empty() { "This is IRREVERSIBLE!" } else { "Untracked files are moved to the Recycle Bin. Tracked changes are IRREVERSIBLE!" };
        self.confirm(&msg, detail, &[&btn], move |app, _| {
            app.op("Discard", move |root| {
                for chunk in tracked.chunks(200) {
                    let mut a = vec!["checkout", "-q", "--"];
                    a.extend(chunk.iter().map(|s| s.as_str()));
                    git::exec(root, &a)?;
                }
                for p in &untracked {
                    recycle(&root.join(p));
                }
                Ok(String::new())
            }, None, None);
        });
    }

    pub fn stage_all(&mut self) {
        let files: Vec<FileChange> = self.status().map(|s| s.files.iter().filter(|f| f.group == Group::Changes).cloned().collect()).unwrap_or_default();
        self.stage(files);
    }

    pub fn unstage_all(&mut self) {
        let has_head = self.status().map(|s| s.head.is_some()).unwrap_or(false);
        self.op("Unstage", move |root| if has_head { git::exec(root, &["reset", "-q", "HEAD", "--", "."]) } else { git::exec(root, &["rm", "--cached", "-r", "-q", "--", "."]) }, None, None);
    }

    pub fn discard_all(&mut self) {
        let files: Vec<FileChange> = self.status().map(|s| s.files.iter().filter(|f| f.group == Group::Changes).cloned().collect()).unwrap_or_default();
        self.discard(files);
    }

    pub fn add_to_gitignore(&mut self, files: Vec<FileChange>) {
        let paths: Vec<String> = files.iter().map(|f| f.path.clone()).collect();
        self.op("Ignore", move |root| {
            let p = root.join(".gitignore");
            let mut cur = std::fs::read_to_string(&p).unwrap_or_default();
            if !cur.is_empty() && !cur.ends_with('\n') {
                cur.push('\n');
            }
            for x in paths {
                cur.push_str(&x);
                cur.push('\n');
            }
            std::fs::write(&p, cur).map_err(|e| GitError::new(e.to_string()))?;
            Ok(String::new())
        }, None, None);
    }

    // ------------------------------------------------------------ partial staging

    pub fn diff_selection_lines(&self, block: Option<usize>) -> HashSet<LineId> {
        let mut set = HashSet::new();
        let MainView::Diff(d) = &self.view else { return set };
        if let Some(b) = block {
            if let Some(bl) = d.blocks.get(b) {
                set.extend(bl.lines.iter().copied());
            }
            return set;
        }
        let Some((a, b)) = d.sel else { return set };
        let (lo, hi) = (a.min(b), a.max(b));
        for r in d.rows.iter().take(hi + 1).skip(lo) {
            for id in [r.left, r.right].into_iter().flatten() {
                let l = &d.fd.hunks[id.0].lines[id.1];
                if matches!(l.kind, diff::LK::Add | diff::LK::Del) {
                    set.insert(id);
                }
            }
        }
        set
    }

    /// action: 0 = stage, 1 = unstage, 2 = revert
    pub fn apply_lines(&mut self, block: Option<usize>, action: u8) {
        let lines = self.diff_selection_lines(block);
        let MainView::Diff(d) = &self.view else { return };
        if lines.is_empty() {
            self.info("Select changed lines first.");
            return;
        }
        let kind = d.kind;
        let change = d.change.clone();
        if kind == DiffKind::Untracked && action == 0 && block.is_some() {
            if let Some(c) = change {
                self.stage(vec![c]);
            }
            return;
        }
        let reverse = action != 0;
        let Some(patch) = diff::build_patch(&d.fd, &lines, reverse) else { return };
        let args: Vec<&'static str> = match (action, kind) {
            (0, DiffKind::WorkingTree) | (0, DiffKind::Untracked) => vec!["apply", "--cached", "--whitespace=nowarn", "--recount", "-"],
            (1, DiffKind::Index) => vec!["apply", "--cached", "-R", "--whitespace=nowarn", "--recount", "-"],
            (2, DiffKind::WorkingTree) => vec!["apply", "-R", "--whitespace=nowarn", "--recount", "-"],
            _ => return,
        };
        let title = ["Stage", "Unstage", "Revert"][action as usize];
        let run = move |app: &mut App| {
            app.op(title, move |root| git::exec_input(root, &args, Some(patch.as_bytes())), None, None);
        };
        if action == 2 {
            self.confirm("Are you sure you want to revert the selected changes?", "This is IRREVERSIBLE!", &["Revert"], move |app, _| run(app));
        } else {
            run(self);
        }
    }

    pub fn resolve_conflict(&mut self, line: usize, which: u8) {
        let MainView::File(f) = &self.view else { return };
        let (fpath, change) = (f.path.clone(), f.change.clone());
        let Some(root) = self.root() else { return };
        let path = root.join(&fpath);
        let Ok(text) = std::fs::read_to_string(&path) else { return };
        let acc = match which {
            0 => diff::Accept::Current,
            1 => diff::Accept::Incoming,
            _ => diff::Accept::Both,
        };
        let out = diff::resolve_conflict(&text, line, acc);
        if let Err(e) = std::fs::write(&path, out) {
            self.error(format!("Unable to write file: {e}"));
        }
        if let Some(c) = change {
            self.load_change_view(c, true);
        }
    }

    // --------------------------------------------------------------- commit

    pub fn commit(&mut self, amend: bool, all: bool, post_cmd: u8) {
        let Some(s) = self.status().cloned() else {
            self.info("There are no available repositories.");
            return;
        };
        let msg = win::get_text(self.commit_edit).replace("\r\n", "\n").trim_end().to_string();
        if s.count(Group::Merge) > 0 {
            self.warn("There are merge conflicts. Resolve them before committing.");
            return;
        }
        if s.rebasing && !amend {
            self.op("Continue Rebase", |root| git::exec(root, &["-c", "core.editor=true", "rebase", "--continue"]), Some(Box::new(|app: &mut App| win::set_text(app.commit_edit, ""))), None);
            return;
        }
        let staged = s.count(Group::Staged);
        let changes = s.count(Group::Changes);
        let mut all = all;
        if staged == 0 && !amend && !s.merging && !all {
            if changes == 0 {
                self.info("There are no changes to commit.");
                return;
            }
            match self.st.smart_commit {
                Some(true) => all = true,
                Some(false) => {
                    self.info("There are no staged changes to commit.");
                    return;
                }
                None => {
                    self.confirm("There are no staged changes to commit.\n\nWould you like to stage all your changes and commit them directly?", "", &["Yes", "Always", "Never"], move |app, i| {
                        match i {
                            0 => app.commit(amend, true, post_cmd),
                            1 => {
                                app.st.smart_commit = Some(true);
                                app.st.save();
                                app.commit(amend, true, post_cmd)
                            }
                            _ => {
                                app.st.smart_commit = Some(false);
                                app.st.save();
                            }
                        }
                    });
                    return;
                }
            }
        }
        if msg.is_empty() && !amend {
            self.input("Commit message", "", move |app, m| {
                if m.trim().is_empty() {
                    app.info("Commit operation was cancelled due to empty commit message.");
                    return;
                }
                win::set_text(app.commit_edit, &m);
                app.commit(amend, all, post_cmd);
            });
            return;
        }
        let then: Then = Box::new(move |app: &mut App| {
            win::set_text(app.commit_edit, "");
            app.edit_changed();
            match post_cmd {
                1 => app.push(false),
                2 => app.sync(),
                _ => {}
            }
        });
        self.op("Committing", move |root| {
            if all {
                git::exec(root, &["add", "-A"])?;
            }
            if amend && msg.is_empty() {
                git::exec(root, &["commit", "--quiet", "--amend", "--no-edit"])
            } else {
                let mut a = vec!["commit", "--quiet", "--allow-empty-message", "-F", "-"];
                if amend {
                    a.push("--amend");
                }
                git::exec_input(root, &a, Some(msg.as_bytes()))
            }
        }, Some(then), None);
    }

    pub fn undo_commit(&mut self) {
        let Some(s) = self.status() else { return };
        if s.head.is_none() {
            self.warn("Can't undo because HEAD doesn't point to any commit.");
            return;
        }
        let root = self.root().unwrap();
        let msg = git::run(&root, &["log", "-1", "--format=%B"], None, true).text().trim().to_string();
        let has_parent = git::run(&root, &["rev-parse", "--verify", "-q", "HEAD~"], None, true).ok();
        self.op("Undo Commit", move |root| if has_parent { git::exec(root, &["reset", "--soft", "HEAD~"]) } else { git::exec(root, &["update-ref", "-d", "HEAD"]) }, Some(Box::new(move |app: &mut App| {
            win::set_text(app.commit_edit, &msg);
            app.edit_changed();
        })), None);
    }

    // ------------------------------------------------------------ branches

    pub fn ref_items(&self, local: bool, remote: bool, tags: bool, exclude_current: bool) -> Vec<PickItem> {
        let mut v = Vec::new();
        let Some(r) = &self.repo else { return v };
        let cur = r.status.branch.clone();
        let mut first = true;
        for rf in r.refs.iter().filter(|x| x.kind == RefKind::Head && local) {
            if exclude_current && Some(&rf.name) == cur.as_ref() {
                continue;
            }
            let mut it = pi(&rf.name, format!("{} \u{2022} {}", &rf.sha[..8], rf.subject), 'B', format!("H:{}", rf.name));
            if first {
                it.sep = "branches".into();
                first = false;
            }
            v.push(it);
        }
        first = true;
        for rf in r.refs.iter().filter(|x| x.kind == RefKind::Remote && remote) {
            let mut it = pi(&rf.name, format!("{} \u{2022} {}", &rf.sha[..8], rf.subject), '\u{E753}', format!("R:{}", rf.name));
            if first {
                it.sep = "remote branches".into();
                first = false;
            }
            v.push(it);
        }
        first = true;
        for rf in r.refs.iter().filter(|x| x.kind == RefKind::Tag && tags) {
            let mut it = pi(&rf.name, format!("{} \u{2022} {}", &rf.sha[..8], rf.subject), '\u{E8EC}', format!("T:{}", rf.name));
            if first {
                it.sep = "tags".into();
                first = false;
            }
            v.push(it);
        }
        v
    }

    pub fn checkout_picker(&mut self) {
        if self.repo.is_none() {
            return;
        }
        let mut items = vec![pi("Create new branch...", "", '\u{E710}', "new"), pi("Create new branch from...", "", '\u{E710}', "newfrom"), pi("Checkout detached...", "", '\u{E71B}', "detached")];
        items.extend(self.ref_items(true, true, true, true));
        self.pick("Select a branch or tag to checkout", items, |app, it| match it.data.as_str() {
            "new" => app.create_branch(None),
            "newfrom" => app.create_branch_from(),
            "detached" => {
                let items = app.ref_items(true, true, true, false);
                app.pick("Select a branch to checkout in detached mode", items, |app, it| {
                    let name = it.data[2..].to_string();
                    app.checkout(format!("D:{name}"));
                });
            }
            d => app.checkout(d.to_string()),
        });
    }

    /// `spec` is "H:name" (local), "R:remote/name", "T:tag", "D:anything detached".
    pub fn checkout(&mut self, spec: String) {
        let (k, name) = spec.split_at(2);
        let name = name.to_string();
        let args: Vec<String> = match k {
            "H:" => vec!["checkout".into(), "-q".into(), name.clone(), "--".into()],
            "R:" => {
                let short = name.split_once('/').map(|x| x.1).unwrap_or(&name).to_string();
                let local = self.repo.as_ref().and_then(|r| r.refs.iter().find(|x| x.kind == RefKind::Head && (x.upstream.as_deref() == Some(name.as_str()) || x.name == short)).map(|x| x.name.clone()));
                match local {
                    Some(l) => vec!["checkout".into(), "-q".into(), l, "--".into()],
                    None => vec!["checkout".into(), "-q".into(), "--track".into(), name.clone()],
                }
            }
            _ => vec!["checkout".into(), "-q".into(), "--detach".into(), name.clone(), "--".into()],
        };
        let args2 = args.clone();
        let on_err: OnErr = Box::new(move |app: &mut App, e: &GitError| {
            if !(e.stderr.contains("would be overwritten") || e.stderr.contains("commit your changes or stash them")) {
                return false;
            }
            let args3 = args2.clone();
            app.confirm("Your local changes would be overwritten by checkout.", "", &["Stash & Checkout", "Migrate Changes", "Force Checkout"], move |app, i| {
                let a = args3.clone();
                app.op("Checkout", move |root| {
                    let refs: Vec<&str> = a.iter().map(|s| s.as_str()).collect();
                    if i == 2 {
                        let mut f = refs.clone();
                        f.insert(2, "--force");
                        return git::exec(root, &f);
                    }
                    git::exec(root, &["stash", "push", "--include-untracked"])?;
                    git::exec(root, &refs)?;
                    if i == 1 {
                        git::exec(root, &["stash", "pop", "--index"]).or_else(|_| git::exec(root, &["stash", "pop"]))?;
                    }
                    Ok(String::new())
                }, None, None);
            });
            true
        });
        self.op("Checkout", move |root| {
            let a: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
            git::exec(root, &a)
        }, None, Some(on_err));
    }

    pub fn create_branch(&mut self, from: Option<String>) {
        self.input("Please provide a new branch name", "", move |app, name| {
            let name = sanitize_branch(&name);
            if name.is_empty() {
                app.warn("Please provide a valid branch name");
                return;
            }
            let from = from.clone();
            app.op("Branch", move |root| {
                let mut a = vec!["checkout", "-q", "-b", name.as_str()];
                if let Some(f) = &from {
                    a.push(f.as_str());
                }
                git::exec(root, &a)
            }, None, None);
        });
    }

    pub fn create_branch_from(&mut self) {
        let mut items = vec![pi("HEAD", "Current", 'B', "HEAD")];
        items.extend(self.ref_items(true, true, true, false));
        self.pick("Select a ref to create the branch from", items, |app, it| {
            let r = if it.data == "HEAD" { "HEAD".to_string() } else { it.data[2..].to_string() };
            app.create_branch(Some(r));
        });
    }

    pub fn rename_branch(&mut self) {
        let cur = self.status().and_then(|s| s.branch.clone()).unwrap_or_default();
        self.input("Please provide a new branch name", &cur, |app, name| {
            let name = sanitize_branch(&name);
            if !name.is_empty() {
                app.git_op("Rename Branch", vec!["branch".into(), "-m".into(), name]);
            }
        });
    }

    pub fn delete_branch(&mut self, name: Option<String>) {
        let run = |app: &mut App, name: String| {
            let n2 = name.clone();
            let on_err: OnErr = Box::new(move |app: &mut App, e: &GitError| {
                if !e.stderr.contains("not fully merged") {
                    return false;
                }
                let n3 = n2.clone();
                app.confirm(&format!("The branch '{n3}' is not fully merged. Delete anyway?"), "", &["Delete Branch"], move |app, _| {
                    app.git_op("Delete Branch", vec!["branch".into(), "-D".into(), n3]);
                });
                true
            });
            app.op("Delete Branch", move |root| git::exec(root, &["branch", "-d", &name]), None, Some(on_err));
        };
        match name {
            Some(n) => run(self, n),
            None => {
                let items = self.ref_items(true, false, false, true);
                self.pick("Select a branch to delete", items, move |app, it| run(app, it.data[2..].to_string()));
            }
        }
    }

    pub fn merge_picker(&mut self) {
        let items = self.ref_items(true, true, true, true);
        self.pick("Select a branch or tag to merge from", items, |app, it| {
            let n = it.data[2..].to_string();
            app.git_op("Merge", vec!["merge".into(), n]);
        });
    }

    pub fn rebase_picker(&mut self) {
        let items = self.ref_items(true, true, false, true);
        let cur = self.status().and_then(|s| s.branch.clone()).unwrap_or_default();
        self.pick(&format!("Select a branch to rebase '{cur}' onto"), items, |app, it| {
            let n = it.data[2..].to_string();
            app.git_op("Rebase", vec!["rebase".into(), n]);
        });
    }

    // -------------------------------------------------------------- remote

    pub fn fetch(&mut self, prune: bool, all: bool) {
        if self.status().map(|s| s.remotes.is_empty()).unwrap_or(true) {
            self.warn("This repository has no remotes configured to fetch from.");
            return;
        }
        let mut a = vec!["fetch".to_string()];
        if all {
            a.push("--all".into());
        }
        if prune {
            a.push("--prune".into());
        }
        self.git_op("Fetching", a);
    }

    pub fn pull(&mut self, rebase: bool) {
        let Some(s) = self.status() else { return };
        if s.remotes.is_empty() {
            self.warn("Your repository has no remotes configured to pull from.");
            return;
        }
        if s.upstream.is_none() {
            self.warn("The current branch has no upstream branch. Publish it first.");
            return;
        }
        let mut a = vec!["pull".to_string(), "--tags".to_string()];
        if rebase {
            a.push("--rebase".into());
        }
        self.git_op("Pulling", a);
    }

    pub fn push(&mut self, force: bool) {
        let Some(s) = self.status().cloned() else { return };
        if s.remotes.is_empty() {
            self.confirm("Your repository has no remotes configured to push to.", "", &["Add Remote"], |app, _| app.add_remote());
            return;
        }
        let Some(branch) = s.branch.clone() else {
            self.warn("Please check out a branch to push to a remote.");
            return;
        };
        if s.upstream.is_none() {
            self.confirm(&format!("The branch \"{branch}\" has no remote branch. Would you like to publish this branch?"), "", &["OK"], |app, _| app.publish());
            return;
        }
        if force {
            self.confirm("You are about to force push your changes, this can be destructive and could inadvertently overwrite changes made by others.\n\nAre you sure to continue?", "", &["OK"], |app, _| {
                app.git_op("Pushing", vec!["push".into(), "--force-with-lease".into()]);
            });
            return;
        }
        self.git_op("Pushing", vec!["push".into()]);
    }

    pub fn publish(&mut self) {
        let Some(s) = self.status().cloned() else { return };
        let Some(branch) = s.branch.clone() else {
            self.warn("Please check out a branch to publish it.");
            return;
        };
        if s.remotes.is_empty() {
            self.confirm("Your repository has no remotes configured to publish to.", "", &["Add Remote"], |app, _| app.add_remote());
            return;
        }
        let go = move |app: &mut App, remote: String| {
            app.git_op("Publishing", vec!["push".into(), "-u".into(), remote, branch.clone()]);
        };
        if s.remotes.len() == 1 {
            go(self, s.remotes[0].clone());
        } else {
            let items = s.remotes.iter().map(|r| pi(r.clone(), "", '\u{E753}', r.clone())).collect();
            self.pick("Pick a remote to publish the branch to:", items, move |app, it| go(app, it.data.clone()));
        }
    }

    pub fn sync(&mut self) {
        let Some(s) = self.status().cloned() else { return };
        if s.upstream.is_none() {
            self.publish();
            return;
        }
        let run = |app: &mut App| {
            app.op("Syncing", |root| {
                git::exec(root, &["pull", "--tags"])?;
                git::exec(root, &["push"])
            }, None, None);
        };
        if self.st.confirm_sync {
            let up = s.upstream.clone().unwrap();
            self.confirm(&format!("This action will pull and push commits from and to \"{up}\"."), "", &["OK", "OK, Don't Show Again"], move |app, i| {
                if i == 1 {
                    app.st.confirm_sync = false;
                    app.st.save();
                }
                run(app);
            });
        } else {
            run(self);
        }
    }

    pub fn add_remote(&mut self) {
        self.input("Provide repository URL", "", |app, url| {
            if url.trim().is_empty() {
                return;
            }
            let has_origin = app.status().map(|s| s.remotes.iter().any(|r| r == "origin")).unwrap_or(false);
            app.input("Please provide a remote name", if has_origin { "" } else { "origin" }, move |app, name| {
                let name = name.trim().to_string();
                if name.is_empty() || name.contains(' ') {
                    app.warn("Remote name format invalid");
                    return;
                }
                app.git_op("Add Remote", vec!["remote".into(), "add".into(), name, url.trim().to_string()]);
            });
        });
    }

    pub fn remove_remote(&mut self) {
        let remotes = self.status().map(|s| s.remotes.clone()).unwrap_or_default();
        let items = remotes.iter().map(|r| pi(r.clone(), "", '\u{E753}', r.clone())).collect();
        self.pick("Select a remote to remove", items, |app, it| app.git_op("Remove Remote", vec!["remote".into(), "remove".into(), it.data.clone()]));
    }

    // --------------------------------------------------------------- stash

    pub fn stash(&mut self, untracked: bool, staged: bool) {
        let Some(s) = self.status() else { return };
        if s.files.is_empty() {
            self.info("There are no changes to stash.");
            return;
        }
        if staged && s.count(Group::Staged) == 0 {
            self.info("There are no staged changes to stash.");
            return;
        }
        self.input("Optionally provide a stash message", "", move |app, m| {
            let mut a = vec!["stash".to_string(), "push".to_string()];
            if untracked {
                a.push("--include-untracked".into());
            }
            if staged {
                a.push("--staged".into());
            }
            if !m.trim().is_empty() {
                a.push("-m".into());
                a.push(m.trim().to_string());
            }
            app.git_op("Stash", a);
        });
    }

    pub fn stash_pick(&mut self, placeholder: &str, f: impl FnOnce(&mut App, usize) + 'static) {
        let stashes = self.repo.as_ref().map(|r| r.stashes.clone()).unwrap_or_default();
        if stashes.is_empty() {
            self.info("There are no stashes in the repository.");
            return;
        }
        let items = stashes.iter().map(|s| pi(format!("#{}:  {}", s.index, s.message), "", '\u{E7B8}', s.index.to_string())).collect();
        self.pick(placeholder, items, move |app, it| f(app, it.data.parse().unwrap_or(0)));
    }

    pub fn stash_apply(&mut self, index: usize, pop: bool) {
        let r = format!("stash@{{{index}}}");
        let verb = if pop { "pop" } else { "apply" };
        self.op(if pop { "Stash Pop" } else { "Stash Apply" }, move |root| git::exec(root, &["stash", verb, "--index", &r]).or_else(|_| git::exec(root, &["stash", verb, &r])), None, None);
    }

    pub fn stash_view(&mut self, index: usize) {
        let Some(root) = self.root() else { return };
        let r = format!("stash@{{{index}}}");
        let out = git::run(&root, &["log", "-1", "--format=%H%x1f%P%x1f%an%x1f%ae%x1f%at%x1f%D%x1f%B%x1e", &r], None, true);
        if let Some(c) = git::parse_log(&out.text()).into_iter().next() {
            self.open_commit(c, Some(index));
        }
    }

    // ---------------------------------------------------------------- tags

    pub fn create_tag(&mut self, at: Option<String>) {
        self.input("Please provide a tag name", "", move |app, name| {
            let name = name.trim().replace(' ', "-");
            if name.is_empty() {
                return;
            }
            let at = at.clone();
            app.input("Please provide a message to annotate the tag", "", move |app, msg| {
                let mut a = vec!["tag".to_string()];
                if !msg.trim().is_empty() {
                    a.extend(["-a".into(), name.clone(), "-m".into(), msg.trim().to_string()]);
                } else {
                    a.push(name.clone());
                }
                if let Some(r) = at {
                    a.push(r);
                }
                app.git_op("Tag", a);
            });
        });
    }

    pub fn delete_tag(&mut self) {
        let items = self.ref_items(false, false, true, false);
        if items.is_empty() {
            self.warn("This repository has no tags.");
            return;
        }
        self.pick("Select a tag to delete", items, |app, it| app.git_op("Delete Tag", vec!["tag".into(), "-d".into(), it.data[2..].to_string()]));
    }

    // ---------------------------------------------------------------- misc

    pub fn configure_user(&mut self) {
        let root = self.root().unwrap_or_else(|| PathBuf::from("."));
        let cur_name = git::run(&root, &["config", "--get", "user.name"], None, true).text().trim().to_string();
        let cur_mail = git::run(&root, &["config", "--get", "user.email"], None, true).text().trim().to_string();
        self.input("Please provide your name (user.name)", &cur_name, move |app, name| {
            app.input("Please provide your email (user.email)", &cur_mail, move |app, email| {
                if name.trim().is_empty() || email.trim().is_empty() {
                    return;
                }
                app.op("Config", move |root| {
                    git::exec(root, &["config", "--global", "user.name", name.trim()])?;
                    git::exec(root, &["config", "--global", "user.email", email.trim()])
                }, None, None);
            });
        });
    }

    pub fn open_folder(&mut self) {
        if let Some(p) = win::pick_folder(self.hwnd, "Open Folder") {
            self.open_repo(Path::new(&p));
        }
    }

    pub fn open_recent(&mut self) {
        let items: Vec<PickItem> = self.st.recent.iter().map(|r| pi(Path::new(r).file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default(), r.clone(), '\u{E8B7}', r.clone())).collect();
        if items.is_empty() {
            self.info("No recently opened folders.");
            return;
        }
        self.pick("Select a folder to open (Ctrl+Enter opens a new window)", items, |app, it| {
            let p = PathBuf::from(&it.data);
            app.open_repo(&p);
        });
    }

    pub fn new_window(&self, folder: Option<&str>) {
        if let Ok(exe) = std::env::current_exe() {
            let mut c = std::process::Command::new(exe);
            if let Some(f) = folder {
                c.arg(f);
            }
            let _ = c.spawn();
        }
    }

    pub fn clone_repo(&mut self) {
        self.input("Provide repository URL", "", |app, url| {
            let url = url.trim().to_string();
            if url.is_empty() {
                return;
            }
            let Some(parent) = win::pick_folder(app.hwnd, "Choose a folder to clone the repository into") else { return };
            let name = url.trim_end_matches('/').trim_end_matches(".git").rsplit(['/', '\\', ':']).next().unwrap_or("repo").to_string();
            let target = PathBuf::from(&parent).join(&name);
            let tgt = target.clone();
            let tx = app.tx.clone();
            let hwnd = app.hwnd as isize;
            app.ops.push("Cloning".into());
            unsafe {
                SetTimer(app.hwnd, TIMER_SPIN, 120, None);
            }
            let then: Then = Box::new(move |app: &mut App| app.open_repo(&tgt));
            std::thread::spawn(move || {
                let r = git::exec(Path::new(&parent), &["clone", "--progress", &url, &target.display().to_string()]);
                let _ = tx.send(JobResult::Op { title: "Cloning".into(), result: r, then: Some(then), on_err: None });
                post(hwnd);
            });
        });
    }

    pub fn init_repo(&mut self) {
        let folder = self.pending_init.clone().or_else(|| win::pick_folder(self.hwnd, "Choose a folder to initialize a Git repository in").map(PathBuf::from));
        let Some(f) = folder else { return };
        let r = git::exec(&f, &["init", "-b", "main"]);
        match r {
            Ok(_) => self.open_repo(&f),
            Err(e) => self.error(e.message),
        }
    }

    pub fn toggle_theme(&mut self) {
        self.st.dark = !self.st.dark;
        self.st.save();
        self.t = if self.st.dark { Theme::dark() } else { Theme::light() };
        self.apply_theme();
    }

    pub fn apply_theme(&mut self) {
        win::set_title_colors(self.hwnd, self.t.dark, self.t.title, self.t.fg, self.t.border);
        win::set_app_dark_mode(self.t.dark);
        win::set_control_theme(self.commit_edit, self.t.dark);
        win::set_control_theme(self.overlay_edit, self.t.dark);
        unsafe {
            if !self.edit_brush.is_null() {
                windows_sys::Win32::Graphics::Gdi::DeleteObject(self.edit_brush as _);
            }
            self.edit_brush = windows_sys::Win32::Graphics::Gdi::CreateSolidBrush(self.t.input_bg);
            windows_sys::Win32::Graphics::Gdi::InvalidateRect(self.commit_edit, std::ptr::null(), 1);
        }
        self.invalidate();
    }

    pub fn palette(&mut self) {
        let items = COMMANDS.iter().filter(|(c, _, _)| *c != Cmd::Palette).map(|(c, t, k)| PickItem { label: t.to_string(), desc: String::new(), icon: '\0', data: format!("{}", *c as u32), sep: k.to_string() }).collect();
        self.pick("Type the name of a command to run.", items, |app, it| {
            let id: u32 = it.data.parse().unwrap_or(0);
            if let Some((c, _, _)) = COMMANDS.iter().find(|(c, _, _)| *c as u32 == id) {
                let c = *c;
                app.exec(c);
            }
        });
    }

    pub fn exec(&mut self, c: Cmd) {
        let needs_repo = !matches!(c, Cmd::Palette | Cmd::OpenFolder | Cmd::OpenRecent | Cmd::NewWindow | Cmd::Clone | Cmd::Init | Cmd::ToggleOutput | Cmd::ToggleTheme | Cmd::About | Cmd::ClearOutput | Cmd::ToggleInline | Cmd::ToggleCompact | Cmd::NextChange | Cmd::PrevChange | Cmd::ConfigureUser | Cmd::ToggleTerminal | Cmd::NewTerminal | Cmd::KillTerminal | Cmd::SelectShell | Cmd::ClearTerminal);
        if needs_repo && self.repo.is_none() {
            self.info("There are no available repositories. Open a folder first.");
            return;
        }
        match c {
            Cmd::Palette => self.palette(),
            Cmd::Refresh => self.refresh(),
            Cmd::Commit => self.commit(false, false, 0),
            Cmd::CommitAmend => self.commit(true, false, 0),
            Cmd::CommitAll => self.commit(false, true, 0),
            Cmd::CommitPush => self.commit(false, false, 1),
            Cmd::CommitSync => self.commit(false, false, 2),
            Cmd::UndoCommit => self.undo_commit(),
            Cmd::StageAll => self.stage_all(),
            Cmd::UnstageAll => self.unstage_all(),
            Cmd::DiscardAll => self.discard_all(),
            Cmd::Checkout => self.checkout_picker(),
            Cmd::Branch => self.create_branch(None),
            Cmd::BranchFrom => self.create_branch_from(),
            Cmd::RenameBranch => self.rename_branch(),
            Cmd::DeleteBranch => self.delete_branch(None),
            Cmd::Merge => self.merge_picker(),
            Cmd::Rebase => self.rebase_picker(),
            Cmd::MergeAbort => self.git_op("Abort Merge", vec!["merge".into(), "--abort".into()]),
            Cmd::RebaseAbort => self.git_op("Abort Rebase", vec!["rebase".into(), "--abort".into()]),
            Cmd::RebaseContinue => self.git_op("Continue Rebase", vec!["-c".into(), "core.editor=true".into(), "rebase".into(), "--continue".into()]),
            Cmd::Fetch => self.fetch(false, false),
            Cmd::FetchPrune => self.fetch(true, false),
            Cmd::FetchAll => self.fetch(false, true),
            Cmd::Pull => self.pull(false),
            Cmd::PullRebase => self.pull(true),
            Cmd::Push => self.push(false),
            Cmd::PushForce => self.push(true),
            Cmd::PushTags => self.git_op("Pushing", vec!["push".into(), "--tags".into()]),
            Cmd::Sync => self.sync(),
            Cmd::Publish => self.publish(),
            Cmd::AddRemote => self.add_remote(),
            Cmd::RemoveRemote => self.remove_remote(),
            Cmd::Stash => self.stash(false, false),
            Cmd::StashUntracked => self.stash(true, false),
            Cmd::StashStaged => self.stash(false, true),
            Cmd::StashPop => {
                if self.repo.as_ref().map(|r| r.stashes.is_empty()).unwrap_or(true) {
                    self.info("There are no stashes in the repository.");
                } else {
                    self.stash_apply(0, true);
                }
            }
            Cmd::StashPopPick => self.stash_pick("Pick a stash to pop", |app, i| app.stash_apply(i, true)),
            Cmd::StashApply => self.stash_pick("Pick a stash to apply", |app, i| app.stash_apply(i, false)),
            Cmd::StashDrop => self.stash_pick("Pick a stash to drop", |app, i| {
                app.confirm(&format!("Are you sure you want to drop the stash #{i}?"), "", &["Yes"], move |app, _| app.git_op("Drop Stash", vec!["stash".into(), "drop".into(), format!("stash@{{{i}}}")]));
            }),
            Cmd::StashView => self.stash_pick("Pick a stash to view", |app, i| app.stash_view(i)),
            Cmd::TagCreate => self.create_tag(None),
            Cmd::TagDelete => self.delete_tag(),
            Cmd::CherryPick => self.input("Please provide the commit hash", "", |app, h| {
                if !h.trim().is_empty() {
                    app.git_op("Cherry Pick", vec!["cherry-pick".into(), h.trim().to_string()]);
                }
            }),
            Cmd::ConfigureUser => self.configure_user(),
            Cmd::OpenFolder => self.open_folder(),
            Cmd::OpenRecent => self.open_recent(),
            Cmd::NewWindow => self.new_window(None),
            Cmd::Clone => self.clone_repo(),
            Cmd::Init => self.init_repo(),
            Cmd::OpenExplorer => {
                if let Some(r) = self.root() {
                    win::shell_open(&r.display().to_string());
                }
            }
            Cmd::OpenTerminal => {
                if let Some(r) = self.root() {
                    let d = r.display().to_string();
                    win::shell_exec("cmd.exe", "", &d);
                }
            }
            Cmd::ToggleOutput => {
                if self.st.output_visible && !self.st.panel_terminal {
                    self.st.output_visible = false;
                } else {
                    self.st.output_visible = true;
                    self.st.panel_terminal = false;
                }
                if self.focus == Focus::Terminal {
                    self.focus = Focus::Changes;
                }
                self.st.save();
                self.layout();
            }
            Cmd::ToggleTerminal => self.toggle_terminal(),
            Cmd::NewTerminal => {
                self.show_terminal(true);
                self.start_terminal(None);
            }
            Cmd::KillTerminal => self.kill_terminal(),
            Cmd::SelectShell => self.select_shell(),
            Cmd::ClearTerminal => {
                if let Some(t) = &mut self.term {
                    t.screen.scrollback.clear();
                }
                self.term_write(b"\x0c");
            }
            Cmd::ToggleTheme => self.toggle_theme(),
            Cmd::ToggleInline => {
                self.st.inline_diff = !self.st.inline_diff;
                self.st.save();
                self.rebuild_rows();
            }
            Cmd::ToggleCompact => {
                self.st.compact_diff = !self.st.compact_diff;
                self.st.save();
                self.reload_view();
            }
            Cmd::ToggleGraphAll => {
                self.st.graph_all = !self.st.graph_all;
                self.st.save();
                self.log_key.clear();
                self.load_log(false);
            }
            Cmd::NextChange => self.goto_change(1),
            Cmd::PrevChange => self.goto_change(-1),
            Cmd::FocusCommit => unsafe {
                windows_sys::Win32::UI::Input::KeyboardAndMouse::SetFocus(self.commit_edit);
            },
            Cmd::ClearOutput => {
                git::clear_log();
            }
            Cmd::About => {
                let v = self.git_version.clone().unwrap_or_else(|| "not found".into());
                self.confirm(&format!("GitGui {}", env!("CARGO_PKG_VERSION")), &format!("A native, ultra-light Git client modeled on the VS Code Source Control view.\n\nGit: {v}\nRenderer: Win32 / GDI"), &[], |_, _| {});
            }
        }
        self.invalidate();
    }

    pub fn reload_view(&mut self) {
        let change = match &self.view {
            MainView::Diff(d) if d.back.is_none() => d.change.clone(),
            MainView::File(f) => f.change.clone(),
            _ => None,
        };
        if let Some(c) = change {
            self.load_change_view(c, true);
        } else if let MainView::Diff(d) = &self.view {
            if let Some(b) = &d.back {
                if let MainView::Commit(cv) = b.as_ref() {
                    if let Some(i) = cv.sel {
                        self.go_back();
                        self.open_commit_file(i);
                    }
                }
            }
        }
    }

    pub fn goto_change(&mut self, dir: i32) {
        let lh = self.line_h();
        let body_h = crate::win::rh(&self.l.main_body) - self.s(26);
        let MainView::Diff(d) = &self.view else { return };
        if d.blocks.is_empty() {
            return;
        }
        let cur = (self.scroll[Area::Main as usize] / lh.max(1)) as usize;
        let starts: Vec<usize> = d.blocks.iter().map(|b| b.first_row).collect();
        let target = if dir > 0 { starts.iter().copied().find(|&s| s > cur + 2).unwrap_or(starts[0]) } else { starts.iter().rev().copied().find(|&s| s + 2 < cur).unwrap_or(*starts.last().unwrap()) };
        let y = (target as i32 * lh - body_h / 3).max(0);
        self.scroll[Area::Main as usize] = y;
        self.clamp_scroll(Area::Main);
        self.invalidate();
    }

    pub fn line_h(&self) -> i32 {
        self.fonts.mono_h + self.s(3)
    }
}

pub fn short_name(p: &str) -> &str {
    p.rsplit('/').next().unwrap_or(p)
}

pub fn sanitize_branch(s: &str) -> String {
    let mut out = String::new();
    for ch in s.trim().chars() {
        out.push(match ch {
            ' ' | '~' | '^' | ':' | '?' | '*' | '[' | '\\' => '-',
            c => c,
        });
    }
    while out.contains("..") {
        out = out.replace("..", ".");
    }
    out.trim_matches(|c| c == '/' || c == '.' || c == '-').trim_end_matches(".lock").to_string()
}

fn recycle(path: &Path) {
    use windows_sys::Win32::UI::Shell::{SHFileOperationW, FOF_ALLOWUNDO, FOF_NOCONFIRMATION, FOF_NOERRORUI, FOF_SILENT, FO_DELETE, SHFILEOPSTRUCTW};
    let mut from: Vec<u16> = path.display().to_string().encode_utf16().collect();
    from.push(0);
    from.push(0);
    let mut op = SHFILEOPSTRUCTW {
        hwnd: std::ptr::null_mut(),
        wFunc: FO_DELETE,
        pFrom: from.as_ptr(),
        pTo: std::ptr::null(),
        fFlags: (FOF_ALLOWUNDO | FOF_NOCONFIRMATION | FOF_NOERRORUI | FOF_SILENT) as u16,
        fAnyOperationsAborted: 0,
        hNameMappings: std::ptr::null_mut(),
        lpszProgressTitle: std::ptr::null(),
    };
    let r = unsafe { SHFileOperationW(&mut op) };
    if r != 0 {
        let _ = std::fs::remove_file(path);
    }
}

pub fn diff_view_from(text: &str, f: &FileChange, kind: DiffKind, inline: bool, title: &str) -> MainView {
    let fd = diff::parse(text);
    if fd.binary {
        return MainView::Message(title.into(), "The file is not displayed because it is binary.".into());
    }
    if fd.hunks.is_empty() {
        return MainView::Message(title.into(), "No textual changes (mode, rename or whitespace only).".into());
    }
    let (rows, blocks) = diff::build_rows(&fd, inline);
    let max_cols = fd.hunks.iter().flat_map(|h| h.lines.iter()).map(|l| l.text.chars().count() + l.text.matches('\t').count() * 3).max().unwrap_or(0);
    MainView::Diff(DiffView { title: title.into(), desc: f.path.clone(), path: f.path.clone(), kind, change: Some(f.clone()), fd, rows, blocks, sel: None, back: None, max_cols })
}

pub fn build_change_view(root: &Path, f: &FileChange, ctx: &str, inline: bool) -> MainView {
    let name = short_name(&f.path).to_string();
    match (f.group, f.kind) {
        (Group::Merge, _) => {
            let bytes = std::fs::read(root.join(&f.path)).unwrap_or_default();
            if git::is_binary(&bytes) {
                return MainView::Message(name, "Binary file with conflicts. Stage it to mark as resolved.".into());
            }
            let text = String::from_utf8_lossy(&bytes).into_owned();
            let lines: Vec<String> = text.split('\n').map(|l| l.trim_end_matches('\r').to_string()).collect();
            let refs: Vec<&str> = lines.iter().map(|s| s.as_str()).collect();
            let conflicts = diff::parse_conflicts(&refs);
            MainView::File(FileView { title: format!("{name} (Merge Conflicts)"), path: f.path.clone(), lines, conflicts, change: Some(f.clone()) })
        }
        (Group::Changes, Kind::Untracked) => {
            let o = git::run(root, &["diff", "--no-index", "--no-color", "--no-ext-diff", ctx, "--", "/dev/null", &f.path], None, false);
            diff_view_from(&o.text(), f, DiffKind::Untracked, inline, &format!("{name} (Untracked)"))
        }
        (Group::Changes, _) => {
            let o = git::run(root, &["diff", "--no-color", "--no-ext-diff", ctx, "--", &f.path], None, false);
            let suffix = if f.kind == Kind::Deleted { "Deleted" } else { "Working Tree" };
            diff_view_from(&o.text(), f, DiffKind::WorkingTree, inline, &format!("{name} ({suffix})"))
        }
        (Group::Staged, _) => {
            let mut a = vec!["diff", "--cached", "--no-color", "--no-ext-diff", "-M", ctx, "--"];
            if let Some(o) = &f.orig {
                a.push(o);
            }
            a.push(&f.path);
            let o = git::run(root, &a, None, false);
            diff_view_from(&o.text(), f, DiffKind::Index, inline, &format!("{name} (Index)"))
        }
    }
}
