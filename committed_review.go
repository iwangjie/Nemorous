package main

import (
	"context"
	"fmt"
	"os"
	"path/filepath"
	"slices"
	"sort"
	"strings"
	"time"

	"github.com/egoist/mygo/nemorouseditor"
	"github.com/egoist/mygo/ui"
	"github.com/iwangjie/nemorous/internal/gitops"
)

type revisionHistory struct {
	entry         gitops.CommitEntry
	path          string
	entries       []gitops.FileRevision
	selected      int
	leftPct       float32
	focusList     bool
	loading       bool
	err           string
	cancel        context.CancelFunc
	listHandle    ui.Handle
	preview       *revisionPreview
	previewHandle ui.Handle
}

func codeTabTitle(tab *codeTab) string {
	name := filepath.Base(tab.file.RelPath)
	if tab.revision != "" {
		name += " (" + shortRevision(tab.revision) + ")"
	}
	return name
}

func shortRevision(revision string) string {
	return revision[:min(8, len(revision))]
}

func committedFile(detail gitops.CommitDetail, path string) gitops.WorkspaceFile {
	return gitops.WorkspaceFile{RepoID: detail.RepoID, RepoName: detail.RepoName, RepoPath: detail.RepoPath, RelPath: path}
}

func (a *App) openRepositoryVersion(detail gitops.CommitDetail, file gitops.CommitFile) {
	path, revision := file.Path, detail.Hash
	if file.Status == "D" {
		revision = commitDiffBase(detail)
	}
	a.openHistoricalFile(committedFile(detail, path), revision, editor.Pos{})
}

func (a *App) openHistoricalFile(wf gitops.WorkspaceFile, revision string, pos editor.Pos) {
	wf = a.normalizeWorkspaceFile(wf)
	a.gitRevisionOpenSeq++
	seq := a.gitRevisionOpenSeq
	for i, tab := range a.codeTabs {
		if tab.revision == revision && tab.file.AbsPath == wf.AbsPath {
			a.activateCodeTab(i)
			tab.state.GoTo(pos)
			tab.viewHandle.Focus()
			return
		}
	}
	load := func() {
		text, err := gitops.ReadFileRevision(context.Background(), wf.RepoPath, revision, wf.RelPath)
		a.runUI(func() {
			if a.gitRevisionOpenSeq != seq || a.relativeWorkspacePath(wf.AbsPath) == "" {
				return
			}
			if err != nil {
				a.showToast("err", err.Error())
				return
			}
			buf := editor.NewBuffer(text)
			buf.SetLanguage(editor.LanguageFor(wf.RelPath))
			tab := &codeTab{
				file: wf, revision: revision,
				state: &editor.State{Buffer: buf, ReadOnly: true, SoftWrap: a.cfg.EditorSoftWrap, Font: ui.Font{Size: a.cfg.EditorFontSize},
					Bindings: []editor.Binding{{Mods: ui.Cmd, Key: reviewEditorGoToLineKey(), Command: editor.GoToLine}}, RenderFindBar: renderReviewFindBar},
			}
			a.codeTabs = append(a.codeTabs, tab)
			a.activateCodeTab(len(a.codeTabs) - 1)
			tab.state.GoTo(pos)
			tab.state.ScrollTo(pos.Line)
			tab.viewHandle.Focus()
		})
	}
	if a.win == nil {
		load()
	} else {
		go load()
	}
}

func commitDiffBase(detail gitops.CommitDetail) string {
	if detail.BaseRevision != "" {
		return detail.BaseRevision
	}
	if len(detail.Parents) > 0 {
		return detail.Parents[0]
	}
	return detail.Hash + "^"
}

func (a *App) showWorkingFileHistory(wf gitops.WorkspaceFile) {
	wf = a.normalizeWorkspaceFile(wf)
	if wf.RepoPath == "" || wf.RelPath == "" {
		return
	}
	a.openHistoryUpToHere(gitops.CommitDetail{CommitEntry: gitops.CommitEntry{
		Hash: "HEAD", RepoID: wf.RepoID, RepoName: wf.RepoName, RepoPath: wf.RepoPath,
	}}, gitops.CommitFile{Path: wf.RelPath})
}

func (a *App) gitFileContextMenu(m *ui.Menu, wf gitops.WorkspaceFile) {
	if wf.RepoPath == "" || wf.RelPath == "" {
		return
	}
	m.Submenu("Git", func(git *ui.Menu) {
		if git.Item("查看历史").Chosen() {
			a.showWorkingFileHistory(wf)
		}
		if git.Item("与修订版本比较…").Chosen() {
			a.openFileRevisionPicker(wf)
		}
	})
}

func (a *App) selectedGitFile() gitops.WorkspaceFile {
	if a.activeTab == TabCode && len(a.compareFiles) == 1 {
		file := a.compareFiles[0]
		if a.codeLocatedKey == file.RepoID+"::"+file.RelPath {
			return file
		}
	}
	if a.editorTab == "file" {
		if tab := a.activeCodeTab(); tab != nil {
			return tab.file
		}
	}
	if a.activeTab == TabChanges && a.selectedChangeFile != nil {
		return changedWorkspaceFile(*a.selectedChangeFile)
	}
	return gitops.WorkspaceFile{}
}

func (a *App) openHistoryUpToHere(detail gitops.CommitDetail, file gitops.CommitFile) {
	a.closeRevisionHistory()
	ctx, cancel := context.WithTimeout(context.Background(), 10*time.Second)
	view := &revisionHistory{entry: detail.CommitEntry, path: file.Path, leftPct: 38, focusList: true, loading: true, cancel: cancel}
	a.gitRevisionHistory = view
	a.gitSubTab = "file-history"
	a.showRunPanel, a.showTerminalPanel, a.showLogPanel = false, false, false
	a.showGitPanel = true
	load := func() {
		defer cancel()
		entries, err := gitops.FileHistoryAtRevision(ctx, detail.RepoPath, detail.Hash, file.Path)
		a.runUI(func() {
			if a.gitRevisionHistory != view {
				return
			}
			view.entries, view.loading = entries, false
			if err != nil {
				view.err = err.Error()
			}
		})
	}
	if a.win == nil {
		load()
	} else {
		go load()
	}
}

func (a *App) closeRevisionHistory() {
	a.closeHistoryAffected()
	if view := a.gitRevisionHistory; view != nil {
		view.cancel()
		a.clearRevisionPreview(&view.preview)
		a.gitRevisionHistory = nil
	}
}

func (view *revisionHistory) selectedDetail() gitops.CommitDetail {
	revision := view.entries[view.selected]
	entry := gitops.CommitEntry{
		RepoID: view.entry.RepoID, RepoName: view.entry.RepoName, RepoPath: view.entry.RepoPath,
		Hash: revision.Hash, ShortHash: shortRevision(revision.Hash), Subject: revision.Subject,
		Author: revision.Author, AuthorUnix: revision.AuthorUnix,
	}
	if revision.Parent != "" {
		entry.Parents = []string{revision.Parent}
	}
	return gitops.CommitDetail{CommitEntry: entry, Files: []gitops.CommitFile{revision.File}}
}

func (a *App) historyDiffOptions(view *revisionHistory) diffToolbarOptions {
	preview := view.preview
	return diffToolbarOptions{
		prefix: "file-history-preview", boundary: &preview.boundary, jumpEdge: &preview.edge, loading: preview.loading,
		source:         func() { a.openDiffSourceAtCaret(committedFile(preview.detail, preview.file.Path), preview.diff) },
		sourceEnabled:  preview.diff != nil && preview.file.Status != "D",
		historyPreview: true,
	}
}

func (a *App) buildRevisionHistory(c *ui.Context, p Palette, view *revisionHistory) {
	if view.loading || view.err != "" || len(view.entries) == 0 {
		label := "没有历史提交"
		if view.loading {
			label = "正在读取历史…"
		} else if view.err != "" {
			label = view.err
		}
		emptyState(c, p, icoLog, label, "")
		return
	}
	ui.SplitPercent(c.Key("file-history-split"), &view.leftPct, func() {
		a.buildRevisionList(c.Key("file-history-list"), p, view)
	}, func() {
		if old := view.preview; old == nil || old.detail.Hash != view.entries[view.selected].Hash {
			detail := view.selectedDetail()
			a.loadRevisionPreview(&view.preview, detail, detail.Files[0])
			view.preview.edge = 1
		}
		detail, file := view.preview.detail, view.preview.file
		label := detail.ShortHash
		if len(detail.Parents) > 0 && file.Status != "A" {
			label = shortRevision(detail.Parents[0]) + " → " + label
		}
		a.buildRevisionPreview(c.Key("file-history-preview"), p, view.preview, &view.previewHandle, a.historyDiffOptions(view), label+"  ·  "+file.Path)
	}).Grow(1).Fill()
}

func fileHistoryDate(unix int64, now time.Time) string {
	if unix == 0 {
		return "—"
	}
	when := time.Unix(unix, 0).In(now.Location())
	switch when.Format(time.DateOnly) {
	case now.Format(time.DateOnly):
		return when.Format("今天 15:04")
	case now.AddDate(0, 0, -1).Format(time.DateOnly):
		return when.Format("昨天 15:04")
	default:
		return when.Format("2006-01-02 15:04")
	}
}

func (a *App) buildRevisionList(c *ui.Context, p Palette, view *revisionHistory) {
	ui.Column(c).Fill().Clip().Children(func() {
		ui.Row(c).Height(30).Padding(0, 6).AlignItems(ui.Center).Background(p.RailBg).Children(func() {
			toolButton(c, p, "file-history-affected", icoAffectedFiles, "显示所有受影响的文件", false, true, func() {
				a.openHistoryAffected(view.selectedDetail())
			})
		})
		a.buildRevisionRows(c, p, view)
	})
}

func (a *App) buildRevisionRows(c *ui.Context, p Palette, view *revisionHistory) {
	list := ui.Scroll(c).Grow(1).FillWidth().Focusable().Label("文件提交历史").Bind(&view.listHandle)
	if view.focusList {
		list.Focus()
		view.focusList = false
	}
	previous := view.selected
	switch {
	case list.Shortcut(0, ui.KeyUp):
		view.selected = max(0, previous-1)
	case list.Shortcut(0, ui.KeyDown):
		view.selected = min(len(view.entries)-1, previous+1)
	case list.Shortcut(ui.Cmd, ui.KeyD):
		detail := view.selectedDetail()
		a.TriggerGitFileDiff(detail, detail.Files[0])
	}
	now := time.Now()
	list.Children(func() {
		for i, revision := range view.entries {
			kc := c.Key("file-revision-" + revision.Hash)
			row := ui.Row(kc).Height(26).FillWidth().Padding(0, 8).Gap(6).AlignItems(ui.Center).Clip().Cursor(ui.CursorPointer)
			if row.Clicked() {
				list.Focus()
				view.selected = i
			}
			if row.DoubleClicked() {
				view.selected = i
				detail := view.selectedDetail()
				a.TriggerGitFileDiff(detail, revision.File)
			}
			if view.selected == i {
				row.Background(p.ActiveRowBg).Selected(true)
				if previous != i {
					row.ScrollIntoView()
				}
			} else if row.Hovered() {
				row.Background(p.SurfaceHover)
			}
			row.PopoverMenu(func(m *ui.Menu) {
				if m.Item("查看差异").Chosen() {
					view.selected = i
					a.TriggerGitFileDiff(view.selectedDetail(), revision.File)
				}
				if m.Item("打开仓库版本").Chosen() {
					view.selected = i
					a.openRepositoryVersion(view.selectedDetail(), revision.File)
				}
			})
			row.Children(func() {
				date := fileHistoryDate(revision.AuthorUnix, now)
				fullDate := date
				if revision.AuthorUnix != 0 {
					fullDate = time.Unix(revision.AuthorUnix, 0).In(now.Location()).Format("2006-01-02 15:04")
				}
				ui.Text(kc, revision.Author).Grow(.28).MaxWidth(110).MinWidth(0).SingleLine().FontSize(11).TextColor(p.TextPrimary).Tooltip(revision.Author)
				ui.Text(kc, date).Grow(.30).MaxWidth(104).MinWidth(0).SingleLine().FontSize(11).TextColor(p.TextPrimary).Tooltip(fullDate)
				ui.Text(kc, revision.Subject).Grow(.42).MinWidth(0).SingleLine().FontSize(11.5).TextColor(p.TextPrimary).Tooltip(revision.Subject)
			})
		}
	})
}

func (a *App) openCommittedSource(detail gitops.CommitDetail, file gitops.CommitFile) {
	wf := a.normalizeWorkspaceFile(committedFile(detail, file.Path))
	if _, err := os.Stat(wf.AbsPath); err != nil {
		a.showToast("info", "当前工作区中没有此文件")
		return
	}
	a.openDiffSourceAtCaret(wf, nil)
}

func (a *App) compareSelectedVersions() {
	if len(a.gitSelectedPair) != 2 {
		return
	}
	newer, older := a.gitSelectedPair[0], a.gitSelectedPair[1]
	if newer.RepoPath != older.RepoPath || newer.Hash == older.Hash {
		return
	}
	for _, row := range a.gitUnifiedRows {
		if row.Entry.RepoPath != newer.RepoPath {
			continue
		}
		if row.Entry.Hash == newer.Hash {
			break
		}
		if row.Entry.Hash == older.Hash {
			newer, older = older, newer
			break
		}
	}
	a.gitDetailSeq++
	seq := a.gitDetailSeq
	a.gitDetail, a.gitDetailLoading, a.gitDetailSelectedPath = nil, true, ""
	load := func() {
		ctx, cancel := context.WithTimeout(context.Background(), 10*time.Second)
		defer cancel()
		files, err := gitops.CompareCommitFiles(ctx, newer.RepoPath, older.Hash, newer.Hash)
		a.runUI(func() {
			if a.gitDetailSeq != seq {
				return
			}
			a.gitDetailLoading = false
			if err != nil {
				a.showToast("err", err.Error())
				return
			}
			a.gitDetail = &gitops.CommitDetail{CommitEntry: newer, BaseRevision: older.Hash, Files: files}
		})
	}
	if a.win == nil {
		load()
	} else {
		go load()
	}
}

func (a *App) selectCommitForReview(entry gitops.CommitEntry, modifiers ui.Modifiers) {
	if modifiers&(ui.Cmd|ui.Shift) == 0 {
		a.gitSelectedPair = []gitops.CommitEntry{entry}
		a.TriggerGitDetail(entry)
		return
	}
	if len(a.gitSelectedPair) == 0 && a.gitDetail != nil {
		a.gitSelectedPair = append(a.gitSelectedPair, a.gitDetail.CommitEntry)
	}
	i := slices.IndexFunc(a.gitSelectedPair, func(other gitops.CommitEntry) bool {
		return other.RepoPath == entry.RepoPath && other.Hash == entry.Hash
	})
	if i >= 0 {
		if len(a.gitSelectedPair) == 2 {
			a.gitSelectedPair = slices.Delete(a.gitSelectedPair, i, i+1)
			a.TriggerGitDetail(a.gitSelectedPair[0])
		}
		return
	}
	if len(a.gitSelectedPair) == 2 {
		a.gitSelectedPair[1] = entry
	} else {
		a.gitSelectedPair = append(a.gitSelectedPair, entry)
	}
}

func (a *App) canCompareSelectedVersions() bool {
	return len(a.gitSelectedPair) == 2 && a.gitSelectedPair[0].RepoPath == a.gitSelectedPair[1].RepoPath
}

func (a *App) compareCommittedFileWithLocal(detail gitops.CommitDetail, file gitops.CommitFile) {
	file.OldPath = ""
	detail.Files = []gitops.CommitFile{file}
	a.loadCommittedDiff(detail, file, detail.Hash, "")
}

func (a *App) gitDiffRevision(side editor.DiffSide) string {
	if side == editor.DiffOld {
		return a.gitDiffBefore
	}
	return a.gitDiffAfter
}

func (a *App) buildGitChangedFiles(c *ui.Context, p Palette, detail gitops.CommitDetail) {
	ui.Row(c).Gap(4).AlignItems(ui.Center).Children(func() {
		ui.Text(c, fmt.Sprintf("变更文件 (%d)", len(detail.Files))).FontSize(11).FontWeight(weightSemibold).TextColor(p.TextMuted).Grow(1).SingleLine()
		i := slices.IndexFunc(detail.Files, func(file gitops.CommitFile) bool { return file.Path == a.gitDetailSelectedPath })
		menuIconPill(c, p, "git-file-view-options", icoSettings, "视图选项", a.cfg.GitLogDiffPreview, func(m *ui.Menu) {
			if m.Item("显示差异预览").Checked(a.cfg.GitLogDiffPreview).Chosen() {
				a.cfg.GitLogDiffPreview = !a.cfg.GitLogDiffPreview
				if a.win != nil {
					_ = a.cfg.Save()
				}
				if !a.cfg.GitLogDiffPreview {
					a.clearGitLogPreview()
				} else if len(detail.Files) > 0 {
					a.selectGitChangedFile(detail, detail.Files[max(0, i)])
				}
			}
		})
		if i >= 0 {
			file := detail.Files[i]
			toolButton(c, p, "history-show-diff", icoColumns, "查看差异 (⌘D)", false, true, func() {
				a.TriggerGitFileDiff(detail, file)
			})
			toolButton(c, p, "history-up-to-here", icoRestore, "查看截至此处的历史", false, true, func() {
				a.openHistoryUpToHere(detail, file)
			})
		}
	})
	if len(detail.Files) > 0 {
		a.renderGitCommitTree(c, p, detail, buildGitCommitTree(detail.Files), 0)
	}
}

type revisionBlame struct {
	lines   []gitops.BlameLine
	loading bool
	cancel  context.CancelFunc
}

func blameSideIndex(side editor.DiffSide) int {
	if side == editor.DiffOld {
		return 0
	}
	return 1
}

func (a *App) stopCommitDiffReads() {
	if a.gitDiffCancel != nil {
		a.gitDiffCancel()
		a.gitDiffCancel = nil
	}
	for i, blame := range a.gitDiffBlame {
		if blame != nil {
			blame.cancel()
			a.gitDiffBlame[i] = nil
		}
	}
}

func (a *App) commitDiffSideFile(side editor.DiffSide) gitops.WorkspaceFile {
	wf := a.gitDiffWorkspaceFile()
	if side == editor.DiffOld && a.gitDiffOldPath != "" {
		wf.RelPath = a.gitDiffOldPath
	}
	return wf
}

func (a *App) toggleRevisionBlame(side editor.DiffSide) {
	i := blameSideIndex(side)
	if current := a.gitDiffBlame[i]; current != nil {
		current.cancel()
		a.gitDiffBlame[i] = nil
		return
	}
	wf := a.commitDiffSideFile(side)
	revision := a.gitDiffRevision(side)
	if wf.RelPath == "" {
		return
	}
	ctx, cancel := context.WithTimeout(context.Background(), 12*time.Second)
	state := &revisionBlame{loading: true, cancel: cancel}
	a.gitDiffBlame[i] = state
	load := func() {
		defer cancel()
		lines, err := gitops.FileBlameAtRevision(ctx, wf.RepoPath, revision, wf.RelPath)
		a.runUI(func() {
			if a.gitDiffBlame[i] != state {
				return
			}
			if err != nil {
				a.gitDiffBlame[i] = nil
				a.showToast("err", err.Error())
				return
			}
			state.lines, state.loading = lines, false
		})
	}
	if a.win == nil {
		load()
	} else {
		go load()
	}
}

func (a *App) buildHistoricalDiffEditor(c *ui.Context, p Palette) {
	diff := a.gitCommitDiff
	ui.Row(c).Grow(1).FillWidth().Children(func() {
		if diff.Layout == editor.DiffSplit {
			a.buildRevisionBlame(c, p, diff, editor.DiffOld)
		}
		ui.Box(c).Grow(1).FillHeight().Children(func() {
			a.bindDiffKeymap(diff)
			editor.DiffView(c, diff).Bind(&a.commitDiffHandle).Fill().Label("提交差异")
			// The editor owns its text context menu. Limit annotation hit targets
			// to the leading strip of each line-number gutter.
			gutter := func(side editor.DiffSide, left float32) {
				ui.Box(c.Key("revision-gutter").Key(int(side))).Absolute().LeftPercent(left).Top(0).Bottom(0).Width(8 + diff.Font.Size*2.3).
					PopoverMenu(func(m *ui.Menu) { a.revisionBlameMenu(m, side) })
			}
			if diff.Layout == editor.DiffSplit && diff.Old.Len() > 0 {
				gutter(editor.DiffOld, 0)
			}
			if diff.Layout == editor.DiffSplit && diff.New.Len() > 0 {
				gutter(editor.DiffNew, 50)
			} else if diff.Layout == editor.DiffUnified && diff.New.Len() > 0 {
				gutter(editor.DiffNew, 0)
			}
		})
		a.buildRevisionBlame(c, p, diff, editor.DiffNew)
	})
}

func (a *App) revisionBlameMenu(m *ui.Menu, side editor.DiffSide) {
	label := "使用 Git Blame 注解"
	if a.gitDiffBlame[blameSideIndex(side)] != nil {
		label = "关闭注解"
	}
	if m.Item(label).Chosen() {
		a.toggleRevisionBlame(side)
	}
}

func (a *App) buildRevisionBlame(c *ui.Context, p Palette, diff *editor.DiffState, side editor.DiffSide) {
	state := a.gitDiffBlame[blameSideIndex(side)]
	if state == nil {
		return
	}
	label := "Git Blame"
	if state.loading {
		label = "正在读取 Git Blame"
	}
	col := ui.Box(c.Key(int(side))).Width(138).Shrink(0).FillHeight().Clip().Background(p.RailBg).Label(label)
	col.PopoverMenu(func(m *ui.Menu) { a.revisionBlameMenu(m, side) })
	col.Draw(func(painter *ui.Painter, bounds ui.Rect) {
		scroll := diff.ScrollY()
		top := func(i int) float64 { return diff.LineTop(side, state.lines[i].Line-1) }
		i := max(0, sort.Search(len(state.lines), func(i int) bool { return top(i) >= scroll })-1)
		for ; i < len(state.lines); i++ {
			line := state.lines[i]
			y := float32(top(i) - scroll)
			if y >= bounds.H {
				break
			}
			next := diff.LineTop(side, line.Line)
			if top(i) == next {
				// Jump over folded lines sharing the same position.
				i += sort.Search(len(state.lines)-i, func(j int) bool { return top(i+j) > next }) - 1
				continue
			}
			painter.Text(bounds.X+5, bounds.Y+y+2, truncateRunes(gitops.BlameDisplay(line), 22), 10, p.TextSecondary)
		}
	})
	_, y, over := col.PointerPosition()
	if over && len(state.lines) != 0 {
		yPos := float64(y) + diff.ScrollY()
		i := sort.Search(len(state.lines), func(i int) bool { return diff.LineTop(side, state.lines[i].Line-1) > yPos }) - 1
		if i >= 0 && yPos < diff.LineTop(side, state.lines[i].Line) {
			line := state.lines[i]
			col.Tooltip(gitops.BlameDisplay(line) + " · " + shortRevision(line.Hash) + "\n" + line.Summary)
			if col.Clicked() && strings.Trim(line.Hash, "0") != "" {
				wf := a.gitDiffWorkspaceFile()
				a.openGitCommit(gitops.CommitEntry{RepoID: wf.RepoID, RepoPath: wf.RepoPath, RepoName: wf.RepoName,
					Hash: line.Hash, ShortHash: shortRevision(line.Hash), Author: line.Author, AuthorUnix: line.AuthorUnix, Subject: line.Summary})
			}
		}
	}
}
