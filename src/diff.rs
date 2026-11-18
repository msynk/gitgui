//! Unified diff parsing, side-by-side row layout and partial (line / block) patch building.

use std::collections::HashSet;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum LK {
    Ctx,
    Add,
    Del,
    NoNl,
}

#[derive(Clone, Debug)]
pub struct DLine {
    pub kind: LK,
    /// Raw line content without the +/-/space prefix (may contain a trailing '\r').
    pub text: String,
    pub old: u32,
    pub new: u32,
}

#[derive(Clone, Debug)]
pub struct Hunk {
    pub old_start: u32,
    pub old_len: u32,
    pub new_start: u32,
    pub new_len: u32,
    pub section: String,
    pub lines: Vec<DLine>,
}

#[derive(Clone, Debug, Default)]
pub struct FileDiff {
    pub header: Vec<String>,
    pub hunks: Vec<Hunk>,
    pub binary: bool,
}

pub type LineId = (usize, usize);

fn parse_range(s: &str) -> (u32, u32) {
    let (a, b) = s.split_once(',').unwrap_or((s, "1"));
    (a.parse().unwrap_or(0), b.parse().unwrap_or(1))
}

pub fn parse(text: &str) -> FileDiff {
    let mut fd = FileDiff::default();
    let mut cur: Option<Hunk> = None;
    let (mut o, mut n) = (0u32, 0u32);
    for raw in text.split('\n') {
        if let Some(rest) = raw.strip_prefix("@@ ") {
            if let Some(h) = cur.take() {
                fd.hunks.push(h);
            }
            // @@ -a,b +c,d @@ section
            let end = rest.find(" @@").unwrap_or(rest.len());
            let ranges = &rest[..end];
            let section = rest.get(end + 3..).unwrap_or("").trim().to_string();
            let mut it = ranges.split(' ');
            let (os, ol) = parse_range(it.next().unwrap_or("-0").trim_start_matches('-'));
            let (ns, nl) = parse_range(it.next().unwrap_or("+0").trim_start_matches('+'));
            o = os;
            n = ns;
            cur = Some(Hunk { old_start: os, old_len: ol, new_start: ns, new_len: nl, section, lines: Vec::new() });
            continue;
        }
        match cur.as_mut() {
            None => {
                if raw.starts_with("Binary files") || raw.starts_with("GIT binary patch") {
                    fd.binary = true;
                }
                if !raw.is_empty() {
                    fd.header.push(raw.to_string());
                }
            }
            Some(h) => {
                if raw.is_empty() {
                    continue;
                }
                let (k, body) = raw.split_at(1);
                match k {
                    " " => {
                        h.lines.push(DLine { kind: LK::Ctx, text: body.to_string(), old: o, new: n });
                        o += 1;
                        n += 1;
                    }
                    "+" => {
                        h.lines.push(DLine { kind: LK::Add, text: body.to_string(), old: 0, new: n });
                        n += 1;
                    }
                    "-" => {
                        h.lines.push(DLine { kind: LK::Del, text: body.to_string(), old: o, new: 0 });
                        o += 1;
                    }
                    "\\" => h.lines.push(DLine { kind: LK::NoNl, text: raw.to_string(), old: 0, new: 0 }),
                    _ => {
                        // a new file section (diff --git) would start here; not expected for single-file diffs
                    }
                }
            }
        }
    }
    if let Some(h) = cur.take() {
        fd.hunks.push(h);
    }
    fd
}

/// One rendered row of the diff view.
#[derive(Clone, Debug)]
pub struct Row {
    pub left: Option<LineId>,
    pub right: Option<LineId>,
    pub block: Option<usize>,
    /// Separator row shown between hunks (compact mode): index of the following hunk.
    pub sep: Option<usize>,
}

/// A contiguous run of added/removed lines.
#[derive(Clone, Debug)]
pub struct Block {
    pub lines: Vec<LineId>,
    pub first_row: usize,
}

pub fn build_rows(fd: &FileDiff, inline: bool) -> (Vec<Row>, Vec<Block>) {
    let mut rows = Vec::new();
    let mut blocks: Vec<Block> = Vec::new();
    for (hi, h) in fd.hunks.iter().enumerate() {
        if hi > 0 || h.old_start > 1 || h.new_start > 1 {
            rows.push(Row { left: None, right: None, block: None, sep: Some(hi) });
        }
        let mut li = 0;
        while li < h.lines.len() {
            let l = &h.lines[li];
            match l.kind {
                LK::Ctx => {
                    rows.push(Row { left: Some((hi, li)), right: if inline { None } else { Some((hi, li)) }, block: None, sep: None });
                    li += 1;
                }
                LK::NoNl => li += 1,
                LK::Add | LK::Del => {
                    // collect a change block
                    let start = li;
                    let mut dels = Vec::new();
                    let mut adds = Vec::new();
                    while li < h.lines.len() && matches!(h.lines[li].kind, LK::Add | LK::Del | LK::NoNl) {
                        match h.lines[li].kind {
                            LK::Del => dels.push((hi, li)),
                            LK::Add => adds.push((hi, li)),
                            _ => {}
                        }
                        li += 1;
                    }
                    let bi = blocks.len();
                    let mut lines: Vec<LineId> = (start..li).filter(|&x| h.lines[x].kind != LK::NoNl).map(|x| (hi, x)).collect();
                    lines.sort();
                    blocks.push(Block { lines, first_row: rows.len() });
                    if inline {
                        for d in &dels {
                            rows.push(Row { left: Some(*d), right: None, block: Some(bi), sep: None });
                        }
                        for a in &adds {
                            rows.push(Row { left: Some(*a), right: None, block: Some(bi), sep: None });
                        }
                    } else {
                        let n = dels.len().max(adds.len());
                        for k in 0..n {
                            rows.push(Row { left: dels.get(k).copied(), right: adds.get(k).copied(), block: Some(bi), sep: None });
                        }
                    }
                }
            }
        }
    }
    (rows, blocks)
}

/// Builds a patch containing only the selected change lines.
/// `reverse == false`: patch is applied forward onto the old side (staging).
/// `reverse == true`: patch is applied with `-R` onto the new side (unstaging / reverting).
pub fn build_patch(fd: &FileDiff, selected: &HashSet<LineId>, reverse: bool) -> Option<String> {
    let mut out = String::new();
    for h in &fd.header {
        if h.starts_with("diff --git") || h.starts_with("---") || h.starts_with("+++") || h.starts_with("new file mode") || h.starts_with("deleted file mode") || h.starts_with("old mode") || h.starts_with("new mode") {
            out.push_str(h);
            out.push('\n');
        }
    }
    let mut any = false;
    let mut delta: i64 = 0;
    for (hi, h) in fd.hunks.iter().enumerate() {
        let has = h.lines.iter().enumerate().any(|(li, l)| matches!(l.kind, LK::Add | LK::Del) && selected.contains(&(hi, li)));
        if !has {
            continue;
        }
        let mut body = String::new();
        let (mut oc, mut nc) = (0u32, 0u32);
        let mut last_kept = false;
        for (li, l) in h.lines.iter().enumerate() {
            let sel = selected.contains(&(hi, li));
            let emit: Option<char> = match l.kind {
                LK::Ctx => Some(' '),
                LK::Add => {
                    if sel {
                        Some('+')
                    } else if reverse {
                        Some(' ')
                    } else {
                        None
                    }
                }
                LK::Del => {
                    if sel {
                        Some('-')
                    } else if reverse {
                        None
                    } else {
                        Some(' ')
                    }
                }
                LK::NoNl => {
                    if last_kept {
                        body.push_str(&l.text);
                        body.push('\n');
                    }
                    continue;
                }
            };
            match emit {
                Some(c) => {
                    match c {
                        ' ' => {
                            oc += 1;
                            nc += 1;
                        }
                        '+' => nc += 1,
                        _ => oc += 1,
                    }
                    body.push(c);
                    body.push_str(&l.text);
                    body.push('\n');
                    last_kept = true;
                }
                None => last_kept = false,
            }
        }
        let (os, ns) = if reverse {
            let ns = h.new_start as i64;
            ((ns - delta).max(0), ns)
        } else {
            let os = h.old_start as i64;
            (os, (os + delta).max(if nc > 0 { 1 } else { 0 }))
        };
        delta += nc as i64 - oc as i64;
        let os = if oc == 0 && os > 0 && !reverse { os } else { os };
        out.push_str(&format!("@@ -{},{} +{},{} @@\n", os, oc, ns, nc));
        out.push_str(&body);
        any = true;
    }
    if any {
        Some(out)
    } else {
        None
    }
}

/// All change lines of the diff (used for whole-file operations in tests).
pub fn all_change_lines(fd: &FileDiff) -> HashSet<LineId> {
    let mut s = HashSet::new();
    for (hi, h) in fd.hunks.iter().enumerate() {
        for (li, l) in h.lines.iter().enumerate() {
            if matches!(l.kind, LK::Add | LK::Del) {
                s.insert((hi, li));
            }
        }
    }
    s
}

// ---------------------------------------------------------------- conflicts

#[derive(Clone, Debug)]
pub struct Conflict {
    pub start: usize,
    pub base: Option<usize>,
    pub mid: usize,
    pub end: usize,
}

pub fn parse_conflicts(lines: &[&str]) -> Vec<Conflict> {
    let mut v = Vec::new();
    let mut start = None;
    let mut base = None;
    let mut mid = None;
    for (i, l) in lines.iter().enumerate() {
        let l = l.trim_end_matches('\r');
        if l.starts_with("<<<<<<<") {
            start = Some(i);
            base = None;
            mid = None;
        } else if l.starts_with("|||||||") && start.is_some() && mid.is_none() {
            base = Some(i);
        } else if l == "=======" && start.is_some() && mid.is_none() {
            mid = Some(i);
        } else if l.starts_with(">>>>>>>") && mid.is_some() {
            v.push(Conflict { start: start.unwrap(), base, mid: mid.unwrap(), end: i });
            start = None;
            mid = None;
        }
    }
    v
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Accept {
    Current,
    Incoming,
    Both,
}

/// Resolves the conflict block starting at `start_line` in `text`.
pub fn resolve_conflict(text: &str, start_line: usize, accept: Accept) -> String {
    let lines: Vec<&str> = text.split_inclusive('\n').collect();
    let plain: Vec<&str> = lines.iter().map(|l| l.trim_end_matches('\n')).collect();
    let Some(c) = parse_conflicts(&plain).into_iter().find(|c| c.start == start_line) else { return text.to_string() };
    let cur_end = c.base.unwrap_or(c.mid);
    let mut out = String::new();
    for l in &lines[..c.start] {
        out.push_str(l);
    }
    if matches!(accept, Accept::Current | Accept::Both) {
        for l in &lines[c.start + 1..cur_end] {
            out.push_str(l);
        }
    }
    if matches!(accept, Accept::Incoming | Accept::Both) {
        for l in &lines[c.mid + 1..c.end] {
            out.push_str(l);
        }
    }
    for l in &lines[c.end + 1..] {
        out.push_str(l);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const D: &str = "diff --git a/f.txt b/f.txt\nindex 1..2 100644\n--- a/f.txt\n+++ b/f.txt\n@@ -1,4 +1,4 @@\n a\n-b\n+B\n c\n-d\n+D\n";

    #[test]
    fn parses() {
        let fd = parse(D);
        assert_eq!(fd.hunks.len(), 1);
        assert_eq!(fd.hunks[0].lines.len(), 6);
        assert_eq!(fd.hunks[0].lines[2].new, 2);
    }

    #[test]
    fn rows_side_by_side() {
        let fd = parse(D);
        let (rows, blocks) = build_rows(&fd, false);
        assert_eq!(blocks.len(), 2);
        assert_eq!(rows.len(), 4);
        assert_eq!(rows[1].left, Some((0, 1)));
        assert_eq!(rows[1].right, Some((0, 2)));
    }

    #[test]
    fn forward_partial_patch() {
        let fd = parse(D);
        let sel: HashSet<LineId> = [(0, 1), (0, 2)].into_iter().collect();
        let p = build_patch(&fd, &sel, false).unwrap();
        assert!(p.contains("@@ -1,4 +1,4 @@\n a\n-b\n+B\n c\n d\n"), "{p}");
    }

    #[test]
    fn reverse_partial_patch() {
        let fd = parse(D);
        let sel: HashSet<LineId> = [(0, 4), (0, 5)].into_iter().collect();
        let p = build_patch(&fd, &sel, true).unwrap();
        assert!(p.contains("@@ -1,4 +1,4 @@\n a\n B\n c\n-d\n+D\n"), "{p}");
    }

    #[test]
    fn conflicts() {
        let t = "x\n<<<<<<< HEAD\nours\n=======\ntheirs\n>>>>>>> b\ny\n";
        assert_eq!(resolve_conflict(t, 1, Accept::Current), "x\nours\ny\n");
        assert_eq!(resolve_conflict(t, 1, Accept::Incoming), "x\ntheirs\ny\n");
        assert_eq!(resolve_conflict(t, 1, Accept::Both), "x\nours\ntheirs\ny\n");
    }
}
