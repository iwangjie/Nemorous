use std::ops::Range;

use imara_diff::{Algorithm, Diff, InternedInput};
use nsh_base::{
    EditorDecoration, EditorFoldRange, EditorGutterMark, EditorGutterPlacement,
    EditorOverviewRulerLane, EditorOverviewRulerMark, EditorPosition, EditorRange,
};
use nsh_core::element::Style;
use nsh_core::prelude::*;
use nsh_core::{PropertyValue, StyleKey};

use crate::paths::byte_offset_to_utf16_col;
use crate::theme::Palette;

fn bg_style(color: Hsla) -> Style {
    Style::default().with(
        StyleKey::BackgroundColor,
        PropertyValue::Color(Some(color.into())),
    )
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum WhitespaceMode {
    #[default]
    None,
    Trim,
    All,
    AllAndEmptyLines,
}

impl WhitespaceMode {
    pub fn from_config(s: &str) -> Self {
        match s {
            "trim" | "trim_whitespace" => Self::Trim,
            "all" | "whitespace" | "all_whitespace" => Self::All,
            "all_and_empty" | "whitespace_and_blank" | "blank_lines" => Self::AllAndEmptyLines,
            _ => Self::None,
        }
    }

    pub fn as_config(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Trim => "trim",
            Self::All => "all",
            Self::AllAndEmptyLines => "all_and_empty",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::None => "不忽略空白",
            Self::Trim => "忽略首尾空白",
            Self::All => "忽略空白",
            Self::AllAndEmptyLines => "忽略空白和空行",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum InlineHighlightMode {
    #[default]
    Word,
    Char,
    None,
}

impl InlineHighlightMode {
    pub fn from_config(s: &str) -> Self {
        match s {
            "char" => Self::Char,
            "none" => Self::None,
            _ => Self::Word,
        }
    }

    pub fn as_config(self) -> &'static str {
        match self {
            Self::Word => "word",
            Self::Char => "char",
            Self::None => "none",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Word => "按词",
            Self::Char => "按字符",
            Self::None => "关闭",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiffSide {
    Old,
    New,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HunkKind {
    Insert,
    Delete,
    Modify,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiffHunk {
    /// 0-based 半开区间 `[old_start, old_end)`
    pub old_start: usize,
    pub old_end: usize,
    /// 0-based 半开区间 `[new_start, new_end)`
    pub new_start: usize,
    pub new_end: usize,
    pub kind: HunkKind,
}

impl DiffHunk {
    pub(crate) fn line_range(&self, side: DiffSide) -> Range<usize> {
        match side {
            DiffSide::Old => self.old_start..self.old_end,
            DiffSide::New => self.new_start..self.new_end,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ComputedDiff {
    pub hunks: Vec<DiffHunk>,
    pub old_line_count: usize,
    pub new_line_count: usize,
    pub added_lines: usize,
    pub deleted_lines: usize,
}

pub(crate) struct HunkNavigation {
    pub(crate) current: Option<usize>,
    pub(crate) previous: Option<usize>,
    pub(crate) next: Option<usize>,
}

impl HunkNavigation {
    pub(crate) fn counter_index(&self) -> usize {
        self.current.or(self.previous).or(self.next).unwrap_or(0)
    }
}

impl ComputedDiff {
    pub(crate) fn split_hunk_position(&self, hunk: &DiffHunk, side: DiffSide, text: &str) -> EditorPosition {
        let range = hunk.line_range(side);
        let lines = match side {
            DiffSide::Old => self.old_line_count,
            DiffSide::New => self.new_line_count,
        };
        // An empty range after an unterminated line lands at the actual end of text.
        if range.is_empty() && range.start == lines && lines > 0 && !text.ends_with('\n') {
            let last_line = text.rsplit_once('\n').map_or(text, |(_, last)| last);
            EditorPosition::new((lines - 1) as u32, last_line.encode_utf16().count() as u32)
        } else {
            EditorPosition::new(range.start as u32, 0)
        }
    }

    pub(crate) fn hunk_navigation(
        &self,
        side: DiffSide,
        caret: EditorPosition,
        split_text: Option<&str>,
    ) -> HunkNavigation {
        let line = caret.line as usize;
        let next = self.hunks.partition_point(|hunk| {
            let start = hunk.line_range(side).start;
            start <= line || (start == line + 1 && split_text.is_some_and(|text| {
                self.split_hunk_position(hunk, side, text) <= caret
            }))
        });
        let at_or_before = next.checked_sub(1);
        let current = at_or_before.filter(|&index| {
            let hunk = &self.hunks[index];
            let range = hunk.line_range(side);
            // Only the split viewer represents an empty range on this side.
            range.contains(&line) || (range.is_empty() && split_text.is_some_and(|text| {
                self.split_hunk_position(hunk, side, text).line == caret.line
            }))
        });
        HunkNavigation {
            current,
            previous: match current {
                Some(index) => index.checked_sub(1),
                None => at_or_before,
            },
            next: (next < self.hunks.len()).then_some(next),
        }
    }
}

/// 按逻辑行拆分文本，去掉恰好一个行尾终止换行符（`\n` 或 `\r\n`），
/// 避免单行 `"untracked\n"` 被误判为 2 行，同时保留真实的末尾空行（`"a\n\n"` 为 2 行）。
fn split_lines_keep_logical(text: &str) -> Vec<&str> {
    if text.is_empty() {
        return Vec::new();
    }
    let body = text
        .strip_suffix("\r\n")
        .or_else(|| text.strip_suffix('\n'))
        .unwrap_or(text);
    body.split('\n').collect()
}

pub fn logical_line_count(text: &str) -> usize {
    split_lines_keep_logical(text).len()
}

fn normalize_line(line: &str, mode: WhitespaceMode) -> String {
    let line = line.strip_suffix('\r').unwrap_or(line);
    match mode {
        WhitespaceMode::None => line.to_string(),
        WhitespaceMode::Trim => line.trim().to_string(),
        WhitespaceMode::All | WhitespaceMode::AllAndEmptyLines => {
            line.chars().filter(|c| !c.is_whitespace()).collect()
        }
    }
}

/// 计算两份文本（逻辑行 `\n` 分隔）的 Diff Hunk 列表，支持 4 级空白忽略。
pub fn compute_diff(old_text: &str, new_text: &str, ws_mode: WhitespaceMode) -> ComputedDiff {
    let old_raw = split_lines_keep_logical(old_text);
    let new_raw = split_lines_keep_logical(new_text);

    if ws_mode == WhitespaceMode::None {
        let hunks = crate::selected_changes::exact_hunks(old_text.as_bytes(), new_text.as_bytes());
        return ComputedDiff {
            old_line_count: old_raw.len(),
            new_line_count: new_raw.len(),
            added_lines: hunks.iter().map(|h| h.new_end - h.new_start).sum(),
            deleted_lines: hunks.iter().map(|h| h.old_end - h.old_start).sum(),
            hunks,
        };
    }

    if ws_mode == WhitespaceMode::AllAndEmptyLines {
        return compute_diff_ignoring_empty_lines(&old_raw, &new_raw);
    }

    let old_norm: Vec<String> = old_raw
        .iter()
        .map(|l| normalize_line(l, ws_mode))
        .collect();
    let new_norm: Vec<String> = new_raw
        .iter()
        .map(|l| normalize_line(l, ws_mode))
        .collect();

    let old_joined = old_norm
        .iter()
        .map(|s| format!("{s}\n"))
        .collect::<String>();
    let new_joined = new_norm
        .iter()
        .map(|s| format!("{s}\n"))
        .collect::<String>();

    let input = InternedInput::new(old_joined.as_str(), new_joined.as_str());
    let mut diff = Diff::compute(Algorithm::Histogram, &input);
    diff.postprocess_lines(&input);

    let mut hunks = Vec::new();
    let mut added_lines = 0usize;
    let mut deleted_lines = 0usize;

    for hunk in diff.hunks() {
        let old_start = hunk.before.start as usize;
        let old_end = hunk.before.end as usize;
        let new_start = hunk.after.start as usize;
        let new_end = hunk.after.end as usize;
        let kind = match (old_end > old_start, new_end > new_start) {
            (false, true) => HunkKind::Insert,
            (true, false) => HunkKind::Delete,
            _ => HunkKind::Modify,
        };
        deleted_lines += old_end - old_start;
        added_lines += new_end - new_start;
        hunks.push(DiffHunk {
            old_start,
            old_end,
            new_start,
            new_end,
            kind,
        });
    }

    ComputedDiff {
        hunks,
        old_line_count: old_raw.len(),
        new_line_count: new_raw.len(),
        added_lines,
        deleted_lines,
    }
}

fn compute_diff_ignoring_empty_lines(old_raw: &[&str], new_raw: &[&str]) -> ComputedDiff {
    let mut old_map = Vec::new();
    let mut old_filtered = String::new();
    for (idx, line) in old_raw.iter().enumerate() {
        let norm = normalize_line(line, WhitespaceMode::AllAndEmptyLines);
        if !norm.is_empty() {
            old_map.push(idx);
            old_filtered.push_str(&norm);
            old_filtered.push('\n');
        }
    }

    let mut new_map = Vec::new();
    let mut new_filtered = String::new();
    for (idx, line) in new_raw.iter().enumerate() {
        let norm = normalize_line(line, WhitespaceMode::AllAndEmptyLines);
        if !norm.is_empty() {
            new_map.push(idx);
            new_filtered.push_str(&norm);
            new_filtered.push('\n');
        }
    }

    let input = InternedInput::new(old_filtered.as_str(), new_filtered.as_str());
    let mut diff = Diff::compute(Algorithm::Histogram, &input);
    diff.postprocess_lines(&input);

    let mut hunks = Vec::new();
    let mut added_lines = 0usize;
    let mut deleted_lines = 0usize;

    for hunk in diff.hunks() {
        let b_s = hunk.before.start as usize;
        let b_e = hunk.before.end as usize;
        let a_s = hunk.after.start as usize;
        let a_e = hunk.after.end as usize;

        let (old_start, old_end) = if b_e > b_s {
            (old_map[b_s], old_map[b_e - 1] + 1)
        } else {
            let anchor = if b_s < old_map.len() {
                old_map[b_s]
            } else {
                old_raw.len()
            };
            (anchor, anchor)
        };

        let (new_start, new_end) = if a_e > a_s {
            (new_map[a_s], new_map[a_e - 1] + 1)
        } else {
            let anchor = if a_s < new_map.len() {
                new_map[a_s]
            } else {
                new_raw.len()
            };
            (anchor, anchor)
        };

        let kind = match (old_end > old_start, new_end > new_start) {
            (false, true) => HunkKind::Insert,
            (true, false) => HunkKind::Delete,
            _ => HunkKind::Modify,
        };
        deleted_lines += old_end.saturating_sub(old_start);
        added_lines += new_end.saturating_sub(new_start);
        hunks.push(DiffHunk {
            old_start,
            old_end,
            new_start,
            new_end,
            kind,
        });
    }

    ComputedDiff {
        hunks,
        old_line_count: old_raw.len(),
        new_line_count: new_raw.len(),
        added_lines,
        deleted_lines,
    }
}

/// 将 Diff 中任一侧的光标位置（0-based 行与 UTF-16 列）映射到新侧真实源码位置。
/// 对齐旧版 `review_navigation.go` 的 `sourcePosition` 算法。
pub fn source_position(
    new_text: &str,
    hunks: &[DiffHunk],
    active_side: DiffSide,
    pos: EditorPosition,
) -> EditorPosition {
    let new_lines = split_lines_keep_logical(new_text);
    if active_side == DiffSide::New
        && new_text.ends_with('\n')
        && pos.line as usize == new_lines.len()
    {
        return EditorPosition::new(pos.line, 0);
    }
    if new_lines.is_empty() {
        return EditorPosition::new(0, 0);
    }

    let mapped_line = match active_side {
        DiffSide::New => pos.line as usize,
        DiffSide::Old => map_old_line_to_new(pos.line as usize, hunks),
    };

    let clamped_line = mapped_line.min(new_lines.len().saturating_sub(1));
    let line_utf16_len = new_lines[clamped_line].encode_utf16().count() as u32;
    let clamped_col = pos.column.min(line_utf16_len);
    EditorPosition::new(clamped_line as u32, clamped_col)
}

pub fn map_old_line_to_new(old_line: usize, hunks: &[DiffHunk]) -> usize {
    let mut delta: isize = 0;
    for h in hunks {
        if old_line < h.old_start {
            break;
        }
        if old_line < h.old_end {
            // 位于被删除/修改的块内：映射到新侧该块的对应位置或块起点
            let offset = old_line - h.old_start;
            let new_len = h.new_end.saturating_sub(h.new_start);
            if new_len == 0 {
                return h.new_start;
            }
            return h.new_start + offset.min(new_len - 1);
        }
        delta += (h.new_end as isize - h.new_start as isize)
            - (h.old_end as isize - h.old_start as isize);
    }
    (old_line as isize + delta).max(0) as usize
}

pub fn map_new_line_to_old(new_line: usize, hunks: &[DiffHunk]) -> usize {
    let mut delta: isize = 0;
    for h in hunks {
        if new_line < h.new_start {
            break;
        }
        if new_line < h.new_end {
            let offset = new_line - h.new_start;
            let old_len = h.old_end.saturating_sub(h.old_start);
            if old_len == 0 {
                return h.old_start;
            }
            return h.old_start + offset.min(old_len - 1);
        }
        delta += (h.old_end as isize - h.old_start as isize)
            - (h.new_end as isize - h.new_start as isize);
    }
    (new_line as isize + delta).max(0) as usize
}

/// 生成双栏左右编辑器的行背景、行内词/字符高亮、gutter 标记与 overview ruler 标记。
pub fn build_side_by_side_decorations(
    old_text: &str,
    new_text: &str,
    hunks: &[DiffHunk],
    inline_mode: InlineHighlightMode,
    p: Palette,
) -> (Vec<EditorDecoration>, Vec<EditorDecoration>) {
    let old_lines = split_lines_keep_logical(old_text);
    let new_lines = split_lines_keep_logical(new_text);

    let mut old_decs = Vec::new();
    let mut new_decs = Vec::new();

    for h in hunks {
        // 旧侧行高亮
        if h.old_end > h.old_start {
            let line_bg = if h.kind == HunkKind::Modify {
                p.diff_del_line
            } else {
                p.diff_del_line
            };
            let gutter_col = if h.kind == HunkKind::Modify {
                p.diff_gutter_mod
            } else {
                p.diff_gutter_del
            };
            for line in h.old_start..h.old_end {
                if line >= old_lines.len() {
                    break;
                }
                let mut dec = EditorDecoration::default();
                dec.range = EditorRange::new(
                    EditorPosition::new(line as u32, 0),
                    EditorPosition::new(line as u32, 0),
                );
                dec.line_style = Some(bg_style(line_bg));
                let mut gutter = EditorGutterMark::default();
                gutter.placement = EditorGutterPlacement::Lines;
                gutter.style = Some(bg_style(gutter_col));
                dec.gutter = Some(gutter);
                let mut ruler = EditorOverviewRulerMark::default();
                ruler.lane = EditorOverviewRulerLane::Left;
                ruler.style = Some(bg_style(gutter_col));
                dec.overview_ruler = Some(ruler);
                old_decs.push(dec);
            }
        }

        // 新侧行高亮
        if h.new_end > h.new_start {
            let line_bg = p.diff_add_line;
            let gutter_col = if h.kind == HunkKind::Modify {
                p.diff_gutter_mod
            } else {
                p.diff_gutter_add
            };
            for line in h.new_start..h.new_end {
                if line >= new_lines.len() {
                    break;
                }
                let mut dec = EditorDecoration::default();
                dec.range = EditorRange::new(
                    EditorPosition::new(line as u32, 0),
                    EditorPosition::new(line as u32, 0),
                );
                dec.line_style = Some(bg_style(line_bg));
                let mut gutter = EditorGutterMark::default();
                gutter.placement = EditorGutterPlacement::Lines;
                gutter.style = Some(bg_style(gutter_col));
                dec.gutter = Some(gutter);
                let mut ruler = EditorOverviewRulerMark::default();
                ruler.lane = EditorOverviewRulerLane::Right;
                ruler.style = Some(bg_style(gutter_col));
                dec.overview_ruler = Some(ruler);
                new_decs.push(dec);
            }
        }

        // 行内词级 / 字符级高亮（成对行）
        if inline_mode != InlineHighlightMode::None && h.kind == HunkKind::Modify {
            let pair_count = (h.old_end - h.old_start).min(h.new_end - h.new_start);
            for offset in 0..pair_count {
                let ol = h.old_start + offset;
                let nl = h.new_start + offset;
                if ol >= old_lines.len() || nl >= new_lines.len() {
                    break;
                }
                let (old_ranges, new_ranges) =
                    compute_inline_ranges(old_lines[ol], new_lines[nl], inline_mode);
                old_decs.extend(inline_decorations(ol, 0, old_ranges, p.diff_del_inline));
                new_decs.extend(inline_decorations(nl, 0, new_ranges, p.diff_add_inline));
            }
        }
    }

    (old_decs, new_decs)
}

fn inline_decorations(
    row: usize,
    prefix: u32,
    ranges: Vec<Range<u32>>,
    color: Hsla,
) -> impl Iterator<Item = EditorDecoration> {
    ranges.into_iter().map(move |span| {
        let mut decoration = EditorDecoration::default();
        decoration.range = EditorRange::new(
            EditorPosition::new(row as u32, prefix + span.start),
            EditorPosition::new(row as u32, prefix + span.end),
        );
        decoration.style = Some(bg_style(color));
        decoration
    })
}

/// 计算单行对比的 UTF-16 列高亮区段。
pub fn compute_inline_ranges(
    old_line: &str,
    new_line: &str,
    mode: InlineHighlightMode,
) -> (Vec<Range<u32>>, Vec<Range<u32>>) {
    if mode == InlineHighlightMode::None || old_line == new_line {
        return (Vec::new(), Vec::new());
    }
    // 极长行快速裁剪防 O(N^2)
    if old_line.len() + new_line.len() > 8192 {
        return (Vec::new(), Vec::new());
    }

    let old_tokens = tokenize_line(old_line, mode);
    let new_tokens = tokenize_line(new_line, mode);

    let old_str = old_tokens
        .iter()
        .map(|(tok, _)| format!("{tok}\n"))
        .collect::<String>();
    let new_str = new_tokens
        .iter()
        .map(|(tok, _)| format!("{tok}\n"))
        .collect::<String>();

    let input = InternedInput::new(old_str.as_str(), new_str.as_str());
    let diff = Diff::compute(Algorithm::Histogram, &input);

    let mut old_ranges = Vec::new();
    let mut new_ranges = Vec::new();
    for hunk in diff.hunks() {
        let bs = hunk.before.start as usize;
        let be = hunk.before.end as usize;
        if be > bs && be <= old_tokens.len() {
            let start_b = old_tokens[bs].1.start;
            let end_b = old_tokens[be - 1].1.end;
            let s16 = byte_offset_to_utf16_col(old_line, start_b);
            let e16 = byte_offset_to_utf16_col(old_line, end_b);
            if e16 > s16 {
                old_ranges.push(s16..e16);
            }
        }
        let as_ = hunk.after.start as usize;
        let ae = hunk.after.end as usize;
        if ae > as_ && ae <= new_tokens.len() {
            let start_b = new_tokens[as_].1.start;
            let end_b = new_tokens[ae - 1].1.end;
            let s16 = byte_offset_to_utf16_col(new_line, start_b);
            let e16 = byte_offset_to_utf16_col(new_line, end_b);
            if e16 > s16 {
                new_ranges.push(s16..e16);
            }
        }
    }
    (old_ranges, new_ranges)
}

fn tokenize_line<'a>(line: &'a str, mode: InlineHighlightMode) -> Vec<(&'a str, Range<usize>)> {
    let mut out = Vec::new();
    match mode {
        InlineHighlightMode::Char => {
            for (idx, ch) in line.char_indices() {
                let end = idx + ch.len_utf8();
                out.push((&line[idx..end], idx..end));
            }
        }
        InlineHighlightMode::Word => {
            let mut start = None;
            let mut kind = 0u8; // 1=word, 2=ws, 3=punct
            for (idx, ch) in line.char_indices() {
                let ck = if ch.is_alphanumeric() || ch == '_' {
                    1
                } else if ch.is_whitespace() {
                    2
                } else {
                    3
                };
                match start {
                    None => {
                        start = Some(idx);
                        kind = ck;
                    }
                    Some(s) if ck != kind || ck == 3 => {
                        out.push((&line[s..idx], s..idx));
                        start = Some(idx);
                        kind = ck;
                    }
                    _ => {}
                }
            }
            if let Some(s) = start {
                out.push((&line[s..], s..line.len()));
            }
        }
        InlineHighlightMode::None => {}
    }
    out
}

/// 计算未改动行的折叠范围（保留每个 Hunk 前后 `context` 行上下文）。
pub fn compute_unchanged_fold_ranges(
    line_count: usize,
    changed_ranges: &[Range<usize>],
    context: usize,
) -> Vec<EditorFoldRange> {
    if line_count <= context * 2 + 2 {
        return Vec::new();
    }
    let mut visible = vec![false; line_count];
    for r in changed_ranges {
        let start = r.start.saturating_sub(context);
        let end = (r.end + context).min(line_count);
        for v in &mut visible[start..end] {
            *v = true;
        }
    }
    // 没有任何变更时全部折叠会令人困惑，保留顶部若干行
    if changed_ranges.is_empty() {
        return Vec::new();
    }

    let mut folds = Vec::new();
    let mut i = 0;
    while i < line_count {
        if visible[i] {
            i += 1;
            continue;
        }
        let start = i;
        while i < line_count && !visible[i] {
            i += 1;
        }
        let end = i;
        // 至少 3 行隐藏区间才折叠（NSH EditorFoldRange 为 start_line..=end_line，折叠 start_line 之后直到 end_line）
        if end.saturating_sub(start) >= 3 {
            let fold_start = start as u32;
            let fold_end = (end - 1) as u32;
            if fold_end > fold_start {
                folds.push(EditorFoldRange::new(fold_start, fold_end));
            }
        }
    }
    folds
}

/// 单栏统一 Diff 文档，含行映射与装饰，使单栏模式也支持准确跳源码与 hunk 导航。
#[derive(Debug, Clone, PartialEq)]
pub struct UnifiedDiffDoc {
    pub text: String,
    pub line_map: Vec<UnifiedLineRef>,
    pub hunk_start_lines: Vec<usize>,
    pub decorations: Vec<EditorDecoration>,
    pub unchanged_folds: Vec<EditorFoldRange>,
}

impl UnifiedDiffDoc {
    pub(crate) fn decorations_with_inline(
        &self,
        mode: InlineHighlightMode,
        palette: Palette,
    ) -> Vec<EditorDecoration> {
        let mut decorations = self.decorations.clone();
        if mode == InlineHighlightMode::None || self.hunk_start_lines.is_empty() {
            return decorations;
        }

        let lines: Vec<_> = self.text.split('\n').collect();
        for (index, &start) in self.hunk_start_lines.iter().enumerate() {
            let end = self.hunk_start_lines.get(index + 1).copied().unwrap_or(self.line_map.len());
            let rows = &self.line_map[start..end];
            let old_count = rows.iter().take_while(|row| row.is_changed && row.side == DiffSide::Old).count();
            let new_count = rows[old_count..].iter().take_while(|row| row.is_changed && row.side == DiffSide::New).count();
            for offset in 0..old_count.min(new_count) {
                let old_row = start + offset;
                let new_row = start + old_count + offset;
                // Diff prefixes are two ASCII columns on either side.
                let (old, new) = compute_inline_ranges(&lines[old_row][2..], &lines[new_row][2..], mode);
                decorations.extend(inline_decorations(old_row, 2, old, palette.diff_del_inline));
                decorations.extend(inline_decorations(new_row, 2, new, palette.diff_add_inline));
            }
        }
        decorations
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UnifiedLineRef {
    pub side: DiffSide,
    pub orig_line: usize,
    pub is_changed: bool,
}

/// The editor can place a caret in the empty paragraph after a final line break.
pub(crate) fn unified_source_eof(text: &str) -> Option<UnifiedLineRef> {
    (text.is_empty() || text.ends_with('\n')).then(|| UnifiedLineRef {
        side: DiffSide::New,
        orig_line: logical_line_count(text),
        is_changed: false,
    })
}

/// 计算单栏统一 Diff 的行映射、每个 hunk 起始行以及未改动行折叠区间。
/// 与 `build_unified_diff` 共用完全一致的算法，供单栏 `F7` 跳块、折叠切换、`⌘L` 跳转与右键行号映射使用。
pub fn compute_unified_layout(
    old_text: &str,
    new_text: &str,
    hunks: &[DiffHunk],
) -> (Vec<UnifiedLineRef>, Vec<usize>, Vec<EditorFoldRange>) {
    let new_lines = split_lines_keep_logical(new_text);
    let mut line_map: Vec<UnifiedLineRef> = Vec::new();
    let mut hunk_start_lines: Vec<usize> = Vec::new();
    let mut changed_unified_ranges: Vec<Range<usize>> = Vec::new();
    let mut new_cursor = 0usize;

    for h in hunks {
        while new_cursor < h.new_start {
            line_map.push(UnifiedLineRef {
                side: DiffSide::New,
                orig_line: new_cursor,
                is_changed: false,
            });
            new_cursor += 1;
        }
        let u_start = line_map.len();
        hunk_start_lines.push(u_start);
        for ol in h.old_start..h.old_end {
            line_map.push(UnifiedLineRef {
                side: DiffSide::Old,
                orig_line: ol,
                is_changed: true,
            });
        }
        for nl in h.new_start..h.new_end {
            line_map.push(UnifiedLineRef {
                side: DiffSide::New,
                orig_line: nl,
                is_changed: true,
            });
        }
        let u_end = line_map.len();
        if u_end > u_start {
            changed_unified_ranges.push(u_start..u_end);
        }
        new_cursor = h.new_end;
    }

    while new_cursor < new_lines.len() {
        line_map.push(UnifiedLineRef {
            side: DiffSide::New,
            orig_line: new_cursor,
            is_changed: false,
        });
        new_cursor += 1;
    }
    let _ = old_text;
    let unchanged_folds = compute_unchanged_fold_ranges(line_map.len(), &changed_unified_ranges, 3);
    (line_map, hunk_start_lines, unchanged_folds)
}

/// 将 `(side, orig_line)`（0-based 原文件行号）映射到单栏统一视图的 0-based 行号。
pub fn map_side_line_to_unified(
    old_text: &str,
    new_text: &str,
    hunks: &[DiffHunk],
    side: DiffSide,
    orig_line: usize,
) -> usize {
    map_editable_side_line_to_unified(old_text, new_text, hunks, side, orig_line, false)
}

pub(crate) fn map_editable_side_line_to_unified(
    old_text: &str,
    new_text: &str,
    hunks: &[DiffHunk],
    side: DiffSide,
    orig_line: usize,
    source_editable: bool,
) -> usize {
    let (mut line_map, _, _) = compute_unified_layout(old_text, new_text, hunks);
    if source_editable {
        line_map.extend(unified_source_eof(new_text));
    }
    unified_line_for_side(&line_map, hunks, side, orig_line)
}

pub(crate) fn unified_line_for_side(
    line_map: &[UnifiedLineRef],
    hunks: &[DiffHunk],
    side: DiffSide,
    orig_line: usize,
) -> usize {
    if line_map.is_empty() {
        return 0;
    }
    // 1. 精确命中目标侧的对应原行
    if let Some(idx) = line_map
        .iter()
        .position(|r| r.side == side && r.orig_line == orig_line)
    {
        return idx;
    }
    // 2. 若目标是旧侧未改行（单栏仅以 New 侧上下文呈现），按 old->new 行映射查找对应上下文行
    if side == DiffSide::Old {
        let mapped_new = map_old_line_to_new(orig_line, hunks);
        if let Some(idx) = line_map
            .iter()
            .position(|r| r.side == DiffSide::New && r.orig_line == mapped_new)
        {
            return idx;
        }
    }
    // 3. 回退到同侧最接近（>= orig_line）的行或末尾行
    line_map
        .iter()
        .position(|r| r.side == side && r.orig_line >= orig_line)
        .or_else(|| line_map.iter().position(|r| r.orig_line >= orig_line))
        .unwrap_or(line_map.len().saturating_sub(1))
}

pub fn build_unified_diff(
    old_text: &str,
    new_text: &str,
    hunks: &[DiffHunk],
    p: Palette,
) -> UnifiedDiffDoc {
    let old_lines = split_lines_keep_logical(old_text);
    let new_lines = split_lines_keep_logical(new_text);

    let mut out_lines: Vec<String> = Vec::new();
    let mut line_map: Vec<UnifiedLineRef> = Vec::new();
    let mut hunk_start_lines: Vec<usize> = Vec::new();
    let mut decorations: Vec<EditorDecoration> = Vec::new();
    let mut changed_unified_ranges: Vec<Range<usize>> = Vec::new();

    let mut new_cursor = 0usize;

    for h in hunks {
        // 按新侧游标输出 hunk 之前的全部未改动行（兼容 `AllAndEmptyLines` 下旧/新侧空行数不等时不丢行）
        while new_cursor < h.new_start {
            let text_line = new_lines.get(new_cursor).copied().unwrap_or("");
            out_lines.push(format!("  {text_line}"));
            line_map.push(UnifiedLineRef {
                side: DiffSide::New,
                orig_line: new_cursor,
                is_changed: false,
            });
            new_cursor += 1;
        }

        let u_start = out_lines.len();
        hunk_start_lines.push(u_start);

        for ol in h.old_start..h.old_end {
            let text_line = old_lines.get(ol).copied().unwrap_or("");
            let row = out_lines.len() as u32;
            out_lines.push(format!("- {text_line}"));
            line_map.push(UnifiedLineRef {
                side: DiffSide::Old,
                orig_line: ol,
                is_changed: true,
            });
            let mut dec = EditorDecoration::default();
            dec.range = EditorRange::new(EditorPosition::new(row, 0), EditorPosition::new(row, 0));
            dec.line_style = Some(bg_style(p.diff_del_line));
            let mut g = EditorGutterMark::default();
            g.style = Some(bg_style(p.diff_gutter_del));
            dec.gutter = Some(g);
            decorations.push(dec);
        }

        for nl in h.new_start..h.new_end {
            let text_line = new_lines.get(nl).copied().unwrap_or("");
            let row = out_lines.len() as u32;
            out_lines.push(format!("+ {text_line}"));
            line_map.push(UnifiedLineRef {
                side: DiffSide::New,
                orig_line: nl,
                is_changed: true,
            });
            let mut dec = EditorDecoration::default();
            dec.range = EditorRange::new(EditorPosition::new(row, 0), EditorPosition::new(row, 0));
            dec.line_style = Some(bg_style(p.diff_add_line));
            let mut g = EditorGutterMark::default();
            g.style = Some(bg_style(p.diff_gutter_add));
            dec.gutter = Some(g);
            decorations.push(dec);
        }

        let u_end = out_lines.len();
        if u_end > u_start {
            changed_unified_ranges.push(u_start..u_end);
        }
        new_cursor = h.new_end;
    }

    while new_cursor < new_lines.len() {
        let text_line = new_lines[new_cursor];
        out_lines.push(format!("  {text_line}"));
        line_map.push(UnifiedLineRef {
            side: DiffSide::New,
            orig_line: new_cursor,
            is_changed: false,
        });
        new_cursor += 1;
    }

    let unchanged_folds = compute_unchanged_fold_ranges(out_lines.len(), &changed_unified_ranges, 3);
    UnifiedDiffDoc {
        text: out_lines.join("\n"),
        line_map,
        hunk_start_lines,
        decorations,
        unchanged_folds,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn caret_navigation_distinguishes_a_split_deletion_anchor_from_unified_context() {
        let old = "before\ndelete this\ncontext\nold last\nfinal\n";
        let new = "before\ncontext\nnew last\nfinal\n";
        let diff = compute_diff(old, new, WhitespaceMode::None);
        let caret = EditorPosition::new(1, 0);
        let split = diff.hunk_navigation(DiffSide::New, caret, Some(new));
        assert_eq!((split.previous, split.current, split.next), (None, Some(0), Some(1)));
        let unified = diff.hunk_navigation(DiffSide::New, caret, None);
        assert_eq!((unified.previous, unified.current, unified.next), (Some(0), None, Some(1)));
        let historical = diff.hunk_navigation(DiffSide::Old, caret, None);
        assert_eq!((historical.previous, historical.current, historical.next), (None, Some(0), Some(1)));
    }

    #[test]
    fn split_navigation_places_a_tail_change_at_the_shorter_files_end() {
        let short = "old\ncontext\ntail 🐈";
        let long = "new\ncontext\ntail 🐈\nadded\n";
        for (old, new, side) in [(short, long, DiffSide::Old), (long, short, DiffSide::New)] {
            let diff = compute_diff(old, new, WhitespaceMode::Trim);
            let eof = EditorPosition::new(2, 7);
            assert_eq!(diff.split_hunk_position(&diff.hunks[1], side, short), eof);
            let in_context = diff.hunk_navigation(side, EditorPosition::new(2, 5), Some(short));
            assert_eq!((in_context.previous, in_context.current, in_context.next), (Some(0), None, Some(1)));
            let at_end = diff.hunk_navigation(side, eof, Some(short));
            assert_eq!((at_end.previous, at_end.current, at_end.next), (Some(0), Some(1), None));
        }

        let terminated = "old\ncontext\ntail 🐈\n";
        let diff = compute_diff(terminated, long, WhitespaceMode::None);
        let next_line = EditorPosition::new(3, 0);
        assert_eq!(diff.split_hunk_position(&diff.hunks[1], DiffSide::Old, terminated), next_line);
        let at_end = diff.hunk_navigation(DiffSide::Old, next_line, Some(terminated));
        assert_eq!(at_end.current, Some(1));
    }

    #[test]
    fn unified_inline_highlights_only_the_paired_rows_at_utf16_columns() {
        let old = "head\n变量 valueOld 🐈\nend\n";
        let new = "head\n变量 valueNew 🐕\nextra\nend\n";
        let diff = compute_diff(old, new, WhitespaceMode::None);
        let palette = crate::theme::light_palette();
        let doc = build_unified_diff(old, new, &diff.hunks, palette);
        assert_eq!(doc.decorations_with_inline(InlineHighlightMode::None, palette), doc.decorations);
        assert_eq!(doc.decorations.len(), 3);

        for (mode, start_column) in [(InlineHighlightMode::Word, 5), (InlineHighlightMode::Char, 10)] {
            let decorations = doc.decorations_with_inline(mode, palette);
            assert_eq!(&decorations[..doc.decorations.len()], doc.decorations);
            let ranges: Vec<_> = decorations[doc.decorations.len()..].iter()
                .map(|mark| mark.range.clone()).collect();
            let span = |row, start, end| {
                EditorRange::new(EditorPosition::new(row, start), EditorPosition::new(row, end))
            };
            assert_eq!(ranges, vec![span(1, start_column, 13), span(1, 14, 16),
                span(2, start_column, 13), span(2, 14, 16)]);
            assert_eq!(decorations[3].style, Some(bg_style(palette.diff_del_inline)));
            assert_eq!(decorations[5].style, Some(bg_style(palette.diff_add_inline)));
        }
    }
}
