//! Integration tests against real temporary git repositories.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use crate::app::{build_change_view, DiffKind, MainView};
use crate::diff::{self, LineId, LK};
use crate::git::{self, Group, Kind};

struct TempRepo(PathBuf);

impl TempRepo {
    fn new(name: &str) -> TempRepo {
        let p = std::env::temp_dir().join(format!("gitgui-test-{}-{}", name, std::process::id()));
        let _ = std::fs::remove_dir_all(&p);
        std::fs::create_dir_all(&p).unwrap();
        git::exec(&p, &["init", "-q", "-b", "main"]).unwrap();
        git::exec(&p, &["config", "user.name", "Test"]).unwrap();
        git::exec(&p, &["config", "user.email", "t@example.com"]).unwrap();
        git::exec(&p, &["config", "core.autocrlf", "false"]).unwrap();
        TempRepo(p)
    }
    fn write(&self, f: &str, c: &str) {
        std::fs::write(self.0.join(f), c).unwrap();
    }
    fn read(&self, f: &str) -> String {
        std::fs::read_to_string(self.0.join(f)).unwrap()
    }
    fn root(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempRepo {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn diff_of(root: &Path, f: &git::FileChange) -> diff::FileDiff {
    match build_change_view(root, f, "-U3", false) {
        MainView::Diff(d) => d.fd,
        _ => panic!("expected diff view"),
    }
}

/// Selects the change lines whose text matches `pred`.
fn select(fd: &diff::FileDiff, pred: impl Fn(&str) -> bool) -> HashSet<LineId> {
    let mut s = HashSet::new();
    for (hi, h) in fd.hunks.iter().enumerate() {
        for (li, l) in h.lines.iter().enumerate() {
            if matches!(l.kind, LK::Add | LK::Del) && pred(&l.text) {
                s.insert((hi, li));
            }
        }
    }
    s
}

fn index_content(root: &Path, f: &str) -> String {
    git::exec(root, &["show", &format!(":{f}")]).unwrap()
}

#[test]
fn status_and_partial_staging() {
    let r = TempRepo::new("stage");
    let base: String = (1..=20).map(|i| format!("line{i}\n")).collect();
    r.write("f.txt", &base);
    git::exec(r.root(), &["add", "."]).unwrap();
    git::exec(r.root(), &["commit", "-q", "-m", "base"]).unwrap();

    let modified = base.replace("line2\n", "CHANGED2\n").replace("line18\n", "CHANGED18\n");
    r.write("f.txt", &modified);
    r.write("new file.txt", "a\nb\nc\n");

    let s = git::status(r.root()).unwrap();
    assert_eq!(s.branch.as_deref(), Some("main"));
    let f = s.files.iter().find(|f| f.path == "f.txt").unwrap().clone();
    assert_eq!((f.group, f.kind), (Group::Changes, Kind::Modified));
    let u = s.files.iter().find(|f| f.path == "new file.txt").unwrap().clone();
    assert_eq!(u.kind, Kind::Untracked);

    // stage only the first change
    let fd = diff_of(r.root(), &f);
    let sel = select(&fd, |t| t.contains('2') && !t.contains("18"));
    let patch = diff::build_patch(&fd, &sel, false).unwrap();
    git::exec_input(r.root(), &["apply", "--cached", "--recount", "-"], Some(patch.as_bytes())).unwrap();
    let idx = index_content(r.root(), "f.txt");
    assert!(idx.contains("CHANGED2") && !idx.contains("CHANGED18"), "index:\n{idx}");

    // unstage it again from the staged diff (reverse apply)
    let s = git::status(r.root()).unwrap();
    let staged = s.files.iter().find(|f| f.path == "f.txt" && f.group == Group::Staged).unwrap().clone();
    let fd = diff_of(r.root(), &staged);
    let sel = select(&fd, |t| t.contains("CHANGED2") || t == "line2");
    let patch = diff::build_patch(&fd, &sel, true).unwrap();
    git::exec_input(r.root(), &["apply", "--cached", "-R", "--recount", "-"], Some(patch.as_bytes())).unwrap();
    assert_eq!(index_content(r.root(), "f.txt"), base);

    // revert only the second change in the working tree
    let fd = diff_of(r.root(), &f);
    let sel = select(&fd, |t| t.contains("18"));
    let patch = diff::build_patch(&fd, &sel, true).unwrap();
    git::exec_input(r.root(), &["apply", "-R", "--recount", "-"], Some(patch.as_bytes())).unwrap();
    let wt = r.read("f.txt");
    assert!(wt.contains("CHANGED2") && wt.contains("line18\n") && !wt.contains("CHANGED18"), "worktree:\n{wt}");

    // stage two of three lines of an untracked file
    let fd = match build_change_view(r.root(), &u, "-U3", false) {
        MainView::Diff(d) => {
            assert_eq!(d.kind, DiffKind::Untracked);
            d.fd
        }
        _ => panic!(),
    };
    let sel = select(&fd, |t| t == "a" || t == "c");
    let patch = diff::build_patch(&fd, &sel, false).unwrap();
    git::exec_input(r.root(), &["apply", "--cached", "--recount", "-"], Some(patch.as_bytes())).unwrap();
    assert_eq!(index_content(r.root(), "new file.txt"), "a\nc\n");
}

#[test]
fn log_refs_stash_and_conflicts() {
    let r = TempRepo::new("log");
    r.write("a.txt", "one\n");
    git::exec(r.root(), &["add", "."]).unwrap();
    git::exec(r.root(), &["commit", "-q", "-m", "first"]).unwrap();
    git::exec(r.root(), &["checkout", "-q", "-b", "side"]).unwrap();
    r.write("a.txt", "side\n");
    git::exec(r.root(), &["commit", "-q", "-am", "side change"]).unwrap();
    git::exec(r.root(), &["checkout", "-q", "main"]).unwrap();
    r.write("a.txt", "main\n");
    git::exec(r.root(), &["commit", "-q", "-am", "main change"]).unwrap();
    git::exec(r.root(), &["tag", "v1"]).unwrap();

    let (commits, more) = git::log(r.root(), &["--branches".into(), "--tags".into(), "HEAD".into()], 0, 50);
    assert_eq!(commits.len(), 3);
    assert!(!more);
    assert!(commits.iter().any(|c| c.refs.iter().any(|d| d.name == "main" && d.current)));
    assert!(commits.iter().any(|c| c.refs.iter().any(|d| d.name == "v1")));
    let rows = crate::graph::compute(&commits, Some("main"), None, None);
    assert_eq!(rows.len(), 3);

    let refs = git::refs(r.root());
    assert!(refs.iter().any(|x| x.name == "side"));

    // conflict
    let o = git::run(r.root(), &["merge", "side"], None, true);
    assert!(!o.ok());
    let e = git::error_from(&o);
    assert!(e.message.contains("merge conflicts"), "{}", e.message);
    let s = git::status(r.root()).unwrap();
    assert!(s.merging);
    let c = s.files.iter().find(|f| f.group == Group::Merge).unwrap().clone();
    match build_change_view(r.root(), &c, "-U3", false) {
        MainView::File(f) => assert_eq!(f.conflicts.len(), 1),
        _ => panic!("expected conflict view"),
    }
    git::exec(r.root(), &["merge", "--abort"]).unwrap();

    // stash
    r.write("a.txt", "dirty\n");
    git::exec(r.root(), &["stash", "push", "-m", "wip"]).unwrap();
    let st = git::stashes(r.root());
    assert_eq!(st.len(), 1);
    assert!(st[0].message.contains("wip"));

    // commit files of the merge-base child
    let (files, parent) = git::commit_files(r.root(), &commits[0].hash);
    assert!(parent.is_some());
    assert_eq!(files[0].path, "a.txt");
}
