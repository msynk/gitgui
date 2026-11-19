//! Explorer view (the repository's working tree as a folder tree) and the read-only text
//! file viewer it opens files in.

use std::collections::{HashMap, HashSet};
use std::path::Path;

use crate::app::*;
use crate::git::{self, Group, Kind};
use crate::ui::Menu;
use crate::win::{self, MenuItem};

/// Files larger than this are not loaded into the viewer.
const MAX_VIEW_BYTES: u64 = 16 * 1024 * 1024;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ERow {
    /// Path relative to the repository root, '/' separated.
    pub path: String,
    pub name: String,
    pub depth: u8,
    pub dir: bool,
}

#[derive(Default)]
pub struct Explorer {
    pub rows: Vec<ERow>,
    /// Expanded folders.
    pub open: HashSet<String>,
    /// Selected row, by path.
    pub sel: Option<String>,
    /// Ignored paths from `git ls-files --ignored --directory` (folders without the trailing '/').
    pub ignored: HashSet<String>,
    /// Git status of changed files: (kind, letter).
    pub deco: HashMap<String, (Kind, char)>,
    /// Folders that contain changes, with the kind used for their color.
    pub dir_deco: HashMap<String, Kind>,
}

impl Explorer {
    pub fn is_ignored(&self, path: &str) -> bool {
        if self.ignored.is_empty() {
            return false;
        }
        if self.ignored.contains(path) {
            return true;
        }
        let mut p = path;
        while let Some((parent, _)) = p.rsplit_once('/') {
            if self.ignored.contains(parent) {
                return true;
            }
            p = parent;
        }
        false
    }

    pub fn sel_index(&self) -> Option<usize> {
        let s = self.sel.as_ref()?;
        self.rows.iter().position(|r| &r.path == s)
    }
}

fn join(rel: &str, name: &str) -> String {
    if rel.is_empty() { name.to_string() } else { format!("{rel}/{name}") }
}

/// Entries of a folder: folders first, then files, each sorted by name (case-insensitive).
/// `.git` is left out.
pub fn list_dir(root: &Path, rel: &str) -> Vec<(String, bool)> {
    let dir = if rel.is_empty() { root.to_path_buf() } else { root.join(rel) };
    let Ok(rd) = std::fs::read_dir(&dir) else { return Vec::new() };
    let mut v: Vec<(String, bool)> = rd
        .filter_map(|e| e.ok())
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            if name == ".git" {
                return None;
            }
            let is_dir = e.file_type().map(|t| t.is_dir() || (t.is_symlink() && e.path().is_dir())).unwrap_or(false);
            Some((name, is_dir))
        })
        .collect();
    v.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.to_lowercase().cmp(&b.0.to_lowercase())).then_with(|| a.0.cmp(&b.0)));
    v
}

/// Visible rows: the root's entries plus the contents of every expanded folder.
pub fn build_rows(root: &Path, open: &HashSet<String>) -> Vec<ERow> {
    fn walk(root: &Path, open: &HashSet<String>, rel: &str, depth: u8, out: &mut Vec<ERow>) {
        for (name, dir) in list_dir(root, rel) {
            let path = join(rel, &name);
            let expand = dir && open.contains(&path) && depth < 64;
            out.push(ERow { path: path.clone(), name, depth, dir });
            if expand {
                walk(root, open, &path, depth + 1, out);
            }
        }
    }
    let mut out = Vec::new();
    walk(root, open, "", 0, &mut out);
    out
}

/// Ignored files and folders (folders are reported once, not their contents).
pub fn ignored(root: &Path) -> HashSet<String> {
    let o = git::run(root, &["ls-files", "--others", "--ignored", "--exclude-standard", "--directory", "-z"], None, true);
    o.text().split('\0').filter(|s| !s.is_empty()).map(|s| s.trim_end_matches('/').to_string()).collect()
}

/// Decodes file bytes as text: UTF-8 (with or without BOM) or UTF-16 with a BOM.
/// Returns None for binary content.
pub fn decode_text(bytes: &[u8]) -> Option<String> {
    let utf16 = |be: bool| {
        let units: Vec<u16> = bytes[2..].chunks_exact(2).map(|c| if be { u16::from_be_bytes([c[0], c[1]]) } else { u16::from_le_bytes([c[0], c[1]]) }).collect();
        String::from_utf16_lossy(&units)
    };
    if bytes.starts_with(&[0xFF, 0xFE]) {
        return Some(utf16(false));
    }
    if bytes.starts_with(&[0xFE, 0xFF]) {
        return Some(utf16(true));
    }
    let body = bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]).unwrap_or(bytes);
    if git::is_binary(body) {
        return None;
    }
    Some(String::from_utf8_lossy(body).into_owned())
}

pub fn mtime(root: &Path, rel: &str) -> Option<std::time::SystemTime> {
    std::fs::metadata(root.join(rel)).and_then(|m| m.modified()).ok()
}

/// Builds the read-only text view of a file (or a message when it can't be shown).
pub fn load_text(root: &Path, rel: &str) -> MainView {
    let name = short_name(rel).to_string();
    let p = root.join(rel);
    let meta = match std::fs::metadata(&p) {
        Ok(m) => m,
        Err(e) => return MainView::Message(name, format!("The file could not be read: {e}")),
    };
    if meta.len() > MAX_VIEW_BYTES {
        return MainView::Message(name, format!("The file is not displayed because it is too large ({:.1} MB).", meta.len() as f64 / 1048576.0));
    }
    let bytes = match std::fs::read(&p) {
        Ok(b) => b,
        Err(e) => return MainView::Message(name, format!("The file could not be read: {e}")),
    };
    let Some(text) = decode_text(&bytes) else {
        return MainView::Message(name, "The file is not displayed because it is binary.".into());
    };
    let lines: Vec<String> = text.split('\n').map(|l| l.trim_end_matches('\r').to_string()).collect();
    MainView::File(FileView { title: name, path: rel.to_string(), lines, conflicts: Vec::new(), change: None, mtime: meta.modified().ok() })
}

impl App {
    pub fn explorer_active(&self) -> bool {
        self.st.sidebar_visible && self.st.side_view == 1
    }

    /// Activity bar click: switches the side bar to `view` (0 Source Control, 1 Explorer);
    /// clicking the view that is already shown hides the side bar, like VS Code.
    pub fn set_side_view(&mut self, view: u8) {
        if self.st.sidebar_visible && self.st.side_view == view {
            self.set_sidebar_visible(false);
            return;
        }
        self.st.side_view = view;
        self.focus = if view == 1 { Focus::Explorer } else { Focus::Changes };
        self.set_sidebar_visible(true);
        if view == 1 {
            self.rebuild_explorer();
            // picks up the ignored files for the dimmed entries
            self.refresh();
        }
    }

    /// Re-reads the expanded folders and the git decorations.
    pub fn rebuild_explorer(&mut self) {
        let Some(repo) = &self.repo else {
            self.ex.rows.clear();
            return;
        };
        let root = repo.root.clone();
        let mut deco = HashMap::new();
        let mut dir_deco: HashMap<String, Kind> = HashMap::new();
        // conflicts win over working tree changes, which win over staged ones
        for g in [Group::Staged, Group::Changes, Group::Merge] {
            for f in repo.status.files.iter().filter(|f| f.group == g) {
                deco.insert(f.path.clone(), (f.kind, f.letter()));
                let mut p = f.path.as_str();
                while let Some((parent, _)) = p.rsplit_once('/') {
                    let e = dir_deco.entry(parent.to_string()).or_insert(f.kind);
                    if f.kind == Kind::Conflict {
                        *e = Kind::Conflict;
                    }
                    p = parent;
                }
            }
        }
        self.ex.deco = deco;
        self.ex.dir_deco = dir_deco;
        self.ex.rows = build_rows(&root, &self.ex.open);
        self.clamp_scroll(Area::Explorer);
        self.invalidate();
    }

    pub fn explorer_select(&mut self, i: usize) {
        if let Some(r) = self.ex.rows.get(i) {
            self.ex.sel = Some(r.path.clone());
            self.focus = Focus::Explorer;
        }
    }

    /// Collapses / expands the folder at row `i` (`expand`: None toggles).
    pub fn toggle_explorer_dir(&mut self, i: usize, expand: Option<bool>) {
        let Some(r) = self.ex.rows.get(i).cloned() else { return };
        if !r.dir {
            return;
        }
        self.explorer_select(i);
        let open = self.ex.open.contains(&r.path);
        let want = expand.unwrap_or(!open);
        if want == open {
            return;
        }
        if want {
            self.ex.open.insert(r.path);
        } else {
            let prefix = format!("{}/", r.path);
            self.ex.open.retain(|p| p != &r.path && !p.starts_with(&prefix));
        }
        self.rebuild_explorer();
    }

    pub fn explorer_collapse_all(&mut self) {
        self.ex.open.clear();
        self.rebuild_explorer();
    }

    /// Click / Enter: toggles a folder, opens a file (a preview tab unless `pin`).
    pub fn explorer_activate(&mut self, i: usize, pin: bool) {
        let Some(r) = self.ex.rows.get(i).cloned() else { return };
        if r.dir {
            self.toggle_explorer_dir(i, None);
        } else {
            self.explorer_select(i);
            self.open_file_viewer(r.path, pin);
        }
    }

    /// Opens the read-only text view of `rel` in a tab.
    pub fn open_file_viewer(&mut self, rel: String, pin: bool) {
        let Some(root) = self.root() else { return };
        if self.open_tab(format!("file:{rel}"), pin) && matches!(self.view, MainView::File(_) | MainView::Message(..)) {
            return;
        }
        self.view_gen += 1;
        let gen = self.view_gen;
        self.mark_tab_gen(gen);
        self.view = MainView::Loading(rel.clone());
        let tx = self.tx.clone();
        let hwnd = self.hwnd as isize;
        std::thread::spawn(move || {
            let view = load_text(&root, &rel);
            let _ = tx.send(JobResult::View { gen, view, keep_scroll: false });
            post(hwnd);
        });
        self.invalidate();
    }

    /// Reloads the active text view when its file changed on disk (keeps the scroll position).
    pub fn reload_text_view_if_changed(&mut self) {
        let Some(root) = self.root() else { return };
        let MainView::File(f) = &self.view else { return };
        if f.change.is_some() {
            return;
        }
        let rel = f.path.clone();
        let now = mtime(&root, &rel);
        if now.is_some() && now == f.mtime {
            return;
        }
        self.view_gen += 1;
        let gen = self.view_gen;
        self.mark_tab_gen(gen);
        let tx = self.tx.clone();
        let hwnd = self.hwnd as isize;
        std::thread::spawn(move || {
            let view = load_text(&root, &rel);
            let _ = tx.send(JobResult::View { gen, view, keep_scroll: true });
            post(hwnd);
        });
    }

    /// "Go to File..." (Ctrl+P): picks any tracked or untracked (not ignored) file to view.
    pub fn goto_file(&mut self) {
        let Some(root) = self.root() else { return };
        let o = git::run(&root, &["ls-files", "--cached", "--others", "--exclude-standard", "-z"], None, true);
        let text = o.text();
        let mut paths: Vec<&str> = text.split('\0').filter(|s| !s.is_empty()).collect();
        paths.sort_by_key(|p| p.to_lowercase());
        paths.dedup();
        let items: Vec<PickItem> = paths
            .into_iter()
            .map(|p| {
                let (dir, name) = p.rsplit_once('/').unwrap_or(("", p));
                pi(name, dir, '\u{E8A5}', p)
            })
            .collect();
        if items.is_empty() {
            self.info("There are no files in this repository.");
            return;
        }
        self.pick("Search files by name", items, |app, it| {
            let rel = it.data.clone();
            app.open_file_viewer(rel, true);
        });
    }

    /// Context menu of Explorer row `i` (usize::MAX: empty space).
    pub fn explorer_menu(&mut self, i: usize, m: &mut Menu) -> Vec<MenuItem> {
        let mut v = Vec::new();
        let root = self.root();
        if let (Some(r), Some(root)) = (self.ex.rows.get(i).cloned(), root.clone()) {
            let abs = root.join(&r.path).display().to_string();
            let (a1, a2, a3) = (abs.clone(), abs.clone(), abs);
            let rel = r.path.clone();
            if r.dir {
                let open = self.ex.open.contains(&r.path);
                v.push(m.it(if open { "Collapse" } else { "Expand" }, move |a| a.toggle_explorer_dir(i, Some(!open))));
            } else {
                let (p1, p2) = (r.path.clone(), r.path.clone());
                v.push(m.it("Open", move |a| a.open_file_viewer(p1, true)));
                let change = self.status().and_then(|s| {
                    [Group::Merge, Group::Changes, Group::Staged].iter().find_map(|g| s.files.iter().find(|f| f.group == *g && f.path == p2).cloned())
                });
                v.push(m.it_if("Open Changes", change.is_some(), move |a| {
                    if let Some(c) = change {
                        a.open_change(c, true);
                    }
                }));
                v.push(m.it("Open with Default App", move |_| win::shell_open(&a1)));
            }
            v.push(MenuItem::Sep);
            v.push(m.it("Reveal in File Explorer", move |_| win::reveal(&a2)));
            v.push(MenuItem::Sep);
            v.push(m.it("Copy Path", move |a| win::set_clipboard(a.hwnd, &a3)));
            v.push(m.it("Copy Relative Path", move |a| win::set_clipboard(a.hwnd, &rel)));
            v.push(MenuItem::Sep);
        }
        v.push(m.it("Refresh Explorer", |a| a.refresh()));
        v.push(m.it("Collapse Folders in Explorer", |a| a.explorer_collapse_all()));
        v
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_text_encodings_and_rejects_binary() {
        assert_eq!(decode_text(b"\xEF\xBB\xBFhi\r\nthere").as_deref(), Some("hi\r\nthere"));
        assert_eq!(decode_text(&[0xFF, 0xFE, b'h', 0, b'i', 0]).as_deref(), Some("hi"));
        assert_eq!(decode_text(&[0xFE, 0xFF, 0, b'h', 0, b'i']).as_deref(), Some("hi"));
        assert_eq!(decode_text(&[b'a', 0, b'b']), None);
    }

    #[test]
    fn lists_folders_first_and_expands_open_ones() {
        let dir = std::env::temp_dir().join(format!("gitgui-ex-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("src/inner")).unwrap();
        std::fs::create_dir_all(dir.join(".git")).unwrap();
        for f in ["b.txt", "A.md", "src/main.rs", "src/inner/x.rs"] {
            std::fs::write(dir.join(f), "x").unwrap();
        }
        let rows = build_rows(&dir, &HashSet::new());
        let names: Vec<&str> = rows.iter().map(|r| r.name.as_str()).collect();
        assert_eq!(names, ["src", "A.md", "b.txt"]);
        let open: HashSet<String> = ["src".to_string(), "src/inner".to_string()].into();
        let rows = build_rows(&dir, &open);
        let paths: Vec<(&str, u8)> = rows.iter().map(|r| (r.path.as_str(), r.depth)).collect();
        assert_eq!(paths, [("src", 0), ("src/inner", 1), ("src/inner/x.rs", 2), ("src/main.rs", 1), ("A.md", 0), ("b.txt", 0)]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn ignored_covers_children() {
        let mut ex = Explorer::default();
        ex.ignored.insert("target".into());
        assert!(ex.is_ignored("target"));
        assert!(ex.is_ignored("target/release/x.exe"));
        assert!(!ex.is_ignored("src/target.rs"));
    }
}
