package main

import (
	"github.com/egoist/mygo/ui"
	"github.com/iwangjie/nemorous/internal/gitops"
)

func (a *App) focusCommitToolWindow() {
	a.activeTab = TabChanges
	list := a.visibleChangeFileList()
	if len(list) == 0 {
		a.changesFilterHandle.Focus()
		return
	}
	if item := a.changeTreeSelection; item == nil || changeReviewItemIndex(list, item.file.Key(), item.staged) < 0 {
		first := list[0]
		a.changeTreeSelection = &first
	}
	a.focusChangeTreeFile = true
}

func (a *App) focusProjectToolWindow() {
	a.activeTab = TabCode
	a.focusProjectTree = true
	if tab := a.activeCodeTab(); a.editorTab == "file" && tab != nil && a.codeLocatedKey == "" {
		a.revealCodeFile(tab.file)
		a.codeLocatedKey = tab.file.RepoID + "::" + tab.file.RelPath
	}
}

func (a *App) openSelectIn(wf gitops.WorkspaceFile) {
	wf = a.normalizeWorkspaceFile(wf)
	if wf.AbsPath != "" {
		a.selectInFile = &wf
	}
}

func (a *App) buildSelectIn(c *ui.Context, p Palette) {
	wf := a.selectInFile
	if wf == nil {
		return
	}
	open := true
	closePopup := func() { a.selectInFile = nil }
	chooseProject := func() {
		closePopup()
		a.locateFileInTree(*wf)
	}
	ui.DialogBase(c.Key("select-in"), &open, func(backdrop, panel ui.Element) {
		backdrop.Background(ui.RGBA(20, 22, 30, 0.36))
		panel.Width(380).Padding(14).Gap(8).Radius(10).Background(p.Surface).Border(1, p.Border)
		modalHeader(c, p, icoFolder, "选择定位位置", "", nil, closePopup)
		project := ui.Button(c.Key("select-project"), "Project").AutoFocus().FillWidth()
		if project.Clicked() || c.Shortcut(0, ui.Key1) {
			chooseProject()
		}
	})
	if !open {
		closePopup()
	}
}
