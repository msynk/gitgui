//! Git layer: spawns git.exe (no console window) and parses its porcelain output.

use std::io::Write;
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Mutex;
use std::time::Instant;

const CREATE_NO_WINDOW: u32 = 0x0800_0000;

static LOG: Mutex<Vec<String>> = Mutex::new(Vec::new());

pub fn log_line(s: String) {
    let mut l = LOG.lock().unwrap();
    l.push(s);
    let n = l.len();
    if n > 3000 {
        l.drain(0..n - 3000);
    }
}

pub fn log_snapshot() -> Vec<String> {
    LOG.lock().unwrap().clone()
}

pub fn log_len() -> usize {
    LOG.lock().unwrap().len()
}

pub fn clear_log() {
    LOG.lock().unwrap().clear();
}

fn timestamp() -> String {
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    let local = now as i64 + local_offset_secs();
    let secs = local.rem_euclid(86400);
    format!("{:02}:{:02}:{:02}", secs / 3600, (secs / 60) % 60, secs % 60)
}

pub fn local_offset_secs() -> i64 {
    use windows_sys::Win32::System::Time::{GetTimeZoneInformation, TIME_ZONE_INFORMATION};
    unsafe {
        let mut tz: TIME_ZONE_INFORMATION = std::mem::zeroed();
        let r = GetTimeZoneInformation(&mut tz);
        let bias = tz.Bias + if r == 2 { tz.DaylightBias } else { 0 };
        -(bias as i64) * 60
    }
}

pub struct Output {
    pub code: i32,
    pub stdout: Vec<u8>,
    pub stderr: String,
}

impl Output {
    pub fn ok(&self) -> bool {
        self.code == 0
    }
    pub fn text(&self) -> String {
        String::from_utf8_lossy(&self.stdout).into_owned()
    }
}

#[derive(Debug, Clone)]
pub struct GitError {
    pub message: String,
    pub stderr: String,
}

impl GitError {
    pub fn new(m: impl Into<String>) -> Self {
        let m = m.into();
        GitError { stderr: m.clone(), message: m }
    }
}

pub type GitResult<T> = Result<T, GitError>;

/// Runs git with the given arguments in `cwd`.
pub fn run(cwd: &Path, args: &[&str], input: Option<&[u8]>, quiet: bool) -> Output {
    let start = Instant::now();
    let mut cmd = Command::new("git");
    cmd.current_dir(cwd)
        .args(["-c", "core.quotepath=false", "-c", "color.ui=false"])
        .args(args)
        .env("GIT_OPTIONAL_LOCKS", "0")
        .env("LC_ALL", "C")
        .env("GIT_PAGER", "cat")
        .env("GIT_EDITOR", "true")
        .creation_flags(CREATE_NO_WINDOW)
        .stdin(if input.is_some() { Stdio::piped() } else { Stdio::null() })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let result = match cmd.spawn() {
        Ok(mut child) => {
            if let Some(data) = input {
                if let Some(mut stdin) = child.stdin.take() {
                    let _ = stdin.write_all(data);
                }
            }
            match child.wait_with_output() {
                Ok(o) => Output { code: o.status.code().unwrap_or(-1), stdout: o.stdout, stderr: String::from_utf8_lossy(&o.stderr).into_owned() },
                Err(e) => Output { code: -1, stdout: vec![], stderr: e.to_string() },
            }
        }
        Err(e) => Output { code: -1, stdout: vec![], stderr: format!("Unable to run git: {e}") },
    };
    if !quiet {
        log_line(format!("{} > git {} [{}ms]", timestamp(), args.iter().map(|a| if a.contains(' ') { format!("\"{a}\"") } else { a.to_string() }).collect::<Vec<_>>().join(" "), start.elapsed().as_millis()));
        if result.code != 0 {
            for l in result.stderr.lines().filter(|l| !l.trim().is_empty()).take(20) {
                log_line(format!("{} {}", timestamp(), l));
            }
        }
    }
    result
}

/// Runs git and converts a non-zero exit code into an error.
pub fn exec(cwd: &Path, args: &[&str]) -> GitResult<String> {
    exec_input(cwd, args, None)
}

pub fn exec_input(cwd: &Path, args: &[&str], input: Option<&[u8]>) -> GitResult<String> {
    let o = run(cwd, args, input, false);
    if o.ok() {
        Ok(o.text())
    } else {
        Err(error_from(&o))
    }
}

pub fn error_from(o: &Output) -> GitError {
    let all = format!("{}\n{}", o.stderr, o.text());
    let first = o
        .stderr
        .lines()
        .chain(o.text().lines().map(|_| ""))
        .find(|l| !l.trim().is_empty() && !l.starts_with("hint:"))
        .unwrap_or("")
        .to_string();
    let first = if first.is_empty() { o.text().lines().find(|l| !l.trim().is_empty()).unwrap_or("Git error").to_string() } else { first };
    let message = friendly_error(&all).unwrap_or_else(|| {
        let f = first.trim_start_matches("fatal: ").trim_start_matches("error: ");
        format!("Git: {f}")
    });
    GitError { message, stderr: all }
}

fn friendly_error(all: &str) -> Option<String> {
    let s = all;
    if s.contains("Please tell me who you are") || s.contains("unable to auto-detect email address") {
        return Some("Make sure you configure your \"user.name\" and \"user.email\" in git.".into());
    }
    if s.contains("[rejected]") || s.contains("failed to push some refs") {
        return Some("Can't push refs to remote. Try running \"Pull\" first to integrate your changes.".into());
    }
    if s.contains("would be overwritten by checkout") || s.contains("commit your changes or stash them") {
        return Some("Your local changes would be overwritten. Commit or stash them first.".into());
    }
    if s.contains("CONFLICT") || s.contains("Automatic merge failed") || s.contains("could not apply") {
        return Some("There are merge conflicts. Resolve them before committing.".into());
    }
    if s.contains("has no upstream branch") || s.contains("no tracking information") {
        return Some("The current branch has no upstream branch. Publish it first.".into());
    }
    if s.contains("Authentication failed") {
        return Some("Failed to authenticate to git remote.".into());
    }
    if s.contains("not fully merged") {
        return Some("The branch is not fully merged.".into());
    }
    if s.contains("Another git process seems to be running") {
        return Some("Another git process seems to be running in this repository (index.lock).".into());
    }
    None
}

pub fn toplevel(path: &Path) -> Option<PathBuf> {
    let o = run(path, &["rev-parse", "--show-toplevel"], None, true);
    if !o.ok() {
        return None;
    }
    let s = o.text().trim().replace('/', "\\");
    if s.is_empty() {
        None
    } else {
        Some(PathBuf::from(s))
    }
}

pub fn git_dir(root: &Path) -> PathBuf {
    let o = run(root, &["rev-parse", "--git-dir"], None, true);
    let p = o.text().trim().to_string();
    let pb = PathBuf::from(&p);
    if pb.is_absolute() {
        pb
    } else {
        root.join(pb)
    }
}

pub fn version() -> Option<String> {
    let o = run(Path::new("."), &["--version"], None, true);
    if o.ok() {
        Some(o.text().trim().trim_start_matches("git version ").to_string())
    } else {
        None
    }
}

// ------------------------------------------------------------------ status

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Group {
    Merge,
    Staged,
    Changes,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    Modified,
    Added,
    Deleted,
    Renamed,
    Copied,
    TypeChanged,
    Untracked,
    Conflict,
}

#[derive(Clone, Debug)]
pub struct FileChange {
    pub path: String,
    pub orig: Option<String>,
    pub kind: Kind,
    pub group: Group,
    pub xy: String,
}

impl FileChange {
    pub fn letter(&self) -> char {
        match self.kind {
            Kind::Modified => 'M',
            Kind::Added => 'A',
            Kind::Deleted => 'D',
            Kind::Renamed => 'R',
            Kind::Copied => 'C',
            Kind::TypeChanged => 'T',
            Kind::Untracked => 'U',
            Kind::Conflict => '!',
        }
    }
    pub fn tooltip(&self) -> &'static str {
        match (self.group, self.kind) {
            (Group::Staged, Kind::Modified) => "Index Modified",
            (Group::Staged, Kind::Added) => "Index Added",
            (Group::Staged, Kind::Deleted) => "Index Deleted",
            (Group::Staged, Kind::Renamed) => "Index Renamed",
            (_, Kind::Modified) => "Modified",
            (_, Kind::Deleted) => "Deleted",
            (_, Kind::Untracked) => "Untracked",
            (_, Kind::Conflict) => "Conflict",
            (_, Kind::Added) => "Added",
            (_, Kind::Renamed) => "Renamed",
            (_, Kind::Copied) => "Copied",
            (_, Kind::TypeChanged) => "Type Changed",
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct Status {
    pub branch: Option<String>,
    pub head: Option<String>,
    pub upstream: Option<String>,
    pub ahead: u32,
    pub behind: u32,
    pub files: Vec<FileChange>,
    pub merging: bool,
    pub rebasing: bool,
    pub cherry_picking: bool,
    pub merge_msg: Option<String>,
    pub remotes: Vec<String>,
}

impl Status {
    pub fn count(&self, g: Group) -> usize {
        self.files.iter().filter(|f| f.group == g).count()
    }
}

fn kind_from(c: char) -> Kind {
    match c {
        'A' => Kind::Added,
        'D' => Kind::Deleted,
        'R' => Kind::Renamed,
        'C' => Kind::Copied,
        'T' => Kind::TypeChanged,
        _ => Kind::Modified,
    }
}

pub fn parse_status(out: &str) -> Status {
    let mut s = Status::default();
    let tokens: Vec<&str> = out.split('\0').collect();
    let mut i = 0;
    while i < tokens.len() {
        let t = tokens[i];
        i += 1;
        if t.is_empty() {
            continue;
        }
        if let Some(h) = t.strip_prefix("# ") {
            let (k, v) = h.split_once(' ').unwrap_or((h, ""));
            match k {
                "branch.oid" => s.head = if v == "(initial)" { None } else { Some(v.to_string()) },
                "branch.head" => s.branch = if v == "(detached)" { None } else { Some(v.to_string()) },
                "branch.upstream" => s.upstream = Some(v.to_string()),
                "branch.ab" => {
                    for p in v.split(' ') {
                        if let Some(n) = p.strip_prefix('+') {
                            s.ahead = n.parse().unwrap_or(0);
                        } else if let Some(n) = p.strip_prefix('-') {
                            s.behind = n.parse().unwrap_or(0);
                        }
                    }
                }
                _ => {}
            }
            continue;
        }
        let ty = t.as_bytes()[0];
        match ty {
            b'1' | b'2' => {
                let n = if ty == b'1' { 8 } else { 9 };
                let parts: Vec<&str> = t.splitn(n + 1, ' ').collect();
                if parts.len() < n + 1 {
                    continue;
                }
                let xy = parts[1];
                let path = parts[n].to_string();
                let orig = if ty == b'2' {
                    let o = tokens.get(i).map(|s| s.to_string());
                    i += 1;
                    o
                } else {
                    None
                };
                let x = xy.chars().next().unwrap_or('.');
                let y = xy.chars().nth(1).unwrap_or('.');
                if x != '.' {
                    s.files.push(FileChange { path: path.clone(), orig: if x == 'R' || x == 'C' { orig.clone() } else { None }, kind: kind_from(x), group: Group::Staged, xy: xy.to_string() });
                }
                if y != '.' {
                    let k = if y == 'A' { Kind::Added } else { kind_from(y) };
                    s.files.push(FileChange { path, orig: None, kind: k, group: Group::Changes, xy: xy.to_string() });
                }
            }
            b'u' => {
                let parts: Vec<&str> = t.splitn(11, ' ').collect();
                if parts.len() == 11 {
                    s.files.push(FileChange { path: parts[10].to_string(), orig: None, kind: Kind::Conflict, group: Group::Merge, xy: parts[1].to_string() });
                }
            }
            b'?' => {
                s.files.push(FileChange { path: t[2..].to_string(), orig: None, kind: Kind::Untracked, group: Group::Changes, xy: "??".into() });
            }
            _ => {}
        }
    }
    s
}

pub fn status(root: &Path) -> GitResult<Status> {
    let o = run(root, &["status", "-z", "--porcelain=v2", "--branch", "--untracked-files=all"], None, false);
    if !o.ok() {
        return Err(error_from(&o));
    }
    let mut s = parse_status(&o.text());
    s.files.sort_by(|a, b| a.path.to_lowercase().cmp(&b.path.to_lowercase()));
    let gd = git_dir(root);
    s.merging = gd.join("MERGE_HEAD").exists();
    s.rebasing = gd.join("rebase-merge").exists() || gd.join("rebase-apply").exists();
    s.cherry_picking = gd.join("CHERRY_PICK_HEAD").exists();
    if s.merging || s.cherry_picking {
        if let Ok(m) = std::fs::read_to_string(gd.join("MERGE_MSG")) {
            let msg: Vec<&str> = m.lines().filter(|l| !l.starts_with('#')).collect();
            s.merge_msg = Some(msg.join("\n").trim().to_string());
        }
    }
    let r = run(root, &["remote"], None, true);
    s.remotes = r.text().lines().map(|l| l.trim().to_string()).filter(|l| !l.is_empty()).collect();
    Ok(s)
}

// -------------------------------------------------------------------- refs

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum RefKind {
    Head,
    Remote,
    Tag,
}

#[derive(Clone, Debug)]
pub struct RefInfo {
    pub kind: RefKind,
    pub name: String,
    pub sha: String,
    pub upstream: Option<String>,
    /// Upstream tracking state, e.g. "ahead 1, behind 2" or "gone" (empty when in sync).
    pub track: String,
    pub subject: String,
    pub date: i64,
}

pub fn refs(root: &Path) -> Vec<RefInfo> {
    let o = run(root, &["for-each-ref", "--sort=-committerdate", "--format=%(refname)%00%(objectname)%00%(*objectname)%00%(upstream:short)%00%(upstream:track,nobracket)%00%(committerdate:unix)%00%(subject)", "refs/heads", "refs/remotes", "refs/tags"], None, true);
    let mut v = Vec::new();
    for line in o.text().lines() {
        let f: Vec<&str> = line.split('\0').collect();
        if f.len() < 7 {
            continue;
        }
        let (kind, name) = if let Some(n) = f[0].strip_prefix("refs/heads/") {
            (RefKind::Head, n)
        } else if let Some(n) = f[0].strip_prefix("refs/remotes/") {
            if n.ends_with("/HEAD") {
                continue;
            }
            (RefKind::Remote, n)
        } else if let Some(n) = f[0].strip_prefix("refs/tags/") {
            (RefKind::Tag, n)
        } else {
            continue;
        };
        v.push(RefInfo {
            kind,
            name: name.to_string(),
            sha: if f[2].is_empty() { f[1] } else { f[2] }.to_string(),
            upstream: if f[3].is_empty() { None } else { Some(f[3].to_string()) },
            track: f[4].to_string(),
            date: f[5].parse().unwrap_or(0),
            subject: f[6].to_string(),
        });
    }
    v
}

#[derive(Clone, Debug)]
pub struct Stash {
    pub index: usize,
    pub message: String,
}

pub fn stashes(root: &Path) -> Vec<Stash> {
    let o = run(root, &["stash", "list", "--format=%gd%x1f%s"], None, true);
    o.text()
        .lines()
        .filter_map(|l| {
            let (r, m) = l.split_once('\x1f')?;
            let idx = r.trim_start_matches("stash@{").trim_end_matches('}').parse().ok()?;
            Some(Stash { index: idx, message: m.to_string() })
        })
        .collect()
}

// --------------------------------------------------------------------- log

#[derive(Clone, Debug)]
pub struct Decoration {
    pub kind: RefKind,
    pub name: String,
    pub current: bool,
}

#[derive(Clone, Debug)]
pub struct Commit {
    pub hash: String,
    pub parents: Vec<String>,
    pub author: String,
    pub email: String,
    pub date: i64,
    pub message: String,
    pub refs: Vec<Decoration>,
}

impl Commit {
    pub fn subject(&self) -> &str {
        self.message.lines().next().unwrap_or("")
    }
}

pub fn parse_decorations(s: &str) -> Vec<Decoration> {
    let mut out = Vec::new();
    for part in s.split(", ") {
        let mut p = part.trim();
        let mut current = false;
        if let Some(r) = p.strip_prefix("HEAD -> ") {
            p = r;
            current = true;
        }
        if p.is_empty() || p == "HEAD" || p.starts_with("refs/stash") {
            continue;
        }
        if let Some(t) = p.strip_prefix("tag: ") {
            out.push(Decoration { kind: RefKind::Tag, name: t.trim_start_matches("refs/tags/").to_string(), current: false });
        } else if let Some(n) = p.strip_prefix("refs/heads/") {
            out.push(Decoration { kind: RefKind::Head, name: n.to_string(), current });
        } else if let Some(n) = p.strip_prefix("refs/remotes/") {
            if !n.ends_with("/HEAD") {
                out.push(Decoration { kind: RefKind::Remote, name: n.to_string(), current: false });
            }
        }
    }
    out
}

pub fn parse_log(out: &str) -> Vec<Commit> {
    let mut v = Vec::new();
    for rec in out.split('\x1e') {
        let rec = rec.trim_start();
        if rec.is_empty() {
            continue;
        }
        let f: Vec<&str> = rec.splitn(7, '\x1f').collect();
        if f.len() < 7 {
            continue;
        }
        v.push(Commit {
            hash: f[0].to_string(),
            parents: f[1].split(' ').filter(|s| !s.is_empty()).map(String::from).collect(),
            author: f[2].to_string(),
            email: f[3].to_string(),
            date: f[4].parse().unwrap_or(0),
            refs: parse_decorations(f[5]),
            message: f[6].trim_end().to_string(),
        });
    }
    v
}

/// Returns (commits, has_more).
pub fn log(root: &Path, refs: &[String], skip: usize, limit: usize) -> (Vec<Commit>, bool) {
    let n = format!("-n{}", limit + 1);
    let sk = format!("--skip={skip}");
    let mut args: Vec<&str> = vec!["log", "--format=%H%x1f%P%x1f%an%x1f%ae%x1f%at%x1f%D%x1f%B%x1e", "--decorate=full", "--topo-order", &n, &sk];
    for r in refs {
        args.push(r);
    }
    args.push("--");
    let o = run(root, &args, None, false);
    if !o.ok() {
        return (vec![], false);
    }
    let mut c = parse_log(&o.text());
    let more = c.len() > limit;
    c.truncate(limit);
    (c, more)
}

// ------------------------------------------------------------- diffs / files

/// Parses `--name-status -z` output.
pub fn parse_name_status(out: &str) -> Vec<FileChange> {
    let t: Vec<&str> = out.split('\0').collect();
    let mut v = Vec::new();
    let mut i = 0;
    while i < t.len() {
        let s = t[i];
        i += 1;
        if s.is_empty() {
            continue;
        }
        let c = s.chars().next().unwrap();
        if c == 'R' || c == 'C' {
            let orig = t.get(i).map(|s| s.to_string());
            let path = t.get(i + 1).map(|s| s.to_string()).unwrap_or_default();
            i += 2;
            v.push(FileChange { path, orig, kind: kind_from(c), group: Group::Staged, xy: s.to_string() });
        } else {
            let path = t.get(i).map(|s| s.to_string()).unwrap_or_default();
            i += 1;
            v.push(FileChange { path, orig: None, kind: kind_from(c), group: Group::Staged, xy: s.to_string() });
        }
    }
    v
}

/// Files changed by a commit compared to its first parent (or the empty tree).
pub fn commit_files(root: &Path, sha: &str) -> (Vec<FileChange>, Option<String>) {
    let p = run(root, &["rev-list", "--parents", "-n1", sha], None, true).text();
    let parent = p.split_whitespace().nth(1).map(String::from);
    let o = match &parent {
        Some(par) => run(root, &["diff", "--name-status", "-z", "-M", par, sha], None, false),
        None => run(root, &["diff-tree", "--root", "--no-commit-id", "--name-status", "-z", "-r", "-M", sha], None, false),
    };
    (parse_name_status(&o.text()), parent)
}

pub fn show_file(root: &Path, spec: &str) -> Option<Vec<u8>> {
    let o = run(root, &["show", spec], None, true);
    if o.ok() {
        Some(o.stdout)
    } else {
        None
    }
}

pub fn is_binary(b: &[u8]) -> bool {
    b.iter().take(8000).any(|&c| c == 0)
}
