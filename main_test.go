package main

import (
	"os"
	"path/filepath"
	"strings"
	"testing"

	"github.com/egoist/mygo/ui"
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
		// Create an older branch (> 7 days ago) to verify stale branch folding
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

func TestNemorousHeadlessUI(t *testing.T) {
	t.Setenv("NEMOROUS_CONFIG", filepath.Join(t.TempDir(), "config.json"))
	ws := setupUITestWorkspace(t)
	cfg := config.Default()
	cfg.WorkspacePath = ws
	cfg.IncludeRootRepo = false

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
	if !tt.HasText("无改动仓库 (1)") {
		t.Fatal("expected collapsed clean repositories indicator in Changes view")
	}

	// Expand collapsed commit options
	if err := tt.Click("更多选项"); err != nil {
		t.Fatalf("failed to expand commit options: %v", err)
	}
	if !tt.HasText("常用前缀：") {
		t.Fatal("expected commit prefixes after expanding commit options")
	}
	for _, label := range []string{"修正上一次提交 (--amend)", "仅已暂存", "Stash ▾"} {
		if !tt.HasText(label) {
			t.Fatalf("expected %q in commit options", label)
		}
	}

	// Generate a commit message from the checked changes without leaving the view.
	app.generateCommitMessage()
	if !strings.Contains(app.commitMessage, "cloud-caring") || !strings.Contains(app.commitMessage, "2 个文件") {
		t.Fatalf("expected generated commit message to summarize the checked files, got %q", app.commitMessage)
	}
	if !strings.Contains(app.commitMessage, "修改 2") {
		t.Fatalf("expected generated message to count modified files, got %q", app.commitMessage)
	}
	tt.Frame()
	if !tt.HasText("逐仓说明 · 1") {
		t.Fatal("expected per-repository message badge after generating")
	}

	// Search filters the tree without changing the checked set.
	app.changesFilterQuery = "中文"
	filtered := app.filterChangeFiles("cloud-caring", app.statusByID["cloud-caring"].Files)
	if len(filtered) != 1 || !strings.Contains(filtered[0].Path, "中文模块.go") {
		t.Fatalf("expected search to narrow the file list, got %+v", filtered)
	}
	app.changesFilterQuery = ""

	if !app.perRepoMessagesActive() {
		t.Fatal("generated per-repository messages should match the current selection")
	}
	firstKey := "cloud-caring::main.go"
	app.checkedChanges[firstKey] = false
	if app.perRepoMessagesActive() {
		t.Fatal("per-repository messages must be invalidated when the selection changes")
	}
	app.checkedChanges[firstKey] = true

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
	if !tt.HasText("src/main/go") {
		t.Fatal("expected compacted single-child directory src/main/go in file tree")
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
	// Repository inspector shows a recent-first branch dropdown instead of a flat pill wall.
	app.selectedRepoID = "cloud-caring"
	app.showRepoDetailDrawer = true
	tt.Frame()
	if !tt.HasText("最近提交优先") || !tt.HasText("切换分支") {
		t.Fatal("expected recent-first branch dropdown in repository inspector")
	}
	if tt.HasText("近期活跃分支") {
		t.Fatal("flat branch pill wall should be gone from the inspector")
	}
	app.showRepoDetailDrawer = false

	// 4. Open Branch Switch Modal, verify stale branch folding (>7 days), fuzzy branch search & saving a multi-project branch preset ("AAA 需求")
	app.openBranchSwitchModal()
	tt.Frame()
	if !tt.HasText("切换分支与分支组合") {
		t.Fatal("expected multi-project branch switch modal")
	}
	if !tt.HasText("按层级选择近期分支") {
		t.Fatal("expected hierarchical recent-branch dropdown in branch modal")
	}
	if !tt.HasText("一周前无变更分支 (1)") {
		t.Fatal("expected stale branch (>7 days) to be collapsed under 一周前无变更分支 (1)")
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
