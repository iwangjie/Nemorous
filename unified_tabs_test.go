package main

import (
	"testing"

	"github.com/egoist/mygo/ui"
	"github.com/iwangjie/nemorous/internal/config"
	"github.com/iwangjie/nemorous/internal/gitops"
)

func TestFileAndDiffShareOneTabStrip(t *testing.T) {
	ws := testDiffTabsRepo(t)
	cfg := config.Default()
	cfg.WorkspacePath = ws
	cfg.IncludeRootRepo = false
	app := NewApp(cfg)
	app.LoadWorkspaceSync(ws)
	app.activeTab = TabCode

	var wf gitops.WorkspaceFile
	var changed gitops.ChangedFile
	for _, f := range app.workspaceFiles {
		if f.RelPath == "a.txt" {
			wf = f
		}
	}
	for _, st := range app.statuses {
		for _, f := range st.Files {
			if f.Path == "a.txt" {
				changed = f
			}
		}
	}
	if wf.RelPath == "" || changed.Path == "" {
		t.Fatal("fixture missing a.txt")
	}
	app.selectCodeFile(wf)
	if len(app.codeTabs) != 1 || app.editorTab != "file" {
		t.Fatal("file tab should be open")
	}
	app.selectChangedFile(changed)
	if app.editorTab != "changes" || app.diffState == nil {
		t.Fatal("diff should be active")
	}
	tt := ui.NewTester(app.BuildUI, 1440, 900)
	tt.Frame()
	if _, ok := tt.Find("Diff: a.txt"); !ok {
		t.Fatal("diff tab should be in the editor tab strip")
	}
	if len(app.codeTabs) != 1 {
		t.Fatal("opening a diff must not remove the file tab")
	}
	app.activateCodeTab(0)
	tt.Frame()
	if app.editorTab != "file" {
		t.Fatal("file tab should be reachable next to the diff tab")
	}
	if app.diffState == nil {
		t.Fatal("switching to the file tab must keep the diff alive")
	}
}
