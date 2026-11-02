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

func waitMergeIdle(t *testing.T, app *App) {
	t.Helper()
	time.Sleep(300 * time.Millisecond)
	waitApp(t, "background work settled", func() bool {
		return !app.mergeRunning && !app.mergeScanLoading && !app.isRefreshing
	})
}

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
	gitRun(t, repo, "branch", "test")
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
	waitMergeIdle(t, app)
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
	waitMergeIdle(t, app)
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
	waitMergeIdle(t, app)
}

func waitInlineMergeBar(t *testing.T, app *App, tt *ui.Tester) {
	t.Helper()
	for i := 0; i < 300; i++ {
		tt.Frame()
		if !app.mergeScanLoading && len(app.mergeTargets) > 0 {
			return
		}
		time.Sleep(5 * time.Millisecond)
	}
	t.Fatalf("merge scan not ready: loading=%v targets=%+v", app.mergeScanLoading, app.mergeTargets)
}

func TestInlineMergeBarShowsPendingRepositories(t *testing.T) {
	app := newGapTestApp(t, makeReverseMergeWorkspace(t))
	app.showGitPanel = true
	tt := ui.NewTester(app.BuildUI, 1440, 900)
	tt.Frame()
	app.openReleaseMerge()
	waitInlineMergeBar(t, app, tt)
	tt.Frame()
	if !app.mergeBarOpen {
		t.Fatal("merge bar should stay open")
	}
	if !tt.HasText("已选 1/1") || !tt.HasText("合并并推送 (1)") {
		t.Fatalf("inline merge controls missing; texts=%v", tt.Texts())
	}
	if !tt.HasText("+1") {
		t.Fatal("repository row should show its pending commit count")
	}
	if tt.HasText("合并到目标分支：") {
		t.Fatal("old full-page merge view must not render")
	}
}

func TestInlineMergeBarMergesSelectedRepository(t *testing.T) {
	ws := makeReverseMergeWorkspace(t)
	app := newGapTestApp(t, ws)
	app.showGitPanel = true
	tt := ui.NewTester(app.BuildUI, 1440, 900)
	tt.Frame()
	app.openReleaseMerge()
	waitInlineMergeBar(t, app, tt)
	tt.Frame()
	if err := tt.Click("合并 (1)"); err != nil {
		t.Fatalf("click inline merge: %v; texts=%v", err, tt.Texts())
	}
	tt.Frame()
	repo := filepath.Join(ws, "demo")
	waitApp(t, "test updated", func() bool {
		out, _ := gitops.RunGit(context.Background(), repo, "show", "test:mine.txt")
		return out == "mine"
	})
	out, _ := gitops.RunGit(context.Background(), repo, "branch", "--show-current")
	if out != "feature/mine" {
		t.Fatalf("checked-out branch changed to %q", out)
	}
	waitMergeIdle(t, app)
}

func TestInlineMergeBarSelectionMenu(t *testing.T) {
	app := newGapTestApp(t, makeReverseMergeWorkspace(t))
	app.showGitPanel = true
	tt := ui.NewTester(app.BuildUI, 1440, 900)
	tt.Frame()
	app.openReleaseMerge()
	waitInlineMergeBar(t, app, tt)
	tt.Frame()
	if err := tt.Click("已选 1/1"); err != nil {
		t.Fatalf("open selection menu: %v", err)
	}
	tt.Frame()
	if !tt.HasText("清空") || !tt.HasText("全选可合并") {
		t.Fatalf("selection menu missing; texts=%v", tt.Texts())
	}
	if err := tt.Click("清空"); err != nil {
		t.Fatalf("clear selection: %v", err)
	}
	tt.Frame()
	if !tt.HasText("已选 0/1") || !tt.HasText("合并 (0)") {
		t.Fatalf("selection did not clear; texts=%v", tt.Texts())
	}
	if err := tt.Click("已选 0/1"); err != nil {
		t.Fatalf("reopen selection menu: %v", err)
	}
	tt.Frame()
	if err := tt.Click("全选可合并"); err != nil {
		t.Fatalf("select all mergeable: %v", err)
	}
	tt.Frame()
	if !tt.HasText("已选 1/1") {
		t.Fatalf("select-all did not restore selection; texts=%v", tt.Texts())
	}
}
