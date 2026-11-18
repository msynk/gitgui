//! Commit graph swimlanes (same algorithm as VS Code's SCM graph).

use crate::git::{Commit, RefKind};

pub const COLOR_REF: u8 = 100; // current branch
pub const COLOR_REMOTE: u8 = 101; // its upstream
pub const LANE_COLORS: u8 = 5;

#[derive(Clone, Debug)]
pub struct Lane {
    pub id: String,
    pub color: u8,
}

#[derive(Clone, Debug)]
pub struct GraphRow {
    pub input: Vec<Lane>,
    pub output: Vec<Lane>,
    pub current: bool,
}

pub fn compute(commits: &[Commit], head_branch: Option<&str>, upstream: Option<&str>, head_sha: Option<&str>) -> Vec<GraphRow> {
    let label_color = |c: &Commit| -> Option<u8> {
        for r in &c.refs {
            if r.kind == RefKind::Head && Some(r.name.as_str()) == head_branch {
                return Some(COLOR_REF);
            }
            if r.kind == RefKind::Remote && Some(r.name.as_str()) == upstream {
                return Some(COLOR_REMOTE);
            }
        }
        None
    };
    let index: std::collections::HashMap<&str, &Commit> = commits.iter().map(|c| (c.hash.as_str(), c)).collect();
    let mut color_index: i32 = -1;
    let mut rows: Vec<GraphRow> = Vec::with_capacity(commits.len());
    for c in commits {
        let input: Vec<Lane> = rows.last().map(|r| r.output.clone()).unwrap_or_default();
        let mut output: Vec<Lane> = Vec::new();
        let mut first_added = false;
        if !c.parents.is_empty() {
            for node in &input {
                if node.id == c.hash {
                    if !first_added {
                        output.push(Lane { id: c.parents[0].clone(), color: label_color(c).unwrap_or(node.color) });
                        first_added = true;
                    }
                    continue;
                }
                output.push(node.clone());
            }
        }
        let start = if first_added { 1 } else { 0 };
        for i in start..c.parents.len() {
            let mut color = if i == 0 { label_color(c) } else { index.get(c.parents[i].as_str()).and_then(|p| label_color(p)) };
            if color.is_none() {
                color_index = (color_index + 1) % LANE_COLORS as i32;
                color = Some(color_index as u8);
            }
            output.push(Lane { id: c.parents[i].clone(), color: color.unwrap() });
        }
        rows.push(GraphRow { input, output, current: Some(c.hash.as_str()) == head_sha });
    }
    rows
}

/// Geometry primitives for one row, in lane units (x = lane index + 1, y in 0..1).
#[derive(Clone, Debug)]
pub enum Seg {
    /// vertical line at lane x from y0 to y1 (fractions of row height)
    V { x: f32, y0: f32, y1: f32, color: u8 },
    /// curve from (x0, y0) to (x1, y1)
    C { x0: f32, y0: f32, x1: f32, y1: f32, color: u8 },
    /// horizontal from x0 to x1 at y
    H { x0: f32, x1: f32, y: f32, color: u8 },
}

pub struct RowShape {
    pub segs: Vec<Seg>,
    pub circle_x: f32,
    pub circle_color: u8,
    pub merge: bool,
    pub width: usize,
}

pub fn shape(row: &GraphRow, commit: &Commit) -> RowShape {
    let mut segs = Vec::new();
    let inp = &row.input;
    let out = &row.output;
    let input_index = inp.iter().position(|n| n.id == commit.hash);
    let circle_index = input_index.unwrap_or(inp.len());
    let circle_color = if circle_index < out.len() {
        out[circle_index].color
    } else if circle_index < inp.len() {
        inp[circle_index].color
    } else {
        COLOR_REF
    };
    let mut oi = 0usize;
    for (index, node) in inp.iter().enumerate() {
        if node.id == commit.hash {
            if index != circle_index {
                segs.push(Seg::C { x0: index as f32 + 1.0, y0: 0.0, x1: index as f32, y1: 0.5, color: node.color });
                segs.push(Seg::H { x0: index as f32, x1: circle_index as f32 + 1.0, y: 0.5, color: node.color });
            } else {
                oi += 1;
            }
        } else if oi < out.len() && node.id == out[oi].id {
            if index == oi {
                segs.push(Seg::V { x: index as f32 + 1.0, y0: 0.0, y1: 1.0, color: node.color });
            } else {
                segs.push(Seg::C { x0: index as f32 + 1.0, y0: 0.0, x1: oi as f32 + 1.0, y1: 1.0, color: node.color });
            }
            oi += 1;
        }
    }
    for p in commit.parents.iter().skip(1) {
        if let Some(pi) = out.iter().rposition(|n| &n.id == p) {
            segs.push(Seg::H { x0: circle_index as f32 + 1.0, x1: pi as f32, y: 0.5, color: out[pi].color });
            segs.push(Seg::C { x0: pi as f32, y0: 0.5, x1: pi as f32 + 1.0, y1: 1.0, color: out[pi].color });
        }
    }
    if let Some(ii) = input_index {
        segs.push(Seg::V { x: circle_index as f32 + 1.0, y0: 0.0, y1: 0.5, color: inp[ii].color });
    }
    if !commit.parents.is_empty() {
        segs.push(Seg::V { x: circle_index as f32 + 1.0, y0: 0.5, y1: 1.0, color: circle_color });
    }
    RowShape { segs, circle_x: circle_index as f32 + 1.0, circle_color, merge: commit.parents.len() > 1, width: inp.len().max(out.len()).max(1) + 1 }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git::Decoration;

    fn c(h: &str, p: &[&str], refs: Vec<Decoration>) -> Commit {
        Commit { hash: h.into(), parents: p.iter().map(|s| s.to_string()).collect(), author: String::new(), email: String::new(), date: 0, message: String::new(), refs }
    }

    #[test]
    fn merge_lanes() {
        let commits = vec![
            c("m", &["a", "b"], vec![Decoration { kind: RefKind::Head, name: "main".into(), current: true }]),
            c("b", &["a"], vec![]),
            c("a", &[], vec![]),
        ];
        let rows = compute(&commits, Some("main"), None, Some("m"));
        assert_eq!(rows[0].output.iter().map(|l| l.id.as_str()).collect::<Vec<_>>(), vec!["a", "b"]);
        assert_eq!(rows[0].output[0].color, COLOR_REF);
        assert_eq!(rows[1].output.iter().map(|l| l.id.as_str()).collect::<Vec<_>>(), vec!["a", "a"]);
        assert!(rows[2].output.is_empty());
        assert!(rows[0].current);
    }
}
