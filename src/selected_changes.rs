use std::ops::Range;

use anyhow::{Result, bail, ensure};
use imara_diff::{Algorithm, Diff, InternedInput};

use crate::diff_model::{DiffHunk, DiffSide, HunkKind};

#[derive(Debug, Clone)]
pub(crate) enum Selection {
    Hunks(Vec<DiffHunk>),
    Lines(LineSelection),
}

#[derive(Debug, Clone, Default)]
pub(crate) struct LineSelection {
    old: Vec<Range<usize>>,
    new: Vec<Range<usize>>,
}

impl LineSelection {
    pub(crate) fn push(&mut self, side: DiffSide, range: Range<usize>) {
        let ranges = match side {
            DiffSide::Old => &mut self.old,
            DiffSide::New => &mut self.new,
        };
        append_range(ranges, range);
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.old.is_empty() && self.new.is_empty()
    }
}

fn append_range(ranges: &mut Vec<Range<usize>>, range: Range<usize>) {
    if range.is_empty() {
        return;
    }
    if let Some(last) = ranges.last_mut()
        && last.start <= range.start && range.start <= last.end
    {
        last.end = last.end.max(range.end);
    } else {
        ranges.push(range);
    }
}

fn merge_ranges(mut ranges: Vec<Range<usize>>) -> Vec<Range<usize>> {
    ranges.sort_unstable_by_key(|range| range.start);
    let mut merged = Vec::new();
    for range in ranges {
        append_range(&mut merged, range);
    }
    merged
}

fn selected_offsets(selected: &[Range<usize>], changed: &Range<usize>) -> Vec<Range<usize>> {
    if changed.is_empty() {
        return Vec::new();
    }
    let start = selected.partition_point(|range| range.end <= changed.start);
    selected[start..].iter().take_while(|range| range.start < changed.end).map(|range| {
        range.start.max(changed.start) - changed.start..range.end.min(changed.end) - changed.start
    }).collect()
}

fn line_edits(hunks: &[DiffHunk], selection: &LineSelection) -> Result<Vec<(Range<usize>, Range<usize>)>> {
    ensure!(!selection.is_empty(), "请选中要操作的行");
    let selected_old = merge_ranges(selection.old.clone());
    let selected_new = merge_ranges(selection.new.clone());
    let mut edits = Vec::new();

    for hunk in hunks {
        let old = hunk.line_range(DiffSide::Old);
        let new = hunk.line_range(DiffSide::New);
        let old_offsets = selected_offsets(&selected_old, &old);
        let new_offsets = selected_offsets(&selected_new, &new);
        if old_offsets.is_empty() && new_offsets.is_empty() {
            continue;
        }
        if !old.is_empty() && !new.is_empty() && old.len() != new.len() {
            ensure!(old_offsets == [0..old.len()] && new_offsets == [0..new.len()],
                "该替换块两侧行数不同，请操作整个块");
            edits.push((old, new));
            continue;
        }

        let offsets = merge_ranges(old_offsets.into_iter().chain(new_offsets).collect());
        for offset in offsets {
            let pick = |part: &Range<usize>| {
                if part.is_empty() {
                    part.clone()
                } else {
                    part.start + offset.start..part.start + offset.end
                }
            };
            edits.push((pick(&old), pick(&new)));
        }
    }
    Ok(edits)
}

pub(crate) fn exact_hunks(old: &[u8], new: &[u8]) -> Vec<DiffHunk> {
    let input = InternedInput::new(old, new);
    let mut diff = Diff::compute(Algorithm::Histogram, &input);
    diff.postprocess_lines(&input);
    diff.hunks()
        .map(|hunk| {
            let kind = if hunk.before.is_empty() {
                HunkKind::Insert
            } else if hunk.after.is_empty() {
                HunkKind::Delete
            } else {
                HunkKind::Modify
            };
            DiffHunk {
                old_start: hunk.before.start as usize,
                old_end: hunk.before.end as usize,
                new_start: hunk.after.start as usize,
                new_end: hunk.after.end as usize,
                kind,
            }
        })
        .collect()
}

fn overlap(a: &Range<usize>, b: &Range<usize>) -> bool {
    if a.is_empty() && b.is_empty() {
        a.start == b.start
    } else {
        a.start < b.end && b.start < a.end
    }
}

pub(crate) fn exact_hunk_index(exact: &[DiffHunk], display: &DiffHunk) -> Result<usize> {
    if let Some(index) = exact.iter().position(|hunk| hunk == display) {
        return Ok(index);
    }
    let mut candidates = exact.iter().enumerate().filter(|(_, hunk)| {
        overlap(
            &(hunk.old_start..hunk.old_end),
            &(display.old_start..display.old_end),
        ) || overlap(
            &(hunk.new_start..hunk.new_end),
            &(display.new_start..display.new_end),
        )
    });
    let (index, _) = candidates
        .next()
        .ok_or_else(|| anyhow::anyhow!("没有匹配的原始差异块"))?;
    ensure!(
        candidates.next().is_none(),
        "显示范围包含多个原始差异块，请切换到原始差异"
    );
    Ok(index)
}

pub(crate) fn apply_selection(
    old: &[u8],
    new: &[u8],
    selection: &Selection,
    reverse: bool,
) -> Result<Vec<u8>> {
    let hunks = exact_hunks(old, new);
    let edits: Vec<_> = match selection {
        Selection::Hunks(display) => {
            let mut indices = display
                .iter()
                .map(|hunk| exact_hunk_index(&hunks, hunk))
                .collect::<Result<Vec<_>>>()?;
            indices.sort_unstable();
            indices.dedup();
            indices
                .into_iter()
                .map(|index| {
                    let h = &hunks[index];
                    (h.old_start..h.old_end, h.new_start..h.new_end)
                })
                .collect()
        }
        Selection::Lines(selection) => line_edits(&hunks, selection)?,
    };
    ensure!(!edits.is_empty(), "所选行不在变更块内");
    let (base, target) = if reverse { (new, old) } else { (old, new) };
    let base_offsets = line_offsets(base);
    let target_offsets = line_offsets(target);
    let mut result = Vec::new();
    let mut cursor = 0;
    for (old_range, new_range) in edits {
        let (from, to) = if reverse {
            (new_range, old_range)
        } else {
            (old_range, new_range)
        };
        let start = base_offsets[from.start];
        let end = base_offsets[from.end];
        let target_start = target_offsets[to.start];
        let target_end = target_offsets[to.end];
        ensure!(cursor <= start, "选中的原始差异范围重叠");
        result.extend_from_slice(&base[cursor..start]);
        let insert = &target[target_start..target_end];
        ensure!(
            insert.is_empty() || result.is_empty() || result.ends_with(b"\n"),
            "此操作需要连同上一行的末尾换行变更一起选择"
        );
        result.extend_from_slice(insert);
        if end < base.len() && !result.is_empty() && !result.ends_with(b"\n") {
            bail!("此操作需要连同最后一行的末尾换行变更一起选择");
        }
        cursor = end;
    }
    result.extend_from_slice(&base[cursor..]);
    Ok(result)
}

pub(crate) fn line_offsets(bytes: &[u8]) -> Vec<usize> {
    let mut offsets = vec![0];
    let mut len = 0;
    for line in imara_diff::sources::byte_lines(bytes) {
        len += line.len();
        offsets.push(len);
    }
    offsets
}

#[cfg(test)]
mod tests {
    use super::*;

    fn selected_line_ranges(ranges: &[(DiffSide, Range<usize>)]) -> Selection {
        let mut selection = LineSelection::default();
        for (side, range) in ranges {
            selection.push(*side, range.clone());
        }
        Selection::Lines(selection)
    }

    #[test]
    fn selected_lines_and_hunks_reverse_preserve_crlf_and_missing_final_newline() {
        let old = b"one\r\nold a\r\nold b\r\nkeep\r\nno newline";
        let new = b"one\r\nnew a\r\nnew b\r\nkeep\r\ninsert1\r\ninsert2\r\nno newline";
        let edit = selected_line_ranges(&[(DiffSide::New, 2..3)]);
        assert_eq!(
            apply_selection(old, new, &edit, false).unwrap(),
            b"one\r\nold a\r\nnew b\r\nkeep\r\nno newline"
        );
        assert_eq!(
            apply_selection(old, new, &edit, true).unwrap(),
            b"one\r\nnew a\r\nold b\r\nkeep\r\ninsert1\r\ninsert2\r\nno newline"
        );
        let edit = selected_line_ranges(&[(DiffSide::New, 5..6)]);
        assert_eq!(
            apply_selection(old, new, &edit, false).unwrap(),
            b"one\r\nold a\r\nold b\r\nkeep\r\ninsert2\r\nno newline"
        );
        let edit = Selection::Hunks(exact_hunks(b"tail", b"tail\n"));
        assert_eq!(
            apply_selection(b"tail", b"tail\n", &edit, false).unwrap(),
            b"tail\n"
        );
        assert_eq!(
            apply_selection(b"tail", b"tail\n", &edit, true).unwrap(),
            b"tail"
        );
    }

    #[test]
    fn selected_lines_delete_and_require_complete_unequal_replacement() {
        let old = b"start\na\nb\nc\nend\n";
        let new = b"start\nend\n";
        let edit = selected_line_ranges(&[(DiffSide::Old, 2..3)]);
        assert_eq!(
            apply_selection(old, new, &edit, false).unwrap(),
            b"start\na\nc\nend\n"
        );
        assert_eq!(
            apply_selection(old, new, &edit, true).unwrap(),
            b"start\nb\nend\n"
        );
        assert!(apply_selection(old, b"start\nreplacement\nend\n", &edit, false).is_err());
    }

    #[test]
    fn mixed_line_selection_deduplicates_pairs_and_preserves_disjoint_rows() {
        let old = b"top\r\nold a\r\nold b\r\nold c\r\nlast";
        let new = b"top\r\nnew a\r\nnew b\r\nnew c\r\nlast";
        let edit = selected_line_ranges(&[(DiffSide::Old, 3..4), (DiffSide::New, 1..2), (DiffSide::New, 3..4)]);
        assert_eq!(apply_selection(old, new, &edit, false).unwrap(), b"top\r\nnew a\r\nold b\r\nnew c\r\nlast");
        assert_eq!(apply_selection(old, new, &edit, true).unwrap(), b"top\r\nold a\r\nnew b\r\nold c\r\nlast");
    }

    #[test]
    fn unequal_replacement_requires_the_whole_selection_on_both_sides() {
        let old = b"top\nold\nkeep\nold tail\n";
        let new = b"top\nnew a\nnew b\nkeep\nnew tail\n";
        let full = selected_line_ranges(&[(DiffSide::Old, 1..2), (DiffSide::New, 1..3)]);
        let partial = selected_line_ranges(&[(DiffSide::Old, 1..2), (DiffSide::New, 2..3), (DiffSide::New, 4..5)]);
        assert_eq!(apply_selection(old, new, &full, false).unwrap(), b"top\nnew a\nnew b\nkeep\nold tail\n");
        assert!(apply_selection(old, new, &partial, false).is_err());
    }
}
