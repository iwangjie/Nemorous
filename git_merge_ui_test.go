package main

import (
	"context"
	"os"
	"path/filepath"
	"strings"
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

func makeMergeConflictWorkspace(t *testing.T) string {
	t.Helper()
	dir := t.TempDir()
	gitRun(t, dir, "init", "-b", "main")
	gitRun(t, dir, "config", "user.name", "Test")
	gitRun(t, dir, "config", "user.email", "test@example.com")
	path := filepath.Join(dir, "base.txt")
	if err := os.WriteFile(path, []byte("base\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	gitRun(t, dir, "add", ".")
	gitRun(t, dir, "commit", "-m", "base")
	gitRun(t, dir, "branch", "test")
	gitRun(t, dir, "switch", "-c", "feature/mine")
	if err := os.WriteFile(path, []byte("feature\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	gitRun(t, dir, "commit", "-am", "feature")
	gitRun(t, dir, "switch", "main")
	if err := os.WriteFile(path, []byte("main\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	gitRun(t, dir, "commit", "-am", "main")
	gitRun(t, dir, "branch", "-f", "test", "main")
	gitRun(t, dir, "switch", "feature/mine")
	return dir
}

func TestInlineMergeConflictOpensResolver(t *testing.T) {
	app := newGapTestApp(t, makeMergeConflictWorkspace(t))
	app.showGitPanel = true
	tt := ui.NewTester(app.BuildUI, 1440, 900)
	tt.Frame()
	app.openReleaseMerge()
	waitInlineMergeBar(t, app, tt)
	tt.Frame()
	if err := tt.Click("合并 (1)"); err != nil {
		t.Fatalf("start merge: %v", err)
	}
	waitApp(t, "conflict resolver to open", func() bool {
		return app.showConflict && !app.mergeRunning && len(app.manualMerges) == 1
	})
	if app.conflictRepoID == "" || !app.conflictManual {
		t.Fatalf("resolver opened without a manual worktree: repo=%q manual=%v", app.conflictRepoID, app.conflictManual)
	}
	tt.Frame()
	if !tt.HasText("冲突解决") {
		t.Fatalf("conflict modal did not render; texts=%v", tt.Texts())
	}
	if !tt.HasText("解决冲突 (1)") {
		t.Fatalf("conflict entry missing from merge bar; texts=%v", tt.Texts())
	}
	app.showConflict = false
	tt.Frame()
	if err := tt.Click("解决冲突 (1)"); err != nil {
		t.Fatalf("reopen conflict from merge bar: %v", err)
	}
	tt.Frame()
	if !app.showConflict {
		t.Fatal("merge bar conflict entry did not reopen the resolver")
	}
}

func TestManualConflictCanFinishFromResolver(t *testing.T) {
	app := newGapTestApp(t, makeMergeConflictWorkspace(t))
	app.showGitPanel = true
	tt := ui.NewTester(app.BuildUI, 1440, 900)
	tt.Frame()
	app.openReleaseMerge()
	waitInlineMergeBar(t, app, tt)
	tt.Frame()
	if err := tt.Click("合并 (1)"); err != nil {
		t.Fatalf("start merge: %v", err)
	}
	waitApp(t, "conflict file", func() bool {
		return app.showConflict && app.conflictEditor != nil && len(app.conflictFiles) == 1
	})
	resolved := "resolved\n"
	if err := os.WriteFile(filepath.Join(app.conflictRepoPath, "base.txt"), []byte(resolved), 0o644); err != nil {
		t.Fatal(err)
	}
	gitRun(t, app.conflictRepoPath, "add", "base.txt")
	app.loadConflictFiles()
	waitApp(t, "all conflicts staged", func() bool {
		return len(app.conflictFiles) == 0 && !app.conflictLoading
	})
	tt.Frame()
	if !tt.HasText("完成（本地）") || !tt.HasText("完成并推送") {
		t.Fatalf("finish actions missing from resolver; texts=%v", tt.Texts())
	}
	if err := tt.Click("完成（本地）"); err != nil {
		t.Fatalf("finish manual merge: %v", err)
	}
	waitApp(t, "manual merge finished", func() bool {
		return len(app.manualMerges) == 0 && app.busyLabel == ""
	})
	out, _ := gitops.RunGit(context.Background(), app.repos[0].AbsPath, "show", "test:base.txt")
	if strings.TrimSpace(out) != strings.TrimSpace(resolved) {
		t.Fatalf("test branch was not updated: %q", out)
	}
}

func TestMergeConflictRecoversAfterRestart(t *testing.T) {
	ws := makeMergeConflictWorkspace(t)
	first := newGapTestApp(t, ws)
	first.showGitPanel = true
	firstTT := ui.NewTester(first.BuildUI, 1440, 900)
	firstTT.Frame()
	first.openReleaseMerge()
	waitInlineMergeBar(t, first, firstTT)
	firstTT.Frame()
	if err := firstTT.Click("合并 (1)"); err != nil {
		t.Fatalf("start merge: %v", err)
	}
	waitApp(t, "first conflict", func() bool {
		return first.showConflict && len(first.manualMerges) == 1
	})

	// A fresh app models the restart: the conflict lives only in a temporary
	// worktree and must be discovered again when the merge bar opens.
	second := newGapTestApp(t, ws)
	second.showGitPanel = true
	secondTT := ui.NewTester(second.BuildUI, 1440, 900)
	secondTT.Frame()
	second.openReleaseMerge()
	waitInlineMergeBar(t, second, secondTT)
	waitApp(t, "recovered conflict", func() bool {
		return len(second.manualMerges) == 1
	})
	secondTT.Frame()
	if !secondTT.HasText("解决冲突 (1)") {
		t.Fatalf("recovered conflict is not reachable; texts=%v", secondTT.Texts())
	}
	for id := range second.manualMerges {
		second.cleanupManualMerge(id)
	}
	time.Sleep(300 * time.Millisecond)
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
	if err := tt.Click("demo · feature/mine → test"); err != nil {
		t.Fatalf("toggle repository row: %v", err)
	}
	tt.Frame()
	if !tt.HasText("已选 0/1") {
		t.Fatalf("repository row did not toggle; texts=%v", tt.Texts())
	}
	if !tt.HasText("全选可合并") {
		t.Fatalf("selection picker closed after one row click; texts=%v", tt.Texts())
	}
	if err := tt.Click("全选可合并"); err != nil {
		t.Fatalf("select all mergeable: %v", err)
	}
	tt.Frame()
	if !tt.HasText("已选 1/1") {
		t.Fatalf("select-all did not restore selection; texts=%v", tt.Texts())
	}
	if err := tt.Click("清空"); err != nil {
		t.Fatalf("clear selection: %v", err)
	}
	tt.Frame()
	if !tt.HasText("已选 0/1") || !tt.HasText("合并 (0)") {
		t.Fatalf("selection did not clear; texts=%v", tt.Texts())
	}
	if !tt.HasText("全选可合并") {
		t.Fatalf("selection picker closed after clear; texts=%v", tt.Texts())
	}
}
