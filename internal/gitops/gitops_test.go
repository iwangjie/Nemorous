package gitops_test

import (
	"context"
	"os"
	"path/filepath"
	"testing"

	"github.com/iwangjie/nemorous/internal/aimerge"
	"github.com/iwangjie/nemorous/internal/gitops"
	"github.com/iwangjie/nemorous/internal/mani"
)

func initTestRepo(t *testing.T, dir string, branch string, files map[string]string) {
	t.Helper()
	if err := os.MkdirAll(dir, 0o755); err != nil {
		t.Fatal(err)
	}
	ctx := context.Background()
	mustGit := func(args ...string) {
		t.Helper()
		if _, err := gitops.RunGit(ctx, dir, args...); err != nil {
			t.Fatalf("git %v in %s failed: %v", args, dir, err)
		}
	}
	mustGit("init", "-b", branch)
	mustGit("config", "user.name", "Nemorous Test")
	mustGit("config", "user.email", "test@nemorous.dev")
	for rel, content := range files {
		full := filepath.Join(dir, rel)
		if err := os.MkdirAll(filepath.Dir(full), 0o755); err != nil {
			t.Fatal(err)
		}
		if err := os.WriteFile(full, []byte(content), 0o644); err != nil {
			t.Fatal(err)
		}
	}
	mustGit("add", "-A")
	mustGit("commit", "-m", "initial commit")
}

func TestMultiRepoDiscoveryAndOperations(t *testing.T) {
	ctx := context.Background()
	wsDir := t.TempDir()

	// Write mani.yaml & AGENTS.md
	maniYAML := `projects:
  service-alpha:
    path: service-alpha
    desc: Alpha Service
  service-beta:
    path: service-beta
    desc: Beta Service
tasks:
  hello:
    desc: Echo hello
    cmd: echo "OK      hello from $MANI_PROJECT_NAME"
`
	if err := os.WriteFile(filepath.Join(wsDir, "mani.yaml"), []byte(maniYAML), 0o644); err != nil {
		t.Fatal(err)
	}
	agentsMD := `# Workspace
| 项目 | 说明 |
|---|---|
| service-alpha | 核心交易服务 |
| service-gamma | 网关边缘服务 |
`
	if err := os.WriteFile(filepath.Join(wsDir, "AGENTS.md"), []byte(agentsMD), 0o644); err != nil {
		t.Fatal(err)
	}

	initTestRepo(t, filepath.Join(wsDir, "service-alpha"), "main", map[string]string{
		"main.go": "package main\n\nfunc main() {\n\tprintln(\"alpha v1\")\n}\n",
	})
	initTestRepo(t, filepath.Join(wsDir, "service-beta"), "main", map[string]string{
		"app.py": "def run():\n    return 'beta v1'\n",
	})
	// Unlisted in mani.yaml, but present on disk
	initTestRepo(t, filepath.Join(wsDir, "service-gamma"), "main", map[string]string{
		"index.ts": "export const gamma = 1;\n",
	})

	repos, maniCfg := mani.DiscoverRepos(wsDir, false)
	if maniCfg == nil {
		t.Fatal("expected mani.yaml to be parsed")
	}
	if len(repos) != 3 {
		t.Fatalf("expected 3 discovered repos (2 mani + 1 auto-discovered), got %d", len(repos))
	}
	if repos[0].Description != "核心交易服务" {
		t.Errorf("expected AGENTS.md description override for service-alpha, got %q", repos[0].Description)
	}

	// Test SwitchBranchAll (create feature/order-v2 across all 3 repos)
	switchRes := gitops.SwitchBranchAll(ctx, repos, "feature/order-v2", true, 4)
	for _, r := range switchRes {
		if r.Status != "OK" {
			t.Fatalf("SwitchBranchAll failed for %s: %s", r.RepoName, r.Message)
		}
	}

	// Modify files in service-alpha and service-beta via SaveMicroEdit
	if err := gitops.SaveMicroEdit(repos[0].AbsPath, "main.go", "package main\n\nfunc main() {\n\tprintln(\"alpha v2 updated\")\n}\n"); err != nil {
		t.Fatal(err)
	}
	if err := gitops.SaveMicroEdit(repos[1].AbsPath, "app.py", "def run():\n    return 'beta v2 updated'\n"); err != nil {
		t.Fatal(err)
	}

	statuses := gitops.InspectRepos(ctx, repos, 4)
	if !statuses[0].Dirty() || !statuses[1].Dirty() || statuses[2].Dirty() {
		t.Fatalf("unexpected dirty states: %v, %v, %v", statuses[0].Dirty(), statuses[1].Dirty(), statuses[2].Dirty())
	}

	// Verify GetFileDiff
	oldTxt, newTxt, patch, err := gitops.GetFileDiff(ctx, repos[0].AbsPath, "main.go", false)
	if err != nil {
		t.Fatal(err)
	}
	if oldTxt == newTxt || patch == "" {
		t.Fatalf("expected non-empty diff for main.go")
	}

	// Unified BatchCommit across service-alpha and service-beta
	commitTargets := []gitops.BatchCommitTarget{
		{Repo: repos[0], Files: []string{"main.go"}},
		{Repo: repos[1], Files: []string{"app.py"}},
	}
	commitRes := gitops.BatchCommit(ctx, commitTargets, "feat: unified multi-repo update", false, 4)
	for _, cr := range commitRes {
		if cr.Status != "OK" {
			t.Fatalf("BatchCommit failed on %s: %s", cr.RepoName, cr.Message)
		}
	}

	// Test Pre-flight Merge Analysis (feature/order-v2 -> main)
	plans := aimerge.PreflightMergeAll(ctx, repos, "main", false, []string{"main", "master", "test"}, 4)
	if plans[0].Kind != aimerge.PlanClean || plans[0].AheadCommits != 1 {
		t.Fatalf("expected service-alpha PlanClean with 1 commit, got %+v", plans[0])
	}
	if plans[2].Kind != aimerge.PlanSkipUpToDate {
		t.Fatalf("expected service-gamma PlanSkipUpToDate, got %+v", plans[2])
	}

	// Execute clean worktree merge for service-alpha into main
	mergeOp := aimerge.ExecuteCleanMerge(ctx, plans[0], false)
	if mergeOp.Status != "OK" {
		t.Fatalf("ExecuteCleanMerge failed: %s", mergeOp.Message)
	}

	// Verify mani task execution
	taskRes := mani.RunTask(ctx, wsDir, "hello", maniCfg.Tasks["hello"], repos, nil, 4)
	if len(taskRes) != 2 || taskRes[0].Status != "OK" {
		t.Fatalf("unexpected mani task results: %+v", taskRes)
	}
}

func TestConflictDetectionAndAIMergeVerification(t *testing.T) {
	ctx := context.Background()
	wsDir := t.TempDir()
	repoDir := filepath.Join(wsDir, "risk-cloud")
	initTestRepo(t, repoDir, "test", map[string]string{
		"config.yaml": "timeout: 100\nretries: 3\n",
	})

	// Create feature branch modifying config.yaml
	_, _ = gitops.RunGit(ctx, repoDir, "switch", "-c", "feature/timeout-tune")
	_ = os.WriteFile(filepath.Join(repoDir, "config.yaml"), []byte("timeout: 250\nretries: 5\n"), 0o644)
	_, _ = gitops.RunGit(ctx, repoDir, "commit", "-am", "feat: increase timeout to 250")
	sourceTip, _ := gitops.RunGit(ctx, repoDir, "rev-parse", "HEAD")

	// Modify same line on test branch to create a true 3-way conflict
	_, _ = gitops.RunGit(ctx, repoDir, "switch", "test")
	mergeBase, _ := gitops.RunGit(ctx, repoDir, "rev-parse", "HEAD")
	_ = os.WriteFile(filepath.Join(repoDir, "config.yaml"), []byte("timeout: 180\nretries: 3\n"), 0o644)
	_, _ = gitops.RunGit(ctx, repoDir, "commit", "-am", "chore: adjust timeout on test")
	preTarget, _ := gitops.RunGit(ctx, repoDir, "rev-parse", "HEAD")

	// Switch back to feature/timeout-tune and run pre-flight merge check into test
	_, _ = gitops.RunGit(ctx, repoDir, "switch", "feature/timeout-tune")
	repos, _ := mani.DiscoverRepos(wsDir, false)
	plans := aimerge.PreflightMergeAll(ctx, repos, "test", false, []string{"main", "test"}, 2)
	if len(plans) != 1 || plans[0].Kind != aimerge.PlanConflict {
		t.Fatalf("expected PlanConflict, got %+v", plans)
	}
	if len(plans[0].ConflictFiles) != 1 || plans[0].ConflictFiles[0] != "config.yaml" {
		t.Fatalf("expected conflict file config.yaml, got %v", plans[0].ConflictFiles)
	}

	// Simulate AI resolving the conflict cleanly on test branch
	_, _ = gitops.RunGit(ctx, repoDir, "switch", "test")
	_, _ = gitops.RunGit(ctx, repoDir, "merge", "--no-commit", "--no-ff", "feature/timeout-tune")
	_ = os.WriteFile(filepath.Join(repoDir, "config.yaml"), []byte("timeout: 250\nretries: 5\n"), 0o644)
	_, _ = gitops.RunGit(ctx, repoDir, "add", "config.yaml")
	_, _ = gitops.RunGit(ctx, repoDir, "commit", "--no-edit")

	report := aimerge.VerifyAIMerge(ctx, repoDir, "feature/timeout-tune", preTarget, sourceTip, mergeBase, []string{"config.yaml"})
	if !report.Passed {
		t.Fatalf("expected VerifyAIMerge to pass, failed with: %s", report.FailureReason)
	}
}

func TestChineseFilenamesAndMultiRepoBranchPlan(t *testing.T) {
	ctx := context.Background()
	wsDir := t.TempDir()

	repoA := filepath.Join(wsDir, "cloud-caring")
	repoB := filepath.Join(wsDir, "risk-cloud")
	repoC := filepath.Join(wsDir, "gateway")

	initTestRepo(t, repoA, "main", map[string]string{
		"文档/中文需求说明.md": "# 核心需求\n初始版本\n",
	})
	initTestRepo(t, repoB, "main", map[string]string{
		"风控规则/额度计算.go": "package risk\n",
	})
	initTestRepo(t, repoC, "main", map[string]string{
		"路由配置.yaml": "port: 8080\n",
	})

	repos, _ := mani.DiscoverRepos(wsDir, false)
	if len(repos) != 3 {
		t.Fatalf("expected 3 repos, got %d", len(repos))
	}

	// 1. Test Chinese filename modification, status inspection, diff, and workspace listing
	if err := gitops.SaveMicroEdit(repoA, "文档/中文需求说明.md", "# 核心需求\n第二版更新内容\n"); err != nil {
		t.Fatal(err)
	}
	if err := gitops.SaveMicroEdit(repoA, "文档/新增中文设计.md", "新增文档\n"); err != nil {
		t.Fatal(err)
	}

	st := gitops.InspectRepo(ctx, repos[0])
	if len(st.Files) != 2 {
		t.Fatalf("expected 2 changed Chinese files, got %+v", st.Files)
	}
	foundModified := false
	foundUntracked := false
	for _, cf := range st.Files {
		if cf.Path == "文档/中文需求说明.md" {
			foundModified = true
		}
		if cf.Path == "文档/新增中文设计.md" {
			foundUntracked = true
		}
	}
	if !foundModified || !foundUntracked {
		t.Fatalf("expected unescaped Chinese file paths, got %+v", st.Files)
	}

	oldTxt, newTxt, patch, err := gitops.GetFileDiff(ctx, repoA, "文档/中文需求说明.md", false)
	if err != nil || oldTxt == newTxt || patch == "" {
		t.Fatalf("expected valid diff for Chinese filename, err=%v", err)
	}

	files := gitops.ListWorkspaceFiles(ctx, repos, 4)
	foundListed := false
	for _, f := range files {
		if f.RelPath == "文档/中文需求说明.md" {
			foundListed = true
			break
		}
	}
	if !foundListed {
		t.Fatalf("expected ListWorkspaceFiles to return unescaped Chinese path, got %+v", files)
	}

	// Commit Chinese files cleanly so we can test SwitchBranchPlan
	if _, err := gitops.CommitRepo(ctx, repos[0].AbsPath, "feat: 更新中文文档", []string{"文档/中文需求说明.md", "文档/新增中文设计.md"}); err != nil {
		t.Fatalf("CommitRepo failed on Chinese files: %v", err)
	}

	// 2. Test SwitchBranchPlan with 3 distinct branches for 3 repos (AAA 需求)
	planTargets := []gitops.RepoBranchTarget{
		{Repo: repos[0], TargetBranch: "feat/aaa-" + repos[0].Name},
		{Repo: repos[1], TargetBranch: "feat/aaa-" + repos[1].Name},
		{Repo: repos[2], TargetBranch: "feat/aaa-" + repos[2].Name},
	}
	switchRes := gitops.SwitchBranchPlan(ctx, planTargets, true, 3)
	for _, r := range switchRes {
		if r.Status != "OK" {
			t.Fatalf("SwitchBranchPlan failed for %s: %s", r.RepoName, r.Message)
		}
	}

	afterStatuses := gitops.InspectRepos(ctx, repos, 3)
	for _, s := range afterStatuses {
		want := "feat/aaa-" + s.Repo.Name
		if s.Branch != want {
			t.Fatalf("repo %s expected branch %s, got %s", s.Repo.Name, want, s.Branch)
		}
		if len(s.BranchInfos) == 0 {
			t.Fatalf("expected BranchInfos populated for %s", s.Repo.Name)
		}
	}
}


