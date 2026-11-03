package main

import (
	"fmt"
	"math"
	"os"
	"path/filepath"
	"strings"
	"testing"

	"github.com/egoist/mygo/ui"
	"github.com/egoist/mygo/ui/editor"
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
	for i := 1; i <= 400; i++ {
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
	for _, i := range []int{99, 199, 299} {
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
	app.diffState.ExpandAll()
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

	// The target difference is centered even though it was already visible.
	if err := tt.Click("下一个差异"); err != nil {
		t.Fatalf("next difference again: %v", err)
	}
	tt.Frame()
	cur, _ := app.diffState.CurrentHunk()
	hunks := app.diffState.Hunks()
	if cur < 0 || cur >= len(hunks) {
		t.Fatalf("no current hunk after navigation: %d", cur)
	}
	top := app.diffState.LineTop(editor.DiffNew, hunks[cur].NewStart)
	center := app.diffState.ScrollY() + app.diffState.ViewportHeight()/2
	if math.Abs(top-center) > 30 {
		t.Fatalf("difference should be centered: line top %.0f, viewport center %.0f", top, center)
	}
	if !app.diffEditable || !app.diffState.Editable {
		t.Fatal("working-tree diff should be directly editable")
	}
	if _, ok := tt.Find("编辑差异"); ok {
		t.Fatal("working-tree diff should not have an edit toggle")
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

	// Difference arrows cross file boundaries: from the last difference of one
	// file to the first of the next, and back to the previous file's last one.
	for i := 0; i < 4; i++ {
		if cur, _ := app.diffState.CurrentHunk(); cur == len(app.diffState.Hunks())-1 {
			break
		}
		if err := tt.Click("下一个差异"); err != nil {
			t.Fatalf("go to last difference: %v", err)
		}
		tt.Frame()
	}
	bigKey := app.selectedChangeKey
	if cur, _ := app.diffState.CurrentHunk(); cur != len(app.diffState.Hunks())-1 {
		t.Fatalf("expected to be at the last difference, got %d", cur)
	}
	if err := tt.Click("下一个差异"); err != nil {
		t.Fatalf("next difference across files: %v", err)
	}
	tt.Frame()
	if app.selectedChangeKey == bigKey {
		t.Fatal("next difference at the last hunk should move to the next file")
	}
	if cur, _ := app.diffState.CurrentHunk(); cur != 0 {
		t.Fatalf("next file should start at its first difference, got %d", cur)
	}
	if err := tt.Click("上一个差异"); err != nil {
		t.Fatalf("previous difference across files: %v", err)
	}
	tt.Frame()
	if app.selectedChangeKey != bigKey {
		t.Fatalf("previous difference should return to %q, got %q", bigKey, app.selectedChangeKey)
	}
	if cur, _ := app.diffState.CurrentHunk(); cur != len(app.diffState.Hunks())-1 {
		t.Fatalf("previous file should end at its last difference, got %d", cur)
	}
}

// TestGitCommitDiffToolbar checks the same diff actions appear when a commit
// file diff is open from the history.
func TestGitCommitDiffToolbar(t *testing.T) {
	t.Setenv("NEMOROUS_CONFIG", filepath.Join(t.TempDir(), "config.json"))
	ws := t.TempDir()
	repo := filepath.Join(ws, "demo")
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
	for _, name := range []string{"a.txt", "b.txt"} {
		if err := os.WriteFile(filepath.Join(repo, name), []byte(name+"\n"), 0o644); err != nil {
			t.Fatal(err)
		}
	}
	if _, err := gitops.RunGit(ctx, repo, "add", "-A"); err != nil {
		t.Fatal(err)
	}
	if _, err := gitops.RunGit(ctx, repo, "commit", "-m", "init"); err != nil {
		t.Fatal(err)
	}
	out, err := gitops.RunGit(ctx, repo, "rev-parse", "HEAD")
	if err != nil {
		t.Fatal(err)
	}
	hash := strings.TrimSpace(out)

	cfg := config.Default()
	cfg.WorkspacePath = ws
	cfg.IncludeRootRepo = false
	app := NewApp(cfg)
	app.LoadWorkspaceSync(ws)
	app.gitDetail = &gitops.CommitDetail{
		CommitEntry: gitops.CommitEntry{RepoName: "demo", RepoPath: repo, Hash: hash},
		Files:       []gitops.CommitFile{{Path: "a.txt"}, {Path: "b.txt"}},
	}
	app.gitDiffOpen = true
	app.editorTab = "commit"
	app.gitDiffRepo = "demo"
	app.gitDiffPath = "a.txt"
	app.gitDiffHash = hash
	app.gitCommitDiff = &editor.DiffState{
		Old:    editor.NewBuffer("a.txt\n"),
		New:    editor.NewBuffer("a changed\n"),
		Layout: editor.DiffSplit,
	}
	tt := ui.NewTester(app.BuildUI, 1440, 900)
	tt.Frame()
	for _, label := range []string{"上一个差异", "下一个差异", "修改 (F4)", "上一个文件", "下一个文件"} {
		if _, ok := tt.Find(label); !ok {
			t.Fatalf("commit diff toolbar is missing %q", label)
		}
	}
	if err := tt.Click("修改 (F4)"); err != nil {
		t.Fatalf("open file: %v", err)
	}
	tt.Frame()
	if app.activeTab != TabCode {
		t.Fatal("open file should switch to the code tab")
	}
}
