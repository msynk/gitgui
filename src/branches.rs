//! Branches view in the side bar: local branches, remote branches grouped by remote, and tags,
//! with a context menu of branch actions.

use std::collections::HashSet;

use crate::app::*;
use crate::git::{RefInfo, RefKind};
use crate::ui::Menu;
use crate::win::{self, MenuItem};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BRow {
    /// Collapsible group. `key` is "local", "remote:<name>" or "tags".
    Group { key: String, label: String, count: usize },
    /// A ref (index into `Repo::refs`).
    Ref(usize),
}

/// Splits a remote ref name ("origin/feature/x") into (remote, branch), preferring the longest
/// configured remote name that matches.
pub fn split_remote(remotes: &[String], name: &str) -> (String, String) {
    let best = remotes.iter().filter(|r| name.len() > r.len() && name.starts_with(r.as_str()) && name.as_bytes()[r.len()] == b'/').max_by_key(|r| r.len());
    match best {
        Some(r) => (r.clone(), name[r.len() + 1..].to_string()),
        None => match name.split_once('/') {
            Some((a, b)) => (a.to_string(), b.to_string()),
            None => (String::new(), name.to_string()),
        },
    }
}

fn push_group(rows: &mut Vec<BRow>, collapsed: &HashSet<String>, key: String, label: String, idx: Vec<usize>) {
    let open = !collapsed.contains(&key);
    rows.push(BRow::Group { key, label, count: idx.len() });
    if open {
        rows.extend(idx.into_iter().map(BRow::Ref));
    }
}

/// Rows of the Branches view: Local, one group per remote, then Tags (only when there are any).
/// Branches are sorted by name; tags keep their order (newest first).
pub fn build_rows(refs: &[RefInfo], remotes: &[String], collapsed: &HashSet<String>) -> Vec<BRow> {
    let by_name = |v: &mut Vec<usize>| v.sort_by(|&a, &b| refs[a].name.to_lowercase().cmp(&refs[b].name.to_lowercase()));
    let mut rows = Vec::new();
    let mut local: Vec<usize> = (0..refs.len()).filter(|&i| refs[i].kind == RefKind::Head).collect();
    by_name(&mut local);
    push_group(&mut rows, collapsed, "local".into(), "Local".into(), local);
    let mut by_remote: Vec<(String, Vec<usize>)> = remotes.iter().map(|r| (r.clone(), Vec::new())).collect();
    for (i, rf) in refs.iter().enumerate().filter(|(_, r)| r.kind == RefKind::Remote) {
        let (remote, _) = split_remote(remotes, &rf.name);
        match by_remote.iter_mut().find(|x| x.0 == remote) {
            Some(x) => x.1.push(i),
            None => by_remote.push((remote, vec![i])),
        }
    }
    for (remote, mut idx) in by_remote {
        by_name(&mut idx);
        push_group(&mut rows, collapsed, format!("remote:{remote}"), remote, idx);
    }
    let tags: Vec<usize> = (0..refs.len()).filter(|&i| refs[i].kind == RefKind::Tag).collect();
    if !tags.is_empty() {
        push_group(&mut rows, collapsed, "tags".into(), "Tags".into(), tags);
    }
    rows
}

/// Short tracking text for a local branch: "↑1 ↓2", "gone" or "".
pub fn track_label(track: &str) -> String {
    if track == "gone" {
        return "gone".into();
    }
    let mut out = Vec::new();
    for part in track.split(", ") {
        if let Some(n) = part.strip_prefix("ahead ") {
            out.push(format!("{n}\u{2191}"));
        } else if let Some(n) = part.strip_prefix("behind ") {
            out.push(format!("{n}\u{2193}"));
        }
    }
    out.join(" ")
}

fn kind_prefix(k: RefKind) -> &'static str {
    match k {
        RefKind::Head => "H:",
        RefKind::Remote => "R:",
        RefKind::Tag => "T:",
    }
}

impl App {
    pub fn rebuild_branches(&mut self) {
        self.brows = match &self.repo {
            Some(r) => build_rows(&r.refs, &r.status.remotes, &self.collapsed_bgroups),
            None => Vec::new(),
        };
    }

    /// Stable key of a row, used to keep the selection across refreshes.
    pub fn brow_key(&self, i: usize) -> Option<String> {
        match self.brows.get(i)? {
            BRow::Group { key, .. } => Some(format!("G:{key}")),
            BRow::Ref(ri) => self.repo.as_ref()?.refs.get(*ri).map(|r| format!("{}{}", kind_prefix(r.kind), r.name)),
        }
    }

    pub fn branch_sel_index(&self) -> Option<usize> {
        let k = self.branch_sel.as_ref()?;
        (0..self.brows.len()).find(|&i| self.brow_key(i).as_ref() == Some(k))
    }

    pub fn select_branch_row(&mut self, i: usize) {
        self.branch_sel = self.brow_key(i);
        self.focus = Focus::Branches;
    }

    pub fn branch_ref(&self, i: usize) -> Option<RefInfo> {
        match self.brows.get(i)? {
            BRow::Ref(ri) => self.repo.as_ref()?.refs.get(*ri).cloned(),
            _ => None,
        }
    }

    fn current_branch(&self) -> Option<String> {
        self.status().and_then(|s| s.branch.clone())
    }

    fn remotes(&self) -> Vec<String> {
        self.status().map(|s| s.remotes.clone()).unwrap_or_default()
    }

    /// Collapses / expands the group at row `i` (`expand`: None toggles).
    pub fn toggle_branch_group(&mut self, i: usize, expand: Option<bool>) {
        let Some(BRow::Group { key, .. }) = self.brows.get(i).cloned() else { return };
        let collapsed = self.collapsed_bgroups.contains(&key);
        let collapse = expand.map(|e| !e).unwrap_or(!collapsed);
        self.select_branch_row(i);
        if collapse == collapsed {
            return;
        }
        if collapse {
            self.collapsed_bgroups.insert(key);
        } else {
            self.collapsed_bgroups.remove(&key);
        }
        self.rebuild_branches();
        self.clamp_scroll(Area::Branches);
        self.invalidate();
    }

    pub fn set_all_branch_groups(&mut self, expand: bool) {
        let keys: Vec<String> = self.brows.iter().filter_map(|r| if let BRow::Group { key, .. } = r { Some(key.clone()) } else { None }).collect();
        if expand {
            self.collapsed_bgroups.clear();
        } else {
            self.collapsed_bgroups.extend(keys);
        }
        self.rebuild_branches();
        self.clamp_scroll(Area::Branches);
        self.invalidate();
    }

    pub fn toggle_branches_section(&mut self) {
        self.st.branches_open = !self.st.branches_open;
        if !self.st.branches_open && self.focus == Focus::Branches {
            self.focus = Focus::Changes;
        }
        self.st.save();
        self.layout();
    }

    pub fn toggle_graph_section(&mut self) {
        self.st.graph_open = !self.st.graph_open;
        if !self.st.graph_open && self.focus == Focus::Graph {
            self.focus = Focus::Changes;
        }
        self.st.save();
        self.layout();
    }

    /// Enter / double click: toggle a group, check out a ref.
    pub fn branch_activate(&mut self, i: usize) {
        if matches!(self.brows.get(i), Some(BRow::Group { .. })) {
            self.toggle_branch_group(i, None);
            return;
        }
        let Some(rf) = self.branch_ref(i) else { return };
        if rf.kind == RefKind::Head && Some(&rf.name) == self.current_branch().as_ref() {
            return;
        }
        self.checkout(format!("{}{}", kind_prefix(rf.kind), rf.name));
    }

    // ------------------------------------------------------------ actions

    pub fn rename_branch_named(&mut self, old: String) {
        self.input("Please provide a new branch name", &old.clone(), move |app, name| {
            let name = sanitize_branch(&name);
            if !name.is_empty() && name != old {
                app.git_op("Rename Branch", vec!["branch".into(), "-m".into(), old.clone(), name]);
            }
        });
    }

    /// Pushes a local branch to its upstream, or publishes it when it has none.
    pub fn push_branch(&mut self, rf: RefInfo) {
        if Some(&rf.name) == self.current_branch().as_ref() {
            self.push(false);
            return;
        }
        let remotes = self.remotes();
        if remotes.is_empty() {
            self.confirm("Your repository has no remotes configured to push to.", "", &["Add Remote"], |app, _| app.add_remote());
            return;
        }
        match &rf.upstream {
            Some(up) => {
                let (remote, b) = split_remote(&remotes, up);
                self.git_op("Pushing", vec!["push".into(), remote, format!("{}:{b}", rf.name)]);
            }
            None => self.publish_branch(rf.name),
        }
    }

    pub fn publish_branch(&mut self, name: String) {
        let remotes = self.remotes();
        let go = move |app: &mut App, remote: String| app.git_op("Publishing", vec!["push".into(), "-u".into(), remote, name.clone()]);
        if remotes.len() == 1 {
            go(self, remotes[0].clone());
        } else {
            let items = remotes.iter().map(|r| pi(r.clone(), "", '\u{E753}', r.clone())).collect();
            self.pick("Pick a remote to publish the branch to:", items, move |app, it| go(app, it.data.clone()));
        }
    }

    /// Fast-forwards a local branch that isn't checked out from its upstream (`git fetch remote b:name`).
    pub fn fast_forward_branch(&mut self, rf: RefInfo) {
        let Some(up) = rf.upstream.clone() else { return };
        let (remote, b) = split_remote(&self.remotes(), &up);
        self.git_op("Fetching", vec!["fetch".into(), remote, format!("{b}:{}", rf.name)]);
    }

    pub fn set_upstream(&mut self, name: String) {
        let items = self.ref_items(false, true, false, false);
        if items.is_empty() {
            self.warn("There are no remote branches to track. Fetch or publish first.");
            return;
        }
        self.pick(&format!("Select the upstream branch for '{name}'"), items, move |app, it| {
            app.git_op("Set Upstream", vec!["branch".into(), format!("--set-upstream-to={}", &it.data[2..]), name.clone()]);
        });
    }

    pub fn delete_remote_branch(&mut self, full: String) {
        let (remote, b) = split_remote(&self.remotes(), &full);
        self.confirm(&format!("Are you sure you want to delete the remote branch '{full}'?"), &format!("The branch '{b}' will be deleted on the remote '{remote}'."), &["Delete Remote Branch"], move |app, _| {
            app.git_op("Delete Remote Branch", vec!["push".into(), remote.clone(), "--delete".into(), b.clone()]);
        });
    }

    pub fn delete_tag_named(&mut self, name: String) {
        self.confirm(&format!("Are you sure you want to delete the tag '{name}'?"), "", &["Delete Tag"], move |app, _| {
            app.git_op("Delete Tag", vec!["tag".into(), "-d".into(), name.clone()]);
        });
    }

    pub fn push_tag(&mut self, name: String) {
        let remotes = self.remotes();
        let go = move |app: &mut App, remote: String| app.git_op("Pushing", vec!["push".into(), remote, format!("refs/tags/{name}")]);
        match remotes.len() {
            0 => self.warn("This repository has no remotes configured to push to."),
            1 => go(self, remotes[0].clone()),
            _ => {
                let items = remotes.iter().map(|r| pi(r.clone(), "", '\u{E753}', r.clone())).collect();
                self.pick("Pick a remote to push the tag to:", items, move |app, it| go(app, it.data.clone()));
            }
        }
    }

    pub fn remove_remote_named(&mut self, remote: String) {
        self.confirm(&format!("Are you sure you want to remove the remote '{remote}'?"), "Its remote-tracking branches are removed too.", &["Remove Remote"], move |app, _| {
            app.git_op("Remove Remote", vec!["remote".into(), "remove".into(), remote.clone()]);
        });
    }

    // ------------------------------------------------------------ menus

    /// Context menu of the Branches row `i`.
    pub fn branch_menu(&mut self, i: usize, m: &mut Menu) -> Vec<MenuItem> {
        let mut v = Vec::new();
        let common = |m: &mut Menu, v: &mut Vec<MenuItem>| {
            v.push(MenuItem::Sep);
            v.push(m.it("Collapse All", |a| a.set_all_branch_groups(false)));
            v.push(m.it("Expand All", |a| a.set_all_branch_groups(true)));
        };
        match self.brows.get(i).cloned() {
            Some(BRow::Group { key, .. }) => {
                if key == "local" {
                    v.push(m.cmd("Create Branch...", Cmd::Branch));
                    v.push(m.cmd("Create Branch From...", Cmd::BranchFrom));
                } else if key == "tags" {
                    v.push(m.cmd("Create Tag...", Cmd::TagCreate));
                    v.push(m.cmd("Push Tags", Cmd::PushTags));
                } else if let Some(remote) = key.strip_prefix("remote:") {
                    let (r1, r2, r3) = (remote.to_string(), remote.to_string(), remote.to_string());
                    v.push(m.it("Fetch", move |a| a.git_op("Fetching", vec!["fetch".into(), r1])));
                    v.push(m.it("Fetch (Prune)", move |a| a.git_op("Fetching", vec!["fetch".into(), "--prune".into(), r2])));
                    v.push(MenuItem::Sep);
                    v.push(m.cmd("Add Remote...", Cmd::AddRemote));
                    v.push(m.it("Remove Remote", move |a| a.remove_remote_named(r3)));
                }
                common(m, &mut v);
            }
            Some(BRow::Ref(_)) => {
                let Some(rf) = self.branch_ref(i) else { return v };
                let cur = self.current_branch();
                let is_cur = rf.kind == RefKind::Head && Some(&rf.name) == cur.as_ref();
                let name = rf.name.clone();
                let n = || name.clone();
                let spec = format!("{}{}", kind_prefix(rf.kind), rf.name);
                v.push(m.it_if("Checkout", !is_cur, move |a| a.checkout(spec)));
                let nn = n();
                v.push(m.it("Checkout (Detached)", move |a| a.checkout(format!("D:{nn}"))));
                v.push(MenuItem::Sep);
                let nn = n();
                v.push(m.it("Create Branch From...", move |a| a.create_branch(Some(nn))));
                let nn = n();
                v.push(m.it("Create Tag...", move |a| a.create_tag(Some(nn))));
                v.push(MenuItem::Sep);
                let target = cur.clone().unwrap_or_else(|| "HEAD".into());
                let nn = n();
                v.push(m.it_if(&format!("Merge into '{target}'"), !is_cur, move |a| a.git_op("Merge", vec!["merge".into(), nn])));
                if rf.kind != RefKind::Tag {
                    let nn = n();
                    v.push(m.it_if(&format!("Rebase '{target}' onto '{}'", rf.name), !is_cur && cur.is_some(), move |a| a.git_op("Rebase", vec!["rebase".into(), nn])));
                }
                match rf.kind {
                    RefKind::Head => {
                        v.push(MenuItem::Sep);
                        let r1 = rf.clone();
                        v.push(m.it(if rf.upstream.is_some() { "Push" } else { "Publish Branch" }, move |a| a.push_branch(r1)));
                        if is_cur {
                            v.push(m.it_if("Pull", rf.upstream.is_some(), |a| a.pull(false)));
                        } else {
                            let r2 = rf.clone();
                            let label = match &rf.upstream {
                                Some(u) => format!("Fast-Forward from '{u}'"),
                                None => "Fast-Forward from Upstream".into(),
                            };
                            v.push(m.it_if(&label, rf.upstream.is_some() && rf.track != "gone", move |a| a.fast_forward_branch(r2)));
                        }
                        let nn = n();
                        v.push(m.it("Set Upstream Branch...", move |a| a.set_upstream(nn)));
                        let nn = n();
                        v.push(m.it_if("Unset Upstream Branch", rf.upstream.is_some(), move |a| a.git_op("Unset Upstream", vec!["branch".into(), "--unset-upstream".into(), nn])));
                        v.push(MenuItem::Sep);
                        let nn = n();
                        v.push(m.it("Rename...", move |a| a.rename_branch_named(nn)));
                        let nn = n();
                        v.push(m.it_if("Delete", !is_cur, move |a| a.delete_branch(Some(nn))));
                        let nn = n();
                        v.push(m.it_if("Force Delete...", !is_cur, move |a| {
                            a.confirm(&format!("Are you sure you want to force delete the branch '{nn}'?"), "Commits only on this branch may be lost.", &["Force Delete"], move |a, _| {
                                a.git_op("Delete Branch", vec!["branch".into(), "-D".into(), nn.clone()]);
                            })
                        }));
                    }
                    RefKind::Remote => {
                        v.push(MenuItem::Sep);
                        let nn = n();
                        v.push(m.it("Delete Remote Branch...", move |a| a.delete_remote_branch(nn)));
                    }
                    RefKind::Tag => {
                        v.push(MenuItem::Sep);
                        let nn = n();
                        v.push(m.it("Push Tag", move |a| a.push_tag(nn)));
                        let nn = n();
                        v.push(m.it("Delete Tag...", move |a| a.delete_tag_named(nn)));
                    }
                }
                v.push(MenuItem::Sep);
                let nn = n();
                v.push(m.it("Copy Name", move |a| win::set_clipboard(a.hwnd, &nn)));
                let sha = rf.sha.clone();
                v.push(m.it("Copy Commit ID", move |a| win::set_clipboard(a.hwnd, &sha)));
            }
            None => {
                v.push(m.cmd("Create Branch...", Cmd::Branch));
                v.push(m.cmd("Fetch From All Remotes", Cmd::FetchAll));
                common(m, &mut v);
            }
        }
        v
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn r(kind: RefKind, name: &str) -> RefInfo {
        RefInfo { kind, name: name.into(), sha: "0123456789abcdef".into(), upstream: None, track: String::new(), subject: String::new(), date: 0 }
    }

    #[test]
    fn groups_local_remotes_tags() {
        let refs = vec![r(RefKind::Head, "zeta"), r(RefKind::Remote, "origin/main"), r(RefKind::Head, "Alpha"), r(RefKind::Tag, "v2"), r(RefKind::Remote, "up/stream/x"), r(RefKind::Tag, "v1")];
        let remotes = vec!["origin".to_string(), "up/stream".to_string()];
        let rows = build_rows(&refs, &remotes, &HashSet::new());
        let labels: Vec<String> = rows
            .iter()
            .map(|b| match b {
                BRow::Group { label, count, .. } => format!("[{label} {count}]"),
                BRow::Ref(i) => refs[*i].name.clone(),
            })
            .collect();
        assert_eq!(labels, ["[Local 2]", "Alpha", "zeta", "[origin 1]", "origin/main", "[up/stream 1]", "up/stream/x", "[Tags 2]", "v2", "v1"]);
        let collapsed: HashSet<String> = ["local".to_string(), "tags".to_string()].into();
        let rows = build_rows(&refs, &remotes, &collapsed);
        assert_eq!(rows.len(), 6);
    }

    #[test]
    fn remote_split_and_track() {
        let remotes = vec!["origin".to_string(), "origin/mirror".to_string()];
        assert_eq!(split_remote(&remotes, "origin/mirror/main"), ("origin/mirror".into(), "main".into()));
        assert_eq!(split_remote(&remotes, "origin/feature/x"), ("origin".into(), "feature/x".into()));
        assert_eq!(split_remote(&[], "fork/dev"), ("fork".into(), "dev".into()));
        assert_eq!(track_label("ahead 1, behind 2"), "1\u{2191} 2\u{2193}");
        assert_eq!(track_label("gone"), "gone");
        assert_eq!(track_label(""), "");
    }
}
