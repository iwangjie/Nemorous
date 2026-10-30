package main

import (
	"os"
	"path/filepath"
	"strings"
	"testing"

	"github.com/egoist/mygo/ui"
	"github.com/egoist/mygo/ui/editor"
	"github.com/iwangjie/nemorous/internal/config"
	"github.com/iwangjie/nemorous/internal/gitops"
)

func setupUITestWorkspace(t *testing.T) string {
	t.Helper()
	ws := t.TempDir()
	maniContent := `projects:
  cloud-caring:
    path: cloud-caring
    desc: 心理测评与筛查核心服务
  risk-cloud:
    path: risk-cloud
    desc: 风险评估微服务
tasks:
  status:
    desc: 查看所有项目状态
    cmd: git status -sb
`
	_ = os.WriteFile(filepath.Join(ws, "mani.yaml"), []byte(maniContent), 0o644)

	for _, name := range []string{"cloud-caring", "risk-cloud"} {
		dir := filepath.Join(ws, name)
		_ = os.MkdirAll(dir, 0o755)
		ctx := t.Context()
		_, _ = gitops.RunGit(ctx, dir, "init", "-b", "main")
		_, _ = gitops.RunGit(ctx, dir, "config", "user.name", "Test")
		_, _ = gitops.RunGit(ctx, dir, "config", "user.email", "test@nemorous.dev")
		_ = os.WriteFile(filepath.Join(dir, "main.go"), []byte("package main\nfunc main() {}\n"), 0o644)
		_ = os.MkdirAll(filepath.Join(dir, "src", "main", "go"), 0o755)
		_ = os.WriteFile(filepath.Join(dir, "src", "main", "go", "中文模块.go"), []byte("package main\n// 中文模块\n"), 0o644)
		_, _ = gitops.RunGit(ctx, dir, "add", "-A")
		_, _ = gitops.RunGit(ctx, dir, "commit", "-m", "init "+name)
		// Create an older branch (> 7 days ago) to verify tier ordering keeps it below recent work.
		_, _ = gitops.RunGit(ctx, dir, "checkout", "-b", "legacy/old-archive")
		_ = os.WriteFile(filepath.Join(dir, "legacy.txt"), []byte("old\n"), 0o644)
		_, _ = gitops.RunGit(ctx, dir, "add", "legacy.txt")
		t.Setenv("GIT_AUTHOR_DATE", "2025-01-01T00:00:00Z")
		t.Setenv("GIT_COMMITTER_DATE", "2025-01-01T00:00:00Z")
		_, _ = gitops.RunGit(ctx, dir, "commit", "-m", "old commit")
		t.Setenv("GIT_AUTHOR_DATE", "")
		t.Setenv("GIT_COMMITTER_DATE", "")
		_, _ = gitops.RunGit(ctx, dir, "checkout", "main")
	}
	// Make cloud-caring dirty (including a Chinese filename) so Changes & Diff view has active content
	_ = os.WriteFile(filepath.Join(ws, "cloud-caring", "main.go"), []byte("package main\nfunc main() {\n\tprintln(\"modified\")\n}\n"), 0o644)
	_ = os.WriteFile(filepath.Join(ws, "cloud-caring", "src", "main", "go", "中文模块.go"), []byte("package main\n// 中文模块已更新\n"), 0o644)
	return ws
}

func TestCodeTabsReuseAndClose(t *testing.T) {
	t.Setenv("NEMOROUS_CONFIG", filepath.Join(t.TempDir(), "config.json"))
	ws := setupUITestWorkspace(t)
	cfg := config.Default()
	cfg.WorkspacePath = ws
	cfg.IncludeRootRepo = false
	app := NewApp(cfg)
	app.LoadWorkspaceSync(ws)

	mainFile := gitops.WorkspaceFile{
		RepoID: "cloud-caring", RepoName: "cloud-caring",
		RepoPath: filepath.Join(ws, "cloud-caring"), RelPath: "main.go",
	}
	cnFile := gitops.WorkspaceFile{
		RepoID: "cloud-caring", RepoName: "cloud-caring",
		RepoPath: filepath.Join(ws, "cloud-caring"), RelPath: "src/main/go/中文模块.go",
	}

	app.selectCodeFile(mainFile)
	app.selectCodeFile(cnFile)
	if len(app.codeTabs) != 2 || app.codeTabIdx != 1 {
		t.Fatalf("expected two tabs with the second active, got %d tabs idx %d", len(app.codeTabs), app.codeTabIdx)
	}
	if !app.expandedCodeRepos["cloud-caring"] {
		t.Fatal("opening a file should reveal its repository")
	}
	if app.codeTabs[0].state.ReadOnly {
		t.Fatal("files should be editable directly without an edit-mode toggle")
	}
	gotoBound := false
	for _, b := range app.codeTabs[0].state.Bindings {
		if b.Mods == ui.Cmd && b.Key == ui.KeyG && b.Command == editor.GoToLine {
			gotoBound = true
		}
	}
	if !gotoBound {
		t.Fatal("Cmd+G should open go-to-line in the code editor")
	}

	app.codeTabs[1].state.Buffer.Set(app.codeTabs[1].state.Buffer.String() + "\n// unsaved\n")
	app.selectCodeFile(mainFile)
	if len(app.codeTabs) != 2 || app.codeTabIdx != 0 {
		t.Fatalf("reopening a file must activate its tab, got %d tabs idx %d", len(app.codeTabs), app.codeTabIdx)
	}
	if !strings.Contains(app.codeTabs[1].state.Buffer.String(), "// unsaved") {
		t.Fatal("switching tabs must keep unsaved buffers")
	}

	app.requestCloseCodeTab(app.codeTabs[1])
	if app.pendingCodeTabClose == nil {
		t.Fatal("closing a dirty tab should ask for confirmation")
	}
	app.pendingCodeTabClose = nil
	app.closeCodeTab(app.codeTabs[1])
	if len(app.codeTabs) != 1 || app.codeTabIdx != 0 {
		t.Fatalf("closing an inactive tab should keep the active one, got %d tabs idx %d", len(app.codeTabs), app.codeTabIdx)
	}
	app.closeCodeTab(app.codeTabs[0])
	if len(app.codeTabs) != 0 || app.codeTabIdx != -1 {
		t.Fatalf("closing the last tab should leave no active tab, got %d tabs idx %d", len(app.codeTabs), app.codeTabIdx)
	}
}

func TestBatchNotificationFoldsResultToast(t *testing.T) {
	app := NewApp(config.Default())
	app.setBatchResults([]gitops.OpResult{{Status: "OK"}, {Status: "FAIL"}})
	if app.notify == nil || !app.notify.Clickable || app.notify.Kind != "warn" {
		t.Fatalf("expected a clickable warning result card, got %+v", app.notify)
	}
	app.showToast("warn", "推送完成：1 成功，1 失败")
	if app.notify == nil || !app.notify.Clickable || app.notify.Message != "推送完成：1 成功，1 失败" {
		t.Fatalf("the result toast should keep the clickable card, got %+v", app.notify)
	}
	app.setBatchResults(nil)
	if app.notify != nil {
		t.Fatalf("clearing results should dismiss the card, got %+v", app.notify)
	}
}

func TestNotificationOpensOperationDetails(t *testing.T) {
	t.Setenv("NEMOROUS_CONFIG", filepath.Join(t.TempDir(), "config.json"))
	ws := setupUITestWorkspace(t)
	cfg := config.Default()
	cfg.WorkspacePath = ws
	cfg.IncludeRootRepo = false
	app := NewApp(cfg)
	app.LoadWorkspaceSync(ws)
	tt := ui.NewTester(app.BuildUI, 1440, 900)
	tt.Frame()

	app.setBatchResults([]gitops.OpResult{{Status: "OK"}})
	app.showToast("ok", "推送完成：1 成功")
	tt.Frame()
	if err := tt.Click("点击查看详情"); err != nil {
		t.Fatalf("result card should offer details: %v", err)
	}
	tt.Frame()
	if !app.showOpLog {
		t.Fatal("clicking the result card should open the operation log")
	}
}

func TestBranchPickerViews(t *testing.T) {
	t.Setenv("NEMOROUS_CONFIG", filepath.Join(t.TempDir(), "config.json"))
	ws := setupUITestWorkspace(t)
	cfg := config.Default()
	cfg.WorkspacePath = ws
	cfg.IncludeRootRepo = false
	app := NewApp(cfg)
	app.LoadWorkspaceSync(ws)
	tt := ui.NewTester(app.BuildUI, 1440, 900)
	tt.Frame()

	app.showBranchPicker = true
	tt.Frame()
	for _, want := range []string{"拉取全部仓库", "推送待推送仓库", "切换或新建分支…", "我的最近提交", "全部分支与分支组合…"} {
		if !tt.HasText(want) {
			t.Fatalf("expected %q in the branch picker", want)
		}
	}

	app.branchPickerRepoID = "cloud-caring"
	tt.Frame()
	if !tt.HasText("← 全部仓库") || !tt.HasText("legacy/old-archive") {
		t.Fatal("expected the repository branch view with its local branches")
	}

	app.branchPickerRepoID = ""
	app.branchPickerQuery = "main"
	tt.Frame()
	if !tt.HasText("分支") || !tt.HasText("main") {
		t.Fatal("expected branch search results")
	}
}

func TestNemorousHeadlessUI(t *testing.T) {
	t.Setenv("NEMOROUS_CONFIG", filepath.Join(t.TempDir(), "config.json"))
	ws := setupUITestWorkspace(t)
	cfg := config.Default()
	cfg.WorkspacePath = ws
	cfg.IncludeRootRepo = false
	cfg.NavShowLabels = true

	app := NewApp(cfg)
	app.LoadWorkspaceSync(ws)

	tt := ui.NewTester(app.BuildUI, 1440, 900)

	// 1. Default view is Changes & Diff (变更), including Chinese file support
	if app.activeTab != TabChanges {
		t.Fatalf("expected initial TabChanges, got %s", app.activeTab)
	}
	if !tt.HasText("本地变更 (2/2)") {
		t.Fatal("expected local changes header in Changes view")
	}
	if !tt.HasText("中文模块.go") {
		t.Fatal("expected Chinese filename 中文模块.go in Changes view")
	}
	if tt.HasText("无改动仓库") {
		t.Fatal("clean repositories should not be listed in the changes view")
	}
	if tt.HasText("实时同步中") {
		t.Fatal("the live sync indicator should be gone from the status bar")
	}
	if app.diffState == nil {
		t.Fatal("expected a diff state after loading the workspace")
	}
	if !app.diffState.Wrap || app.diffState.Font.Size != 14 {
		t.Fatalf("diff should default to wrap with 14pt, got wrap=%v size=%v", app.diffState.Wrap, app.diffState.Font.Size)
	}

	// Commit box keeps only the message, actions and a visible amend toggle.
	if !tt.HasText("修正") {
		t.Fatal("amend should be visible next to the commit actions")
	}
	for _, gone := range []string{"常用前缀", "提交范围", "生成说明", "逐仓说明", "更多选项", "仅已暂存", "仅当前仓库", "⌘Enter / ⌘⇧Enter"} {
		if tt.HasText(gone) {
			t.Fatalf("%q should be removed from the commit box", gone)
		}
	}
	if !tt.HasText("Stash ▾") {
		t.Fatal("Stash menu should remain in the changes header")
	}

	// Search filters the tree without changing the checked set.
	app.changesFilterQuery = "中文"
	filtered := app.filterChangeFiles("cloud-caring", app.statusByID["cloud-caring"].Files)
	if len(filtered) != 1 || !strings.Contains(filtered[0].Path, "中文模块.go") {
		t.Fatalf("expected search to narrow the file list, got %+v", filtered)
	}
	app.changesFilterQuery = ""

	// Keyboard selection: Space toggles the checkbox, arrows move the diff selection.
	selectedKey := app.selectedChangeKey
	before := app.checkedChanges[selectedKey]
	tt.Key(0, ui.KeySpace)
	tt.Frame()
	if app.checkedChanges[selectedKey] == before {
		t.Fatal("Space should toggle the selected file checkbox")
	}
	tt.Key(0, ui.KeyDown)
	tt.Frame()
	if app.selectedChangeKey == selectedKey {
		t.Fatal("Down should move the diff selection")
	}

	// 2. Navigate to Files / Code view (文件) and verify hierarchical file tree + compacted folders + Chinese filename
	if err := tt.Click("文件"); err != nil {
		t.Fatalf("failed to click Files tab: %v", err)
	}
	if app.activeTab != TabCode {
		t.Fatalf("expected TabCode, got %s", app.activeTab)
	}
	if !tt.HasText("cloud-caring") || !tt.HasText("risk-cloud") {
		t.Fatal("expected repository headers in the file tree")
	}
	if tt.HasText("src/main/go") {
		t.Fatal("repositories should start collapsed in the file tree")
	}
	app.expandedCodeRepos["cloud-caring"] = true
	tt.Frame()
	if !tt.HasText("src/main/go") {
		t.Fatal("expected compacted single-child directory src/main/go after expanding the repository")
	}
	if err := tt.Click("src/main/go"); err != nil {
		t.Fatalf("failed to click folder src/main/go in file tree: %v", err)
	}
	if !tt.HasText("中文模块.go") {
		t.Fatal("expected Chinese filename 中文模块.go after expanding src/main/go in file tree")
	}

	// 3. Navigate to Repositories Overview (仓库) and verify recent/stale branch activity folding
	if err := tt.Click("仓库"); err != nil {
		t.Fatalf("failed to click Repositories tab: %v", err)
	}
	if app.activeTab != TabOverview {
		t.Fatalf("expected TabOverview, got %s", app.activeTab)
	}
	if !tt.HasText("cloud-caring") || !tt.HasText("risk-cloud") {
		t.Fatal("expected discovered repositories in Overview table")
	}
	// Repository inspector shows a tier-ordered branch dropdown instead of a flat pill wall.
	app.selectedRepoID = "cloud-caring"
	app.showRepoDetailDrawer = true
	tt.Frame()
	if !tt.HasText("我的分支优先") || !tt.HasText("切换分支") {
		t.Fatal("expected tier-ordered branch dropdown in repository inspector")
	}
	if tt.HasText("近期活跃分支") {
		t.Fatal("flat branch pill wall should be gone from the inspector")
	}
	app.showRepoDetailDrawer = false

	// 4. Open Branch Switch Modal, verify the 7-day activity sections are gone, fuzzy branch search & saving a multi-project branch preset ("AAA 需求")
	app.openBranchSwitchModal()
	tt.Frame()
	if !tt.HasText("切换分支与分支组合") {
		t.Fatal("expected multi-project branch switch modal")
	}
	if !tt.HasText("按层级选择分支") {
		t.Fatal("expected a single hierarchical branch dropdown in branch modal")
	}
	if tt.HasText("近 7 天活跃") || tt.HasText("一周前无变更分支") {
		t.Fatal("branch modal must not split branches by 7-day activity anymore")
	}
	// Opening the fuzzy list marks branches whose tip I authored with a me badge.
	tt.Key(0, ui.KeyDown)
	tt.Frame()
	meBadge := false
	for _, text := range tt.Texts() {
		if text == "me" {
			meBadge = true
		}
	}
	if !meBadge {
		t.Fatal("expected a me badge in the branch picker for branches I committed to")
	}
	if !tt.HasText("有未提交改动 · 将跳过") {
		t.Fatal("expected dirty repository warning in branch modal")
	}
	if !tt.HasText("新建并切换 (2)") {
		t.Fatal("expected an explicit create-and-switch action")
	}

	// Global shortcuts must not fire behind a dialog.
	tabBefore := app.activeTab
	tt.Key(ui.Cmd, ui.Key1)
	tt.Key(ui.Cmd, ui.KeyEnter)
	tt.Frame()
	if app.activeTab != tabBefore {
		t.Fatal("dialog should own the keyboard: view changed behind the branch modal")
	}
	if app.busyLabel != "" {
		t.Fatal("dialog should own the keyboard: commit started behind the branch modal")
	}
	// Verify fuzzy branch matching (subsequence + segment boundary)
	cands := app.BranchCandidatesForRepo("cloud-caring")
	fuzzyHits := fuzzyMatchBranches(cands, "l/oa", 5)
	if len(fuzzyHits) == 0 || fuzzyHits[0].Name != "legacy/old-archive" {
		t.Fatalf("expected subsequence fuzzy match 'l/oa' -> 'legacy/old-archive', got %+v", fuzzyHits)
	}
	segHits := fuzzyMatchBranches(cands, "archive", 5)
	if len(segHits) == 0 || segHits[0].Name != "legacy/old-archive" {
		t.Fatalf("expected segment fuzzy match 'archive' -> 'legacy/old-archive', got %+v", segHits)
	}
	caringBranch := "feat/aaa-caring"
	riskBranch := "feat/aaa-risk"
	app.branchPerRepoTarget["cloud-caring"] = &caringBranch
	app.branchPerRepoTarget["risk-cloud"] = &riskBranch
	app.presetNameInput = "AAA 需求"
	app.SaveCurrentSelectionAsPreset()
	tt.Frame()
	if len(app.cfg.BranchPresets) != 1 || app.cfg.BranchPresets[0].Name != "AAA 需求" {
		t.Fatalf("expected saved preset AAA 需求, got %+v", app.cfg.BranchPresets)
	}
	app.showBranchModal = false
	tt.Frame()

	// 5. Navigate to Merge view (合并)
	if err := tt.Click("合并"); err != nil {
		t.Fatalf("failed to click Merge tab: %v", err)
	}
	if app.activeTab != TabMerge {
		t.Fatalf("expected TabMerge, got %s", app.activeTab)
	}
	if !tt.HasText("合并到目标分支：") {
		t.Fatal("expected merge target header in Merge view")
	}
	if !tt.HasText("回到开发分支") || !tt.HasText("合并方案") {
		t.Fatal("expected repository selection, merge presets and return-to-development controls")
	}

	// 6. Navigate to Scripts view (脚本)
	if err := tt.Click("脚本"); err != nil {
		t.Fatalf("failed to click Scripts tab: %v", err)
	}
	if app.activeTab != TabMani {
		t.Fatalf("expected TabMani, got %s", app.activeTab)
	}
	if !tt.HasText("批量脚本 (mani.yaml)") {
		t.Fatal("expected mani scripts title in Scripts view")
	}

	// 7. Open Settings modal and verify omp CLI support
	app.showSettings = true
	tt.Frame()
	if !tt.HasText("omp") {
		t.Fatal("expected omp option in Settings modal")
	}
	app.showSettings = false

	// 8. Toggle theme (Light <-> Dark)
	if app.cfg.Theme != "light" {
		t.Fatalf("expected default light theme, got %s", app.cfg.Theme)
	}
	app.toggleTheme()
	tt.Frame()
	if app.cfg.Theme != "dark" {
		t.Fatalf("expected dark theme after toggle, got %s", app.cfg.Theme)
	}
}
