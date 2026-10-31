package main

import (
	"testing"

	"github.com/egoist/mygo/ui"
	"github.com/egoist/mygo/ui/editor"
	"github.com/iwangjie/nemorous/internal/lsp"
)

func TestLSPDepthModals(t *testing.T) {
	ws := makeP0Repo(t, 1)
	app := newGapTestApp(t, ws)
	app.cfg.LSPEnabled = true
	wf := app.workspaceFiles[0]
	app.selectCodeFile(wf)
	tab := app.activeCodeTab()
	if tab == nil {
		t.Fatal("no code tab")
	}
	tab.state.SetDiagnostics(tab.state.Buffer.Version(), []editor.Diagnostic{{
		Start: 0, End: 1, Severity: editor.SeverityWarning, Message: "unused variable", Source: "fake",
	}})
	app.refreshLSPProblems()
	if len(app.lspProblems) != 1 || app.lspProblems[0].Message != "unused variable" {
		t.Fatalf("problems=%+v", app.lspProblems)
	}
	tt := ui.NewTester(app.BuildUI, 1440, 900)
	app.showLSPProblems = true
	tt.Frame()
	if !tt.HasText("Problems") || !tt.HasText("unused variable") {
		t.Fatal("problems modal did not render")
	}
	app.showLSPProblems = false

	app.lspReferences = []lsp.Location{{URI: lsp.FileURI(wf.AbsPath), Range: lsp.Range{Start: lsp.Position{Line: 0, Character: 1}}}}
	app.lspReferencesTitle = wf.RelPath
	app.showLSPReferences = true
	tt.Frame()
	if !tt.HasText("引用") || !tt.HasText("1:2") {
		t.Fatal("references modal did not render")
	}
	app.showLSPReferences = false

	app.lspCodeActions = []lsp.CodeAction{{Title: "Remove unused variable", Kind: "quickfix"}}
	app.lspCodeActionTab = tab
	app.showLSPCodeActions = true
	tt.Frame()
	if !tt.HasText("快速修复") || !tt.HasText("Remove unused variable") {
		t.Fatal("code actions modal did not render")
	}
}
