//! Color themes (VS Code "Dark Modern" / "Light Modern"). Colors are COLORREF (0x00BBGGRR).

pub const fn hex(rgb: u32) -> u32 {
    ((rgb & 0xFF) << 16) | (rgb & 0xFF00) | ((rgb >> 16) & 0xFF)
}

/// Alpha-blends `fg` (0xRRGGBB) over `bg` (0xRRGGBB); returns COLORREF.
pub fn blend(bg: u32, fg: u32, alpha: f32) -> u32 {
    let c = |shift: u32| -> u32 {
        let b = ((bg >> shift) & 0xFF) as f32;
        let f = ((fg >> shift) & 0xFF) as f32;
        (b + (f - b) * alpha).round() as u32
    };
    hex((c(16) << 16) | (c(8) << 8) | c(0))
}

#[derive(Clone)]
pub struct Theme {
    pub dark: bool,
    pub title: u32,
    pub sidebar: u32,
    pub editor: u32,
    pub border: u32,
    pub fg: u32,
    pub fg_dim: u32,
    pub fg_strong: u32,
    pub accent: u32,
    pub accent_hover: u32,
    pub accent_fg: u32,
    pub hover: u32,
    pub sel: u32,
    pub sel_inactive: u32,
    pub focus: u32,
    pub input_bg: u32,
    pub input_border: u32,
    pub badge_bg: u32,
    pub badge_fg: u32,
    pub git_modified: u32,
    pub git_added: u32,
    pub git_deleted: u32,
    pub git_untracked: u32,
    pub git_conflict: u32,
    pub ins_line: u32,
    pub del_line: u32,
    pub ins_text: u32,
    pub del_text: u32,
    pub ins_gutter: u32,
    pub del_gutter: u32,
    pub empty_line: u32,
    pub line_no: u32,
    pub scroll_thumb: u32,
    pub scroll_thumb_hover: u32,
    pub widget_bg: u32,
    pub widget_border: u32,
    pub link: u32,
    pub warning: u32,
    pub error: u32,
    pub info: u32,
    pub lanes: [u32; 5],
    pub ref_color: u32,
    pub remote_color: u32,
    pub label_fg: u32,
    pub merge_current: u32,
    pub merge_current_header: u32,
    pub merge_incoming: u32,
    pub merge_incoming_header: u32,
    pub highlight: u32,
    pub button2: u32,
    pub button2_hover: u32,
    pub toolbar_hover: u32,
}

impl Theme {
    pub fn dark() -> Theme {
        let editor = 0x1F1F1F;
        Theme {
            dark: true,
            title: hex(0x181818),
            sidebar: hex(0x181818),
            editor: hex(editor),
            border: hex(0x2B2B2B),
            fg: hex(0xCCCCCC),
            fg_dim: hex(0x9D9D9D),
            fg_strong: hex(0xFFFFFF),
            accent: hex(0x0078D4),
            accent_hover: hex(0x026EC1),
            accent_fg: hex(0xFFFFFF),
            hover: hex(0x2A2D2E),
            sel: hex(0x04395E),
            sel_inactive: hex(0x37373D),
            focus: hex(0x0078D4),
            input_bg: hex(0x313131),
            input_border: hex(0x3C3C3C),
            badge_bg: hex(0x616161),
            badge_fg: hex(0xF8F8F8),
            git_modified: hex(0xE2C08D),
            git_added: hex(0x81B88B),
            git_deleted: hex(0xC74E39),
            git_untracked: hex(0x73C991),
            git_conflict: hex(0xE4676B),
            ins_line: blend(editor, 0x9BB955, 0.2),
            del_line: blend(editor, 0xFF0000, 0.2),
            ins_text: blend(editor, 0x9CCC2C, 0.35),
            del_text: blend(editor, 0xFF0000, 0.35),
            ins_gutter: hex(0x2EA043),
            del_gutter: hex(0xF85149),
            empty_line: blend(editor, 0xCCCCCC, 0.04),
            line_no: hex(0x6E7681),
            scroll_thumb: blend(editor, 0x797979, 0.4),
            scroll_thumb_hover: blend(editor, 0x646464, 0.7),
            widget_bg: hex(0x222222),
            widget_border: hex(0x454545),
            link: hex(0x4DAAFC),
            warning: hex(0xCCA700),
            error: hex(0xF14C4C),
            info: hex(0x3794FF),
            lanes: [hex(0xFFB000), hex(0xDC267F), hex(0x994F00), hex(0x40B0A6), hex(0xB66DFF)],
            ref_color: hex(0x59A4F9),
            remote_color: hex(0xB180D7),
            label_fg: hex(0xFFFFFF),
            merge_current: blend(editor, 0x40C8AE, 0.2),
            merge_current_header: blend(editor, 0x40C8AE, 0.5),
            merge_incoming: blend(editor, 0x40A6FF, 0.2),
            merge_incoming_header: blend(editor, 0x40A6FF, 0.5),
            highlight: hex(0x2AAAFF),
            button2: hex(0x313131),
            button2_hover: hex(0x3C3C3C),
            toolbar_hover: blend(0x181818, 0x5A5D5E, 0.31),
        }
    }

    pub fn light() -> Theme {
        let editor = 0xFFFFFF;
        Theme {
            dark: false,
            title: hex(0xF8F8F8),
            sidebar: hex(0xF8F8F8),
            editor: hex(editor),
            border: hex(0xE5E5E5),
            fg: hex(0x3B3B3B),
            fg_dim: hex(0x717171),
            fg_strong: hex(0x000000),
            accent: hex(0x005FB8),
            accent_hover: hex(0x0258A8),
            accent_fg: hex(0xFFFFFF),
            hover: hex(0xF2F2F2),
            sel: hex(0xE8E8E8),
            sel_inactive: hex(0xE4E6F1),
            focus: hex(0x005FB8),
            input_bg: hex(0xFFFFFF),
            input_border: hex(0xCECECE),
            badge_bg: hex(0xCCCCCC),
            badge_fg: hex(0x3B3B3B),
            git_modified: hex(0x895503),
            git_added: hex(0x587C0C),
            git_deleted: hex(0xAD0707),
            git_untracked: hex(0x007100),
            git_conflict: hex(0xAD0707),
            ins_line: blend(editor, 0x9BB955, 0.2),
            del_line: blend(editor, 0xFF0000, 0.12),
            ins_text: blend(editor, 0x9CCC2C, 0.4),
            del_text: blend(editor, 0xFF0000, 0.25),
            ins_gutter: hex(0x2EA043),
            del_gutter: hex(0xF85149),
            empty_line: blend(editor, 0x000000, 0.03),
            line_no: hex(0x6E7681),
            scroll_thumb: blend(editor, 0x646464, 0.35),
            scroll_thumb_hover: blend(editor, 0x646464, 0.6),
            widget_bg: hex(0xF8F8F8),
            widget_border: hex(0xCECECE),
            link: hex(0x005FB8),
            warning: hex(0xBF8803),
            error: hex(0xE51400),
            info: hex(0x1A85FF),
            lanes: [hex(0xFFB000), hex(0xDC267F), hex(0x994F00), hex(0x40B0A6), hex(0xB66DFF)],
            ref_color: hex(0x1A85FF),
            remote_color: hex(0x652D90),
            label_fg: hex(0xFFFFFF),
            merge_current: blend(editor, 0x40C8AE, 0.2),
            merge_current_header: blend(editor, 0x40C8AE, 0.5),
            merge_incoming: blend(editor, 0x40A6FF, 0.2),
            merge_incoming_header: blend(editor, 0x40A6FF, 0.5),
            highlight: hex(0x0066BF),
            button2: hex(0xE5E5E5),
            button2_hover: hex(0xCCCCCC),
            toolbar_hover: blend(0xF8F8F8, 0xB8B8B8, 0.31),
        }
    }

    pub fn lane(&self, c: u8) -> u32 {
        match c {
            crate::graph::COLOR_REF => self.ref_color,
            crate::graph::COLOR_REMOTE => self.remote_color,
            n => self.lanes[(n % 5) as usize],
        }
    }
}

/// Alpha-blends two COLORREF values.
pub fn blend_colorref(bg: u32, fg: u32, alpha: f32) -> u32 {
    let c = |shift: u32| -> u32 {
        let b = ((bg >> shift) & 0xFF) as f32;
        let f = ((fg >> shift) & 0xFF) as f32;
        (b + (f - b) * alpha).round().clamp(0.0, 255.0) as u32
    };
    c(0) | (c(8) << 8) | (c(16) << 16)
}
