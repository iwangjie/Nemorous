// Package conflict parses and resolves Git conflict markers.
package conflict

import (
	"context"
	"errors"
	"fmt"
	"os"
	"os/exec"
	"path/filepath"
	"strconv"
	"strings"

	"github.com/iwangjie/nemorous/internal/gitops"
)

// Strategy is how one conflict block is resolved.
type Strategy string

const (
	Ours        Strategy = "ours"
	Theirs      Strategy = "theirs"
	Both        Strategy = "both"
	BothReverse Strategy = "both-reverse"
	Base        Strategy = "base"
)

// Hunk is one <<<<<<< ... >>>>>>> block.
type Hunk struct {
	StartLine int
	Raw       string
	Ours      string
	Base      string
	Theirs    string
	HasBase   bool

	// Line ranges of this hunk in the resolved ours, theirs and base
	// versions (0-based), for the three-pane merge view's highlights.
	OursStart   int
	OursCount   int
	TheirsStart int
	TheirsCount int
	BaseStart   int
	BaseCount   int
}

// Document is a conflicted file split into plain text and hunks.
type Document struct {
	Prefix string
	Hunks  []Hunk
	Suffix string
	// CRLF is whether the source used \r\n endings.
	CRLF bool

	parts []part
}

type part struct {
	text string
	hunk int // -1 for plain text
}

// ErrNoMarkers reports a file with no conflict markers.
var ErrNoMarkers = errors.New("文件没有冲突标记")

// Parse splits content into text before, between and after its conflict
// hunks. It accepts both two-way and diff3 (base section) markers.
func Parse(content string) (*Document, error) {
	crlf := strings.Contains(content, "\r\n")
	lines := strings.SplitAfter(content, "\n")
	doc := &Document{CRLF: crlf}
	var plain strings.Builder
	var hunk *Hunk
	var ours, base, theirs strings.Builder
	stage := 0 // 1 ours, 2 base, 3 theirs
	flushPlain := func() {
		if plain.Len() > 0 {
			if len(doc.Hunks) == 0 && doc.Prefix == "" {
				doc.Prefix = plain.String()
			}
			doc.parts = append(doc.parts, part{text: plain.String(), hunk: -1})
			plain.Reset()
		}
	}
	for i, line := range lines {
		trimmed := strings.TrimSuffix(line, "\n")
		trimmed = strings.TrimSuffix(trimmed, "\r")
		switch {
		case strings.HasPrefix(trimmed, "<<<<<<<"):
			if hunk != nil {
				return nil, fmt.Errorf("第 %d 行：冲突块嵌套", i+1)
			}
			flushPlain()
			hunk = &Hunk{StartLine: i}
			stage = 1
			ours.Reset()
			base.Reset()
			theirs.Reset()
		case strings.HasPrefix(trimmed, "|||||||") && hunk != nil && stage == 1:
			hunk.HasBase = true
			stage = 2
		case trimmed == "=======" && hunk != nil && (stage == 1 || stage == 2):
			stage = 3
		case strings.HasPrefix(trimmed, ">>>>>>>") && hunk != nil && stage == 3:
			hunk.Ours = ours.String()
			hunk.Base = base.String()
			hunk.Theirs = theirs.String()
			hunk.Raw = rawHunk(content, hunk.StartLine, i)
			doc.Hunks = append(doc.Hunks, *hunk)
			doc.parts = append(doc.parts, part{hunk: len(doc.Hunks) - 1})
			hunk = nil
			stage = 0
		default:
			switch stage {
			case 1:
				ours.WriteString(line)
			case 2:
				base.WriteString(line)
			case 3:
				theirs.WriteString(line)
			default:
				plain.WriteString(line)
			}
		}
	}
	if hunk != nil {
		return nil, fmt.Errorf("第 %d 行：冲突块没有结束", hunk.StartLine+1)
	}
	if len(doc.Hunks) == 0 {
		return nil, ErrNoMarkers
	}
	flushPlain()
	lineCursor, oursCursor, theirsCursor, baseCursor := 0, 0, 0, 0
	for _, p := range doc.parts {
		if p.hunk < 0 {
			lineCursor += textLineCount(p.text)
			continue
		}
		h := &doc.Hunks[p.hunk]
		h.OursCount = textLineCount(h.Ours)
		h.TheirsCount = textLineCount(h.Theirs)
		h.BaseCount = textLineCount(h.Base)
		h.OursStart = lineCursor + oursCursor
		h.TheirsStart = lineCursor + theirsCursor
		h.BaseStart = lineCursor + baseCursor
		oursCursor += h.OursCount
		theirsCursor += h.TheirsCount
		baseCursor += h.BaseCount
	}
	if len(doc.parts) > 0 {
		last := doc.parts[len(doc.parts)-1]
		if last.hunk < 0 {
			doc.Suffix = last.text
		}
	}
	return doc, nil
}

func rawHunk(content string, startLine, endLine int) string {
	lines := strings.SplitAfter(content, "\n")
	if startLine < 0 || endLine >= len(lines) {
		return ""
	}
	return strings.Join(lines[startLine:endLine+1], "")
}

// Resolve replaces every hunk with strategy.
func (d *Document) Resolve(strategy Strategy) (string, error) {
	choices := make([]Strategy, len(d.Hunks))
	for i := range choices {
		choices[i] = strategy
	}
	return d.Render(choices)
}

// Render builds the resolved file. choices has one entry per hunk; when
// it is shorter, later hunks keep their markers.
func (d *Document) Render(choices []Strategy) (string, error) {
	values := make([]string, len(choices))
	set := make([]bool, len(choices))
	for i, choice := range choices {
		if choice == "" || i >= len(d.Hunks) {
			continue
		}
		text, err := hunkText(d.Hunks[i], choice)
		if err != nil {
			return "", err
		}
		values[i] = text
		set[i] = true
	}
	return d.renderReplaced(set, values), nil
}

func (d *Document) renderReplaced(set []bool, values []string) string {
	var b strings.Builder
	for _, p := range d.parts {
		if p.hunk < 0 {
			b.WriteString(p.text)
			continue
		}
		if p.hunk < len(set) && set[p.hunk] {
			b.WriteString(values[p.hunk])
		} else {
			b.WriteString(d.Hunks[p.hunk].Raw)
		}
	}
	return b.String()
}

func hunkText(h Hunk, strategy Strategy) (string, error) {
	switch strategy {
	case Ours:
		return h.Ours, nil
	case Theirs:
		return h.Theirs, nil
	case Both:
		return h.Ours + h.Theirs, nil
	case BothReverse:
		return h.Theirs + h.Ours, nil
	case Base:
		if !h.HasBase {
			return "", fmt.Errorf("该冲突没有可用的 Base 版本")
		}
		return h.Base, nil
	default:
		return "", fmt.Errorf("未知冲突解决方式 %q", strategy)
	}
}

func textLineCount(s string) int {
	if s == "" {
		return 0
	}
	n := strings.Count(s, "\n")
	if !strings.HasSuffix(s, "\n") {
		n++
	}
	return n
}

// SmartResolve automatically resolves hunks whose changes do not really
// overlap: identical sides, one side equal to the base, or line changes on
// disjoint base lines that can be merged. It returns the new content, how
// many hunks it resolved and how many real conflicts remain.
func (d *Document) SmartResolve() (string, int, int) {
	replacements := make([]string, len(d.Hunks))
	set := make([]bool, len(d.Hunks))
	resolved := 0
	remaining := 0
	for i, h := range d.Hunks {
		switch {
		case h.Ours == h.Theirs:
			replacements[i] = h.Ours
			set[i] = true
			resolved++
		case h.HasBase && h.Ours == h.Base:
			replacements[i] = h.Theirs
			set[i] = true
			resolved++
		case h.HasBase && h.Theirs == h.Base:
			replacements[i] = h.Ours
			set[i] = true
			resolved++
		default:
			if merged, ok := mergeHunkText(h); ok {
				replacements[i] = merged
				set[i] = true
				resolved++
			} else {
				remaining++
			}
		}
	}
	return d.renderReplaced(set, replacements), resolved, remaining
}

// ApplyTo replaces one hunk's marker block in content with strategy. It is
// used by the conflict editor, where each hunk is resolved independently
// and the buffer may have been edited elsewhere.
func ApplyTo(content string, h Hunk, strategy Strategy) (string, error) {
	replacement, err := hunkText(h, strategy)
	if err != nil {
		return "", err
	}
	if h.Raw == "" {
		return "", fmt.Errorf("冲突块为空")
	}
	idx := strings.Index(content, h.Raw)
	if idx < 0 {
		return "", fmt.Errorf("找不到第 %d 行的冲突块（文件已被修改，请重新加载）", h.StartLine+1)
	}
	return content[:idx] + replacement + content[idx+len(h.Raw):], nil
}

// ReadFile reads relPath under repoPath.
func ReadFile(repoPath, relPath string) (string, error) {
	p, err := safePath(repoPath, relPath)
	if err != nil {
		return "", err
	}
	data, err := os.ReadFile(p)
	if err != nil {
		return "", err
	}
	return string(data), nil
}

// Versions returns the conflict's base, ours and theirs stages from the
// index, or "" for a stage the file lacks.
func Versions(ctx context.Context, repoPath, relPath string) (base, ours, theirs string) {
	for i, dst := range []*string{&base, &ours, &theirs} {
		out, err := gitops.RunGitRaw(ctx, repoPath, "show", fmt.Sprintf(":%d:%s", i+1, relPath))
		if err == nil {
			*dst = out
		}
	}
	return
}

// WriteAndStage writes content to a conflicted file and marks it resolved.
func WriteAndStage(ctx context.Context, repoPath, relPath, content string) error {
	p, err := safePath(repoPath, relPath)
	if err != nil {
		return err
	}
	mode := os.FileMode(0o644)
	if info, err := os.Stat(p); err == nil {
		mode = info.Mode().Perm()
	}
	if err := os.MkdirAll(filepath.Dir(p), 0o755); err != nil {
		return err
	}
	if err := os.WriteFile(p, []byte(content), mode); err != nil {
		return err
	}
	return gitops.StageFile(ctx, repoPath, relPath)
}

// RemoveAndStage deletes a conflicted file and stages the deletion, for
// delete/modify conflicts resolved in favour of deletion.
func RemoveAndStage(ctx context.Context, repoPath, relPath string) error {
	p, err := safePath(repoPath, relPath)
	if err != nil {
		return err
	}
	if err := os.Remove(p); err != nil && !os.IsNotExist(err) {
		return err
	}
	if _, err := gitops.RunGit(ctx, repoPath, "rm", "-f", "--", relPath); err != nil {
		_, addErr := gitops.RunGit(ctx, repoPath, "add", "-A", "--", relPath)
		return addErr
	}
	return nil
}

// UnresolvedFiles lists files with unmerged index entries.
func UnresolvedFiles(ctx context.Context, repoPath string) ([]string, error) {
	out, err := gitops.RunGit(ctx, repoPath, "diff", "--name-only", "--diff-filter=U")
	if err != nil {
		return nil, err
	}
	var files []string
	for _, line := range strings.Split(out, "\n") {
		if strings.TrimSpace(line) != "" {
			files = append(files, strings.TrimSpace(line))
		}
	}
	return files, nil
}

func safePath(repoPath, relPath string) (string, error) {
	clean := filepath.Clean(filepath.FromSlash(relPath))
	if clean == "." || filepath.IsAbs(clean) || strings.HasPrefix(clean, ".."+string(filepath.Separator)) || clean == ".." {
		return "", fmt.Errorf("无效的文件路径 %q", relPath)
	}
	return filepath.Join(repoPath, clean), nil
}

// MergeFile runs a fresh three-way merge from the index stages with diff3
// markers, so hunk ranges and base text are available even when the
// worktree used the merge conflict style.
func MergeFile(ctx context.Context, repoPath, relPath string) (string, error) {
	base, ours, theirs := Versions(ctx, repoPath, relPath)
	if ours == "" && theirs == "" {
		return "", fmt.Errorf("索引中没有可合并的版本")
	}
	dir, err := os.MkdirTemp("", "nemorous-merge-")
	if err != nil {
		return "", err
	}
	defer os.RemoveAll(dir)
	write := func(name, content string) (string, error) {
		p := filepath.Join(dir, name)
		return p, os.WriteFile(p, []byte(content), 0o600)
	}
	oursPath, err := write("ours", ours)
	if err != nil {
		return "", err
	}
	basePath, err := write("base", base)
	if err != nil {
		return "", err
	}
	theirsPath, err := write("theirs", theirs)
	if err != nil {
		return "", err
	}
	cmd := exec.CommandContext(ctx, "git", "merge-file", "-p", "--diff3",
		"-L", "Ours", "-L", "Base", "-L", "Theirs", oursPath, basePath, theirsPath)
	out, runErr := cmd.CombinedOutput()
	if runErr != nil {
		var exitErr *exec.ExitError
		if !errors.As(runErr, &exitErr) || exitErr.ExitCode() > 1 {
			msg := strings.TrimSpace(string(out))
			if msg == "" {
				msg = runErr.Error()
			}
			return "", fmt.Errorf("%s", msg)
		}
	}
	return string(out), nil
}

// EnrichWithDiff3 copies base sections and line ranges from a diff3 parse
// of the same conflict into doc, when the hunks correspond one to one.
func EnrichWithDiff3(doc, diff3 *Document) bool {
	if doc == nil || diff3 == nil || len(doc.Hunks) != len(diff3.Hunks) {
		return false
	}
	changed := false
	normalize := func(s string) string { return strings.ReplaceAll(s, "\r\n", "\n") }
	for i := range doc.Hunks {
		src := diff3.Hunks[i]
		dst := &doc.Hunks[i]
		if !src.HasBase || normalize(src.Ours) != normalize(dst.Ours) || normalize(src.Theirs) != normalize(dst.Theirs) {
			continue
		}
		base := src.Base
		if strings.Contains(dst.Ours, "\r\n") && !strings.Contains(base, "\r\n") {
			base = strings.ReplaceAll(base, "\n", "\r\n")
		}
		dst.Base = base
		dst.HasBase = true
		dst.BaseStart = src.BaseStart
		dst.BaseCount = src.BaseCount
		changed = true
	}
	return changed
}

// StageSize returns the byte size of an index stage (:1:, :2:, :3:) for a
// conflicted path. It distinguishes a deleted side from an empty file.
func StageSize(ctx context.Context, repoPath, relPath string, stage int) (int64, error) {
	if stage < 1 || stage > 3 {
		return 0, fmt.Errorf("无效的索引阶段 %d", stage)
	}
	out, err := gitops.RunGit(ctx, repoPath, "cat-file", "-s", fmt.Sprintf(":%d:%s", stage, relPath))
	if err != nil {
		return 0, err
	}
	n, err := strconv.ParseInt(strings.TrimSpace(out), 10, 64)
	if err != nil {
		return 0, err
	}
	return n, nil
}
