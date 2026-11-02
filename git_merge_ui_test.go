package main

import (
	"context"
	"os"
	"path/filepath"
	"testing"
	"time"

	"github.com/egoist/mygo/ui"
	"github.com/iwangjie/nemorous/internal/gitops"
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
	gitRun(t, repo, "switch", "main")
	if err := os.WriteFile(filepath.Join(repo, "incoming.txt"), []byte("incoming\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	gitRun(t, repo, "add", "incoming.txt")
	gitRun(t, repo, "commit", "-m", "main advanced")
	gitRun(t, repo, "branch", "incoming")
	gitRun(t, repo, "switch", "feature/mine")
	return ws
}

func TestBranchTreeMergesSourceIntoCurrentBranchDirectly(t *testing.T) {
	app := newGapTestApp(t, makeReverseMergeWorkspace(t))
	app.showGitPanel = true
	app.gitSubTab = "history"
	app.gitEditorPct = 30
	app.gitCollapsedRepos["demo"] = false
	tt := ui.NewTester(app.BuildUI, 1440, 900)
	tt.Frame()

	if err := tt.RightClick("incoming"); err != nil {
		t.Fatalf("right-click incoming: %v", err)
	}
	tt.Frame()
	if !tt.HasText("合并 incoming 到当前分支") {
		t.Fatalf("branch menu item missing; texts=%v", tt.Texts())
	}
	if err := tt.Click("合并 incoming 到当前分支"); err != nil {
		t.Fatalf("click merge item: %v", err)
	}
	if app.showGitOps {
		t.Fatal("merge must not open the Git operations dialog")
	}
	repo := filepath.Join(app.cfg.WorkspacePath, "demo")
	waitApp(t, "incoming merged into current", func() bool {
		_, err := os.Stat(filepath.Join(repo, "incoming.txt"))
		return err == nil
	})
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
	if app.showGitOps {
		t.Fatal("merge must not open the Git operations dialog")
	}
	repo := filepath.Join(app.cfg.WorkspacePath, "demo")
	waitApp(t, "main merged into current", func() bool {
		_, err := os.Stat(filepath.Join(repo, "incoming.txt"))
		return err == nil
	})
}

func TestBranchPickerMergesCurrentBranchIntoTarget(t *testing.T) {
	ws := makeReverseMergeWorkspace(t)
	app := newGapTestApp(t, ws)
	app.showBranchPicker = true
	app.branchPickerRepoID = "demo"
	tt := ui.NewTester(app.BuildUI, 1440, 900)
	tt.Frame()

	if err := tt.RightClick("main"); err != nil {
		t.Fatalf("right-click main in branch picker: %v", err)
	}
	tt.Frame()
	if !tt.HasText("合并当前分支到 main") {
		t.Fatalf("branch picker merge item missing; texts=%v", tt.Texts())
	}
	if err := tt.Click("合并当前分支到 main"); err != nil {
		t.Fatalf("open merge submenu: %v", err)
	}
	tt.Frame()
	if !tt.HasText("本地合并") || !tt.HasText("合并并推送") {
		t.Fatalf("merge direction submenu missing; texts=%v", tt.Texts())
	}
	if err := tt.Click("本地合并"); err != nil {
		t.Fatalf("confirm merge: %v", err)
	}
	tt.Frame()
	repo := filepath.Join(ws, "demo")
	waitApp(t, "main updated", func() bool {
		out, _ := gitops.RunGit(context.Background(), repo, "show", "main:mine.txt")
		return out == "mine"
	})
	out, _ := gitops.RunGit(context.Background(), repo, "branch", "--show-current")
	if out != "feature/mine" {
		t.Fatalf("checked-out branch changed to %q", out)
	}
	time.Sleep(300 * time.Millisecond)
	waitApp(t, "background work settled", func() bool {
		return !app.mergeRunning && !app.mergeScanLoading && !app.isRefreshing
	})
}
