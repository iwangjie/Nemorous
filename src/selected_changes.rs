use std::ops::Range;

use anyhow::{Result, bail, ensure};
use imara_diff::{Algorithm, Diff, InternedInput};

use crate::diff_model::{DiffHunk, HunkKind};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Side {
    Old,
    New,
}

#[derive(Debug, Clone)]
pub(crate) enum Selection {
    Hunks(Vec<DiffHunk>),
    Lines { side: Side, range: Range<usize> },
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
        Selection::Lines { side, range } => {
            ensure!(!range.is_empty(), "请选中要操作的行");
            let mut edits = Vec::new();
            for h in hunks {
                let old_range = h.old_start..h.old_end;
                let new_range = h.new_start..h.new_end;
                let chosen = match side {
                    Side::Old => &old_range,
                    Side::New => &new_range,
                };
                let start = range.start.max(chosen.start);
                let end = range.end.min(chosen.end);
                if start >= end {
                    continue;
                }
                if !old_range.is_empty()
                    && !new_range.is_empty()
                    && old_range.len() != new_range.len()
                {
                    bail!("该替换块两侧行数不同，请操作整个块");
                }
                let offset = start - chosen.start;
                let count = end - start;
                let pick = |part: Range<usize>| {
                    if part.is_empty() {
                        part
                    } else {
                        part.start + offset..part.start + offset + count
                    }
                };
                edits.push((pick(old_range), pick(new_range)));
            }
            edits
        }
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

    #[test]
    fn selected_lines_and_hunks_reverse_preserve_crlf_and_missing_final_newline() {
        let old = b"one\r\nold a\r\nold b\r\nkeep\r\nno newline";
        let new = b"one\r\nnew a\r\nnew b\r\nkeep\r\ninsert1\r\ninsert2\r\nno newline";
        let edit = Selection::Lines {
            side: Side::New,
            range: 2..3,
        };
        assert_eq!(
            apply_selection(old, new, &edit, false).unwrap(),
            b"one\r\nold a\r\nnew b\r\nkeep\r\nno newline"
        );
        assert_eq!(
            apply_selection(old, new, &edit, true).unwrap(),
            b"one\r\nnew a\r\nold b\r\nkeep\r\ninsert1\r\ninsert2\r\nno newline"
        );
        let edit = Selection::Lines {
            side: Side::New,
            range: 5..6,
        };
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
        let edit = Selection::Lines {
            side: Side::Old,
            range: 2..3,
        };
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
}
