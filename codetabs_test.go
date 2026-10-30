package main

import (
	"testing"

	"github.com/egoist/mygo/ui/editor"
	"github.com/iwangjie/nemorous/internal/config"
	"github.com/iwangjie/nemorous/internal/gitops"
)

func testCodeTab(name string, dirty bool) *codeTab {
	buf := editor.NewBuffer("package main\n")
	if dirty {
		buf.Edit(editor.Edit{Start: 0, End: 0, Text: "// edit\n"})
	}
	return &codeTab{
		file:  gitops.WorkspaceFile{RelPath: name},
		state: &editor.State{Buffer: buf},
	}
}

func TestCloseOtherAndAllCodeTabs(t *testing.T) {
	app := NewApp(config.Default())
	a, b, c := testCodeTab("a.go", false), testCodeTab("b.go", false), testCodeTab("c.go", false)
	app.codeTabs = []*codeTab{a, b, c}
	app.codeTabIdx = 1

	app.requestCloseOtherCodeTabs(b)
	if len(app.codeTabs) != 1 || app.codeTabs[0] != b || app.codeTabIdx != 0 {
		t.Fatalf("close others left %+v active=%d", app.codeTabs, app.codeTabIdx)
	}

	app.requestCloseAllCodeTabs()
	if len(app.codeTabs) != 0 || app.codeTabIdx != -1 {
		t.Fatalf("close all left %+v active=%d", app.codeTabs, app.codeTabIdx)
	}
}

func TestDirtyTabsCloseTogether(t *testing.T) {
	app := NewApp(config.Default())
	dirty, clean := testCodeTab("dirty.go", true), testCodeTab("clean.go", false)
	app.codeTabs = []*codeTab{dirty, clean}
	app.codeTabIdx = 0

	app.requestCloseOtherCodeTabs(clean)
	if len(app.pendingCodeTabsClose) != 1 || len(app.codeTabs) != 2 {
		t.Fatalf("dirty tab should wait for confirmation, pending=%+v tabs=%+v", app.pendingCodeTabsClose, app.codeTabs)
	}
	app.discardAndCloseCodeTabs(app.pendingCodeTabsClose)
	if len(app.codeTabs) != 1 || app.codeTabs[0] != clean || len(app.pendingCodeTabsClose) != 0 {
		t.Fatalf("discard close left %+v pending=%+v", app.codeTabs, app.pendingCodeTabsClose)
	}
}
