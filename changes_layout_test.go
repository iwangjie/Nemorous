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

func TestChangesHeaderFitsNarrowPane(t *testing.T) {
	app := newReviewTestApp(t)
	app.changesSplit = 320
	tt := ui.NewTester(app.BuildUI, 1040, 660)
	tt.Frame()
	title, ok := tt.Find("本地变更")
	count, hasCount := tt.Find("2 个文件")
	if !ok || !hasCount || title.H > 22 || count.H > 22 || title.W < 45 || count.W < 38 {
		t.Fatalf("title and count must fit on one line: title=%+v, count=%+v", title, count)
	}
	divider, ok := tt.Find("Divider")
	if !ok {
		t.Fatal("cannot find the Commit pane divider")
	}
	for _, label := range []string{"变更视图选项", "平铺", "变更列表", "Stash"} {
		button, ok := tt.Find(label)
		if !ok || button.W < 24 || button.X+button.W > divider.X || button.Y < title.Y+title.H {
			t.Fatalf("%s must fully fit below the title: %+v, divider=%+v", label, button, divider)
		}
	}
	for _, menu := range []struct{ label, item string }{
		{"变更视图选项", "单击时预览差异"},
		{"变更列表", "全部变更"},
		{"Stash", "管理 Stash…"},
	} {
		if err := tt.Click(menu.label); err != nil {
			t.Fatal(err)
		}
		if !tt.HasText(menu.item) {
			t.Fatalf("could not open %s in the narrow pane", menu.label)
		}
		tt.Key(0, ui.KeyEscape)
	}

	app.changesSplit = 100
	tt.Frame()
	if err := tt.Click("More"); err != nil {
		t.Fatal(err)
	}
	if err := tt.ChooseMenuItem("Stash"); err != nil {
		t.Fatal(err)
	}
	if !tt.HasText("管理 Stash…") {
		t.Fatal("Stash must remain accessible through the overflow menu")
	}
}
