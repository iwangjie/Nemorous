package main

import (
	"fmt"
	"os"
	"path/filepath"
	"testing"

	"github.com/egoist/mygo/ui"
	"github.com/iwangjie/nemorous/internal/config"
	"github.com/iwangjie/nemorous/internal/gitops"
)

func makeRootCodeWorkspace(t *testing.T) string {
	t.Helper()
	ws := t.TempDir()
	gitRun(t, ws, "init", "-b", "main")
	gitRun(t, ws, "config", "user.name", "Test")
	gitRun(t, ws, "config", "user.email", "test@example.com")
	if err := os.MkdirAll(filepath.Join(ws, "docs"), 0o755); err != nil {
		t.Fatal(err)
	}
	if err := os.MkdirAll(filepath.Join(ws, "scripts"), 0o755); err != nil {
		t.Fatal(err)
	}
	for path, content := range map[string]string{
		"mani.yaml":      "projects:\n  cloud-caring:\n    path: cloud-caring\n",
		"notes.txt":      "root level file\n",
		"docs/guide.md":  "# Guide\n",
		"scripts/run.sh": "#!/bin/sh\n",
	} {
		if err := os.WriteFile(filepath.Join(ws, path), []byte(content), 0o644); err != nil {
			t.Fatal(err)
		}
	}

	child := filepath.Join(ws, "cloud-caring")
	if err := os.MkdirAll(child, 0o755); err != nil {
		t.Fatal(err)
	}
	gitRun(t, child, "init", "-b", "main")
	gitRun(t, child, "config", "user.name", "Test")
	gitRun(t, child, "config", "user.email", "test@example.com")
	if err := os.WriteFile(filepath.Join(child, "main.go"), []byte("package main\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	gitRun(t, child, "add", "main.go")
	gitRun(t, child, "commit", "-m", "init")
	return ws
}

func TestCodeExplorerShowsWorkspaceRootFiles(t *testing.T) {
	ws := makeRootCodeWorkspace(t)
	app := newGapTestApp(t, ws)
	app.activeTab = TabCode
	tt := ui.NewTester(app.BuildUI, 900, 800)
	tt.Frame()

	for _, want := range []string{"mani.yaml", "notes.txt", "docs", "scripts", "cloud-caring"} {
		if !tt.HasText(want) {
			t.Errorf("workspace root tree should show %q", want)
		}
	}
}

func TestCodeExplorerKeepsAllRepositoryFiles(t *testing.T) {
	app := NewApp(config.Default())
	for i := range 250 {
		app.workspaceFiles = append(app.workspaceFiles, gitops.WorkspaceFile{
			RepoID:   "big",
			RepoName: "big",
			RelPath:  fmt.Sprintf("pkg/file%03d.go", i),
		})
	}
	groups := app.codeRepoGroups("")
	if len(groups) != 1 {
		t.Fatalf("expected one group, got %d", len(groups))
	}
	if got := len(groups[0].files); got != 250 {
		t.Fatalf("file tree truncated repository files: got %d, want 250", got)
	}
}
