package main

import (
	"fmt"
	"os"
	"path/filepath"
	"strings"
	"testing"

	"github.com/egoist/mygo/ui"
	"github.com/iwangjie/nemorous/internal/config"
	"github.com/iwangjie/nemorous/internal/gitops"
)

// TestDiffToolbar verifies IDEA's diff actions: change navigation, collapse and
// the display options toggle.
func TestDiffToolbar(t *testing.T) {
	t.Setenv("NEMOROUS_CONFIG", filepath.Join(t.TempDir(), "config.json"))
	ws := t.TempDir()
	repo := filepath.Join(ws, "demo-service")
	if err := os.MkdirAll(repo, 0o755); err != nil {
		t.Fatal(err)
	}
	ctx := t.Context()
	for _, args := range [][]string{
		{"init", "-b", "main"},
		{"config", "user.name", "Test"},
		{"config", "user.email", "test@example.com"},
	} {
		if _, err := gitops.RunGit(ctx, repo, args...); err != nil {
			t.Fatal(err)
		}
	}
	var lines []string
	for i := 1; i <= 60; i++ {
		lines = append(lines, fmt.Sprintf("line %d", i))
	}
	path := filepath.Join(repo, "big.txt")
	if err := os.WriteFile(path, []byte(strings.Join(lines, "\n")+"\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	if _, err := gitops.RunGit(ctx, repo, "add", "-A"); err != nil {
		t.Fatal(err)
	}
	if _, err := gitops.RunGit(ctx, repo, "commit", "-m", "init"); err != nil {
		t.Fatal(err)
	}
	for _, i := range []int{4, 24, 44} {
		lines[i] += " changed"
	}
	if err := os.WriteFile(path, []byte(strings.Join(lines, "\n")+"\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(filepath.Join(repo, "other.txt"), []byte("other\n"), 0o644); err != nil {
		t.Fatal(err)
	}

	cfg := config.Default()
	cfg.WorkspacePath = ws
	cfg.IncludeRootRepo = false
	app := NewApp(cfg)
	app.LoadWorkspaceSync(ws)
	for _, st := range app.statuses {
		for _, f := range st.Files {
			if f.Path == "big.txt" {
				app.selectChangedFile(f)
			}
		}
	}
	tt := ui.NewTester(app.BuildUI, 1440, 900)
	tt.Frame()
	if app.diffState == nil {
		t.Fatal("expected a diff state")
	}
	if got := len(app.diffState.Hunks()); got != 3 {
		t.Fatalf("expected 3 hunks, got %d", got)
	}
	before, _ := app.diffState.CurrentHunk()
	if err := tt.Click("下一个差异"); err != nil {
		t.Fatalf("next difference: %v", err)
	}
	tt.Frame()
	if after, _ := app.diffState.CurrentHunk(); after != before+1 {
		t.Fatalf("next difference should advance the hunk: %d -> %d", before, after)
	}
	if app.diffEditable {
		t.Fatal("diff should start read-only")
	}
	if err := tt.Click("编辑差异"); err != nil {
		t.Fatalf("edit: %v", err)
	}
	tt.Frame()
	if !app.diffEditable || !app.diffState.Editable {
		t.Fatal("edit should make the new side editable")
	}
	if err := tt.Click("编辑差异"); err != nil {
		t.Fatalf("leave edit: %v", err)
	}
	tt.Frame()
	if app.diffEditable {
		t.Fatal("second click should leave edit mode")
	}
	selected := app.selectedChangeKey
	if err := tt.Click("下一个文件"); err != nil {
		t.Fatalf("next file: %v", err)
	}
	tt.Frame()
	if app.selectedChangeKey == selected {
		t.Fatal("next file should select another changed file")
	}
	if err := tt.Click("上一个文件"); err != nil {
		t.Fatalf("previous file: %v", err)
	}
	tt.Frame()
	if app.selectedChangeKey != selected {
		t.Fatalf("previous file should return to %q, got %q", selected, app.selectedChangeKey)
	}
}
