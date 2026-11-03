package main

import (
	"testing"

	"github.com/egoist/mygo/ui"
	"github.com/iwangjie/nemorous/internal/config"
)

func TestChangesLayoutToggle(t *testing.T) {
	ws := testDiffTabsRepo(t)
	cfg := config.Default()
	cfg.WorkspacePath = ws
	cfg.IncludeRootRepo = false
	app := NewApp(cfg)
	app.LoadWorkspaceSync(ws)
	tt := ui.NewTester(app.BuildUI, 1440, 900)
	tt.Frame()
	if err := tt.Click("平铺"); err != nil {
		t.Fatalf("switch to flat: %v", err)
	}
	tt.Frame()
	if !app.changesFlat {
		t.Fatal("flat layout should be enabled")
	}
	if _, ok := tt.Find("树形"); !ok {
		t.Fatal("toggle should offer tree layout after switching flat")
	}
}
