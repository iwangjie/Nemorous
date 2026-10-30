package main

import (
	"bytes"
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"os"
	"os/exec"
	"path/filepath"
	"regexp"
	"sort"
	"strings"
	"sync"
	"time"
	"unicode/utf8"

	"github.com/egoist/mygo/ui"
	"github.com/iwangjie/nemorous/internal/gitops"
)

const (
	globalSearchLimit    = 300
	globalSearchPerFile  = 40
	globalSearchMaxBytes = 2 << 20
	globalSearchDebounce = 220 * time.Millisecond
	globalSearchWorkers  = 8
)

type globalSearchOptions struct {
	CaseSensitive bool
	Regex         bool
}

type globalSearchHit struct {
	AbsPath   string
	Line      int
	Column    int
	Snippet   string
	MatchFrom int
	MatchTo   int

	RepoID   string
	RepoName string
	RepoPath string
	RelPath  string
	Monogram string
	ColorIdx int
}

type globalSearchGroup struct {
	RepoID   string
	RepoName string
	RelPath  string
	Monogram string
	ColorIdx int
	Hits     []globalSearchHit
}

type globalSearchOutcome struct {
	Hits      []globalSearchHit
	Truncated bool
}

type ripgrepEvent struct {
	Type string `json:"type"`
	Data struct {
		Path struct {
			Text string `json:"text"`
		} `json:"path"`
		Lines struct {
			Text string `json:"text"`
		} `json:"lines"`
		LineNumber int `json:"line_number"`
		Submatches []struct {
			Start int `json:"start"`
			End   int `json:"end"`
		} `json:"submatches"`
	} `json:"data"`
}

// openGlobalSearch resets the find-in-files session and focuses its query
// field. The actual search starts after the user types.
func (a *App) openGlobalSearch() {
	a.showCmdPalette = false
	a.showBranchPicker = false
	a.showGlobalSearch = true
	a.globalSearchQuery = ""
	a.globalSearchGroups = nil
	a.globalSearchTotal = 0
	a.globalSearchRunning = false
	a.globalSearchErr = ""
	a.globalSearchPartial = false
	a.cancelGlobalSearch()
	a.globalSearchHandle.Focus()
}

func (a *App) closeGlobalSearch() {
	a.showGlobalSearch = false
	a.cancelGlobalSearch()
}

func (a *App) cancelGlobalSearch() {
	a.globalSearchSeq++
	if a.globalSearchCancel != nil {
		a.globalSearchCancel()
		a.globalSearchCancel = nil
	}
	a.globalSearchRunning = false
}

// scheduleGlobalSearch debounces keystrokes and discards stale results with a
// generation counter, so typing quickly never flashes an older match set.
func (a *App) scheduleGlobalSearch() {
	a.cancelGlobalSearch()
	a.globalSearchGroups = nil
	a.globalSearchTotal = 0
	a.globalSearchErr = ""
	a.globalSearchPartial = false

	query := strings.TrimSpace(a.globalSearchQuery)
	if query == "" {
		return
	}
	opts := globalSearchOptions{CaseSensitive: a.globalSearchCase, Regex: a.globalSearchRegex}
	if opts.Regex {
		if _, err := regexp.Compile(query); err != nil {
			a.globalSearchErr = "正则表达式无效：" + err.Error()
			return
		}
	}
	root := strings.TrimSpace(a.cfg.WorkspacePath)
	if root == "" {
		a.globalSearchErr = "尚未选择工作区"
		return
	}

	files := append([]gitops.WorkspaceFile(nil), a.workspaceFiles...)
	ctx, cancel := context.WithCancel(context.Background())
	a.globalSearchCancel = cancel
	a.globalSearchRunning = true
	gen := a.globalSearchSeq

	go func() {
		defer cancel()
		select {
		case <-time.After(globalSearchDebounce):
		case <-ctx.Done():
			return
		}
		out, err := searchWorkspace(ctx, root, files, query, opts)
		a.runUI(func() {
			if gen != a.globalSearchSeq {
				return
			}
			a.globalSearchCancel = nil
			a.globalSearchRunning = false
			if err != nil {
				if errors.Is(err, context.Canceled) {
					return
				}
				a.globalSearchErr = err.Error()
				return
			}
			groups := a.groupGlobalSearchHits(out.Hits)
			total := 0
			for _, grp := range groups {
				total += len(grp.Hits)
			}
			a.globalSearchGroups = groups
			a.globalSearchTotal = total
			a.globalSearchPartial = out.Truncated
		})
	}()
}

// groupGlobalSearchHits maps absolute paths back to repository-relative files
// and produces one section per file, in stable repo/path order.
func (a *App) groupGlobalSearchHits(hits []globalSearchHit) []globalSearchGroup {
	byFile := make(map[string]*globalSearchGroup)
	order := make([]string, 0, len(hits))
	for _, hit := range hits {
		wf, ok := a.workspaceFileForSearch(hit.AbsPath)
		if !ok {
			continue
		}
		hit.RepoID = wf.RepoID
		hit.RepoName = wf.RepoName
		hit.RepoPath = wf.RepoPath
		hit.RelPath = wf.RelPath
		hit.Monogram = wf.Monogram
		hit.ColorIdx = wf.ColorIdx
		key := wf.RepoID + "\x00" + wf.RelPath
		grp := byFile[key]
		if grp == nil {
			grp = &globalSearchGroup{
				RepoID: wf.RepoID, RepoName: wf.RepoName, RelPath: wf.RelPath,
				Monogram: wf.Monogram, ColorIdx: wf.ColorIdx,
			}
			byFile[key] = grp
			order = append(order, key)
		}
		grp.Hits = append(grp.Hits, hit)
	}
	groups := make([]globalSearchGroup, 0, len(order))
	for _, key := range order {
		grp := byFile[key]
		sort.SliceStable(grp.Hits, func(i, j int) bool {
			if grp.Hits[i].Line == grp.Hits[j].Line {
				return grp.Hits[i].Column < grp.Hits[j].Column
			}
			return grp.Hits[i].Line < grp.Hits[j].Line
		})
		groups = append(groups, *grp)
	}
	sort.SliceStable(groups, func(i, j int) bool {
		if groups[i].RepoName != groups[j].RepoName {
			return strings.ToLower(groups[i].RepoName) < strings.ToLower(groups[j].RepoName)
		}
		return groups[i].RelPath < groups[j].RelPath
	})
	return groups
}

// workspaceFileForSearch resolves an absolute path to the longest containing
// repository. Files directly under the workspace root still open in the code
// viewer under a synthetic workspace section.
func (a *App) workspaceFileForSearch(absPath string) (gitops.WorkspaceFile, bool) {
	if strings.TrimSpace(absPath) == "" {
		return gitops.WorkspaceFile{}, false
	}
	abs := filepath.Clean(absPath)
	bestRoot := ""
	var best gitops.WorkspaceFile
	for _, st := range a.statuses {
		root := filepath.Clean(st.Repo.AbsPath)
		if root == "" || len(root) <= len(bestRoot) {
			continue
		}
		if abs == root || !strings.HasPrefix(abs, root+string(filepath.Separator)) {
			continue
		}
		rel, err := filepath.Rel(root, abs)
		if err != nil || rel == "." || strings.HasPrefix(rel, ".."+string(filepath.Separator)) {
			continue
		}
		bestRoot = root
		best = a.normalizeWorkspaceFile(gitops.WorkspaceFile{
			RepoID: st.Repo.ID, RepoName: st.Repo.Name, RepoPath: root,
			RelPath: filepath.ToSlash(rel), AbsPath: abs,
		})
	}
	if bestRoot != "" {
		return best, true
	}
	for _, wf := range a.workspaceFiles {
		if filepath.Clean(wf.AbsPath) == abs {
			return a.normalizeWorkspaceFile(wf), true
		}
	}
	root := filepath.Clean(a.cfg.WorkspacePath)
	if root == "" || root == "." {
		return gitops.WorkspaceFile{}, false
	}
	rel, err := filepath.Rel(root, abs)
	if err != nil || rel == "." || rel == ".." || strings.HasPrefix(rel, ".."+string(filepath.Separator)) {
		return gitops.WorkspaceFile{}, false
	}
	name := filepath.Base(root)
	if name == "" || name == "." {
		name = "workspace"
	}
	return gitops.WorkspaceFile{
		RepoID: "__workspace__", RepoName: name, RepoPath: root,
		RelPath: filepath.ToSlash(rel), AbsPath: abs, Monogram: searchMonogram(name),
	}, true
}

func searchMonogram(name string) string {
	for _, r := range name {
		if r == ' ' || r == '-' || r == '_' {
			continue
		}
		return strings.ToUpper(string(r))
	}
	return "W"
}

// openGlobalSearchHit opens the matching file and scrolls the editor to the
// exact line. The tab is reused when already open, preserving unsaved edits.
func (a *App) openGlobalSearchHit(hit globalSearchHit) {
	a.closeGlobalSearch()
	if hit.AbsPath == "" {
		return
	}
	a.selectCodeFile(gitops.WorkspaceFile{
		RepoID: hit.RepoID, RepoName: hit.RepoName, RepoPath: hit.RepoPath,
		RelPath: hit.RelPath, AbsPath: hit.AbsPath, Monogram: hit.Monogram, ColorIdx: hit.ColorIdx,
	})
	a.activeTab = TabCode
	if a.codeTabIdx >= 0 && a.codeTabIdx < len(a.codeTabs) {
		if st := a.codeTabs[a.codeTabIdx].state; st != nil {
			line := hit.Line - 1
			if line < 0 {
				line = 0
			}
			st.ScrollTo(line)
		}
	}
}

// searchWorkspace prefers ripgrep for speed and falls back to a bounded Go
// scan so the app still works on machines without rg installed.
func searchWorkspace(ctx context.Context, root string, files []gitops.WorkspaceFile, query string, opts globalSearchOptions) (globalSearchOutcome, error) {
	if strings.TrimSpace(root) == "" {
		return globalSearchOutcome{}, errors.New("尚未选择工作区")
	}
	if strings.TrimSpace(query) == "" {
		return globalSearchOutcome{}, nil
	}
	if opts.Regex {
		if _, err := regexp.Compile(query); err != nil {
			return globalSearchOutcome{}, fmt.Errorf("正则表达式无效：%w", err)
		}
	}
	if _, err := exec.LookPath("rg"); err == nil {
		out, rgErr := ripgrepSearch(ctx, root, query, opts)
		if rgErr == nil {
			return out, nil
		}
		if ctx.Err() != nil {
			return globalSearchOutcome{}, ctx.Err()
		}
		// A failed rg (permissions, old version, transient I/O) should not make
		// the feature unusable; the Go scan is slower but deterministic.
	}
	return goSearch(ctx, files, query, opts)
}

func ripgrepSearch(ctx context.Context, root, query string, opts globalSearchOptions) (globalSearchOutcome, error) {
	rg, err := exec.LookPath("rg")
	if err != nil {
		return globalSearchOutcome{}, err
	}
	args := []string{
		"--json", "--hidden", "--glob=!**/.git/**", "--no-messages",
		fmt.Sprintf("--max-count=%d", globalSearchPerFile),
		fmt.Sprintf("--max-filesize=%d", globalSearchMaxBytes),
	}
	if opts.CaseSensitive {
		args = append(args, "--case-sensitive")
	} else {
		args = append(args, "--ignore-case")
	}
	if !opts.Regex {
		args = append(args, "--fixed-strings")
	}
	args = append(args, "--regexp", query, ".")

	procCtx, cancel := context.WithCancel(ctx)
	defer cancel()
	cmd := exec.CommandContext(procCtx, rg, args...)
	cmd.Dir = root
	var stderr bytes.Buffer
	cmd.Stderr = &stderr
	stdout, err := cmd.StdoutPipe()
	if err != nil {
		return globalSearchOutcome{}, err
	}
	if err := cmd.Start(); err != nil {
		return globalSearchOutcome{}, err
	}

	out := globalSearchOutcome{Hits: make([]globalSearchHit, 0, 64)}
	dec := json.NewDecoder(stdout)
	var readErr error
	for len(out.Hits) < globalSearchLimit {
		var ev ripgrepEvent
		if err := dec.Decode(&ev); err != nil {
			if !errors.Is(err, io.EOF) {
				readErr = err
			}
			break
		}
		if ev.Type != "match" {
			continue
		}
		lineText := strings.TrimRight(ev.Data.Lines.Text, "\r\n")
		start, end := 0, 0
		if len(ev.Data.Submatches) > 0 {
			start = max(0, ev.Data.Submatches[0].Start)
			end = max(start, ev.Data.Submatches[0].End)
		}
		if start > len(lineText) {
			start = len(lineText)
		}
		if end > len(lineText) {
			end = len(lineText)
		}
		snippet, from, to := searchSnippet(lineText, start, end)
		abs := ev.Data.Path.Text
		if !filepath.IsAbs(abs) {
			abs = filepath.Join(root, filepath.FromSlash(abs))
		}
		out.Hits = append(out.Hits, globalSearchHit{
			AbsPath: filepath.Clean(abs), Line: ev.Data.LineNumber, Column: start + 1,
			Snippet: snippet, MatchFrom: from, MatchTo: to,
		})
	}
	if len(out.Hits) >= globalSearchLimit {
		out.Truncated = true
		cancel()
	}
	waitErr := cmd.Wait()
	if ctx.Err() != nil {
		return globalSearchOutcome{}, ctx.Err()
	}
	if out.Truncated {
		return out, nil
	}
	if readErr != nil {
		return out, fmt.Errorf("读取 ripgrep 结果：%w", readErr)
	}
	if waitErr != nil {
		var exitErr *exec.ExitError
		if errors.As(waitErr, &exitErr) && exitErr.ExitCode() == 1 {
			return out, nil
		}
		msg := strings.TrimSpace(stderr.String())
		if msg != "" {
			return out, fmt.Errorf("ripgrep 搜索失败：%s", msg)
		}
		return out, fmt.Errorf("ripgrep 搜索失败：%w", waitErr)
	}
	return out, nil
}

func goSearch(ctx context.Context, files []gitops.WorkspaceFile, query string, opts globalSearchOptions) (globalSearchOutcome, error) {
	pattern := query
	if !opts.Regex {
		pattern = regexp.QuoteMeta(pattern)
	}
	if !opts.CaseSensitive {
		pattern = "(?i)" + pattern
	}
	re, err := regexp.Compile(pattern)
	if err != nil {
		return globalSearchOutcome{}, fmt.Errorf("搜索表达式无效：%w", err)
	}

	runCtx, stop := context.WithCancel(ctx)
	defer stop()
	jobs := make(chan gitops.WorkspaceFile)
	var mu sync.Mutex
	hits := make([]globalSearchHit, 0, 64)
	seen := make(map[string]bool)
	var wg sync.WaitGroup
	for range globalSearchWorkers {
		wg.Add(1)
		go func() {
			defer wg.Done()
			for wf := range jobs {
				if runCtx.Err() != nil {
					return
				}
				abs := wf.AbsPath
				if abs == "" {
					abs = filepath.Join(wf.RepoPath, filepath.FromSlash(wf.RelPath))
				}
				abs = filepath.Clean(abs)
				info, statErr := os.Stat(abs)
				if statErr != nil || info.IsDir() || info.Size() > globalSearchMaxBytes {
					continue
				}
				data, readErr := os.ReadFile(abs)
				if readErr != nil || bytes.IndexByte(data, 0) >= 0 {
					continue
				}
				lines := strings.Split(string(data), "\n")
				for idx, line := range lines {
					line = strings.TrimSuffix(line, "\r")
					for _, loc := range re.FindAllStringIndex(line, globalSearchPerFile) {
						mu.Lock()
						if len(hits) >= globalSearchLimit {
							mu.Unlock()
							stop()
							return
						}
						snippet, from, to := searchSnippet(line, loc[0], loc[1])
						hits = append(hits, globalSearchHit{
							AbsPath: abs, Line: idx + 1, Column: loc[0] + 1,
							Snippet: snippet, MatchFrom: from, MatchTo: to,
						})
						mu.Unlock()
					}
				}
			}
		}()
	}

	go func() {
		defer close(jobs)
		for _, wf := range files {
			abs := wf.AbsPath
			if abs == "" {
				abs = filepath.Join(wf.RepoPath, filepath.FromSlash(wf.RelPath))
			}
			abs = filepath.Clean(abs)
			if abs == "." || seen[abs] {
				continue
			}
			seen[abs] = true
			select {
			case jobs <- wf:
			case <-runCtx.Done():
				return
			}
		}
	}()
	wg.Wait()
	if ctx.Err() != nil {
		return globalSearchOutcome{}, ctx.Err()
	}
	mu.Lock()
	truncated := len(hits) >= globalSearchLimit
	mu.Unlock()
	return globalSearchOutcome{Hits: hits, Truncated: truncated}, nil
}

// searchSnippet keeps the match visible while limiting very long minified
// lines. Offsets are byte offsets into the trimmed snippet.
func searchSnippet(line string, start, end int) (string, int, int) {
	if start < 0 {
		start = 0
	}
	if start > len(line) {
		start = len(line)
	}
	if end < start {
		end = start
	}
	if end > len(line) {
		end = len(line)
	}
	const before, after = 76, 120
	from := start - before
	if from < 0 {
		from = 0
	}
	for from > 0 && !utf8.RuneStart(line[from]) {
		from--
	}
	to := end + after
	if to > len(line) {
		to = len(line)
	}
	for to < len(line) && !utf8.RuneStart(line[to]) {
		to++
	}
	prefix, suffix := "", ""
	if from > 0 {
		prefix = "…"
	}
	if to < len(line) {
		suffix = "…"
	}
	snippet := prefix + line[from:to] + suffix
	matchFrom := len(prefix) + start - from
	matchTo := matchFrom + (end - start)
	return snippet, matchFrom, matchTo
}

func (a *App) buildGlobalSearchModal(c *ui.Context, p Palette) {
	openAtBuild := a.showGlobalSearch
	ui.DialogBase(c, &a.showGlobalSearch, func(backdrop, panel ui.Element) {
		backdrop.Background(ui.RGBA(20, 22, 30, 0.36))
		panel.Width(940).Padding(14).Gap(10).Radius(12).Background(p.Surface).Border(1, p.Border)

		ui.Row(c).Gap(8).AlignItems(ui.Center).Children(func() {
			drawIcon(c, icoSearch, 15, p.TextMuted)
			ui.Text(c, "在文件中查找").FontSize(14).FontWeight(weightBold).TextColor(p.TextPrimary)
			ui.Text(c, "跨仓库文件内容").FontSize(11).TextColor(p.TextMuted)
			ui.Spacer(c)
			ui.Text(c, "⇧⌘F").FontSize(10.5).TextColor(p.TextMuted)
		})

		ui.Row(c).Gap(6).AlignItems(ui.Center).Children(func() {
			field := ui.SearchField(c.Key("global-search-q"), &a.globalSearchQuery).
				Bind(&a.globalSearchHandle).Placeholder("搜索文件内容，支持正则与大小写…").AutoFocus().Grow(1)
			if field.Changed() {
				a.scheduleGlobalSearch()
			}
			if field.Submitted() && len(a.globalSearchGroups) > 0 && len(a.globalSearchGroups[0].Hits) > 0 {
				a.openGlobalSearchHit(a.globalSearchGroups[0].Hits[0])
				return
			}
			searchToggle := func(key, label, tip string, active bool, onClick func()) {
				kc := c.Key(key)
				bg, fg := p.Surface2, p.TextMuted
				if active {
					bg, fg = p.AccentSoft, p.AccentSoftText
				}
				el := ui.Row(kc).Padding(4, 8).Radius(5).Background(bg).Cursor(ui.CursorPointer).
					AlignItems(ui.Center).Tooltip(tip).Label(tip)
				if el.Hovered() {
					el.Background(p.SurfaceHover)
				}
				if el.Clicked() && onClick != nil {
					onClick()
				}
				el.Children(func() {
					ui.Text(kc, label).Font("SF Mono, Menlo, monospace").FontSize(11).
						FontWeight(weightSemibold).TextColor(fg).SingleLine()
				})
			}
			searchToggle("global-case", "Aa", "区分大小写", a.globalSearchCase, func() {
				a.globalSearchCase = !a.globalSearchCase
				a.scheduleGlobalSearch()
				a.globalSearchHandle.Focus()
			})
			searchToggle("global-regex", ".*", "使用正则表达式", a.globalSearchRegex, func() {
				a.globalSearchRegex = !a.globalSearchRegex
				a.scheduleGlobalSearch()
				a.globalSearchHandle.Focus()
			})
			miniBtn(c, p, "关闭", func() { a.closeGlobalSearch() })
		})

		status, statusCol := a.globalSearchStatusText()
		ui.Row(c).Gap(6).AlignItems(ui.Center).Children(func() {
			if a.globalSearchRunning {
				ui.Text(c, "●").FontSize(9).TextColor(p.Accent)
			}
			ui.Text(c, status).FontSize(11).TextColor(statusCol)
			ui.Spacer(c)
			if a.globalSearchErr != "" {
				miniBtn(c, p, "清除", func() {
					a.globalSearchQuery = ""
					a.scheduleGlobalSearch()
					a.globalSearchHandle.Focus()
				})
			}
		})

		ui.Scroll(c).MaxHeight(470).Grow(1).Gap(2).Children(func() {
			for _, grp := range a.globalSearchGroups {
				grpCopy := grp
				badge := repoBadgeColors(grpCopy.ColorIdx, p.IsDark)
				headerKc := c.Key("gs-file-" + grpCopy.RepoID + "-" + grpCopy.RelPath)
				ui.Row(headerKc).Height(26).Padding(0, 8).Gap(6).AlignItems(ui.Center).Children(func() {
					badgeBox(headerKc, grpCopy.Monogram, badge.Text, badge.Bg, ui.Color{}, 9, true)
					ui.Text(headerKc, grpCopy.RepoName).FontSize(11).TextColor(p.TextMuted)
					ui.Text(headerKc, grpCopy.RelPath).FontSize(12).FontWeight(weightSemibold).TextColor(p.TextPrimary).SingleLine().Grow(1)
					ui.Text(headerKc, fmt.Sprintf("%d", len(grpCopy.Hits))).FontSize(10.5).TextColor(p.TextMuted)
				})
				for hitIdx, hit := range grpCopy.Hits {
					hitCopy := hit
					hitKey := fmt.Sprintf("gs-hit-%s-%s-%d-%d", grpCopy.RepoID, grpCopy.RelPath, hitIdx, hitCopy.Line)
					kc := c.Key(hitKey)
					row := ui.Row(kc).Height(25).Padding(0, 8, 0, 28).Gap(8).Radius(4).
						Cursor(ui.CursorPointer).AlignItems(ui.Center).Tooltip(fmt.Sprintf("%s:%d", grpCopy.RelPath, hitCopy.Line))
					if row.Hovered() {
						row.Background(p.SurfaceHover)
					}
					if row.Clicked() {
						a.openGlobalSearchHit(hitCopy)
					}
					row.Children(func() {
						ui.Text(c.Key(hitKey+"-ln"), fmt.Sprintf("%d", hitCopy.Line)).Font("SF Mono, Menlo, monospace").
							FontSize(10.5).TextColor(p.TextMuted).Width(40)
						ui.Row(c.Key(hitKey + "-code")).Gap(0).Grow(1).AlignItems(ui.Center).Children(func() {
							before, match, after := splitSearchSnippet(hitCopy)
							if before != "" {
								ui.Text(c.Key(hitKey+"-before"), before).Font("SF Mono, Menlo, monospace").FontSize(11.5).
									TextColor(p.TextSecondary).SingleLine()
							}
							if match != "" {
								ui.Text(c.Key(hitKey+"-match"), match).Font("SF Mono, Menlo, monospace").FontSize(11.5).
									FontWeight(weightSemibold).TextColor(p.Accent).SingleLine()
							}
							if after != "" {
								ui.Text(c.Key(hitKey+"-after"), after).Font("SF Mono, Menlo, monospace").FontSize(11.5).
									TextColor(p.TextSecondary).SingleLine()
							}
						})
					})
				}
			}
		})
	})
	if openAtBuild && !a.showGlobalSearch {
		a.closeGlobalSearch()
	}
}

func (a *App) globalSearchStatusText() (string, ui.Color) {
	p := a.palette()
	switch {
	case a.globalSearchErr != "":
		return a.globalSearchErr, p.Danger
	case a.globalSearchRunning:
		return "正在搜索…", p.TextMuted
	case strings.TrimSpace(a.globalSearchQuery) == "":
		return "输入关键字，跨仓库搜索文件内容", p.TextMuted
	case a.globalSearchTotal == 0:
		return "没有匹配结果", p.TextMuted
	default:
		status := fmt.Sprintf("%d 个文件 · %d 处匹配", len(a.globalSearchGroups), a.globalSearchTotal)
		if a.globalSearchPartial {
			status += "（结果已截断）"
		}
		return status, p.TextMuted
	}
}

func splitSearchSnippet(hit globalSearchHit) (before, match, after string) {
	from := max(0, min(hit.MatchFrom, len(hit.Snippet)))
	to := max(from, min(hit.MatchTo, len(hit.Snippet)))
	return hit.Snippet[:from], hit.Snippet[from:to], hit.Snippet[to:]
}
