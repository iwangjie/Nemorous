use nsh_core::prelude::*;

/// 精准复刻旧版 `theme.go` 的四主题色板（light / dark / darcula / system）。
#[derive(Debug, Clone, Copy)]
pub struct Palette {
    pub is_dark: bool,
    pub app_bg: Hsla,
    pub rail_bg: Hsla,
    pub surface: Hsla,
    pub surface2: Hsla,
    pub surface3: Hsla,
    pub surface_hover: Hsla,
    pub active_row_bg: Hsla,
    pub border: Hsla,
    pub border_strong: Hsla,

    pub text_primary: Hsla,
    pub text_secondary: Hsla,
    pub text_muted: Hsla,
    pub text_control: Hsla,

    pub accent: Hsla,
    pub accent_hover: Hsla,
    pub accent_soft: Hsla,
    pub accent_soft_text: Hsla,
    pub accent_text: Hsla,

    pub green: Hsla,
    pub green_soft: Hsla,
    pub amber: Hsla,
    pub amber_soft: Hsla,
    pub danger: Hsla,
    pub danger_soft: Hsla,
    pub blue: Hsla,
    pub blue_soft: Hsla,
    pub purple: Hsla,
    pub purple_soft: Hsla,

    // Diff 专属高亮色（行背景与行内强调）
    pub diff_add_line: Hsla,
    pub diff_add_inline: Hsla,
    pub diff_del_line: Hsla,
    pub diff_del_inline: Hsla,
    pub diff_mod_line: Hsla,
    pub diff_gutter_add: Hsla,
    pub diff_gutter_del: Hsla,
    pub diff_gutter_mod: Hsla,
}

#[derive(Debug, Clone, Copy)]
pub struct RepoBadgeColors {
    pub bg: Hsla,
    pub text: Hsla,
}

fn hex(v: u32) -> Hsla {
    rgb(v).into()
}

fn hex_a(v: u32, alpha: f32) -> Hsla {
    let mut c: Hsla = rgb(v).into();
    c.a = alpha;
    c
}

pub fn light_palette() -> Palette {
    Palette {
        is_dark: false,
        app_bg: hex(0xF4F4F6),
        rail_bg: hex(0xF9F9FB),
        surface: hex(0xFFFFFF),
        surface2: hex(0xF1F1F4),
        surface3: hex(0xE6E6EB),
        surface_hover: hex(0xE9E9EE),
        active_row_bg: hex(0xECEBFB),
        border: hex(0xE2E2E8),
        border_strong: hex(0xCCCCD4),

        text_primary: hex(0x18181B),
        text_secondary: hex(0x52525B),
        text_muted: hex(0x71717A),
        text_control: hex(0x3F3F46),

        accent: hex(0x4F46E5),
        accent_hover: hex(0x4338CA),
        accent_soft: hex(0xEEEDFD),
        accent_soft_text: hex(0x3730A3),
        accent_text: hex(0xFFFFFF),

        green: hex(0x0F7A45),
        green_soft: hex(0xE7F7EE),
        amber: hex(0xB45309),
        amber_soft: hex(0xFEF3C7),
        danger: hex(0xC43D3D),
        danger_soft: hex(0xFEECEC),
        blue: hex(0x0369A1),
        blue_soft: hex(0xE0F2FE),
        purple: hex(0x7C3AED),
        purple_soft: hex(0xF3E8FF),

        diff_add_line: hex_a(0x22C55E, 0.14),
        diff_add_inline: hex_a(0x16A34A, 0.28),
        diff_del_line: hex_a(0xEF4444, 0.14),
        diff_del_inline: hex_a(0xDC2626, 0.28),
        diff_mod_line: hex_a(0x3B82F6, 0.12),
        diff_gutter_add: hex(0x16A34A),
        diff_gutter_del: hex(0xDC2626),
        diff_gutter_mod: hex(0x2563EB),
    }
}

pub fn dark_palette() -> Palette {
    Palette {
        is_dark: true,
        app_bg: hex(0x141519),
        rail_bg: hex(0x181A20),
        surface: hex(0x1D2027),
        surface2: hex(0x252831),
        surface3: hex(0x2F333F),
        surface_hover: hex(0x2A2E39),
        active_row_bg: hex(0x2A2D46),
        border: hex(0x2D313D),
        border_strong: hex(0x434857),

        text_primary: hex(0xF4F4F6),
        text_secondary: hex(0xB4B8C4),
        text_muted: hex(0x888D9B),
        text_control: hex(0xD4D7E0),

        accent: hex(0x6366F1),
        accent_hover: hex(0x818CF8),
        accent_soft: hex(0x272A4D),
        accent_soft_text: hex(0xC7D2FE),
        accent_text: hex(0xFFFFFF),

        green: hex(0x34D399),
        green_soft: hex(0x16382C),
        amber: hex(0xFBBF24),
        amber_soft: hex(0x3D2E14),
        danger: hex(0xF87171),
        danger_soft: hex(0x3F1D21),
        blue: hex(0x38BDF8),
        blue_soft: hex(0x163247),
        purple: hex(0xC084FC),
        purple_soft: hex(0x301F47),

        diff_add_line: hex_a(0x22C55E, 0.16),
        diff_add_inline: hex_a(0x4ADE80, 0.30),
        diff_del_line: hex_a(0xEF4444, 0.17),
        diff_del_inline: hex_a(0xF87171, 0.32),
        diff_mod_line: hex_a(0x60A5FA, 0.14),
        diff_gutter_add: hex(0x34D399),
        diff_gutter_del: hex(0xF87171),
        diff_gutter_mod: hex(0x60A5FA),
    }
}

pub fn darcula_palette() -> Palette {
    Palette {
        is_dark: true,
        app_bg: hex(0x2B2B2B),
        rail_bg: hex(0x313335),
        surface: hex(0x2B2B2B),
        surface2: hex(0x3C3F41),
        surface3: hex(0x4E5254),
        surface_hover: hex(0x434648),
        active_row_bg: hex(0x2F65CA),
        border: hex(0x3F4244),
        border_strong: hex(0x55585A),

        text_primary: hex(0xBBBBBB),
        text_secondary: hex(0xA9B7C6),
        text_muted: hex(0x808080),
        text_control: hex(0xD0D0D0),

        accent: hex(0x4A88C7),
        accent_hover: hex(0x589DF6),
        accent_soft: hex(0x2D445E),
        accent_soft_text: hex(0xA7CEF7),
        accent_text: hex(0xFFFFFF),

        green: hex(0x629755),
        green_soft: hex(0x2B3B28),
        amber: hex(0xCC7832),
        amber_soft: hex(0x3D2E1D),
        danger: hex(0xBC3F3C),
        danger_soft: hex(0x442726),
        blue: hex(0x6897BB),
        blue_soft: hex(0x263847),
        purple: hex(0x9876AA),
        purple_soft: hex(0x372B40),

        diff_add_line: hex_a(0x629755, 0.22),
        diff_add_inline: hex_a(0x6A8759, 0.42),
        diff_del_line: hex_a(0xBC3F3C, 0.24),
        diff_del_inline: hex_a(0xCC666E, 0.42),
        diff_mod_line: hex_a(0x6897BB, 0.20),
        diff_gutter_add: hex(0x629755),
        diff_gutter_del: hex(0xBC3F3C),
        diff_gutter_mod: hex(0x6897BB),
    }
}

pub fn palette_for(theme_name: &str, system_dark: bool) -> Palette {
    match theme_name {
        "dark" => dark_palette(),
        "darcula" => darcula_palette(),
        "system" => {
            if system_dark {
                dark_palette()
            } else {
                light_palette()
            }
        }
        _ => light_palette(),
    }
}

pub fn next_theme(current: &str) -> &'static str {
    match current {
        "light" => "dark",
        "dark" => "darcula",
        "darcula" => "system",
        _ => "light",
    }
}

/// VCS 文件状态色（与旧版 `statusTextColor` 逐一对齐）。
pub fn status_text_color(kind: &str, p: Palette) -> Hsla {
    match kind {
        "A" | "C" => p.green,
        "M" => p.blue,
        "D" => p.text_muted,
        "R" => p.purple,
        "U" | "?" => p.danger,
        "T" => p.amber,
        _ => p.text_primary,
    }
}

pub fn repo_badge_colors(color_idx: usize, is_dark: bool) -> RepoBadgeColors {
    let light = [
        (0xE0E7FF, 0x3730A3),
        (0xDCFCE7, 0x166534),
        (0xFEF3C7, 0x92400E),
        (0xF3E8FF, 0x6B21A8),
        (0xE0F2FE, 0x075985),
        (0xFFE4E6, 0x9F1239),
    ];
    let dark = [
        (0x272A4D, 0xA5B4FC),
        (0x16382C, 0x6EE7B7),
        (0x3D2E14, 0xFCD34D),
        (0x301F47, 0xD8B4FE),
        (0x163247, 0x7DD3FC),
        (0x451A25, 0xFDA4AF),
    ];
    let set = if is_dark { dark } else { light };
    let (bg, fg) = set[color_idx % set.len()];
    RepoBadgeColors {
        bg: hex(bg),
        text: hex(fg),
    }
}
