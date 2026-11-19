//! Editor tabs for the main area (diffs, files, commits), with VS Code's preview tab:
//! a single click opens a preview tab (italic title) that the next open replaces; a double
//! click, double-clicking the tab or "Keep Open" pins it.

use crate::app::*;
use crate::ui::Menu;
use crate::win::{self, MenuItem};

pub struct EditorTab {
    /// The tab's view. For the active tab the real view lives in `App::view` and this is
    /// `MainView::Empty`.
    pub view: MainView,
    /// Identity used to reuse a tab that already shows the same thing.
    pub key: String,
    pub preview: bool,
    pub scroll: i32,
    pub hscroll: i32,
    /// `view_gen` of the load that fills this tab (results for other tabs are routed by it).
    pub gen: u64,
}

/// (title, description) shown for a view.
pub fn view_title(v: &MainView) -> (String, String) {
    match v {
        MainView::Diff(d) => (d.title.clone(), d.desc.clone()),
        MainView::File(f) => (f.title.clone(), f.path.clone()),
        MainView::Commit(c) => (format!("{} ({})", c.commit.subject(), &c.commit.hash[..8.min(c.commit.hash.len())]), if c.stash.is_some() { "Stash".into() } else { "Commit".into() }),
        MainView::Message(t, _) => (t.clone(), String::new()),
        MainView::Loading(t) => (t.clone(), "Loading...".into()),
        MainView::Empty => (String::new(), String::new()),
    }
}

impl App {
    pub fn tab_view(&self, i: usize) -> &MainView {
        if i == self.tab_active {
            &self.view
        } else {
            &self.tabs[i].view
        }
    }

    /// Picks the tab the next view goes into: a tab that already has `key` (activated and
    /// refreshed; returns true, so the caller doesn't load it again), else the preview tab
    /// (replaced), else a new tab after the active one. The caller then fills `self.view`.
    pub fn open_tab(&mut self, key: String, pin: bool) -> bool {
        if let Some(i) = self.tabs.iter().position(|t| t.key == key) {
            self.activate_tab(i);
            if pin {
                self.tabs[i].preview = false;
            }
            return true;
        }
        let reuse = self.tabs.get(self.tab_active).map(|t| t.preview).unwrap_or(false);
        if reuse {
            let t = &mut self.tabs[self.tab_active];
            t.key = key;
            t.preview = !pin;
        } else {
            // park the current view in its tab (a view without a tab, like the empty page, is dropped)
            let cur = std::mem::replace(&mut self.view, MainView::Empty);
            if let Some(t) = self.tabs.get_mut(self.tab_active) {
                t.view = cur;
                t.scroll = self.scroll[Area::Main as usize];
                t.hscroll = self.hscroll;
            }
            let at = if self.tabs.is_empty() { 0 } else { self.tab_active + 1 };
            self.tabs.insert(at, EditorTab { view: MainView::Empty, key, preview: !pin, scroll: 0, hscroll: 0, gen: 0 });
            self.tab_active = at;
        }
        self.scroll[Area::Main as usize] = 0;
        self.hscroll = 0;
        false
    }

    /// Records that the load with `gen` fills the active tab.
    pub fn mark_tab_gen(&mut self, gen: u64) {
        if let Some(t) = self.tabs.get_mut(self.tab_active) {
            t.gen = gen;
        }
    }

    /// Switches tabs without refreshing the newly active view.
    fn activate_tab_raw(&mut self, i: usize) {
        if i >= self.tabs.len() || i == self.tab_active {
            return;
        }
        let cur = std::mem::replace(&mut self.view, MainView::Empty);
        if let Some(t) = self.tabs.get_mut(self.tab_active) {
            t.view = cur;
            t.scroll = self.scroll[Area::Main as usize];
            t.hscroll = self.hscroll;
        }
        self.tab_active = i;
        let t = &mut self.tabs[i];
        self.view = std::mem::replace(&mut t.view, MainView::Empty);
        self.scroll[Area::Main as usize] = t.scroll;
        self.hscroll = t.hscroll;
    }

    pub fn activate_tab(&mut self, i: usize) {
        if i >= self.tabs.len() || i == self.tab_active {
            return;
        }
        self.activate_tab_raw(i);
        self.refresh_tab_view();
        self.clamp_scroll(Area::Main);
        self.invalidate();
    }

    /// Brings the active view up to date after it was in the background: working tree diffs
    /// are reloaded (keeping the scroll position), other diffs follow the inline setting.
    fn refresh_tab_view(&mut self) {
        let change = match &self.view {
            MainView::Diff(d) if d.kind != DiffKind::Commit && d.back.is_none() => d.change.clone(),
            MainView::File(f) => f.change.clone(),
            _ => None,
        };
        if let Some(c) = change {
            let still = self.status().and_then(|s| s.files.iter().find(|f| f.path == c.path && f.group == c.group).cloned());
            if let Some(f) = still {
                self.load_change_view(f, true);
                return;
            }
        }
        if matches!(&self.view, MainView::Diff(d) if d.inline != self.st.inline_diff) {
            self.rebuild_rows();
        }
    }

    pub fn close_tab(&mut self, i: usize) {
        if i >= self.tabs.len() {
            return;
        }
        if i == self.tab_active {
            self.tabs.remove(i);
            if self.tabs.is_empty() {
                self.tab_active = 0;
                self.view = MainView::Empty;
                self.scroll[Area::Main as usize] = 0;
                self.hscroll = 0;
            } else {
                // like VS Code: the tab to the right takes over, else the one to the left
                let n = i.min(self.tabs.len() - 1);
                let t = &mut self.tabs[n];
                self.view = std::mem::replace(&mut t.view, MainView::Empty);
                self.scroll[Area::Main as usize] = t.scroll;
                self.hscroll = t.hscroll;
                self.tab_active = n;
                self.refresh_tab_view();
            }
        } else {
            self.tabs.remove(i);
            if i < self.tab_active {
                self.tab_active -= 1;
            }
        }
        self.clamp_scroll(Area::Main);
        self.invalidate();
    }

    pub fn close_active_tab(&mut self) {
        if !self.tabs.is_empty() {
            self.close_tab(self.tab_active);
        }
    }

    pub fn close_other_tabs(&mut self, keep: usize) {
        if keep >= self.tabs.len() {
            return;
        }
        self.activate_tab(keep);
        let t = self.tabs.remove(self.tab_active);
        self.tabs = vec![t];
        self.tab_active = 0;
        self.invalidate();
    }

    pub fn close_tabs_right(&mut self, of: usize) {
        if of >= self.tabs.len() {
            return;
        }
        if self.tab_active > of {
            self.activate_tab(of);
        }
        self.tabs.truncate(of + 1);
        self.invalidate();
    }

    pub fn close_all_tabs(&mut self) {
        self.tabs.clear();
        self.tab_active = 0;
        self.view = MainView::Empty;
        self.scroll[Area::Main as usize] = 0;
        self.hscroll = 0;
        self.invalidate();
    }

    pub fn pin_tab(&mut self, i: usize) {
        if let Some(t) = self.tabs.get_mut(i) {
            t.preview = false;
            self.invalidate();
        }
    }

    /// Activates the next (`delta` 1) or previous (-1) tab, wrapping around.
    pub fn cycle_tab(&mut self, delta: i32) {
        let n = self.tabs.len() as i32;
        if n > 1 {
            self.activate_tab((self.tab_active as i32 + delta).rem_euclid(n) as usize);
        }
    }

    /// Context menu of tab `i`.
    pub fn tab_menu(&mut self, i: usize, m: &mut Menu) -> Vec<MenuItem> {
        let n = self.tabs.len();
        let preview = self.tabs.get(i).map(|t| t.preview).unwrap_or(false);
        let path = match self.tab_view(i) {
            MainView::Diff(d) => Some(d.path.clone()),
            MainView::File(f) => Some(f.path.clone()),
            _ => None,
        };
        let mut v = vec![
            m.it("Close", move |a| a.close_tab(i)),
            m.it_if("Close Others", n > 1, move |a| a.close_other_tabs(i)),
            m.it_if("Close to the Right", i + 1 < n, move |a| a.close_tabs_right(i)),
            m.it("Close All", |a| a.close_all_tabs()),
            MenuItem::Sep,
            m.it_if("Keep Open", preview, move |a| a.pin_tab(i)),
        ];
        if let (Some(p), Some(root)) = (path.filter(|p| !p.is_empty()), self.root()) {
            let abs = root.join(&p).display().to_string();
            v.push(MenuItem::Sep);
            v.push(m.it("Copy Path", move |a| win::set_clipboard(a.hwnd, &abs)));
            v.push(m.it("Copy Relative Path", move |a| win::set_clipboard(a.hwnd, &p)));
        }
        v
    }
}
