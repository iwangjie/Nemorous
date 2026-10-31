package main

import (
	"context"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"testing"
	"time"

	"github.com/egoist/mygo/ui"
	"github.com/egoist/mygo/ui/editor"
	"github.com/iwangjie/nemorous/internal/config"
	"github.com/iwangjie/nemorous/internal/conflict"
	"github.com/iwangjie/nemorous/internal/gitops"
)

func waitApp(t *testing.T, what string, cond func() bool) {
	t.Helper()
	deadline := time.Now().Add(45 * time.Second)
	for time.Now().Before(deadline) {
		if cond() {
			return
		}
		time.Sleep(15 * time.Millisecond)
	}
	t.Fatalf("timed out waiting for %s", what)
}

func newGapTestApp(t *testing.T, ws string) *App {
	t.Helper()
	t.Setenv("NEMOROUS_CONFIG", filepath.Join(t.TempDir(), "config.json"))
	cfg := config.Default()
	cfg.WorkspacePath = ws
	cfg.IncludeRootRepo = true
	app := NewApp(cfg)
	app.LoadWorkspaceSync(ws)
	return app
}

func gitRun(t *testing.T, dir string, args ...string) string {
	t.Helper()
	out, err := gitops.RunGit(context.Background(), dir, args...)
	if err != nil {
		t.Fatalf("git %v: %v (%s)", args, err, out)
	}
	return out
}

func makeConflictWorkspace(t *testing.T) string {
	t.Helper()
	dir := t.TempDir()
	gitRun(t, dir, "init", "-b", "main")
	gitRun(t, dir, "config", "user.name", "Test")
	gitRun(t, dir, "config", "user.email", "test@example.com")
	path := filepath.Join(dir, "file.txt")
	if err := os.WriteFile(path, []byte("base\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	gitRun(t, dir, "add", ".")
	gitRun(t, dir, "commit", "-m", "base")
	gitRun(t, dir, "checkout", "-b", "feature")
	_ = os.WriteFile(path, []byte("feature\n"), 0o644)
	gitRun(t, dir, "commit", "-am", "feature")
	gitRun(t, dir, "checkout", "main")
	_ = os.WriteFile(path, []byte("main\n"), 0o644)
	gitRun(t, dir, "commit", "-am", "main")
	gitRun(t, dir, "checkout", "feature")
	if _, err := gitops.Rebase(context.Background(), dir, "main"); err == nil {
		t.Fatal("expected rebase conflict")
	}
	return dir
}

func TestConflictResolverUIEndToEnd(t *testing.T) {
	ws := makeConflictWorkspace(t)
	app := newGapTestApp(t, ws)
	if len(app.statuses) == 0 {
		t.Fatal("workspace repository not discovered")
	}
	repoID := app.statuses[0].Repo.ID
	if st := gitops.RepoOperationState(context.Background(), ws); !st.Rebase || len(st.Conflicts) == 0 {
		t.Fatalf("expected rebase conflict, got %+v", st)
	}
	app.openConflictResolver(repoID)
	waitApp(t, "conflict files", func() bool {
		return len(app.conflictFiles) > 0 && app.conflictEditor != nil && app.conflictDoc != nil
	})
	waitApp(t, "conflict operation label", func() bool { return strings.Contains(app.conflictOpLabel, "Rebase") })
	tt := ui.NewTester(app.BuildUI, 1440, 900)
	tt.Frame()
	if !tt.HasText("冲突解决") || !tt.HasText("file.txt") || !tt.HasText("RESULT") || !tt.HasText("OURS") || !tt.HasText("THEIRS") {
		t.Fatal("three-pane conflict workbench did not render")
	}
	if !tt.HasText("变基目标（上游）") || !tt.HasText("你的提交") {
		t.Fatal("rebase-specific ours/theirs labels are missing")
	}
	if err := tt.Click("采用 Ours (⌥1)"); err != nil {
		t.Fatalf("accept ours button: %v", err)
	}
	tt.Frame()
	if app.conflictDoc != nil {
		t.Fatal("markers remain after resolving all hunks")
	}
	if err := tt.Click("保存并标记解决 (⌘S)"); err != nil {
		t.Fatalf("save resolved: %v", err)
	}
	waitApp(t, "conflict files cleared", func() bool {
		return len(app.conflictFiles) == 0
	})
	tt.Frame()
	if err := tt.Click("继续 Rebase"); err != nil {
		t.Fatalf("continue button: %v", err)
	}
	waitApp(t, "rebase finished", func() bool {
		return !gitops.RepoOperationState(context.Background(), ws).Busy()
	})
	if data, _ := os.ReadFile(filepath.Join(ws, "file.txt")); strings.TrimSpace(string(data)) != "main" {
		t.Fatalf("resolved content = %q", data)
	}
}

func TestGitOpsStashTagsRemotesUI(t *testing.T) {
	ws := setupUITestWorkspace(t)
	app := newGapTestApp(t, ws)
	repoID := "cloud-caring"
	app.gitOpsRepo = repoID
	path := filepath.Join(ws, "cloud-caring")
	if err := os.WriteFile(filepath.Join(path, "stash-me.txt"), []byte("stash\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	app.runGitOpsAction(func(ctx context.Context, repoPath string) (string, error) {
		return gitops.StashCreate(ctx, repoPath, "ui stash", true)
	})
	waitApp(t, "stash list", func() bool { return len(app.gitOpsStashes) > 0 })
	tt := ui.NewTester(app.BuildUI, 1440, 900)
	app.openGitOps("stash")
	waitApp(t, "git ops loaded", func() bool { return !app.gitOpsLoading })
	tt.Frame()
	if !tt.HasText("Git") || !tt.HasText("Stash") || !tt.HasText("ui stash") {
		t.Fatal("stash tab did not render")
	}
	if err := tt.Click("标签"); err != nil {
		t.Fatalf("tags tab: %v", err)
	}
	tt.Frame()
	if !tt.HasText("标签名") && !tt.HasText("标签") {
		t.Fatal("tags tab did not render")
	}
	if err := tt.Click("远端"); err != nil {
		t.Fatalf("remotes tab: %v", err)
	}
	tt.Frame()
	if !tt.HasText("远端") {
		t.Fatal("remotes tab did not render")
	}
	if err := tt.Click("子模块"); err != nil {
		t.Fatalf("submodules tab: %v", err)
	}
	tt.Frame()
	if !tt.HasText("子模块") {
		t.Fatal("submodules tab did not render")
	}
	// Apply the stash back through the same action the button uses.
	if len(app.gitOpsStashes) == 0 {
		t.Fatal("stash disappeared")
	}
	ref := app.gitOpsStashes[0].Ref
	app.runGitOpsAction(func(ctx context.Context, repoPath string) (string, error) {
		return "applied", gitops.StashPop(ctx, repoPath, ref)
	})
	waitApp(t, "stash pop", func() bool {
		data, _ := os.ReadFile(filepath.Join(path, "stash-me.txt"))
		return strings.TrimSpace(string(data)) == "stash"
	})
}

func TestLocalHistoryUIAndRestore(t *testing.T) {
	ws := setupUITestWorkspace(t)
	app := newGapTestApp(t, ws)
	wf, ok := app.workspaceFileByKey("cloud-caring", "main.go")
	if !ok {
		t.Fatal("main.go not found")
	}
	app.selectCodeFile(wf)
	tab := app.activeCodeTab()
	if tab == nil {
		t.Fatal("no active tab")
	}
	tab.state.Buffer.Set("package main\nfunc main() {\n\tprintln(\"v2\")\n}\n")
	if !app.saveCodeTab(tab) {
		t.Fatal("save failed")
	}
	entries, err := app.historyStore().List("cloud-caring", "main.go")
	if err != nil || len(entries) < 2 {
		t.Fatalf("history entries = %+v err=%v", entries, err)
	}
	base := entries[len(entries)-1]
	for _, candidate := range entries {
		if entry, readErr := app.historyStore().Read("cloud-caring", "main.go", candidate.ID); readErr == nil && !strings.Contains(entry.Content, "v2") {
			base = candidate
			break
		}
	}
	app.openLocalHistory("cloud-caring", wf.RepoPath, "main.go")
	waitApp(t, "history list", func() bool { return len(app.historyEntries) > 0 })
	waitApp(t, "history content", func() bool { return app.historyEditor != nil })
	tt := ui.NewTester(app.BuildUI, 1440, 900)
	tt.Frame()
	if !tt.HasText("本地历史") {
		t.Fatal("local history modal did not render")
	}
	app.selectLocalHistoryEntry(base.ID)
	waitApp(t, "base snapshot", func() bool {
		return app.historyEditor != nil && strings.Contains(app.historyEditor.Buffer.String(), "中文模块") || strings.Contains(app.historyEditor.Buffer.String(), "package main")
	})
	app.restoreLocalHistoryEntry()
	deadline := time.Now().Add(15 * time.Second)
	for time.Now().Before(deadline) {
		data, _ := os.ReadFile(filepath.Join(wf.RepoPath, wf.RelPath))
		if len(data) > 0 && !strings.Contains(string(data), "v2") {
			return
		}
		time.Sleep(20 * time.Millisecond)
	}
	data, _ := os.ReadFile(filepath.Join(wf.RepoPath, wf.RelPath))
	t.Fatalf("timed out waiting for restored file: selected=%q loading=%v err=%q notice=%q entries=%d data=%q", app.historySelected, app.historyLoading, app.historyErr, app.historyNotice, len(app.historyEntries), data)
}

func TestChangeListFilterAndModal(t *testing.T) {
	ws := setupUITestWorkspace(t)
	app := newGapTestApp(t, ws)
	app.cfg.AssignToChangeList("登录需求", []string{"cloud-caring::main.go"})
	app.changeListFilter = "登录需求"
	files := app.filterChangeFiles("cloud-caring", app.statusByID["cloud-caring"].Files)
	if len(files) == 0 {
		t.Fatal("change list filter hid its file")
	}
	for _, f := range files {
		if f.Key() != "cloud-caring::main.go" {
			t.Fatalf("unexpected file in change list: %s", f.Key())
		}
	}
	app.openChangeListModal()
	tt := ui.NewTester(app.BuildUI, 1440, 900)
	tt.Frame()
	if !tt.HasText("变更列表") || !tt.HasText("登录需求") {
		t.Fatal("change list modal did not render")
	}
	app.changeListNameInput = "支付需求"
	app.checkedChanges["cloud-caring::main.go"] = true
	app.createChangeList(true)
	if app.cfg.ChangeListFor("cloud-caring::main.go") != "支付需求" {
		t.Fatalf("file not moved: %q", app.cfg.ChangeListFor("cloud-caring::main.go"))
	}
}

func TestEditorStructureTodoBookmarksAndFormat(t *testing.T) {
	ws := setupUITestWorkspace(t)
	todo := "package main\n\n// TODO: finish this\nfunc todoFunc() {\n\tx := 1\n\t_ = x\n}\n\ntype Thing struct{}\n"
	if err := os.WriteFile(filepath.Join(ws, "cloud-caring", "todo.go"), []byte(todo), 0o644); err != nil {
		t.Fatal(err)
	}
	unformatted := filepath.Join(ws, "cloud-caring", "fmt.go")
	if err := os.WriteFile(unformatted, []byte("package main\nfunc  fmtMe( ){println(\"x\")}\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	app := newGapTestApp(t, ws)
	wf, ok := app.workspaceFileByKey("cloud-caring", "todo.go")
	if !ok {
		t.Fatal("todo.go not in workspace files")
	}
	app.selectCodeFile(wf)
	tab := app.activeCodeTab()
	if tab == nil {
		t.Fatal("no tab")
	}
	app.openStructure()
	if len(app.structureItems) == 0 {
		t.Fatal("structure view found no symbols")
	}
	tab.state.SetSelections(editor.Selection{Anchor: 0, Head: 0}, editor.Selection{Anchor: 1, Head: 1})
	if len(tab.state.Selections()) != 2 {
		t.Fatal("editor multi-cursor selections are not available")
	}
	tab.state.Fold(3)
	if len(tab.state.Folded()) == 0 {
		t.Fatal("editor folding is not available")
	}
	app.showStructure = false
	app.openTodos()
	waitApp(t, "todo scan", func() bool { return !app.todosLoading })
	if len(app.todos) == 0 {
		t.Fatal("TODO scan found nothing")
	}
	tab.state.GoTo(editor.Pos{Line: 2})
	app.toggleBookmarkAtCursor()
	if len(app.cfg.Bookmarks) == 0 {
		t.Fatal("bookmark not added")
	}
	app.openBookmarks()
	tt := ui.NewTester(app.BuildUI, 1440, 900)
	tt.Frame()
	if !tt.HasText("书签") {
		t.Fatal("bookmarks modal did not render")
	}

	fmtWF, ok := app.workspaceFileByKey("cloud-caring", "fmt.go")
	if !ok {
		t.Fatal("fmt.go not found")
	}
	app.selectCodeFile(fmtWF)
	fmtTab := app.activeCodeTab()
	app.cfg.FormatOnSave = true
	fmtTab.state.Buffer.Set("package main\nfunc  fmtMe( ){println(\"y\")}\n")
	if !app.saveCodeTab(fmtTab) {
		t.Fatal("format save failed")
	}
	data, _ := os.ReadFile(unformatted)
	if strings.Contains(string(data), "func  fmtMe( )") {
		t.Fatalf("file was not formatted: %s", data)
	}
}

func TestSessionRestoreOpensTabs(t *testing.T) {
	ws := setupUITestWorkspace(t)
	configDir := t.TempDir()
	t.Setenv("NEMOROUS_CONFIG", filepath.Join(configDir, "config.json"))
	cfg := config.Default()
	cfg.WorkspacePath = ws
	cfg.IncludeRootRepo = false
	app := NewApp(cfg)
	app.LoadWorkspaceSync(ws)
	wf, ok := app.workspaceFileByKey("cloud-caring", "main.go")
	if !ok {
		t.Fatal("main.go missing")
	}
	app.selectCodeFile(wf)
	app.activeTab = TabCode
	app.saveSession()
	app2 := NewApp(cfg)
	app2.LoadWorkspaceSync(ws)
	if len(app2.codeTabs) == 0 {
		t.Fatal("session did not restore open tabs")
	}
	if app2.activeTab != TabCode {
		t.Fatalf("active tab = %s", app2.activeTab)
	}
}

func TestNewWindowLaunchesWorkspaceProcess(t *testing.T) {
	ws := setupUITestWorkspace(t)
	app := newGapTestApp(t, ws)
	oldExe, oldStart := executablePath, startProcess
	defer func() { executablePath, startProcess = oldExe, oldStart }()
	executablePath = func() (string, error) { return "/bin/echo", nil }
	var got *exec.Cmd
	startProcess = func(cmd *exec.Cmd) error {
		got = cmd
		return nil
	}
	app.newWindow("/tmp/other-workspace")
	if got == nil {
		t.Fatal("new window did not start a process")
	}
	env := strings.Join(got.Env, "\n")
	if !strings.Contains(env, "NEMOROUS_WORKSPACE=/tmp/other-workspace") {
		t.Fatalf("new window env missing workspace:\n%s", env)
	}
}

var _ = exec.Command

func TestConflictSmartMergeAndCloseGuard(t *testing.T) {
	ws := setupUITestWorkspace(t)
	app := newGapTestApp(t, ws)
	content := "<<<<<<< HEAD\na\nB\nc\nd\n||||||| base\na\nb\nc\nd\n=======\na\nb\nc\nD\n>>>>>>> other\n"
	doc, err := conflict.Parse(content)
	if err != nil {
		t.Fatal(err)
	}
	buf := editor.NewBuffer(content)
	app.showConflict = true
	app.conflictSelected = "smart.txt"
	app.conflictEditor = &editor.State{Buffer: buf}
	app.conflictDoc = doc
	app.conflictDrafts = make(map[string]*editor.State)
	app.conflictHunkCount = map[string]int{"smart.txt": 1}
	app.conflictAutoResolve()
	if app.conflictDoc != nil || buf.String() != "a\nB\nc\nD\n" {
		t.Fatalf("auto merge = %q doc=%v", buf.String(), app.conflictDoc)
	}
	if !strings.Contains(app.conflictNotice, "已自动解决 1") {
		t.Fatalf("notice = %q", app.conflictNotice)
	}
	// Editing must guard closing.
	buf.Set("edited\n")
	app.closeConflictModal()
	if !app.showConflict || !app.conflictConfirmClose {
		t.Fatal("unsaved conflict edits should guard closing")
	}
	app.conflictConfirmClose = false
	app.conflictDrafts = map[string]*editor.State{}
	app.conflictEditor = nil
	app.showConflict = false
}

func waitAppUI(t *testing.T, app *App, what string, cond func() bool) {
	t.Helper()
	deadline := time.Now().Add(45 * time.Second)
	for time.Now().Before(deadline) {
		app.uiMu.Lock()
		ok := cond()
		app.uiMu.Unlock()
		if ok {
			return
		}
		time.Sleep(15 * time.Millisecond)
	}
	t.Fatalf("timed out waiting for %s", what)
}

func TestBinaryConflictResolution(t *testing.T) {
	dir := t.TempDir()
	gitRun(t, dir, "init", "-b", "main")
	gitRun(t, dir, "config", "user.name", "Test")
	gitRun(t, dir, "config", "user.email", "test@example.com")
	path := filepath.Join(dir, "blob.bin")
	if err := os.WriteFile(path, []byte{0x00, 0x01, 0x02, 0x03}, 0o644); err != nil {
		t.Fatal(err)
	}
	gitRun(t, dir, "add", ".")
	gitRun(t, dir, "commit", "-m", "base")
	gitRun(t, dir, "checkout", "-b", "feature")
	_ = os.WriteFile(path, []byte{0x00, 0x0a, 0x0b}, 0o644)
	gitRun(t, dir, "commit", "-am", "feature")
	gitRun(t, dir, "checkout", "main")
	_ = os.WriteFile(path, []byte{0x00, 0x0c, 0x0d}, 0o644)
	gitRun(t, dir, "commit", "-am", "main")
	gitRun(t, dir, "checkout", "feature")
	if _, err := gitops.Rebase(context.Background(), dir, "main"); err == nil {
		t.Fatal("expected binary conflict")
	}
	app := newGapTestApp(t, dir)
	app.openConflictResolver(app.statuses[0].Repo.ID)
	waitAppUI(t, app, "binary conflict", func() bool { return app.conflictBinary && app.conflictSelected != "" })
	tt := ui.NewTester(app.BuildUI, 1440, 900)
	tt.Frame()
	if !tt.HasText("二进制文件冲突") {
		t.Fatal("binary conflict UI did not render")
	}
	app.runUI(func() { app.conflictBinaryApply(conflict.Theirs) })
	waitApp(t, "binary resolved", func() bool {
		data, _ := os.ReadFile(path)
		return len(data) == 3 && data[1] == 0x0c && data[2] == 0x0d
	})
}

func TestConflictDraftSurvivesReopen(t *testing.T) {
	ws := makeConflictWorkspace(t)
	app := newGapTestApp(t, ws)
	app.openConflictResolver(app.statuses[0].Repo.ID)
	waitApp(t, "conflict file", func() bool { return app.conflictEditor != nil })
	app.conflictEditor.Buffer.Set("draft result\n")
	app.showConflict = false
	app.openConflictResolver(app.statuses[0].Repo.ID)
	waitApp(t, "draft restored", func() bool {
		return app.conflictEditor != nil && strings.Contains(editorText(app.conflictEditor.Buffer), "draft result")
	})
}
