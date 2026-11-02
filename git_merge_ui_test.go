package main

import (
	"os"
	"path/filepath"
	"testing"

	"github.com/egoist/mygo/ui"
)

func makeReverseMergeWorkspace(t *testing.T) string {
	t.Helper()
	ws := t.TempDir()
	repo := filepath.Join(ws, "demo")
	if err := os.MkdirAll(repo, 0o755); err != nil {
		t.Fatal(err)
	}
	gitRun(t, repo, "init", "-b", "main")
	gitRun(t, repo, "config", "user.name", "Test")
	gitRun(t, repo, "config", "user.email", "test@example.com")
	if err := os.WriteFile(filepath.Join(repo, "base.txt"), []byte("base\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	gitRun(t, repo, "add", "base.txt")
	gitRun(t, repo, "commit", "-m", "init")
	gitRun(t, repo, "branch", "upstream")
	gitRun(t, repo, "switch", "-c", "feature/mine")
	if err := os.WriteFile(filepath.Join(repo, "mine.txt"), []byte("mine\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	gitRun(t, repo, "add", "mine.txt")
	gitRun(t, repo, "commit", "-m", "feature work")
	return ws
}

func TestBranchTreePrefillsMergeIntoCurrentBranch(t *testing.T) {
	app := newGapTestApp(t, makeReverseMergeWorkspace(t))
	app.showGitPanel = true
	app.gitSubTab = "history"
	app.gitEditorPct = 30
	app.gitCollapsedRepos["demo"] = false
	tt := ui.NewTester(app.BuildUI, 1440, 900)
	tt.Frame()

	if err := tt.RightClick("upstream"); err != nil {
		t.Fatalf("right-click upstream: %v", err)
	}
	tt.Frame()
	if !tt.HasText("合并 upstream 到当前分支") {
		t.Fatalf("branch menu item missing; texts=%v", tt.Texts())
	}
	if err := tt.Click("合并 upstream 到当前分支"); err != nil {
		t.Fatalf("click merge item: %v", err)
	}
	tt.Frame()
	if !app.showGitOps || app.gitOpsTab != "merge" || app.gitOpsRepo != "demo" || app.gitOpsMergeBranch != "upstream" {
		t.Fatalf("merge modal not prefilled: show=%v tab=%q repo=%q source=%q",
			app.showGitOps, app.gitOpsTab, app.gitOpsRepo, app.gitOpsMergeBranch)
	}
	if !tt.HasText("将分支合并到当前分支") || !tt.HasText("feature/mine") {
		t.Fatal("merge modal should show the source input and current branch")
	}
}

func TestRepositoryMenuOffersMergeMainIntoCurrentBranch(t *testing.T) {
	app := newGapTestApp(t, makeReverseMergeWorkspace(t))
	app.showGitPanel = true
	app.gitSubTab = "history"
	tt := ui.NewTester(app.BuildUI, 1440, 900)
	tt.Frame()

	if err := tt.RightClick("demo"); err != nil {
		t.Fatalf("right-click repository: %v", err)
	}
	tt.Frame()
	if !tt.HasText("合并 main 到当前分支") {
		t.Fatalf("repository menu item missing; texts=%v", tt.Texts())
	}
	if err := tt.Click("合并 main 到当前分支"); err != nil {
		t.Fatalf("click repository merge item: %v", err)
	}
	tt.Frame()
	if !app.showGitOps || app.gitOpsRepo != "demo" || app.gitOpsMergeBranch != "main" {
		t.Fatalf("repository shortcut did not prefill merge: %+v", app.gitOpsMergeBranch)
	}
}
