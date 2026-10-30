package gitops_test

import (
	"context"
	"os"
	"path/filepath"
	"strings"
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

func TestPullAllRestoresLocalChanges(t *testing.T) {
	ctx := context.Background()
	root := t.TempDir()
	remote := filepath.Join(root, "remote.git")
	if _, err := gitops.RunGit(ctx, root, "init", "--bare", remote); err != nil {
		t.Fatal(err)
	}
	seed := filepath.Join(root, "seed")
	initTestRepo(t, seed, "main", map[string]string{"app.txt": "v1\n", "local.txt": "keep\n"})
	mustGit := func(dir string, args ...string) {
		t.Helper()
		if _, err := gitops.RunGit(ctx, dir, args...); err != nil {
			t.Fatalf("git %v in %s failed: %v", args, dir, err)
		}
	}
	mustGit(seed, "remote", "add", "origin", remote)
	mustGit(seed, "push", "-u", "origin", "main")

	work := filepath.Join(root, "work")
	mustGit(root, "clone", remote, work)
	if err := os.WriteFile(filepath.Join(work, "local.txt"), []byte("local edit\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(filepath.Join(seed, "app.txt"), []byte("remote v2\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	mustGit(seed, "commit", "-am", "remote v2")
	mustGit(seed, "push")

	res := gitops.PullAll(ctx, []mani.DiscoveredRepo{{Name: "work", AbsPath: work}}, 1)
	if len(res) != 1 || res[0].Status != "OK" {
		t.Fatalf("expected OK pull, got %+v", res)
	}
	if res[0].Message != "main 已快进更新" {
		t.Fatalf("expected concise pull message, got %q", res[0].Message)
	}
	content, err := os.ReadFile(filepath.Join(work, "local.txt"))
	if err != nil {
		t.Fatal(err)
	}
	if string(content) != "local edit\n" {
		t.Fatalf("local change was not restored, got %q", content)
	}
	head, _ := gitops.RunGit(ctx, work, "log", "-1", "--format=%s")
	if head != "remote v2" {
		t.Fatalf("pull did not fast-forward, head=%q", head)
	}
	stashes, _ := gitops.RunGit(ctx, work, "stash", "list")
	if strings.TrimSpace(stashes) != "" {
		t.Fatalf("autostash should be popped, got %q", stashes)
	}
}

func TestRepoLogGlobAndCommitVersions(t *testing.T) {
	ctx := context.Background()
	dir := t.TempDir()
	initTestRepo(t, dir, "main", map[string]string{"src/a.go": "v1\n", "b.py": "print(1)\n"})
	if err := os.WriteFile(filepath.Join(dir, "src", "a.go"), []byte("v2\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	if _, err := gitops.RunGit(ctx, dir, "commit", "-am", "update a"); err != nil {
		t.Fatal(err)
	}
	repo := gitops.RepoStatus{Repo: mani.DiscoveredRepo{ID: "demo", Name: "demo", AbsPath: dir}}
	commits, err := gitops.RepoLog(ctx, repo, gitops.LogOptions{Limit: 10})
	if err != nil || len(commits) < 2 {
		t.Fatalf("expected commit history, got %v / %v", commits, err)
	}
	oldText, newText, err := gitops.CommitFileVersions(ctx, dir, commits[0].Hash, "src/a.go", "", "M")
	if err != nil {
		t.Fatal(err)
	}
	if oldText != "v1\n" || newText != "v2\n" {
		t.Fatalf("unexpected commit versions: old=%q new=%q", oldText, newText)
	}

	globbed, err := gitops.RepoLog(ctx, repo, gitops.LogOptions{Path: "*.go", Limit: 10})
	if err != nil {
		t.Fatal(err)
	}
	if len(globbed) == 0 {
		t.Fatal("rg-style *.go glob should match nested src/a.go")
	}
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

	// Direct merge (feature/order-v2 -> main) in an isolated worktree.
	target := aimerge.InspectMergeTarget(ctx, repos[0], "main")
	if !target.TargetExists || target.AheadCommits != 1 {
		t.Fatalf("expected service-alpha with 1 commit to merge, got %+v", target)
	}
	if gamma := aimerge.InspectMergeTarget(ctx, repos[2], "main"); gamma.AheadCommits != 0 {
		t.Fatalf("expected service-gamma to have nothing to merge, got %+v", gamma)
	}
	outcome := aimerge.MergeIntoBranch(ctx, target, false)
	if outcome.Result.Status != "OK" {
		t.Fatalf("MergeIntoBranch failed: %s", outcome.Result.Message)
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

	// Switch back to feature/timeout-tune and merge directly into test.
	_, _ = gitops.RunGit(ctx, repoDir, "switch", "feature/timeout-tune")
	repos, _ := mani.DiscoverRepos(wsDir, false)
	target := aimerge.InspectMergeTarget(ctx, repos[0], "test")
	outcome := aimerge.MergeIntoBranch(ctx, target, false)
	if outcome.Result.Status != "CONFLICT" {
		t.Fatalf("expected a direct conflict, got %+v", outcome.Result)
	}
	if len(outcome.Conflicts) != 1 || outcome.Conflicts[0] != "config.yaml" {
		t.Fatalf("expected conflict file config.yaml, got %v", outcome.Conflicts)
	}
	_ = aimerge.CleanupManualMerge(ctx, repoDir, outcome.Worktree, outcome.Branch)

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
		mineSeen := false
		for _, bi := range s.BranchInfos {
			if !bi.IsRemote && bi.Mine {
				mineSeen = true
			}
		}
		if !mineSeen {
			t.Fatalf("expected branches committed with user.email to be marked Mine for %s", s.Repo.Name)
		}
	}
}

func TestAmendCommitAndStashRoundTrip(t *testing.T) {
	ctx := context.Background()
	repoDir := filepath.Join(t.TempDir(), "alpha")
	initTestRepo(t, repoDir, "main", map[string]string{"app.go": "package app\n"})
	repo := mani.DiscoveredRepo{ID: "alpha", Name: "alpha", AbsPath: repoDir}

	if err := os.WriteFile(filepath.Join(repoDir, "app.go"), []byte("package app\n// v2\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	res := gitops.BatchCommit(ctx, []gitops.BatchCommitTarget{{
		Repo:  repo,
		Files: []string{"app.go"},
		Amend: true,
	}}, "feat: v2", false, 1)
	if res[0].Status != "OK" {
		t.Fatalf("amend failed: %+v", res[0])
	}
	count, _ := gitops.RunGit(ctx, repoDir, "rev-list", "--count", "HEAD")
	if strings.TrimSpace(count) != "1" {
		t.Fatalf("amend should rewrite the only commit, got %s commits", strings.TrimSpace(count))
	}
	subject, _ := gitops.RunGit(ctx, repoDir, "log", "-1", "--pretty=%s")
	if strings.TrimSpace(subject) != "feat: v2" {
		t.Fatalf("expected amended subject, got %q", strings.TrimSpace(subject))
	}

	if err := os.WriteFile(filepath.Join(repoDir, "app.go"), []byte("package app\n// v3\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(filepath.Join(repoDir, "new.txt"), []byte("new\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	res = gitops.BatchStash(ctx, []mani.DiscoveredRepo{repo}, "test stash", 1)
	if res[0].Status != "OK" {
		t.Fatalf("stash failed: %+v", res[0])
	}
	if status, _ := gitops.RunGit(ctx, repoDir, "status", "--porcelain"); strings.TrimSpace(status) != "" {
		t.Fatalf("worktree should be clean after stash, got %q", status)
	}

	res = gitops.BatchStashPop(ctx, []mani.DiscoveredRepo{repo}, 1)
	if res[0].Status != "OK" {
		t.Fatalf("stash pop failed: %+v", res[0])
	}
	data, err := os.ReadFile(filepath.Join(repoDir, "new.txt"))
	if err != nil || string(data) != "new\n" {
		t.Fatalf("untracked file should return from stash, got %q err=%v", data, err)
	}
}

func TestBatchCommitUsesPerRepoMessages(t *testing.T) {
	ctx := context.Background()
	wsDir := t.TempDir()
	dirA := filepath.Join(wsDir, "alpha")
	dirB := filepath.Join(wsDir, "beta")
	initTestRepo(t, dirA, "main", map[string]string{"a.go": "package a\n"})
	initTestRepo(t, dirB, "main", map[string]string{"b.go": "package b\n"})
	if err := os.WriteFile(filepath.Join(dirA, "a.go"), []byte("package a\n// changed\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(filepath.Join(dirB, "b.go"), []byte("package b\n// changed\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	res := gitops.BatchCommit(ctx, []gitops.BatchCommitTarget{
		{Repo: mani.DiscoveredRepo{ID: "alpha", Name: "alpha", AbsPath: dirA}, Files: []string{"a.go"}, Message: "alpha only"},
		{Repo: mani.DiscoveredRepo{ID: "beta", Name: "beta", AbsPath: dirB}, Files: []string{"b.go"}},
	}, "shared message", false, 2)
	for _, r := range res {
		if r.Status != "OK" {
			t.Fatalf("commit failed: %+v", r)
		}
	}
	subjA, _ := gitops.RunGit(ctx, dirA, "log", "-1", "--pretty=%s")
	subjB, _ := gitops.RunGit(ctx, dirB, "log", "-1", "--pretty=%s")
	if strings.TrimSpace(subjA) != "alpha only" {
		t.Fatalf("expected per-repo message for alpha, got %q", subjA)
	}
	if strings.TrimSpace(subjB) != "shared message" {
		t.Fatalf("expected shared message for beta, got %q", subjB)
	}
}

func TestUndoLastCommitAndFileHistory(t *testing.T) {
	ctx := context.Background()
	repoDir := filepath.Join(t.TempDir(), "alpha")
	initTestRepo(t, repoDir, "main", map[string]string{"app.go": "package app\n// v1\n"})
	write := func(v string) {
		if err := os.WriteFile(filepath.Join(repoDir, "app.go"), []byte("package app\n// "+v+"\n"), 0o644); err != nil {
			t.Fatal(err)
		}
	}
	run := func(args ...string) {
		t.Helper()
		if _, err := gitops.RunGit(ctx, repoDir, args...); err != nil {
			t.Fatalf("git %v: %v", args, err)
		}
	}
	write("v2")
	run("commit", "-am", "feat: v2")
	write("v3")
	run("commit", "-am", "feat: v3")

	history := gitops.FileHistory(ctx, repoDir, "app.go", 5)
	if len(history) < 3 || history[0].Subject != "feat: v3" {
		t.Fatalf("expected newest-first file history, got %+v", history)
	}

	msg, err := gitops.UndoLastCommit(ctx, repoDir)
	if err != nil {
		t.Fatalf("undo failed: %v", err)
	}
	if !strings.Contains(msg, "撤销") {
		t.Fatalf("unexpected undo message %q", msg)
	}
	count, _ := gitops.RunGit(ctx, repoDir, "rev-list", "--count", "HEAD")
	if strings.TrimSpace(count) != "2" {
		t.Fatalf("expected two commits after undo, got %s", strings.TrimSpace(count))
	}
	status, _ := gitops.RunGit(ctx, repoDir, "status", "--porcelain")
	if !strings.Contains(status, "M  app.go") {
		t.Fatalf("undo should keep changes staged, got %q", status)
	}

	run("reset", "--hard")
	if _, err := gitops.UndoLastCommit(ctx, repoDir); err != nil {
		t.Fatalf("second undo should succeed: %v", err)
	}
	if _, err := gitops.UndoLastCommit(ctx, repoDir); err == nil {
		t.Fatal("undoing the root commit must fail")
	}
}

func TestMergeIntoBranchLeavesConflictWorktree(t *testing.T) {
	ctx := context.Background()
	repoDir := filepath.Join(t.TempDir(), "risk")
	initTestRepo(t, repoDir, "main", map[string]string{"risk.go": "package risk\n\nfunc Score(base int) int {\n\treturn base\n}\n"})
	run := func(args ...string) {
		t.Helper()
		if _, err := gitops.RunGit(ctx, repoDir, args...); err != nil {
			t.Fatalf("git %v: %v", args, err)
		}
	}
	run("checkout", "-b", "test")
	if err := os.WriteFile(filepath.Join(repoDir, "risk.go"), []byte("package risk\n\nfunc Score(base int) int {\n\treturn base + 10\n}\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	run("commit", "-am", "test: threshold")
	run("checkout", "main")
	run("checkout", "-b", "feat/login")
	if err := os.WriteFile(filepath.Join(repoDir, "risk.go"), []byte("package risk\n\nfunc Score(base int) int {\n\treturn base + 20\n}\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	run("commit", "-am", "feat: login")

	repo := mani.DiscoveredRepo{ID: "risk", Name: "risk", AbsPath: repoDir}
	target := aimerge.InspectMergeTarget(ctx, repo, "test")
	outcome := aimerge.MergeIntoBranch(ctx, target, false)
	if outcome.Result.Status != "CONFLICT" {
		t.Fatalf("expected a direct conflict, got %+v", outcome.Result)
	}
	worktree, conflicts := outcome.Worktree, outcome.Conflicts
	if len(conflicts) != 1 || conflicts[0] != "risk.go" {
		t.Fatalf("expected risk.go conflict, got %+v", conflicts)
	}
	data, err := os.ReadFile(filepath.Join(worktree, "risk.go"))
	if err != nil || !strings.Contains(string(data), "<<<<<<<") {
		t.Fatalf("worktree should contain conflict markers, got %q err=%v", data, err)
	}
	if err := os.WriteFile(filepath.Join(worktree, "risk.go"), []byte("package risk\n\nfunc Score(base int) int {\n\treturn base + 30\n}\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	if _, err := gitops.RunGit(ctx, worktree, "add", "risk.go"); err != nil {
		t.Fatal(err)
	}
	res, err := aimerge.FinishManualMerge(ctx, repoDir, worktree, outcome.Branch, "test", false)
	if err != nil || res.Status != "OK" {
		t.Fatalf("finishing manual merge failed: %v %+v", err, res)
	}
	if _, err := os.Stat(worktree); !os.IsNotExist(err) {
		t.Fatalf("worktree directory should be removed, stat err=%v", err)
	}
	merged, _ := gitops.RunGit(ctx, repoDir, "show", "test:risk.go")
	if !strings.Contains(merged, "base + 30") {
		t.Fatalf("target branch should contain the manual resolution, got %q", merged)
	}
}

func TestTrailingBlankLinesSurviveFileReads(t *testing.T) {
	ctx := context.Background()
	repoDir := filepath.Join(t.TempDir(), "alpha")
	content := "line one\n\n"
	initTestRepo(t, repoDir, "main", map[string]string{"README.md": content})

	current, head, err := gitops.ReadRepoFile(ctx, repoDir, "README.md")
	if err != nil {
		t.Fatal(err)
	}
	if current != content || head != content {
		t.Fatalf("trailing blank lines must survive a file read: current=%q head=%q", current, head)
	}

	oldText, newText, _, err := gitops.GetFileDiff(ctx, repoDir, "README.md", false)
	if err != nil {
		t.Fatal(err)
	}
	if oldText != newText {
		t.Fatalf("an unchanged file must diff equal: old=%q new=%q", oldText, newText)
	}
}
