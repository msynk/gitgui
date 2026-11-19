//! Tiny key=value settings file in %APPDATA%\GitGui\settings.ini.

use std::path::PathBuf;

#[derive(Clone, Debug)]
pub struct Settings {
    pub dark: bool,
    pub inline_diff: bool,
    pub compact_diff: bool,
    pub sidebar_w: i32,
    /// Activity bar and side bar on the right edge (VS Code "Toggle Primary Side Bar Position").
    pub sidebar_right: bool,
    /// Primary side bar shown (VS Code "Toggle Primary Side Bar Visibility", Ctrl+B).
    pub sidebar_visible: bool,
    /// Show the Changes list as a folder tree (VS Code "View as Tree").
    pub tree_view: bool,
    pub graph_split: f32,
    /// Branches section expanded, and its list height (unscaled pixels).
    pub branches_open: bool,
    /// Graph section expanded.
    pub graph_open: bool,
    /// Bottom panel size (`PanelMode` as u8) and the maximized mode the toggle button restores.
    pub panel_mode: u8,
    pub panel_max: u8,
    pub branches_h: i32,
    pub output_h: i32,
    pub output_visible: bool,
    pub graph_all: bool,
    pub smart_commit: Option<bool>,
    pub confirm_sync: bool,
    pub recent: Vec<String>,
    pub panel_terminal: bool,
    pub terminal_shell: String,
    pub window: Option<(i32, i32, i32, i32, bool)>,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            dark: true,
            inline_diff: false,
            compact_diff: false,
            sidebar_w: 320,
            sidebar_right: false,
            sidebar_visible: true,
            tree_view: false,
            graph_split: 0.55,
            branches_open: true,
            graph_open: true,
            panel_mode: 0,
            panel_max: 2,
            branches_h: 140,
            output_h: 180,
            output_visible: false,
            graph_all: false,
            smart_commit: None,
            confirm_sync: true,
            recent: Vec::new(),
            panel_terminal: false,
            terminal_shell: String::new(),
            window: None,
        }
    }
}

fn path() -> PathBuf {
    let base = std::env::var("APPDATA").unwrap_or_else(|_| ".".into());
    PathBuf::from(base).join("GitGui").join("settings.ini")
}

impl Settings {
    pub fn load() -> Settings {
        let mut s = Settings::default();
        let Ok(text) = std::fs::read_to_string(path()) else { return s };
        for line in text.lines() {
            let Some((k, v)) = line.split_once('=') else { continue };
            let b = v == "1" || v == "true";
            match k.trim() {
                "dark" => s.dark = b,
                "inline_diff" => s.inline_diff = b,
                "compact_diff" => s.compact_diff = b,
                "sidebar_w" => s.sidebar_w = v.parse().unwrap_or(320).clamp(180, 2000),
                "sidebar_right" => s.sidebar_right = b,
                "sidebar_visible" => s.sidebar_visible = b,
                "tree_view" => s.tree_view = b,
                "graph_split" => s.graph_split = v.parse().unwrap_or(0.55f32).clamp(0.1, 0.9),
                "branches_open" => s.branches_open = b,
                "graph_open" => s.graph_open = b,
                "panel_mode" => s.panel_mode = v.parse().unwrap_or(0).min(3),
                "panel_max" => s.panel_max = v.parse().unwrap_or(2).clamp(1, 3),
                "branches_h" => s.branches_h = v.parse().unwrap_or(140).clamp(44, 2000),
                "output_h" => s.output_h = v.parse().unwrap_or(180).clamp(60, 2000),
                "output_visible" => s.output_visible = b,
                "graph_all" => s.graph_all = b,
                "smart_commit" => s.smart_commit = if v.is_empty() { None } else { Some(b) },
                "confirm_sync" => s.confirm_sync = b,
                "panel_terminal" => s.panel_terminal = b,
                "terminal_shell" => s.terminal_shell = v.to_string(),
                "recent" => s.recent = v.split('|').filter(|x| !x.is_empty()).map(String::from).collect(),
                "window" => {
                    let p: Vec<i32> = v.split(',').filter_map(|x| x.parse().ok()).collect();
                    if p.len() == 5 {
                        s.window = Some((p[0], p[1], p[2], p[3], p[4] != 0));
                    }
                }
                _ => {}
            }
        }
        s
    }

    pub fn save(&self) {
        let p = path();
        if let Some(d) = p.parent() {
            let _ = std::fs::create_dir_all(d);
        }
        let b = |x: bool| if x { "1" } else { "0" };
        let mut out = String::new();
        out += &format!("dark={}\n", b(self.dark));
        out += &format!("inline_diff={}\n", b(self.inline_diff));
        out += &format!("compact_diff={}\n", b(self.compact_diff));
        out += &format!("sidebar_w={}\n", self.sidebar_w);
        out += &format!("sidebar_right={}\n", b(self.sidebar_right));
        out += &format!("sidebar_visible={}\n", b(self.sidebar_visible));
        out += &format!("tree_view={}\n", b(self.tree_view));
        out += &format!("graph_split={}\n", self.graph_split);
        out += &format!("branches_open={}\n", b(self.branches_open));
        out += &format!("graph_open={}\n", b(self.graph_open));
        out += &format!("panel_mode={}\n", self.panel_mode);
        out += &format!("panel_max={}\n", self.panel_max);
        out += &format!("branches_h={}\n", self.branches_h);
        out += &format!("output_h={}\n", self.output_h);
        out += &format!("output_visible={}\n", b(self.output_visible));
        out += &format!("graph_all={}\n", b(self.graph_all));
        out += &format!("smart_commit={}\n", self.smart_commit.map(b).unwrap_or(""));
        out += &format!("confirm_sync={}\n", b(self.confirm_sync));
        out += &format!("panel_terminal={}\n", b(self.panel_terminal));
        out += &format!("terminal_shell={}\n", self.terminal_shell);
        out += &format!("recent={}\n", self.recent.join("|"));
        if let Some((x, y, w, h, m)) = self.window {
            out += &format!("window={x},{y},{w},{h},{}\n", if m { 1 } else { 0 });
        }
        let _ = std::fs::write(p, out);
    }

    pub fn add_recent(&mut self, folder: &str) {
        self.recent.retain(|r| !r.eq_ignore_ascii_case(folder));
        self.recent.insert(0, folder.to_string());
        self.recent.truncate(15);
    }
}
