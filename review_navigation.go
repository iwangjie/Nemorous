package main

import (
	"context"
	"fmt"
	"os"
	"path/filepath"
	"runtime"
	"slices"
	"strconv"
	"strings"
	"time"

	"github.com/egoist/mygo/ui"
	"github.com/egoist/mygo/ui/editor"
	"github.com/iwangjie/nemorous/internal/gitops"
	"github.com/iwangjie/nemorous/internal/mani"
)

type reviewLocation struct {
	file gitops.WorkspaceFile
	pos  editor.Pos
}

type reviewReturnLocation struct {
	editor string
	tab    NavTab
	diff   *editor.DiffState
	source reviewLocation
	back   bool
}

// Convert an old-side caret to the corresponding position on the new side.
func sourcePosition(diff *editor.DiffState) editor.Pos {
	if diff == nil {
		return editor.Pos{}
	}
	pos := diff.CursorPos()
	if diff.ActiveSide() != editor.DiffOld {
		return pos
	}
	delta := 0
	for _, hunk := range diff.Hunks() {
		if pos.Line < hunk.OldStart {
			break
		}
		if pos.Line < hunk.OldEnd {
			line := hunk.NewStart + min(pos.Line-hunk.OldStart, max(0, hunk.NewEnd-hunk.NewStart-1))
			return editor.Pos{Line: line, Col: pos.Col}
		}
		delta = hunk.NewEnd - hunk.OldEnd
	}
	pos.Line += delta
	return pos
}

func (a *App) canReturnToReview() bool {
	loc := a.reviewReturn
	tab := a.activeCodeTab()
	if loc == nil || loc.back || tab == nil || tab.file.AbsPath != loc.source.file.AbsPath {
		return false
	}
	return (loc.editor == "changes" && loc.diff == a.diffState) || (loc.editor == "commit" && loc.diff == a.gitCommitDiff)
}

func (a *App) returnToReview() {
	if !a.canReturnToReview() {
		return
	}
	a.rememberCodeLocation()
	if tab := a.activeCodeTab(); tab != nil && tab.file.AbsPath == a.reviewReturn.source.file.AbsPath {
		a.reviewReturn.source.pos = tab.state.CursorPos()
	}
	a.editorTab, a.activeTab = a.reviewReturn.editor, a.reviewReturn.tab
	a.gitDiffOpen = a.editorTab == "commit"
	if a.gitDiffOpen {
		a.commitDiffHandle.Focus()
	} else {
		a.workingDiffHandle.Focus()
	}
	a.reviewReturn.back = true
}

func (a *App) forwardToReviewSource() {
	loc := a.reviewReturn
	if loc == nil || !loc.back || a.editorTab != loc.editor {
		return
	}
	loc.back = false
	a.openReviewLocation(loc.source)
}

func reviewEditorGoToLineKey() ui.Key {
	if runtime.GOOS == "darwin" {
		return ui.KeyL
	}
	return ui.KeyG
}

func (a *App) openReviewLocation(loc reviewLocation) {
	a.activeTab = TabCode
	a.selectCodeFile(loc.file)
	if tab := a.activeCodeTab(); tab != nil {
		if loc.pos.Line >= 0 {
			if _, ok := a.editorPluginForFile(tab.file.AbsPath); ok {
				a.setPluginEditorTextMode(tab.file.RepoID, tab.file.RelPath, true)
			}
			tab.state.GoTo(loc.pos)
			tab.state.ScrollTo(loc.pos.Line)
		}
		tab.viewHandle.Focus()
	}
}

func (a *App) rememberCodeLocation() {
	if a.editorTab != "file" {
		return
	}
	tab := a.activeCodeTab()
	if tab == nil || tab.state == nil || tab.revision != "" {
		return
	}
	loc := reviewLocation{file: tab.file, pos: tab.state.CursorPos()}
	for i, previous := range a.recentCode {
		if previous.file.AbsPath == loc.file.AbsPath && previous.pos.Line == loc.pos.Line {
			copy(a.recentCode[1:i+1], a.recentCode[:i])
			a.recentCode[0] = loc
			return
		}
	}
	if len(a.recentCode) < 48 {
		a.recentCode = append(a.recentCode, reviewLocation{})
	}
	copy(a.recentCode[1:], a.recentCode)
	a.recentCode[0] = loc
}

type reviewPickerMode uint8

const (
	reviewGoToFile reviewPickerMode = iota
	reviewRecentFiles
	reviewRecentLocations
)

func (a *App) openReviewPicker(mode reviewPickerMode) {
	a.closeReviewPicker()
	a.rememberCodeLocation()
	a.reviewPickerOpen = true
	a.reviewPickerMode = mode
	a.reviewPickerQuery = ""
	a.reviewPickerResultKey = "\x00"
	a.reviewPickerSelection = 0
}

func (a *App) closeReviewPicker() {
	a.reviewPickerOpen = false
	a.cancelReviewPickerSearch()
}

func (a *App) cancelReviewPickerSearch() {
	a.reviewPickerGeneration++
	if a.reviewPickerCancel != nil {
		a.reviewPickerCancel()
		a.reviewPickerCancel = nil
	}
}

func parseReviewLocation(query string) (string, editor.Pos) {
	pos := editor.Pos{Line: -1}
	name, tail, found := strings.CutLast(query, ":")
	last, err := strconv.Atoi(tail)
	if !found || err != nil || last < 1 || name == "" {
		return query, pos
	}
	pos.Line = last - 1
	if file, number, exists := strings.CutLast(name, ":"); exists && file != "" {
		if line, err := strconv.Atoi(number); err == nil && line > 0 {
			return file, editor.Pos{Line: line - 1, Col: last - 1}
		}
	}
	return name, pos
}

func (a *App) reviewPickerResults(query string) []reviewLocation {
	if a.reviewPickerResultKey == query {
		return a.reviewPickerMatches
	}
	a.reviewPickerResultKey = query
	a.cancelReviewPickerSearch()
	out := a.reviewPickerMatches[:0]
	if a.reviewPickerMode == reviewGoToFile {
		name, pos := parseReviewLocation(query)
		if relative, ok := strings.CutPrefix(name, "~/"); ok {
			if home, err := os.UserHomeDir(); err == nil && home != "" {
				name = filepath.Join(home, filepath.FromSlash(relative))
			}
		}
		name = strings.ToLower(filepath.ToSlash(name))
		// Scan once for an actual query change. Retain only a small ordered set
		// so a broad query never creates one element per workspace file.
		const limit = 32
		var partial []reviewLocation
		for _, file := range a.workspaceFiles {
			loc := reviewLocation{file: file, pos: pos}
			switch gitops.WorkspacePathMatch(file, name) {
			case gitops.FilePathExact:
				out = append(out, loc)
			case gitops.FilePathPartial:
				if len(partial) < limit {
					partial = append(partial, loc)
				}
			}
			if len(out) == limit {
				break
			}
		}
		out = append(out, partial[:min(len(partial), limit-len(out))]...)
		if name != "" && len(out) < limit {
			a.searchBeyondFilePreview(query, name, pos, limit)
		}
	} else {
		query = strings.ToLower(query)
		for _, loc := range a.recentCode {
			if a.relativeWorkspacePath(loc.file.AbsPath) == "" || !strings.Contains(strings.ToLower(loc.file.RepoName+"/"+loc.file.RelPath), query) {
				continue
			}
			if a.reviewPickerMode == reviewRecentFiles && slices.ContainsFunc(out, func(other reviewLocation) bool {
				return other.file.AbsPath == loc.file.AbsPath
			}) {
				continue
			}
			out = append(out, loc)
		}
	}
	a.reviewPickerMatches = out
	return out
}

func (a *App) searchBeyondFilePreview(query, name string, pos editor.Pos, limit int) {
	if len(a.workspaceFiles) < gitops.WorkspaceFilePreviewLimit {
		return
	}
	counts := make(map[string]int)
	for _, file := range a.workspaceFiles {
		counts[file.RepoID]++
	}
	var capped []mani.DiscoveredRepo
	for _, repo := range a.repos {
		if counts[repo.ID] >= gitops.WorkspaceFilePreviewLimit {
			capped = append(capped, repo)
		}
	}
	if len(capped) == 0 {
		return
	}
	ctx, cancel := context.WithTimeout(context.Background(), 4*time.Second)
	a.reviewPickerCancel = cancel
	generation := a.reviewPickerGeneration
	go func() {
		defer cancel()
		select {
		case <-time.After(180 * time.Millisecond):
		case <-ctx.Done():
			return
		}
		files := gitops.SearchWorkspaceFiles(ctx, capped, name, limit)
		a.runUI(func() {
			if !a.reviewPickerOpen || a.reviewPickerGeneration != generation || a.reviewPickerResultKey != query {
				return
			}
			a.reviewPickerCancel = nil
			matches := a.reviewPickerMatches
			for _, file := range files {
				if !slices.ContainsFunc(matches, func(loc reviewLocation) bool { return loc.file.AbsPath == file.AbsPath }) {
					matches = append(matches, reviewLocation{file: file, pos: pos})
				}
			}
			slices.SortStableFunc(matches, func(left, right reviewLocation) int {
				return gitops.WorkspacePathMatch(right.file, name) - gitops.WorkspacePathMatch(left.file, name)
			})
			a.reviewPickerMatches = matches[:min(len(matches), limit)]
		})
	}()
}

func (a *App) buildReviewPicker(c *ui.Context, p Palette) {
	ui.DialogBase(c.Key("review-picker"), &a.reviewPickerOpen, func(backdrop, panel ui.Element) {
		backdrop.Background(ui.RGBA(20, 22, 30, 0.36))
		panel.Width(650).Padding(14).Gap(8).Radius(10).Background(p.Surface).Border(1, p.Border)
		title := "转到文件"
		placeholder := "文件名或路径，可附 :行号:列号"
		if a.reviewPickerMode == reviewRecentFiles {
			title, placeholder = "最近文件", "筛选文件"
		} else if a.reviewPickerMode == reviewRecentLocations {
			title, placeholder = "最近位置", "筛选文件或路径"
		}
		modalHeader(c, p, icoSearch, title, "", nil, a.closeReviewPicker)
		field := ui.SearchField(c.Key("review-picker-query"), &a.reviewPickerQuery).Placeholder(placeholder).AutoFocus().FillWidth()
		scrollSelected := field.Changed()
		if scrollSelected {
			a.reviewPickerSelection = 0
		}
		matches := a.reviewPickerResults(strings.TrimSpace(a.reviewPickerQuery))
		last := len(matches) - 1
		if c.Shortcut(0, ui.KeyUp) {
			a.reviewPickerSelection--
			scrollSelected = true
		}
		if c.Shortcut(0, ui.KeyDown) {
			a.reviewPickerSelection++
			scrollSelected = true
		}
		a.reviewPickerSelection = max(0, min(a.reviewPickerSelection, last))
		open := func(loc reviewLocation) {
			a.closeReviewPicker()
			a.openReviewLocation(loc)
		}
		if field.Submitted() && last >= 0 {
			open(matches[a.reviewPickerSelection])
			return
		}
		if last < 0 {
			if a.reviewPickerCancel != nil {
				emptyState(c, p, icoSearch, "正在搜索仓库文件…", "")
			} else {
				emptyState(c, p, icoSearch, "没有匹配项", "")
			}
			return
		}
		ui.Scroll(c).MaxHeight(400).Children(func() {
			for i, loc := range matches {
				kc := c.Key(i)
				row := ui.Row(kc).Padding(5, 8).Gap(8).AlignItems(ui.Center).Cursor(ui.CursorPointer).Radius(4)
				if i == a.reviewPickerSelection {
					row.Background(p.AccentSoft)
					if scrollSelected {
						row.ScrollIntoView()
					}
				} else if row.Hovered() {
					row.Background(p.SurfaceHover)
				}
				if row.Clicked() {
					open(loc)
				}
				row.Children(func() {
					drawIcon(kc, icoFile, 12, p.TextMuted)
					ui.Text(kc, loc.file.RepoName+"/"+loc.file.RelPath).FontSize(12).MaxLines(1).Grow(1)
					if loc.pos.Line >= 0 {
						ui.Text(kc, fmt.Sprintf("第 %d 行", loc.pos.Line+1)).FontSize(10).TextColor(p.TextMuted)
					}
				})
			}
		})
	})
	if !a.reviewPickerOpen && a.reviewPickerCancel != nil {
		a.closeReviewPicker()
	}
}
