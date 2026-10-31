package main

import (
	"context"
	"os"
	"path/filepath"
	"strings"
	"testing"
	"time"

	"github.com/egoist/mygo/ui"
	"github.com/egoist/mygo/ui/editor"
	"github.com/iwangjie/nemorous/internal/gitops"
)

func makeP0Repo(t *testing.T, commits int) string {
	t.Helper()
	dir := t.TempDir()
	gitRun(t, dir, "init", "-b", "main")
	gitRun(t, dir, "config", "user.name", "Test")
	gitRun(t, dir, "config", "user.email", "test@example.com")
	for i := 0; i < commits; i++ {
		name := filepath.Join(dir, "f.txt")
		if err := os.WriteFile(name, []byte(strings.Repeat("line\n", i+1)+"value "+string(rune('a'+i))+"\n"), 0o644); err != nil {
			t.Fatal(err)
		}
		gitRun(t, dir, "add", ".")
		gitRun(t, dir, "commit", "-m", "commit "+string(rune('A'+i)))
	}
	return dir
}

func TestWorkspaceManagerUIAndClone(t *testing.T) {
	src := makeP0Repo(t, 1)
	bare := filepath.Join(t.TempDir(), "source.git")
	if _, err := gitops.RunGit(context.Background(), src, "clone", "--bare", src, bare); err != nil {
		t.Fatal(err)
	}
	ws := t.TempDir()
	app := newGapTestApp(t, ws)
	app.openWorkspaceManager()
	tt := ui.NewTester(app.BuildUI, 1440, 900)
	tt.Frame()
	if !tt.HasText("工作区") || !tt.HasText("仓库") || !tt.HasText("克隆 / 初始化") || !tt.HasText("凭据") || !tt.HasText("LFS / Sparse") || !tt.HasText("Worktree") || !tt.HasText("签名") {
		t.Fatal("workspace manager tabs missing")
	}
	if err := tt.Click("克隆 / 初始化"); err != nil {
		t.Fatal(err)
	}
	tt.Frame()
	if !tt.HasText("克隆仓库") || !tt.HasText("初始化仓库") {
		t.Fatal("clone/init forms missing")
	}
	app.wsCloneURL = bare
	app.wsCloneName = "cloned"
	tt.Frame()
	if err := tt.Click("克隆"); err != nil {
		t.Fatalf("clone button: %v", err)
	}
	waitApp(t, "clone completed", func() bool {
		return !app.wsBusy && len(app.repos) >= 1
	})
	if _, err := os.Stat(filepath.Join(ws, "cloned", ".git")); err != nil {
		t.Fatalf("cloned repo missing: %v", err)
	}
	app.wsBusy = false
	app.workspaceTab = "credentials"
	app.loadWorkspaceTab()
	tt.Frame()
	if !tt.HasText("HTTPS 凭据") || !tt.HasText("SSH Key") {
		t.Fatal("credential forms missing")
	}
}

func TestGitAdvancedP0Tabs(t *testing.T) {
	ws := makeP0Repo(t, 3)
	app := newGapTestApp(t, ws)
	app.openGitOps("rebase")
	app.gitOpsBranch = "HEAD~2"
	app.loadRebasePlan()
	waitApp(t, "rebase plan", func() bool { return len(app.gitOpsRebaseCommits) == 2 && !app.gitOpsRebaseLoading })
	tt := ui.NewTester(app.BuildUI, 1440, 900)
	tt.Frame()
	if !tt.HasText("交互式变基计划") || !tt.HasText("执行交互式变基") || !tt.HasText("pick") {
		t.Fatal("interactive rebase plan missing")
	}
	if err := tt.Click("Reflog"); err != nil {
		t.Fatalf("reflog tab: %v", err)
	}
	tt.Frame()
	if !tt.HasText("Reset 模式") {
		t.Fatal("reflog tab did not render")
	}
	if err := tt.Click("合并"); err != nil {
		t.Fatalf("merge tab: %v", err)
	}
	tt.Frame()
	if !tt.HasText("安全强制推送") {
		t.Fatal("merge tab did not render")
	}
	if err := tt.Click("对比"); err != nil {
		t.Fatalf("compare tab: %v", err)
	}
	tt.Frame()
	if !tt.HasText("三点（merge-base）") {
		t.Fatal("compare tab did not render")
	}
}

func TestDiffHunkStagingUI(t *testing.T) {
	ws := makeP0Repo(t, 1)
	path := filepath.Join(ws, "f.txt")
	if err := os.WriteFile(path, []byte("line\nnew value\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	app := newGapTestApp(t, ws)
	if len(app.statuses) == 0 || len(app.statuses[0].Files) == 0 {
		t.Fatal("changed file not discovered")
	}
	cf := app.statuses[0].Files[0]
	app.selectChangedFile(cf)
	app.activeTab = TabChanges
	tt := ui.NewTester(app.BuildUI, 1440, 900)
	tt.Frame()
	if !tt.HasText("暂存块") {
		t.Fatal("stage hunk control missing")
	}
	if err := tt.Click("暂存块"); err != nil {
		t.Fatalf("stage hunk: %v", err)
	}
	waitApp(t, "hunk staged", func() bool {
		out, _ := gitops.RunGit(context.Background(), ws, "diff", "--cached", "--", "f.txt")
		return strings.Contains(out, "new value")
	})
}

func TestLSPOptionalSettingUI(t *testing.T) {
	ws := makeP0Repo(t, 1)
	app := newGapTestApp(t, ws)
	app.cfg.LSPEnabled = false
	app.showSettings = true
	tt := ui.NewTester(app.BuildUI, 1440, 900)
	tt.Frame()
	if !tt.HasText("可选代码智能 (LSP)") || !tt.HasText("启用 LSP（默认关闭）") {
		t.Fatal("LSP setting missing")
	}
	if err := tt.Click("启用 LSP（默认关闭）"); err != nil {
		t.Fatalf("toggle LSP: %v", err)
	}
	tt.Frame()
	if !app.cfg.LSPEnabled {
		t.Fatal("LSP setting did not turn on")
	}
	if app.cfg.LSPEnabled {
		app.cfg.LSPEnabled = false
		app.closeLSPClients()
	}
}

func TestDiffSelectedLineControlAppears(t *testing.T) {
	ws := makeP0Repo(t, 1)
	if err := os.WriteFile(filepath.Join(ws, "f.txt"), []byte("line\nnew value\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	app := newGapTestApp(t, ws)
	cf := app.statuses[0].Files[0]
	app.selectChangedFile(cf)
	hunks := app.diffState.Hunks()
	if len(hunks) == 0 {
		t.Fatal("no hunks")
	}
	app.diffState.SetSelectedLines(editor.DiffLineRange{Side: editor.DiffNew, Start: hunks[0].NewStart, End: hunks[0].NewStart}, true)
	app.activeTab = TabChanges
	tt := ui.NewTester(app.BuildUI, 1440, 900)
	tt.Frame()
	if !tt.HasText("暂存选中行") {
		t.Fatal("selected-line staging control missing")
	}
	_ = time.Now()
}

func TestInteractiveRebaseExecuteUI(t *testing.T) {
	ws := makeP0Repo(t, 3)
	app := newGapTestApp(t, ws)
	app.openGitOps("rebase")
	app.gitOpsBranch = "HEAD~2"
	app.loadRebasePlan()
	waitApp(t, "rebase plan", func() bool {
		return len(app.gitOpsRebaseCommits) == 2 && !app.gitOpsRebaseLoading && !app.gitOpsLoading
	})
	app.gitOpsRebaseActions[app.gitOpsRebaseCommits[1].Hash] = "drop"
	tt := ui.NewTester(app.BuildUI, 1440, 900)
	tt.Frame()
	if err := tt.Click("执行交互式变基"); err != nil {
		t.Fatalf("execute rebase: %v", err)
	}
	tt.Frame()
	if err := tt.Click("确认重写历史"); err != nil {
		t.Fatalf("confirm rebase: %v", err)
	}
	waitApp(t, "interactive rebase finished", func() bool { return !app.gitOpsLoading })
	if app.gitOpsErr != "" {
		t.Fatalf("rebase error: %s", app.gitOpsErr)
	}
	out, _ := gitops.RunGit(context.Background(), ws, "log", "--format=%s")
	if strings.Contains(out, "commit C") {
		t.Fatalf("dropped commit still present: %q", out)
	}
}

func TestWorkspaceHideAndRestoreUI(t *testing.T) {
	ws := t.TempDir()
	repo := filepath.Join(ws, "demo")
	if err := os.MkdirAll(repo, 0o755); err != nil {
		t.Fatal(err)
	}
	gitRun(t, repo, "init", "-b", "main")
	gitRun(t, repo, "config", "user.name", "Test")
	gitRun(t, repo, "config", "user.email", "test@example.com")
	if err := os.WriteFile(filepath.Join(repo, "a.txt"), []byte("a\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	gitRun(t, repo, "add", ".")
	gitRun(t, repo, "commit", "-m", "initial")

	app := newGapTestApp(t, ws)
	app.openWorkspaceManager()
	tt := ui.NewTester(app.BuildUI, 1440, 900)
	tt.Frame()
	if err := tt.Click("从视图隐藏"); err != nil {
		t.Fatalf("hide button: %v", err)
	}
	tt.Frame()
	if err := tt.Click("确认隐藏"); err != nil {
		t.Fatalf("confirm hide: %v", err)
	}
	waitApp(t, "repo hidden", func() bool { return len(app.repos) == 0 && app.cfg.IsRepoHidden("demo") })
	tt.Frame()
	if err := tt.Click("恢复显示"); err != nil {
		t.Fatalf("restore button: %v", err)
	}
	waitApp(t, "repo restored", func() bool { return len(app.repos) == 1 && !app.cfg.IsRepoHidden("demo") })
}
