package conflict

import "strings"

type lineOp struct {
	start int
	end   int
	lines []string
}

// mergeHunkText merges the non-overlapping line changes of both sides
// within one conflict block. It reports false when the sides edit the same
// base lines differently.
func mergeHunkText(h Hunk) (string, bool) {
	if !h.HasBase {
		return "", false
	}
	base := splitLines(h.Base)
	ours := splitLines(h.Ours)
	theirs := splitLines(h.Theirs)
	if len(base)*max(len(ours), len(theirs)) > 4_000_000 {
		return "", false
	}
	ourOps := diffLineOps(base, ours)
	theirOps := diffLineOps(base, theirs)
	type opPair struct {
		a, b lineOp
	}
	merged := make([]lineOp, 0, len(ourOps)+len(theirOps))
	for _, op := range ourOps {
		merged = append(merged, op)
	}
	for _, op := range theirOps {
		conflictOp := false
		for _, existing := range merged {
			if opsOverlap(existing, op) && !sameOp(existing, op) {
				conflictOp = true
				break
			}
		}
		if conflictOp {
			return "", false
		}
		duplicate := false
		for _, existing := range merged {
			if sameOp(existing, op) {
				duplicate = true
				break
			}
		}
		if !duplicate {
			merged = append(merged, op)
		}
	}
	// Sort by base start.
	for i := 1; i < len(merged); i++ {
		for j := i; j > 0 && merged[j].start < merged[j-1].start; j-- {
			merged[j], merged[j-1] = merged[j-1], merged[j]
		}
	}
	var out []string
	cursor := 0
	for _, op := range merged {
		if op.start < cursor {
			return "", false
		}
		out = append(out, base[cursor:op.start]...)
		out = append(out, op.lines...)
		cursor = op.end
	}
	out = append(out, base[cursor:]...)
	return strings.Join(out, ""), true
}

func sameOp(a, b lineOp) bool {
	if a.start != b.start || a.end != b.end || len(a.lines) != len(b.lines) {
		return false
	}
	for i := range a.lines {
		if a.lines[i] != b.lines[i] {
			return false
		}
	}
	return true
}

func opsOverlap(a, b lineOp) bool {
	aInsert := a.start == a.end
	bInsert := b.start == b.end
	switch {
	case aInsert && bInsert:
		return a.start == b.start
	case aInsert:
		return b.start <= a.start && a.start < b.end
	case bInsert:
		return a.start <= b.start && b.start < a.end
	default:
		return a.start < b.end && b.start < a.end
	}
}

func diffLineOps(base, side []string) []lineOp {
	n, m := len(base), len(side)
	dp := make([][]int, n+1)
	for i := range dp {
		dp[i] = make([]int, m+1)
	}
	for i := n - 1; i >= 0; i-- {
		for j := m - 1; j >= 0; j-- {
			if base[i] == side[j] {
				dp[i][j] = dp[i+1][j+1] + 1
			} else if dp[i+1][j] >= dp[i][j+1] {
				dp[i][j] = dp[i+1][j]
			} else {
				dp[i][j] = dp[i][j+1]
			}
		}
	}
	var ops []lineOp
	i, j := 0, 0
	for i < n || j < m {
		if i < n && j < m && base[i] == side[j] {
			i++
			j++
			continue
		}
		start := i
		var repl []string
		for i < n || j < m {
			if i < n && j < m && base[i] == side[j] {
				break
			}
			if j < m && (i == n || dp[i][j+1] >= dp[i+1][j]) {
				repl = append(repl, side[j])
				j++
			} else {
				i++
			}
		}
		if start != i || len(repl) > 0 {
			ops = append(ops, lineOp{start: start, end: i, lines: repl})
		}
	}
	return ops
}

func splitLines(s string) []string {
	if s == "" {
		return nil
	}
	lines := strings.SplitAfter(s, "\n")
	if len(lines) > 0 && lines[len(lines)-1] == "" {
		lines = lines[:len(lines)-1]
	}
	return lines
}
