//! Restore one specific diff hunk from a snapshot into the current text.
//!
//! Inputs are LF-normalized. The caller is responsible for converting back to
//! CRLF and writing the file.

use crate::diff_model::{DiffHunk, HunkKind};

/// Restores exactly one hunk: the current range is replaced with the
/// corresponding snapshot range. Everything outside that range comes from
/// `current`.
///
/// Returns `None` when the hunk is internally inconsistent, has a `kind` that
/// disagrees with its empty/non-empty ranges, or is out of bounds for either
/// input. No panics on malformed or stale hunks.
pub fn restore_hunk(snapshot: &str, current: &str, hunk: &DiffHunk) -> Option<String> {
    let snapshot = LogicalLines::parse(snapshot);
    let current = LogicalLines::parse(current);

    if !valid_hunk(hunk, snapshot.lines.len(), current.lines.len()) {
        return None;
    }

    let mut restored = Vec::with_capacity(
        (current.lines.len() - (hunk.new_end - hunk.new_start))
            .saturating_add(hunk.old_end - hunk.old_start),
    );

    restored.extend_from_slice(&current.lines[..hunk.new_start]);
    restored.extend_from_slice(&snapshot.lines[hunk.old_start..hunk.old_end]);
    restored.extend_from_slice(&current.lines[hunk.new_end..]);

    // The output ends at the current EOF unless the splice reaches current EOF
    // and the source reaches snapshot EOF. A zero-length source at snapshot EOF
    // still counts as touching that EOF.
    let take_snapshot_eof =
        hunk.new_end == current.lines.len() && hunk.old_end == snapshot.lines.len();
    let trailing_newline = if take_snapshot_eof {
        snapshot.trailing_newline
    } else {
        current.trailing_newline
    };

    // Zero logical lines cannot carry a terminal newline.
    if restored.is_empty() {
        return Some(String::new());
    }

    let mut result = restored.join("\n");
    if trailing_newline {
        result.push('\n');
    }
    Some(result)
}

fn valid_hunk(hunk: &DiffHunk, snapshot_lines: usize, current_lines: usize) -> bool {
    if hunk.old_start > hunk.old_end || hunk.new_start > hunk.new_end {
        return false;
    }
    if hunk.old_end > snapshot_lines || hunk.new_end > current_lines {
        return false;
    }

    let old_empty = hunk.old_start == hunk.old_end;
    let new_empty = hunk.new_start == hunk.new_end;

    match &hunk.kind {
        HunkKind::Insert => old_empty && !new_empty,
        HunkKind::Delete => !old_empty && new_empty,
        HunkKind::Modify => !old_empty && !new_empty,
    }
}

struct LogicalLines<'a> {
    lines: Vec<&'a str>,
    trailing_newline: bool,
}

impl<'a> LogicalLines<'a> {
    fn parse(text: &'a str) -> Self {
        let trailing_newline = text.ends_with('\n');
        let body = if trailing_newline {
            &text[..text.len() - 1]
        } else {
            text
        };

        // `""` has zero logical lines. `"\n"` is one empty logical line.
        let lines = if text.is_empty() {
            Vec::new()
        } else {
            body.split('\n').collect()
        };

        Self {
            lines,
            trailing_newline,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::restore_hunk;
    use crate::diff_model::{DiffHunk, HunkKind};

    fn hunk(
        kind: HunkKind,
        old_start: usize,
        old_end: usize,
        new_start: usize,
        new_end: usize,
    ) -> DiffHunk {
        DiffHunk {
            old_start,
            old_end,
            new_start,
            new_end,
            kind,
        }
    }

    #[test]
    fn restores_one_of_two_unrelated_changes() {
        let snapshot = "one\ntwo\nthree\n";
        let current = "one\nTWO\nthree\nfour\n";

        let modify = hunk(HunkKind::Modify, 1, 2, 1, 2);
        assert_eq!(
            restore_hunk(snapshot, current, &modify).as_deref(),
            Some("one\ntwo\nthree\nfour\n")
        );

        let insert = hunk(HunkKind::Insert, 3, 3, 3, 4);
        assert_eq!(
            restore_hunk(snapshot, current, &insert).as_deref(),
            Some("one\nTWO\nthree\n")
        );
    }

    #[test]
    fn restores_deletions_at_start_and_end() {
        let at_start = hunk(HunkKind::Delete, 0, 1, 0, 0);
        assert_eq!(
            restore_hunk("a\nb\n", "b\n", &at_start).as_deref(),
            Some("a\nb\n")
        );

        let at_end = hunk(HunkKind::Delete, 1, 2, 1, 1);
        assert_eq!(
            restore_hunk("a\nb\n", "a\n", &at_end).as_deref(),
            Some("a\nb\n")
        );
    }

    #[test]
    fn restores_into_and_out_of_empty_files() {
        let add_line = hunk(HunkKind::Delete, 0, 1, 0, 0);
        assert_eq!(restore_hunk("α\n", "", &add_line).as_deref(), Some("α\n"));

        let add_empty_line = hunk(HunkKind::Delete, 0, 1, 0, 0);
        assert_eq!(
            restore_hunk("\n", "", &add_empty_line).as_deref(),
            Some("\n")
        );

        let remove_line = hunk(HunkKind::Insert, 0, 0, 0, 1);
        assert_eq!(restore_hunk("", "α\n", &remove_line).as_deref(), Some(""));
        assert_eq!(restore_hunk("", "\n", &remove_line).as_deref(), Some(""));
    }

    #[test]
    fn restores_multibyte_lines() {
        let snapshot = "标题\nnaïve café\n🙂 end\n";
        let current = "标题\nNAIVE CAFE\n🙂 end\n";
        let modify = hunk(HunkKind::Modify, 1, 2, 1, 2);

        assert_eq!(
            restore_hunk(snapshot, current, &modify).as_deref(),
            Some("标题\nnaïve café\n🙂 end\n")
        );
    }

    #[test]
    fn follows_eof_newline_rules() {
        // Restoring the current last line takes the snapshot EOF terminator.
        let last_line = hunk(HunkKind::Modify, 1, 2, 1, 2);
        assert_eq!(
            restore_hunk("a\nB", "a\nb\n", &last_line).as_deref(),
            Some("a\nB")
        );

        // A non-EOF splice keeps the current EOF terminator.
        let non_eof = hunk(HunkKind::Modify, 0, 1, 0, 1);
        assert_eq!(
            restore_hunk("A\nb", "a\nb\n", &non_eof).as_deref(),
            Some("A\nb\n")
        );

        // An EOF splice whose source is not at snapshot EOF keeps the current
        // terminator.
        let mid_source_at_eof = hunk(HunkKind::Delete, 0, 1, 1, 1);
        assert_eq!(
            restore_hunk("x\ny\n", "a", &mid_source_at_eof).as_deref(),
            Some("a\nx")
        );

        // Restoring a missing final line takes the snapshot EOF terminator.
        let add_final = hunk(HunkKind::Delete, 1, 2, 1, 1);
        assert_eq!(
            restore_hunk("a\nb\n", "a", &add_final).as_deref(),
            Some("a\nb\n")
        );

        // Removing a final line with an empty source at snapshot EOF also
        // takes the snapshot EOF terminator.
        let remove_final = hunk(HunkKind::Insert, 1, 1, 1, 2);
        assert_eq!(
            restore_hunk("a", "a\nb\n", &remove_final).as_deref(),
            Some("a")
        );
    }

    #[test]
    fn rejects_malformed_and_stale_hunks() {
        let snapshot = "a\nb\n";
        let current = "a\nb\nc\n";

        let valid = hunk(HunkKind::Modify, 1, 2, 1, 2);
        assert!(restore_hunk(snapshot, current, &valid).is_some());

        let reversed_old = hunk(HunkKind::Modify, 2, 1, 1, 2);
        assert_eq!(restore_hunk(snapshot, current, &reversed_old), None);

        let reversed_new = hunk(HunkKind::Modify, 1, 2, 2, 1);
        assert_eq!(restore_hunk(snapshot, current, &reversed_new), None);

        let stale_snapshot = hunk(HunkKind::Modify, 1, 3, 1, 2);
        assert_eq!(restore_hunk(snapshot, current, &stale_snapshot), None);

        let stale_current = hunk(HunkKind::Modify, 1, 2, 1, 4);
        assert_eq!(restore_hunk(snapshot, current, &stale_current), None);

        let wrong_kind = hunk(HunkKind::Insert, 1, 2, 1, 2);
        assert_eq!(restore_hunk(snapshot, current, &wrong_kind), None);

        let empty_change = hunk(HunkKind::Insert, 1, 1, 1, 1);
        assert_eq!(restore_hunk(snapshot, current, &empty_change), None);
    }
}
