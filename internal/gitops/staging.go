package gitops

import (
	"context"
	"fmt"
	"regexp"
	"strconv"
	"strings"
)

var hunkHeaderRe = regexp.MustCompile(`^@@ -(\d+)(?:,(\d+))? \+(\d+)(?:,(\d+))? @@`)

type rawHunk struct {
	header             string
	oldStart, oldCount int
	newStart, newCount int
	lines              []string
}

type rawDiff struct {
	preamble []string
	hunks    []rawHunk
}

func parseRawDiff(raw string) (*rawDiff, error) {
	d := &rawDiff{}
	lines := strings.Split(strings.TrimSuffix(raw, "\n"), "\n")
	inHunk := false
	for _, line := range lines {
		if strings.HasPrefix(line, "@@ ") {
			m := hunkHeaderRe.FindStringSubmatch(line)
			if m == nil {
				return nil, fmt.Errorf("无法解析 diff hunk：%s", line)
			}
			h := rawHunk{header: line, oldStart: atoiDefault(m[1], 0), oldCount: atoiDefault(m[2], 1), newStart: atoiDefault(m[3], 0), newCount: atoiDefault(m[4], 1)}
			d.hunks = append(d.hunks, h)
			inHunk = true
			continue
		}
		if inHunk {
			i := len(d.hunks) - 1
			d.hunks[i].lines = append(d.hunks[i].lines, line)
		} else {
			d.preamble = append(d.preamble, line)
		}
	}
	if len(d.hunks) == 0 {
		return nil, fmt.Errorf("没有可暂存的差异")
	}
	return d, nil
}

func atoiDefault(s string, def int) int {
	if s == "" {
		return def
	}
	n, err := strconv.Atoi(s)
	if err != nil {
		return def
	}
	return n
}

func (d *rawDiff) patch(indices []int) string {
	var b strings.Builder
	for _, line := range d.preamble {
		b.WriteString(line)
		b.WriteByte('\n')
	}
	for _, i := range indices {
		b.WriteString(d.hunks[i].header)
		b.WriteByte('\n')
		for _, line := range d.hunks[i].lines {
			b.WriteString(line)
			b.WriteByte('\n')
		}
	}
	return b.String()
}

func diffText(ctx context.Context, repoPath, rel string, staged bool, zeroContext bool) (string, error) {
	args := []string{"diff", "--no-ext-diff", "--no-color", "--no-renames"}
	if staged {
		args = append(args, "--cached")
	}
	if zeroContext {
		args = append(args, "--unified=0")
	}
	args = append(args, "--", rel)
	return RunGitRaw(ctx, repoPath, args...)
}

func applyCached(ctx context.Context, repoPath, patch string, reverse bool) error {
	args := []string{"apply", "--cached", "--unidiff-zero", "--recount", "--whitespace=nowarn"}
	if reverse {
		args = append(args, "--reverse")
	}
	args = append(args, "-")
	out, err := runGitInput(ctx, repoPath, patch, args...)
	if err != nil {
		msg := strings.TrimSpace(out)
		if msg == "" {
			msg = err.Error()
		}
		return fmt.Errorf("应用补丁失败：%s", msg)
	}
	return nil
}

// StageHunks stages the selected zero-based hunks of a worktree file.
func StageHunks(ctx context.Context, repoPath, rel string, indices []int) error {
	raw, err := diffText(ctx, repoPath, rel, false, false)
	if err != nil {
		return err
	}
	d, err := parseRawDiff(raw)
	if err != nil {
		return err
	}
	sel, err := validHunkIndices(d, indices)
	if err != nil {
		return err
	}
	return applyCached(ctx, repoPath, d.patch(sel), false)
}

// UnstageHunks reverses the selected zero-based hunks of an index file.
func UnstageHunks(ctx context.Context, repoPath, rel string, indices []int) error {
	raw, err := diffText(ctx, repoPath, rel, true, false)
	if err != nil {
		return err
	}
	d, err := parseRawDiff(raw)
	if err != nil {
		return err
	}
	sel, err := validHunkIndices(d, indices)
	if err != nil {
		return err
	}
	return applyCached(ctx, repoPath, d.patch(sel), true)
}

func validHunkIndices(d *rawDiff, indices []int) ([]int, error) {
	seen := make(map[int]bool, len(indices))
	out := make([]int, 0, len(indices))
	for _, i := range indices {
		if i < 0 || i >= len(d.hunks) {
			return nil, fmt.Errorf("变更块序号越界：%d", i)
		}
		if !seen[i] {
			seen[i] = true
			out = append(out, i)
		}
	}
	if len(out) == 0 {
		return nil, fmt.Errorf("没有选择变更块")
	}
	return out, nil
}

// StageLines stages a contiguous run of added/deleted lines. start and end are
// one-based line numbers on the selected side. Replacements that change both
// sides must be staged as a whole hunk so Git never receives an unbalanced
// patch; pure additions and deletions can be staged line by line.
func StageLines(ctx context.Context, repoPath, rel string, oldSide bool, start, end int) error {
	if start < 1 || end < start {
		return fmt.Errorf("无效的行范围：%d-%d", start, end)
	}
	raw, err := diffText(ctx, repoPath, rel, false, true)
	if err != nil {
		return err
	}
	d, err := parseRawDiff(raw)
	if err != nil {
		return err
	}
	var patch strings.Builder
	for _, line := range d.preamble {
		patch.WriteString(line)
		patch.WriteByte('\n')
	}
	found := false
	for _, h := range d.hunks {
		if h.oldCount > 0 && h.newCount > 0 {
			// A one-for-one replacement can be staged a line at a time when
			// the old and new blocks have the same length. Unequal blocks
			// need the whole hunk so Git never receives an unbalanced patch.
			hs, he := h.newStart, h.newStart+h.newCount-1
			if oldSide {
				hs, he = h.oldStart, h.oldStart+h.oldCount-1
			}
			if end < hs || start > he {
				continue
			}
			if h.oldCount != h.newCount {
				return fmt.Errorf("该变更块的行数不对称，请使用「暂存块」")
			}
			ss, ee := max(start, hs), min(end, he)
			skip := ss - hs
			count := ee - ss + 1
			var oldBody, newBody []string
			for _, ln := range h.lines {
				switch {
				case strings.HasPrefix(ln, "-") && !strings.HasPrefix(ln, "---"):
					oldBody = append(oldBody, ln)
				case strings.HasPrefix(ln, "+") && !strings.HasPrefix(ln, "+++"):
					newBody = append(newBody, ln)
				}
			}
			if skip < 0 || skip+count > len(oldBody) || skip+count > len(newBody) {
				continue
			}
			fmt.Fprintf(&patch, "@@ -%d,%d +%d,%d @@\n", h.oldStart+skip, count, h.newStart+skip, count)
			for i := 0; i < count; i++ {
				patch.WriteString(oldBody[skip+i])
				patch.WriteByte('\n')
				patch.WriteString(newBody[skip+i])
				patch.WriteByte('\n')
			}
			found = true
			continue
		}
		if oldSide {
			if h.oldCount == 0 {
				continue
			}
			hs, he := h.oldStart, h.oldStart+h.oldCount-1
			if end < hs || start > he {
				continue
			}
			ss, ee := max(start, hs), min(end, he)
			var body []string
			for _, ln := range h.lines {
				if strings.HasPrefix(ln, "-") && !strings.HasPrefix(ln, "---") {
					body = append(body, ln)
				}
			}
			skip := ss - hs
			count := ee - ss + 1
			if skip < 0 || skip+count > len(body) {
				continue
			}
			fmt.Fprintf(&patch, "@@ -%d,%d +%d,0 @@\n", ss, count, h.newStart)
			for _, ln := range body[skip : skip+count] {
				patch.WriteString(ln)
				patch.WriteByte('\n')
			}
			found = true
			continue
		}
		if h.newCount == 0 {
			continue
		}
		hs, he := h.newStart, h.newStart+h.newCount-1
		if end < hs || start > he {
			continue
		}
		ss, ee := max(start, hs), min(end, he)
		var body []string
		for _, ln := range h.lines {
			if strings.HasPrefix(ln, "+") && !strings.HasPrefix(ln, "+++") {
				body = append(body, ln)
			}
		}
		skip := ss - hs
		count := ee - ss + 1
		if skip < 0 || skip+count > len(body) {
			continue
		}
		fmt.Fprintf(&patch, "@@ -%d,0 +%d,%d @@\n", h.oldStart, ss, count)
		for _, ln := range body[skip : skip+count] {
			patch.WriteString(ln)
			patch.WriteByte('\n')
		}
		found = true
	}
	if !found {
		return fmt.Errorf("所选行不在可拆分的差异块中")
	}
	return applyCached(ctx, repoPath, patch.String(), false)
}
