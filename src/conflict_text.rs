//! Parsing, resolution, and conservative merging of in-file conflict markers.
//!
//! Both Git conflict styles are understood:
//!
//! ```text
//! <<<<<<< ours          <<<<<<< ours
//! ours content          ours content
//! =======               ||||||| base
//! theirs content        base content
//! >>>>>>> theirs        =======
//!                       theirs content
//!                       >>>>>>> theirs
//! ```
//!
//! The marker width is taken from each block's opening line (Git writes seven
//! characters by default, but `merge.conflict-marker-size` can widen them).
//! Nested blocks are tracked on an explicit stack and keep their own width.
//! Every section starts and ends on a line boundary, so sections can be
//! concatenated without normalising CRLF, final-EOF, or surrounding bytes.
//!
//! Everything here is pure byte manipulation: no file system access, no
//! configuration lookups, no model calls.

use std::ops::Range;

use anyhow::{Context, Result, bail};

use crate::selected_changes::exact_hunks;

/// How a single conflict block should be resolved.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ConflictChoice {
    Ours,
    Theirs,
    Base,
    BothOursFirst,
    BothTheirsFirst,
}

/// One parsed conflict block, described by its byte span and sections.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ConflictBlock {
    /// Byte span of the whole block, opening marker through the end of the
    /// closing marker line (including that line's terminator, if any).
    pub(crate) whole: Range<usize>,
    /// Bytes between the opening marker line and the separator.
    pub(crate) ours: Vec<u8>,
    /// Bytes between `|||||||` and `=======` for diff3 blocks.
    pub(crate) base: Option<Vec<u8>>,
    /// Bytes between the separator and the closing marker.
    pub(crate) theirs: Vec<u8>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Phase {
    Ours,
    Base,
    Theirs,
}

struct Frame {
    size: usize,
    open_start: usize,
    phase: Phase,
    ours_start: usize,
    ours_end: usize,
    base_start: Option<usize>,
    base_end: usize,
    theirs_start: usize,
}

struct Marker {
    kind: u8,
    size: usize,
}

/// Parse every top-level conflict block in `bytes`, in file order.
///
/// Binary input (NUL bytes or a UTF-16 BOM), unbalanced markers, duplicate
/// diff3 base markers, and separators/closers in the wrong phase are reported
/// as errors rather than guessed at. Marker-looking lines that do not start a
/// block are left alone, so ordinary text such as a `=======` setext underline
/// is not treated as a conflict.
pub(crate) fn parse(bytes: &[u8]) -> Result<Vec<ConflictBlock>> {
    detect_binary(bytes)?;

    let mut blocks = Vec::new();
    let mut stack: Vec<Frame> = Vec::new();

    for (line_start, line_end, raw) in split_lines(bytes) {
        let Some(marker) = marker_line(raw) else {
            continue;
        };

        match marker.kind {
            b'<' => {
                stack.push(Frame {
                    size: marker.size,
                    open_start: line_start,
                    phase: Phase::Ours,
                    ours_start: line_end,
                    ours_end: line_end,
                    base_start: None,
                    base_end: line_end,
                    theirs_start: line_end,
                });
            }
            b'|' => {
                let top = match stack.last_mut() {
                    Some(top) if top.size == marker.size => top,
                    _ => continue,
                };
                match top.phase {
                    Phase::Ours => {
                        top.ours_end = line_start;
                        top.base_start = Some(line_end);
                        top.base_end = line_end;
                        top.phase = Phase::Base;
                    }
                    Phase::Base => bail!("duplicate '|||||||' marker at byte {line_start}"),
                    Phase::Theirs => {}
                }
            }
            b'=' => {
                let top = match stack.last_mut() {
                    Some(top) if top.size == marker.size => top,
                    _ => continue,
                };
                match top.phase {
                    Phase::Ours => {
                        top.ours_end = line_start;
                        top.base_end = line_start;
                        top.theirs_start = line_end;
                        top.phase = Phase::Theirs;
                    }
                    Phase::Base => {
                        top.base_end = line_start;
                        top.theirs_start = line_end;
                        top.phase = Phase::Theirs;
                    }
                    Phase::Theirs => {
                        bail!("unexpected '=======' marker at byte {line_start}");
                    }
                }
            }
            b'>' => {
                let frame = match stack.last() {
                    Some(frame) if frame.size == marker.size => frame,
                    _ => continue,
                };
                if frame.phase != Phase::Theirs {
                    bail!(
                        "closing '>>>>>>>' marker at byte {line_start} has no preceding '======='"
                    );
                }
                let frame = stack.pop().expect("frame was present");
                let block = ConflictBlock {
                    whole: frame.open_start..line_end,
                    ours: bytes[frame.ours_start..frame.ours_end].to_vec(),
                    base: frame
                        .base_start
                        .map(|start| bytes[start..frame.base_end].to_vec()),
                    theirs: bytes[frame.theirs_start..line_start].to_vec(),
                };
                if stack.is_empty() {
                    blocks.push(block);
                }
            }
            _ => {}
        }
    }

    if let Some(frame) = stack.last() {
        bail!(
            "unterminated conflict block starting at byte {}",
            frame.open_start
        );
    }

    Ok(blocks)
}

/// Replace one conflict block in `bytes` with the requested side.
///
/// Before touching anything the bytes at `block.whole` are re-parsed and
/// compared with the block, so a stale `ConflictBlock` (for example one
/// captured before the file was edited) is rejected instead of silently
/// clobbering unrelated content.
pub(crate) fn apply(
    bytes: &[u8],
    block: &ConflictBlock,
    choice: ConflictChoice,
) -> Result<Vec<u8>> {
    if block.whole.start > block.whole.end || block.whole.end > bytes.len() {
        bail!(
            "conflict block range {:?} is out of bounds for {}-byte source",
            block.whole,
            bytes.len()
        );
    }

    let region = &bytes[block.whole.clone()];
    let reparsed = parse(region).with_context(|| {
        format!(
            "conflict block at {:?} is no longer well-formed",
            block.whole
        )
    })?;
    let exact = match reparsed.as_slice() {
        [only] => {
            only.whole.start == 0
                && only.whole.end == region.len()
                && only.ours == block.ours
                && only.base == block.base
                && only.theirs == block.theirs
        }
        _ => false,
    };
    if !exact {
        bail!(
            "conflict block at {:?} no longer matches the source bytes",
            block.whole
        );
    }

    let replacement: Vec<u8> = match choice {
        ConflictChoice::Ours => block.ours.clone(),
        ConflictChoice::Theirs => block.theirs.clone(),
        ConflictChoice::Base => match &block.base {
            Some(base) => base.clone(),
            None => bail!(
                "conflict block at {:?} has no base section (plain conflict style)",
                block.whole
            ),
        },
        ConflictChoice::BothOursFirst => concat(&block.ours, &block.theirs),
        ConflictChoice::BothTheirsFirst => concat(&block.theirs, &block.ours),
    };

    let mut out = Vec::with_capacity(bytes.len() - region.len() + replacement.len());
    out.extend_from_slice(&bytes[..block.whole.start]);
    out.extend_from_slice(&replacement);
    out.extend_from_slice(&bytes[block.whole.end..]);
    Ok(out)
}

/// Merge `ours` and `theirs` against `base` when their edits are independent.
///
/// Two hunks may be combined only when their line ranges are disjoint (an
/// insertion exactly at another hunk's boundary is allowed, an insertion into
/// its interior is not) or when they describe the identical replacement of the
/// identical base span. Anything else is a conflict and yields `None`.
///
/// The result is stitched out of unchanged `base` bytes plus the replaced
/// hunks, so CRLF endings, a missing final newline, and any unrelated bytes
/// (including other conflict markers) survive verbatim.
pub(crate) fn smart_merge(base: &[u8], ours: &[u8], theirs: &[u8]) -> Option<Vec<u8>> {
    if ours == theirs {
        return Some(ours.to_vec());
    }
    if ours == base {
        return Some(theirs.to_vec());
    }
    if theirs == base {
        return Some(ours.to_vec());
    }

    let offsets = crate::selected_changes::line_offsets(base);
    let mut pieces = edits_for(base, ours, &offsets);
    pieces.extend(edits_for(base, theirs, &offsets));
    pieces.sort_by_key(|edit| (edit.start, edit.end));

    let mut out = Vec::with_capacity(base.len());
    let mut cursor = 0;
    let mut previous = None;
    for edit in pieces {
        if previous.as_ref() == Some(&edit) {
            continue;
        }
        if edit.start < cursor
            || previous
                .as_ref()
                .is_some_and(|last: &Edit<'_>| last.start == edit.start && last.end == edit.end)
        {
            return None;
        }
        out.extend_from_slice(&base[cursor..edit.start]);
        out.extend_from_slice(edit.replacement);
        cursor = edit.end;
        previous = Some(edit);
    }
    out.extend_from_slice(&base[cursor..]);
    Some(out)
}

#[derive(PartialEq, Eq)]
struct Edit<'a> {
    start: usize,
    end: usize,
    replacement: &'a [u8],
}

fn edits_for<'a>(base: &[u8], source: &'a [u8], from: &[usize]) -> Vec<Edit<'a>> {
    let to = crate::selected_changes::line_offsets(source);
    let mut edits = Vec::new();
    for hunk in exact_hunks(base, source) {
        let old_len = hunk.old_end - hunk.old_start;
        let new_len = hunk.new_end - hunk.new_start;
        if old_len == new_len {
            for offset in 0..old_len {
                let i = hunk.old_start + offset;
                let j = hunk.new_start + offset;
                let replacement = &source[to[j]..to[j + 1]];
                if &base[from[i]..from[i + 1]] != replacement {
                    edits.push(Edit {
                        start: from[i],
                        end: from[i + 1],
                        replacement,
                    });
                }
            }
        } else {
            edits.push(Edit {
                start: from[hunk.old_start],
                end: from[hunk.old_end],
                replacement: &source[to[hunk.new_start]..to[hunk.new_end]],
            });
        }
    }
    edits
}

fn concat(first: &[u8], second: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(first.len() + second.len());
    out.extend_from_slice(first);
    out.extend_from_slice(second);
    out
}

fn detect_binary(bytes: &[u8]) -> Result<()> {
    if bytes.starts_with(&[0xFF, 0xFE]) || bytes.starts_with(&[0xFE, 0xFF]) {
        bail!("cannot parse conflicts in UTF-16 encoded data");
    }
    std::str::from_utf8(bytes).context("冲突标记编辑要求 UTF-8 文本")?;
    if let Some(offset) = bytes.iter().position(|&b| b == 0) {
        bail!("cannot parse conflicts in binary data: NUL byte at offset {offset}");
    }
    Ok(())
}

fn marker_line(raw: &[u8]) -> Option<Marker> {
    let mut content = raw;
    if content.ends_with(b"\n") {
        content = &content[..content.len() - 1];
    }
    if content.ends_with(b"\r") {
        content = &content[..content.len() - 1];
    }

    let kind = *content.first()?;
    if !matches!(kind, b'<' | b'=' | b'>' | b'|') {
        return None;
    }
    let size = content.iter().take_while(|&&b| b == kind).count();
    if size < 7 {
        return None;
    }
    let rest = &content[size..];
    if !rest.is_empty() && !(rest[0] == b' ' || rest[0] == b'\t') {
        return None;
    }
    Some(Marker { kind, size })
}

fn split_lines<'a>(bytes: &'a [u8]) -> impl Iterator<Item = (usize, usize, &'a [u8])> + 'a {
    let mut pos = 0usize;
    std::iter::from_fn(move || {
        if pos >= bytes.len() {
            return None;
        }
        let start = pos;
        let end = match bytes[pos..].iter().position(|&b| b == b'\n') {
            Some(offset) => pos + offset + 1,
            None => bytes.len(),
        };
        pos = end;
        Some((start, end, &bytes[start..end]))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plain() -> &'static [u8] {
        b"top\n<<<<<<< HEAD\nours1\nours2\n=======\ntheirs1\n>>>>>>> feature\nbottom\n"
    }

    fn diff3() -> &'static [u8] {
        b"<<<<<<< ours\nA\n||||||| base\nB\n=======\nC\n>>>>>>> theirs\n"
    }

    #[test]
    fn parse_plain_block() {
        let text = plain();
        let blocks = parse(text).unwrap();
        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0].ours.as_slice(), &b"ours1\nours2\n"[..]);
        assert_eq!(blocks[0].base.as_deref(), None);
        assert_eq!(blocks[0].theirs.as_slice(), &b"theirs1\n"[..]);
        assert_eq!(&text[blocks[0].whole.clone()], &text[4..text.len() - 7]);
    }

    #[test]
    fn parse_diff3_block() {
        let blocks = parse(diff3()).unwrap();
        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0].ours.as_slice(), &b"A\n"[..]);
        assert_eq!(blocks[0].base.as_deref(), Some(&b"B\n"[..]));
        assert_eq!(blocks[0].theirs.as_slice(), &b"C\n"[..]);
    }

    #[test]
    fn parse_adjacent_blocks() {
        let text = b"<<<<<<< a\n1\n=======\n2\n>>>>>>> a\n<<<<<<< b\n3\n=======\n4\n>>>>>>> b\n";
        let blocks = parse(text).unwrap();
        assert_eq!(blocks.len(), 2);
        assert_eq!(blocks[0].whole.end, blocks[1].whole.start);
        assert_eq!(blocks[1].theirs.as_slice(), &b"4\n"[..]);
    }

    #[test]
    fn parse_custom_marker_width() {
        let blocks = parse(b"<<<<<<<<< HEAD\nmine\n=========\nyours\n>>>>>>>>> other\n").unwrap();
        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0].ours.as_slice(), &b"mine\n"[..]);
        assert_eq!(blocks[0].theirs.as_slice(), &b"yours\n"[..]);
    }

    #[test]
    fn parse_nested_block_in_ours() {
        let text = b"<<<<<<< outer\nleft\n<<<<<<< inner\nin1\n=======\nin2\n>>>>>>> inner\n=======\nright\n>>>>>>> outer\n";
        let blocks = parse(text).unwrap();
        assert_eq!(blocks.len(), 1);
        assert_eq!(
            blocks[0].ours.as_slice(),
            &b"left\n<<<<<<< inner\nin1\n=======\nin2\n>>>>>>> inner\n"[..]
        );
        assert_eq!(blocks[0].theirs.as_slice(), &b"right\n"[..]);
        assert_eq!(blocks[0].whole, 0..text.len());
    }

    #[test]
    fn parse_ignores_marker_lookalikes() {
        assert!(
            parse(b"Title\n=======\n<<<<<<\nnot a conflict\n>>>>>>\n")
                .unwrap()
                .is_empty()
        );
        assert!(parse(b">>>>>>> stray closer\n").unwrap().is_empty());
    }

    #[test]
    fn parse_reports_malformed() {
        assert!(parse(b"<<<<<<< a\nours\n").is_err());
        assert!(parse(b"<<<<<<< a\nours\n>>>>>>> b\n").is_err());
        assert!(
            parse(b"<<<<<<< a\nours\n||||||| b\nbase\n||||||| c\nmore\n=======\nt\n>>>>>>> d\n")
                .is_err()
        );
        assert!(parse(b"<<<<<<< a\nx\n=======\ny\n=======\nz\n>>>>>>> b\n").is_err());
        assert!(parse(b"<<<<<<< a\nx\n=======\ny\n>>>>>>>> b\n").is_err());
        assert!(parse(b"<<<<<<< a\nx\n||||||| b\nbase\n").is_err());
    }

    #[test]
    fn parse_rejects_binary() {
        assert!(parse(b"abc\x00def").is_err());
        assert!(parse(b"\xff\xfe<\x00").is_err());
    }

    #[test]
    fn parse_preserves_crlf_sections() {
        let text = b"<<<<<<< HEAD\r\nours\r\n=======\r\ntheirs\r\n>>>>>>> x\r\n";
        let blocks = parse(text).unwrap();
        assert_eq!(blocks[0].ours.as_slice(), &b"ours\r\n"[..]);
        assert_eq!(blocks[0].theirs.as_slice(), &b"theirs\r\n"[..]);
        assert_eq!(blocks[0].whole, 0..text.len());
    }

    #[test]
    fn apply_resolves_choices() {
        let text = plain();
        let blocks = parse(text).unwrap();
        let block = &blocks[0];

        let ours = apply(text, block, ConflictChoice::Ours).unwrap();
        assert_eq!(ours.as_slice(), &b"top\nours1\nours2\nbottom\n"[..]);

        let theirs = apply(text, block, ConflictChoice::Theirs).unwrap();
        assert_eq!(theirs.as_slice(), &b"top\ntheirs1\nbottom\n"[..]);

        let both = apply(text, block, ConflictChoice::BothOursFirst).unwrap();
        assert_eq!(
            both.as_slice(),
            &b"top\nours1\nours2\ntheirs1\nbottom\n"[..]
        );

        let both_rev = apply(text, block, ConflictChoice::BothTheirsFirst).unwrap();
        assert_eq!(
            both_rev.as_slice(),
            &b"top\ntheirs1\nours1\nours2\nbottom\n"[..]
        );

        assert!(apply(text, block, ConflictChoice::Base).is_err());
    }

    #[test]
    fn apply_diff3_base() {
        let text = diff3();
        let blocks = parse(text).unwrap();
        let covered = apply(text, &blocks[0], ConflictChoice::Base).unwrap();
        assert_eq!(covered.as_slice(), &b"B\n"[..]);
    }

    #[test]
    fn apply_preserves_other_blocks() {
        let text = b"<<<<<<< a\n1\n=======\n2\n>>>>>>> a\n<<<<<<< b\n3\n=======\n4\n>>>>>>> b\n";
        let blocks = parse(text).unwrap();
        let out = apply(text, &blocks[1], ConflictChoice::Theirs).unwrap();
        assert_eq!(&out[..blocks[0].whole.end], &text[..blocks[0].whole.end]);
        let reparsed = parse(&out).unwrap();
        assert_eq!(reparsed.len(), 1);
        assert_eq!(reparsed[0].ours.as_slice(), &b"1\n"[..]);
    }

    #[test]
    fn apply_preserves_nested_markers() {
        let text = b"<<<<<<< outer\nleft\n<<<<<<< inner\nin1\n=======\nin2\n>>>>>>> inner\n=======\nright\n>>>>>>> outer\n";
        let blocks = parse(text).unwrap();
        let out = apply(text, &blocks[0], ConflictChoice::Ours).unwrap();
        assert_eq!(
            out.as_slice(),
            &b"left\n<<<<<<< inner\nin1\n=======\nin2\n>>>>>>> inner\n"[..]
        );
    }

    #[test]
    fn apply_rejects_stale_block() {
        let text = plain();
        let blocks = parse(text).unwrap();
        let mut bytes = text.to_vec();
        let pos = text.windows(5).position(|w| w == b"ours1").unwrap();
        bytes[pos..pos + 5].copy_from_slice(b"OURS1");
        assert!(apply(&bytes, &blocks[0], ConflictChoice::Ours).is_err());
    }

    #[test]
    fn apply_rejects_out_of_bounds() {
        let text = plain();
        let blocks = parse(text).unwrap();
        let mut block = blocks[0].clone();
        block.whole.end = text.len() + 1;
        assert!(apply(text, &block, ConflictChoice::Theirs).is_err());
    }

    #[test]
    fn apply_preserves_missing_final_newline() {
        let text = b"<<<<<<< a\nours\n=======\ntheirs\n>>>>>>> b";
        let blocks = parse(text).unwrap();
        assert_eq!(blocks[0].whole, 0..text.len());
        let out = apply(text, &blocks[0], ConflictChoice::Theirs).unwrap();
        assert_eq!(out.as_slice(), &b"theirs\n"[..]);
    }

    #[test]
    fn smart_merge_nonoverlapping() {
        let merged = smart_merge(b"a\nb\nc\n", b"A\nb\nc\n", b"a\nb\nC\n");
        assert_eq!(merged.as_deref(), Some(&b"A\nb\nC\n"[..]));
    }

    #[test]
    fn smart_merge_identical_replacements() {
        let merged = smart_merge(b"a\nb\nc\nd\n", b"a\nB\nc\nD\n", b"A\nB\nc\nd\n");
        assert_eq!(merged.as_deref(), Some(&b"A\nB\nc\nD\n"[..]));
    }

    #[test]
    fn smart_merge_insert_then_edit() {
        let merged = smart_merge(b"a\nb\nc\n", b"a\nnew\nb\nc\n", b"a\nb\nC\n");
        assert_eq!(merged.as_deref(), Some(&b"a\nnew\nb\nC\n"[..]));
    }

    #[test]
    fn smart_merge_rejects_different_replacements_of_same_span() {
        assert_eq!(smart_merge(b"a\nb\nc\n", b"X\nb\nc\n", b"Y\nb\nc\n"), None);
    }

    #[test]
    fn smart_merge_rejects_overlapping_line_ranges() {
        let base = b"l1\nl2\nl3\nl4\n";
        let ours = b"A\nB\nl3\nl4\n";
        let theirs = b"l1\nB2\nC3\nl4\n";
        assert_eq!(smart_merge(base, ours, theirs), None);
    }

    #[test]
    fn smart_merge_rejects_same_line_edits() {
        assert_eq!(smart_merge(b"abc\n", b"Xbc\n", b"abY\n"), None);
    }

    #[test]
    fn smart_merge_preserves_crlf() {
        let merged = smart_merge(b"a\r\nb\r\nc\r\n", b"A\r\nb\r\nc\r\n", b"a\r\nb\r\nC\r\n");
        assert_eq!(merged.as_deref(), Some(&b"A\r\nb\r\nC\r\n"[..]));
    }

    #[test]
    fn smart_merge_preserves_final_eof() {
        let merged = smart_merge(b"a\nb", b"A\nb", b"a\nB");
        assert_eq!(merged.as_deref(), Some(&b"A\nB"[..]));
    }

    #[test]
    fn smart_merge_preserves_unrelated_markers() {
        let base = b"a\n<<<<<<< keep\nb\n";
        let ours = b"A\n<<<<<<< keep\nb\n";
        let theirs = b"a\n<<<<<<< keep\nB\n";
        let merged = smart_merge(base, ours, theirs);
        assert_eq!(merged.as_deref(), Some(&b"A\n<<<<<<< keep\nB\n"[..]));
    }

    #[test]
    fn smart_merge_trivial_shortcuts() {
        assert_eq!(
            smart_merge(b"x\n", b"y\n", b"y\n").as_deref(),
            Some(&b"y\n"[..])
        );
        assert_eq!(
            smart_merge(b"x\n", b"x\n", b"z\n").as_deref(),
            Some(&b"z\n"[..])
        );
        assert_eq!(
            smart_merge(b"x\n", b"y\n", b"x\n").as_deref(),
            Some(&b"y\n"[..])
        );
    }
}
