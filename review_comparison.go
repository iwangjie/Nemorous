package main

import (
	"path/filepath"
	"slices"

	"github.com/egoist/mygo/nemorouseditor"
	"github.com/egoist/mygo/ui"
	"github.com/iwangjie/nemorous/internal/gitops"
)

type reviewComparison struct {
	title, left, right string
	diff               *editor.DiffState
	edge               int
	boundary           diffNavigationBoundary
}

func (a *App) comparisonSelectionIndex(wf gitops.WorkspaceFile) int {
	return slices.IndexFunc(a.compareFiles, func(file gitops.WorkspaceFile) bool {
		return wf.RepoID == file.RepoID && wf.RelPath == file.RelPath
	})
}

func (a *App) selectComparisonFile(wf gitops.WorkspaceFile, extend bool) {
	if !extend {
		a.compareFiles = append(a.compareFiles[:0], wf)
		return
	}
	if i := a.comparisonSelectionIndex(wf); i >= 0 {
		a.compareFiles = slices.Delete(a.compareFiles, i, i+1)
	} else {
		a.compareFiles = append(a.compareFiles, wf)
	}
}

func (a *App) compareProjectFiles() {
	if len(a.compareFiles) != 2 {
		return
	}
	left, right := a.normalizeWorkspaceFile(a.compareFiles[0]), a.normalizeWorkspaceFile(a.compareFiles[1])
	if a.relativeWorkspacePath(left.AbsPath) == "" || a.relativeWorkspacePath(right.AbsPath) == "" {
		return
	}
	a.comparisonSequence++
	sequence := a.comparisonSequence
	var texts [2]*string
	for _, tab := range a.codeTabs {
		if tab.revision != "" || tab.state.ReadOnly {
			continue
		}
		for i, file := range []gitops.WorkspaceFile{left, right} {
			if tab.file.AbsPath == file.AbsPath && tab.dirty() {
				content := reviewBufferText(tab.state.Buffer)
				texts[i] = &content
			}
		}
	}
	read := func() {
		var contents [2]string
		var err error
		for i, file := range []gitops.WorkspaceFile{left, right} {
			if content := texts[i]; content != nil {
				contents[i] = *content
			} else if contents[i], err = gitops.ReadReviewWorktree(file.RepoPath, file.RelPath); err != nil {
				break
			}
		}
		a.runUI(func() {
			if a.comparisonSequence != sequence {
				return
			}
			if err != nil {
				a.showToast("err", err.Error())
				return
			}
			leftName, rightName := filepath.Base(left.RelPath), filepath.Base(right.RelPath)
			a.openComparison(leftName+" - "+rightName, leftName, rightName, contents[0], contents[1], left.RelPath, right.RelPath)
		})
	}
	if a.win == nil {
		read()
	} else {
		go read()
	}
}

func (a *App) openComparison(title, left, right, oldText, newText, oldPath, newPath string) {
	a.comparisonSequence++
	old, next := editor.NewBuffer(oldText), editor.NewBuffer(newText)
	old.SetLanguage(editor.LanguageFor(oldPath))
	next.SetLanguage(editor.LanguageFor(newPath))
	view := &reviewComparison{title: title, left: left, right: right}
	view.diff = &editor.DiffState{
		Old: old, New: next, Layout: editor.DiffSplit,
		Inline: parseInlineMode(a.cfg.DiffInlineMode), Wrap: a.cfg.EditorSoftWrap,
		Font: ui.Font{Size: a.cfg.EditorFontSize}, RenderFindBar: renderReviewFindBar,
		Bindings: []editor.Binding{{Mods: ui.Cmd, Key: reviewEditorGoToLineKey(), Command: editor.GoToLine}},
	}
	a.comparison = view
	a.editorTab, a.gitDiffOpen = "compare", false
	view.diff.Focus()
}

func (a *App) closeComparison() {
	a.comparisonSequence++
	a.comparison = nil
	if a.editorTab == "compare" {
		a.focusDefaultEditor()
	}
}

func (v *reviewComparison) options() diffToolbarOptions {
	return diffToolbarOptions{prefix: "file-compare", boundary: &v.boundary, jumpEdge: &v.edge, moveFile: func(int) bool { return false }}
}

func (a *App) buildComparisonTab(c *ui.Context, p Palette) {
	a.buildAuxReviewTab(c, p, "compare", a.comparison.title, false, a.closeComparison)
}

func (a *App) buildAuxReviewTab(c *ui.Context, p Palette, id, title string, dirty bool, closeTab func()) {
	c = c.Key("editor-tab-" + id)
	active := a.editorTab == id
	row := ui.Row(c).Height(28).Padding(0, 8).Gap(6).Radius(6).AlignItems(ui.Center).
		Cursor(ui.CursorPointer).Border(1, p.Border)
	if active || row.Hovered() {
		row.Background(p.Surface)
	}
	closeClicked := false
	row.Children(func() {
		drawIcon(c, icoColumns, 11, p.TextMuted)
		ui.Text(c, title).FontSize(11.5).MaxWidth(220).SingleLine().TextColor(p.TextPrimary)
		if dirty {
			ui.Box(c).Size(6, 6).Radius(3).Background(p.Amber)
		}
		close := ui.ButtonBase(c.Key("close")).Size(16, 16).Center().Label("关闭比较").Cursor(ui.CursorPointer)
		close.Children(func() { drawIcon(c, icoClose, 10, p.TextMuted) })
		closeClicked = close.Clicked()
	})
	if closeClicked {
		closeTab()
	} else if row.Clicked() {
		a.editorTab, a.gitDiffOpen = id, false
	}
}

func (a *App) buildComparison(c *ui.Context, p Palette) {
	v := a.comparison
	ui.Column(c).Fill().Background(p.Surface).Children(func() {
		a.buildDiffToolbar(c, p, v.diff, v.options())
		ui.Row(c).FillWidth().Children(func() {
			for _, title := range []string{v.left, v.right} {
				ui.Text(c, title).Padding(5, 10).Grow(1).SingleLine().FontSize(11).TextColor(p.TextMuted)
			}
		})
		editor.DiffView(c, v.diff).Grow(1).FillWidth()
	})
}
