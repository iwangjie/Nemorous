package main

import (
	"testing"

	"github.com/iwangjie/nemorous/internal/config"
	"github.com/iwangjie/nemorous/internal/gitops"
)

func TestPreviewCodeTabsReuseAndPin(t *testing.T) {
	ws := testDiffTabsRepo(t)
	cfg := config.Default()
	cfg.WorkspacePath = ws
	cfg.IncludeRootRepo = false
	app := NewApp(cfg)
	app.LoadWorkspaceSync(ws)
	files := map[string]gitops.WorkspaceFile{}
	for _, wf := range app.workspaceFiles {
		if wf.RelPath == "a.txt" || wf.RelPath == "b.txt" {
			files[wf.RelPath] = wf
		}
	}
	if len(files) != 2 {
		t.Fatalf("expected both files, got %d", len(files))
	}
	app.selectCodeFilePreview(files["a.txt"])
	if len(app.codeTabs) != 1 || !app.codeTabs[0].preview {
		t.Fatalf("expected one preview tab, got %+v", app.codeTabs)
	}
	app.selectCodeFilePreview(files["b.txt"])
	if len(app.codeTabs) != 1 || app.codeTabs[0].file.RelPath != "b.txt" || !app.codeTabs[0].preview {
		t.Fatalf("preview should replace the previous preview, got %+v", app.codeTabs)
	}
	app.selectCodeFile(files["b.txt"])
	if app.codeTabs[0].preview {
		t.Fatal("pin should clear preview state")
	}
	app.selectCodeFilePreview(files["a.txt"])
	if len(app.codeTabs) != 2 {
		t.Fatalf("preview should not replace a pinned tab, got %d tabs", len(app.codeTabs))
	}
	if !app.codeTabs[1].preview {
		t.Fatal("new tab should stay a preview")
	}
}
