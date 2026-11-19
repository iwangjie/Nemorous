package main

import (
	"context"
	"errors"
	"path/filepath"
	"strings"

	"github.com/egoist/mygo/nemorouseditor"
	"github.com/egoist/mygo/ui"
	"github.com/iwangjie/nemorous/internal/gitops"
)

func reviewBufferText(buffer *editor.Buffer) string {
	var text strings.Builder
	_, _ = buffer.Snapshot().WriteTo(&text)
	return text.String()
}

func renderReviewFindBar(c *ui.Context, bar ui.Element, find *editor.FindBar) {
	editor.DefaultFindBar(c, bar, find)
	if bar.Shortcut(0, ui.KeyEscape) {
		find.Close()
	}
}

func (a *App) workingDiffDirty() bool {
	return a.diffState != nil && a.diffState.New != nil && a.diffState.New.Modified()
}

func verifyReviewDiff(ctx context.Context, file gitops.ChangedFile, staged bool, old, new string) error {
	latest, err := gitops.ReadReviewChange(ctx, file, staged)
	if err != nil {
		return err
	}
	if latest.Old != old || latest.New != new {
		return errors.New("差异已变化，请刷新后重新选择")
	}
	return nil
}

func (a *App) applyReviewDiff(contents gitops.ReviewDiff, staged bool, err error) {
	a.diffReadError = ""
	if err != nil {
		a.diffReadError = err.Error()
		contents = gitops.ReviewDiff{}
	}
	diff := a.diffState
	if reviewBufferText(diff.Old) != contents.Old {
		diff.Old.Set(contents.Old)
		diff.Old.MarkSaved()
	}
	if reviewBufferText(diff.New) != contents.New {
		diff.New.Set(contents.New)
	}
	diff.New.MarkSaved()
	a.diffEditable = !staged && contents.WorktreeExists
	a.diffModified = false
	diff.Editable = a.diffEditable
	a.diffStagedOnly = staged
	a.diffDiskVersion = gitops.ReviewTextVersion(contents.New)
}

func (a *App) loadReviewDiff(staged, fresh bool) {
	if a.diffLoadCancel != nil {
		a.diffLoadCancel()
	}
	ctx, cancel := context.WithCancel(context.Background())
	a.diffLoadCancel = cancel
	a.diffLoading = fresh || a.diffLoading
	selected, diff := a.selectedChangeFile, a.diffState
	load := func() {
		defer cancel()
		contents, err := gitops.ReadReviewChange(ctx, *selected, staged)
		if ctx.Err() != nil {
			return
		}
		apply := func() {
			if a.selectedChangeFile != selected || a.diffState != diff {
				return
			}
			a.diffLoadCancel = nil
			a.diffLoading = false
			if !a.workingDiffDirty() {
				a.applyReviewDiff(contents, staged, err)
			}
		}
		if a.win == nil {
			apply()
		} else {
			a.runUI(apply)
		}
	}
	if a.win == nil {
		load()
	} else {
		go load()
	}
}

// Only the selected working diff is loaded. Its state survives refreshes so
// search, folding and scroll position stay with the file.
func (a *App) refreshReviewDiff(files map[string]gitops.ChangedFile, first *gitops.ChangedFile, touched map[string]bool) {
	if selected := a.selectedChangeFile; selected != nil {
		if touched != nil && !touched[selected.RepoID] {
			return
		}
		if a.workingDiffDirty() {
			return
		}
		if current, ok := files[a.selectedChangeKey]; ok && a.diffState != nil {
			a.selectedChangeFile = &current
			staged := current.Staged && (a.diffStagedOnly || !current.Unstaged)
			a.loadReviewDiff(staged, false)
			return
		}
		a.clearWorkingDiff()
		if first == nil && a.editorTab == "changes" {
			a.focusDefaultEditor()
		}
	}
	if first != nil && !a.workingDiffClosed {
		previousEditor, previousGitDiff := a.editorTab, a.gitDiffOpen
		a.selectChangedFileInScope(*first, first.Staged && !first.Unstaged)
		if previousEditor != "" && previousEditor != "changes" {
			a.editorTab, a.gitDiffOpen = previousEditor, previousGitDiff
		}
	}
}

func (a *App) refreshVisibleCodeFile(touched map[string]bool) {
	tab := a.activeCodeTab()
	if a.editorTab != "file" || tab == nil || tab.revision != "" || tab.dirty() || (touched != nil && !touched[tab.file.RepoID]) {
		return
	}
	tab.reloadSeq++
	seq, version, wasReadOnly := tab.reloadSeq, tab.diskVersion, tab.state.ReadOnly
	wf := tab.file
	load := func() {
		text, err := gitops.ReadReviewWorktree(wf.RepoPath, wf.RelPath)
		if err == nil && !wasReadOnly && gitops.ReviewTextVersion(text) == version {
			return
		}
		apply := func() {
			if a.activeCodeTab() == tab && tab.reloadSeq == seq && !tab.dirty() {
				setCodeTabText(tab, text, err)
			}
		}
		if a.win == nil {
			apply()
		} else {
			a.runUI(apply)
		}
	}
	if a.win == nil {
		load()
	} else {
		go load()
	}
}

func setCodeTabText(tab *codeTab, text string, err error) {
	tab.reloadSeq++
	tab.state.ReadOnly = err != nil
	if err != nil {
		text = "// 无法读取文件: " + err.Error()
	}
	tab.diskVersion = gitops.ReviewTextVersion(text)
	if reviewBufferText(tab.state.Buffer) != text {
		pos, scroll := tab.state.CursorPos(), tab.state.ScrollY()
		tab.state.Buffer.Set(text)
		tab.state.GoTo(pos)
		tab.state.SetScrollY(scroll)
	}
	tab.state.Buffer.MarkSaved()
}

func (a *App) writeCodeTab(tab *codeTab, content string) error {
	if err := gitops.SaveReviewText(tab.file.RepoPath, tab.file.RelPath, content, tab.diskVersion); err != nil {
		return err
	}
	tab.reloadSeq++
	tab.diskVersion = gitops.ReviewTextVersion(content)
	tab.state.Buffer.MarkSaved()
	return nil
}

func (a *App) loadReviewCodeBase(tab *codeTab) {
	wf := tab.file
	load := func() {
		text, err := gitops.ReadFileRevision(context.Background(), wf.RepoPath, "HEAD", filepath.ToSlash(wf.RelPath))
		if err != nil {
			return
		}
		base := editor.NewBuffer(text)
		base.SetLanguage(editor.LanguageFor(wf.RelPath))
		if a.win == nil {
			tab.state.DiffBase = base
		} else {
			a.runUI(func() { tab.state.DiffBase = base })
		}
	}
	if a.win == nil {
		load()
	} else {
		go load()
	}
}

func (a *App) reviewEditorFocused(c *ui.Context) bool {
	switch a.editorTab {
	case "file":
		if tab := a.activeCodeTab(); tab != nil {
			return tab.viewHandle.FocusWithin(c)
		}
	case "changes":
		return a.workingDiffHandle.FocusWithin(c)
	case "commit":
		return a.commitDiffHandle.FocusWithin(c)
	}
	return false
}

func (a *App) registerDiffShortcuts(c *ui.Context) {
	var diff *editor.DiffState
	var options func() diffToolbarOptions
	if a.gitLogPreview != nil && a.gitLogPreviewHandle.FocusWithin(c) {
		diff, options = a.gitLogPreview.diff, a.gitLogPreviewOptions
	} else {
		switch a.editorTab {
		case "changes":
			diff, options = a.diffState, a.workingDiffOptions
		case "commit":
			diff, options = a.gitCommitDiff, a.commitDiffOptions
		case "compare":
			if a.comparison != nil {
				diff, options = a.comparison.diff, a.comparison.options
			}
		}
	}
	if diff == nil {
		return
	}
	switch {
	case c.Shortcut(ui.Cmd|ui.Shift, ui.KeyD):
		a.toggleDiffIgnoreOptions(diff)
	case c.Shortcut(ui.Cmd, ui.KeyF):
		diff.Run(editor.Find)
	case c.Shortcut(ui.Cmd, ui.KeyG):
		diff.Run(editor.FindNext)
	case c.Shortcut(ui.Cmd|ui.Shift, ui.KeyG):
		diff.Run(editor.FindPrevious)
	case c.Shortcut(0, ui.KeyF7):
		a.stepDiffHunk(diff, 1, options())
	case c.Shortcut(ui.Shift, ui.KeyF7):
		a.stepDiffHunk(diff, -1, options())
	case c.Shortcut(ui.Alt, ui.KeyLeft):
		options().moveFile(-1)
	case c.Shortcut(ui.Alt, ui.KeyRight):
		options().moveFile(1)
	}
}
