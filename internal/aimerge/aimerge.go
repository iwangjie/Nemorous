package aimerge

import (
	"bufio"
	"bytes"
	"context"
	"errors"
	"fmt"
	"os"
	"os/exec"
	"path/filepath"
	"regexp"
	"sort"
	"strconv"
	"strings"
	"sync"
	"time"

	"github.com/iwangjie/nemorous/internal/gitops"
	"github.com/iwangjie/nemorous/internal/mani"
)

// PlanKind represents the pre-flight merge classification for a repository.
type PlanKind string

const (
	PlanClean         PlanKind = "clean"          // Clean 3-way merge possible
	PlanConflict      PlanKind = "conflict"       // Merge will conflict; eligible for AI resolution
	PlanSkipProtected PlanKind = "skip_protected" // Currently on main/master/test/dev or target branch
	PlanSkipDirty     PlanKind = "skip_dirty"     // Working tree has uncommitted changes
	PlanSkipNoTarget  PlanKind = "skip_no_target" // Target branch does not exist in repo
	PlanSkipUpToDate  PlanKind = "skip_uptodate"  // Target already contains all commits from source
	PlanError         PlanKind = "error"
)

// RepoMergePlan holds the pre-flight merge analysis for one repository.
type RepoMergePlan struct {
	Repo          mani.DiscoveredRepo
	SourceBranch  string
	TargetBranch  string
	TargetRef     string
	AheadCommits  int
	Kind          PlanKind
	Summary       string
	ConflictFiles []string
	CommitPreview []string
}

// VerificationReport holds the machine verification checks after an AI merge resolution.
type VerificationReport struct {
	Passed          bool
	Checks          []VerifyCheck
	PreTargetSHA    string
	SourceTipSHA    string
	MergeCommitSHA  string
	OrigBranch      string
	DiffStat        string
	FullPatch       string
	FailureReason   string
}

// VerifyCheck represents one machine safety assertion.
type VerifyCheck struct {
	Name   string
	Passed bool
	Detail string
}

var conflictMarkerRe = regexp.MustCompile(`^(<{7}|={7}|>{7}|\|{7})( |$)`)

// PreflightMergeAll runs non-destructive git merge-tree pre-flight analysis across all repos in parallel.
func PreflightMergeAll(ctx context.Context, repos []mani.DiscoveredRepo, targetBranch string, fetchFirst bool, protectedBranches []string, concurrency int) []RepoMergePlan {
	if concurrency <= 0 {
		concurrency = 8
	}
	targetBranch = strings.TrimSpace(targetBranch)
	plans := make([]RepoMergePlan, len(repos))
	sem := make(chan struct{}, concurrency)
	var wg sync.WaitGroup

	protected := make(map[string]bool, len(protectedBranches)+1)
	for _, b := range protectedBranches {
		protected[strings.TrimSpace(b)] = true
	}
	protected[targetBranch] = true

	for i, repo := range repos {
		wg.Add(1)
		go func(idx int, r mani.DiscoveredRepo) {
			defer wg.Done()
			sem <- struct{}{}
			defer func() { <-sem }()
			plans[idx] = PreflightMergeRepo(ctx, r, targetBranch, fetchFirst, protected)
		}(i, repo)
	}
	wg.Wait()
	return plans
}

// PreflightMergeRepo analyzes whether the current branch of repo can be merged into targetBranch without touching the working tree.
func PreflightMergeRepo(ctx context.Context, repo mani.DiscoveredRepo, targetBranch string, fetchFirst bool, protected map[string]bool) RepoMergePlan {
	plan := RepoMergePlan{
		Repo:         repo,
		TargetBranch: targetBranch,
	}
	if targetBranch == "" {
		plan.Kind = PlanError
		plan.Summary = "未指定目标分支"
		return plan
	}

	cur, err := gitops.RunGit(ctx, repo.AbsPath, "branch", "--show-current")
	if err != nil || cur == "" {
		plan.Kind = PlanSkipProtected
		plan.Summary = "跳过 (detached HEAD)"
		return plan
	}
	plan.SourceBranch = cur

	if cur == targetBranch || protected[cur] {
		plan.Kind = PlanSkipProtected
		plan.Summary = fmt.Sprintf("当前位于 %s 分支，无需合并", cur)
		return plan
	}

	// Check working tree cleanliness (tracked modifications or staged changes)
	porcelain, _ := gitops.RunGit(ctx, repo.AbsPath, "status", "--porcelain", "-uno")
	if strings.TrimSpace(porcelain) != "" {
		plan.Kind = PlanSkipDirty
		plan.Summary = "有未提交的改动，请先提交或暂存"
		return plan
	}

	if fetchFirst {
		_, _ = gitops.RunGit(ctx, repo.AbsPath, "fetch", "origin", targetBranch, "--quiet")
	}

	// Locate targetRef (prefer origin/<targetBranch> if available, fallback to local <targetBranch>)
	targetRef := ""
	if _, err := gitops.RunGit(ctx, repo.AbsPath, "show-ref", "--verify", "--quiet", "refs/remotes/origin/"+targetBranch); err == nil {
		targetRef = "origin/" + targetBranch
	} else if _, err := gitops.RunGit(ctx, repo.AbsPath, "show-ref", "--verify", "--quiet", "refs/heads/"+targetBranch); err == nil {
		targetRef = targetBranch
	} else {
		plan.Kind = PlanSkipNoTarget
		plan.Summary = fmt.Sprintf("未找到 %s 分支", targetBranch)
		return plan
	}
	plan.TargetRef = targetRef

	// Count commits in HEAD not yet in targetRef
	cntStr, err := gitops.RunGit(ctx, repo.AbsPath, "rev-list", "--count", targetRef+"..HEAD")
	if err != nil {
		plan.Kind = PlanError
		plan.Summary = "无法计算提交差异: " + err.Error()
		return plan
	}
	ahead, _ := strconv.Atoi(strings.TrimSpace(cntStr))
	plan.AheadCommits = ahead
	if ahead == 0 {
		plan.Kind = PlanSkipUpToDate
		plan.Summary = fmt.Sprintf("%s 已包含当前分支全部提交", targetBranch)
		return plan
	}

	// Grab commit preview lines
	logOut, _ := gitops.RunGit(ctx, repo.AbsPath, "log", "--oneline", "-n", "5", targetRef+"..HEAD")
	if logOut != "" {
		plan.CommitPreview = strings.Split(logOut, "\n")
	}

	// Run non-destructive in-memory merge-tree
	mtOut, mtErr := gitops.RunGit(ctx, repo.AbsPath, "merge-tree", "--write-tree", targetRef, "HEAD")
	if mtErr == nil {
		plan.Kind = PlanClean
		plan.Summary = fmt.Sprintf("可直接合并（%d 个新提交）", ahead)
		return plan
	}

	// Parse conflicted files from merge-tree output
	conflicts := parseMergeTreeConflicts(mtOut)
	plan.Kind = PlanConflict
	plan.ConflictFiles = conflicts
	if len(conflicts) > 0 {
		plan.Summary = fmt.Sprintf("检测到冲突 %s → %s (%d 个冲突文件: %s)", cur, targetBranch, len(conflicts), strings.Join(conflicts, ", "))
	} else {
		plan.Summary = fmt.Sprintf("检测到冲突 %s → %s (可交由 AI 解决)", cur, targetBranch)
	}
	return plan
}

func parseMergeTreeConflicts(out string) []string {
	seen := make(map[string]bool)
	for _, line := range strings.Split(out, "\n") {
		line = strings.TrimSpace(line)
		if strings.HasPrefix(line, "CONFLICT (") {
			if idx := strings.Index(line, " in "); idx >= 0 {
				file := strings.TrimSpace(line[idx+4:])
				if file != "" {
					seen[file] = true
				}
			}
		}
	}
	var list []string
	for f := range seen {
		list = append(list, f)
	}
	sort.Strings(list)
	return list
}

// ExecuteCleanMerge safely merges sourceBranch into targetBranch using an isolated temporary git worktree
// (matching workspace/mani.yaml's safe worktree merge pattern so the user's current branch is untouched).
func ExecuteCleanMerge(ctx context.Context, plan RepoMergePlan, pushRemote bool) gitops.OpResult {
	start := time.Now()
	r := plan.Repo
	if plan.Kind != PlanClean {
		return gitops.OpResult{
			RepoID:   r.ID,
			RepoName: r.Name,
			Status:   "SKIP",
			Message:  plan.Summary,
			Duration: time.Since(start),
		}
	}

	tmpDir, err := os.MkdirTemp("", "nemorous-merge-"+r.Name+"-*")
	if err != nil {
		return gitops.OpResult{RepoID: r.ID, RepoName: r.Name, Status: "FAIL", Message: err.Error(), Err: err}
	}
	_ = os.RemoveAll(tmpDir) // git worktree add expects non-existent or empty target dir
	defer func() {
		_, _ = gitops.RunGit(context.Background(), r.AbsPath, "worktree", "remove", "--force", tmpDir)
		_ = os.RemoveAll(tmpDir)
	}()

	// Add worktree detached at targetRef
	if _, err := gitops.RunGit(ctx, r.AbsPath, "worktree", "add", "--detach", "--quiet", tmpDir, plan.TargetRef); err != nil {
		return gitops.OpResult{RepoID: r.ID, RepoName: r.Name, Status: "FAIL", Message: "创建临时 worktree 失败: " + err.Error(), Duration: time.Since(start), Err: err}
	}

	// Merge sourceBranch into the temporary worktree
	mergeMsg := fmt.Sprintf("Merge branch '%s' into %s", plan.SourceBranch, plan.TargetBranch)
	if _, err := gitops.RunGit(ctx, tmpDir, "merge", "--no-edit", "-m", mergeMsg, plan.SourceBranch); err != nil {
		_, _ = gitops.RunGit(ctx, tmpDir, "merge", "--abort")
		return gitops.OpResult{RepoID: r.ID, RepoName: r.Name, Status: "FAIL", Message: "合并失败: " + err.Error(), Duration: time.Since(start), Err: err}
	}

	// Update local targetBranch ref to the merged HEAD if targetBranch isn't currently checked out
	mergedSHA, err := gitops.RunGit(ctx, tmpDir, "rev-parse", "HEAD")
	if err == nil && mergedSHA != "" {
		_, _ = gitops.RunGit(ctx, r.AbsPath, "branch", "-f", plan.TargetBranch, mergedSHA)
	}

	if pushRemote {
		if _, err := gitops.RunGit(ctx, tmpDir, "push", "origin", "HEAD:refs/heads/"+plan.TargetBranch, "--quiet"); err != nil {
			return gitops.OpResult{
				RepoID:   r.ID,
				RepoName: r.Name,
				Status:   "FAIL",
				Message:  fmt.Sprintf("已在本地合入 %s，但推送远端失败: %v", plan.TargetBranch, err),
				Duration: time.Since(start),
				Err:      err,
			}
		}
		return gitops.OpResult{
			RepoID:   r.ID,
			RepoName: r.Name,
			Status:   "OK",
			Message:  fmt.Sprintf("已安全合并 %s → %s (%d commits) 并推送 origin/%s", plan.SourceBranch, plan.TargetBranch, plan.AheadCommits, plan.TargetBranch),
			Duration: time.Since(start),
		}
	}

	return gitops.OpResult{
		RepoID:   r.ID,
		RepoName: r.Name,
		Status:   "OK",
		Message:  fmt.Sprintf("已在本地安全合并 %s → %s (%d commits)", plan.SourceBranch, plan.TargetBranch, plan.AheadCommits),
		Duration: time.Since(start),
	}
}

// ResolveConflictWithAI performs a controlled merge of sourceBranch into targetBranch, invokes the AI Agent
// to resolve conflicts, streams progress via onLog, and runs strict machine verification (VerifyAIMerge).
// It NEVER auto-pushes: the human user always reviews the VerificationReport and diff before confirming push or rollback.
func ResolveConflictWithAI(
	ctx context.Context,
	workspaceDir string,
	plan RepoMergePlan,
	agentName string,
	agentBin string,
	onLog func(line string),
) (*VerificationReport, error) {
	repoPath := plan.Repo.AbsPath
	emit := func(format string, args ...any) {
		if onLog != nil {
			onLog(fmt.Sprintf(format, args...))
		}
	}

	if agentBin == "" {
		return nil, fmt.Errorf("未检测到可用的 AI 命令行工具 (%s)，请在设置中配置 omp / pi / claude / codex", agentName)
	}

	origBranch, err := gitops.RunGit(ctx, repoPath, "branch", "--show-current")
	if err != nil || origBranch == "" {
		return nil, errors.New("当前仓库处于 detached HEAD，无法安全启动 AI 合并")
	}

	sourceTip, err := gitops.RunGit(ctx, repoPath, "rev-parse", plan.SourceBranch)
	if err != nil {
		return nil, fmt.Errorf("无法解析源分支 %s: %w", plan.SourceBranch, err)
	}

	emit("### 1. 准备安全合并环境 (%s: `%s` → `%s`)", plan.Repo.Name, plan.SourceBranch, plan.TargetBranch)

	// Switch to targetBranch and fast-forward if origin/<targetBranch> exists
	if _, err := gitops.RunGit(ctx, repoPath, "show-ref", "--verify", "--quiet", "refs/heads/"+plan.TargetBranch); err == nil {
		if _, err := gitops.RunGit(ctx, repoPath, "checkout", plan.TargetBranch); err != nil {
			return nil, fmt.Errorf("切换到 %s 失败: %w", plan.TargetBranch, err)
		}
	} else {
		if _, err := gitops.RunGit(ctx, repoPath, "checkout", "-b", plan.TargetBranch, "--track", plan.TargetRef); err != nil {
			return nil, fmt.Errorf("检出目标分支 %s 失败: %w", plan.TargetBranch, err)
		}
	}

	if strings.HasPrefix(plan.TargetRef, "origin/") {
		_, _ = gitops.RunGit(ctx, repoPath, "merge", "--ff-only", plan.TargetRef)
	}

	preTarget, err := gitops.RunGit(ctx, repoPath, "rev-parse", "HEAD")
	if err != nil {
		return nil, err
	}
	mergeBase, err := gitops.RunGit(ctx, repoPath, "merge-base", preTarget, sourceTip)
	if err != nil {
		return nil, fmt.Errorf("无法计算 merge-base: %w", err)
	}

	// Start merge to produce conflict markers if not already clean
	emit("- 正在执行 `git merge --no-commit --no-ff %s` 生成冲突上下文...", plan.SourceBranch)
	_, mergeErr := gitops.RunGit(ctx, repoPath, "merge", "--no-commit", "--no-ff", plan.SourceBranch)
	if mergeErr == nil {
		// Turned out clean! Commit directly
		_, _ = gitops.RunGit(ctx, repoPath, "commit", "--no-edit")
		report := VerifyAIMerge(ctx, repoPath, origBranch, preTarget, sourceTip, mergeBase, nil)
		return &report, nil
	}

	// Collect unmerged conflict files
	unmergedOut, _ := gitops.RunGit(ctx, repoPath, "diff", "--name-only", "--diff-filter=U")
	var conflictFiles []string
	for _, f := range strings.Split(unmergedOut, "\n") {
		f = strings.TrimSpace(f)
		if f != "" {
			conflictFiles = append(conflictFiles, f)
		}
	}
	emit("- 检测到 **%d** 个冲突文件：`%s`", len(conflictFiles), strings.Join(conflictFiles, "`, `"))

	// Build prompt for AI Agent
	prompt := buildConflictPrompt(ctx, workspaceDir, plan.Repo.Name, repoPath, plan.SourceBranch, plan.TargetBranch, mergeBase, preTarget, sourceTip, conflictFiles)

	emit("### 2. 调用 AI 命令行工具 (`%s`) 自动解决冲突", agentName)
	var cmd *exec.Cmd
	switch agentName {
	case "omp":
		cmd = exec.CommandContext(ctx, agentBin, "-p", "--no-session", "--auto-approve", prompt)
	case "claude":
		cmd = exec.CommandContext(ctx, agentBin, "-p", prompt, "--allowedTools", "Bash,Read,Edit")
	case "codex":
		cmd = exec.CommandContext(ctx, agentBin, "exec", prompt)
	default: // "pi"
		cmd = exec.CommandContext(ctx, agentBin, "-p", "--no-session", prompt)
	}
	cmd.Dir = repoPath

	stdoutPipe, err := cmd.StdoutPipe()
	if err != nil {
		return nil, err
	}
	cmd.Stderr = cmd.Stdout

	if err := cmd.Start(); err != nil {
		return nil, fmt.Errorf("启动 AI Agent 失败: %w", err)
	}

	scanner := bufio.NewScanner(stdoutPipe)
	scanner.Buffer(make([]byte, 64*1024), 512*1024)
	for scanner.Scan() {
		emit("%s", scanner.Text())
	}
	_ = cmd.Wait()

	emit("### 3. 执行机器安全校验 (VerifyAIMerge)")
	report := VerifyAIMerge(ctx, repoPath, origBranch, preTarget, sourceTip, mergeBase, conflictFiles)
	for _, chk := range report.Checks {
		mark := "PASS"
		if !chk.Passed {
			mark = "FAIL"
		}
		emit("- **[%s]** %s: %s", mark, chk.Name, chk.Detail)
	}

	return &report, nil
}

// VerifyAIMerge runs all 6 strict post-AI safety checks (matching scripts/git-ai-merge.py).
func VerifyAIMerge(ctx context.Context, repoPath, origBranch, preTarget, sourceTip, mergeBase string, conflictFiles []string) VerificationReport {
	rep := VerificationReport{
		Passed:       true,
		OrigBranch:   origBranch,
		PreTargetSHA: preTarget,
		SourceTipSHA: sourceTip,
	}

	addCheck := func(name string, passed bool, detail string) {
		rep.Checks = append(rep.Checks, VerifyCheck{Name: name, Passed: passed, Detail: detail})
		if !passed && rep.Passed {
			rep.Passed = false
			rep.FailureReason = name + ": " + detail
		}
	}

	// Check 1: MERGE_HEAD must be cleared
	if _, err := gitops.RunGit(ctx, repoPath, "rev-parse", "-q", "--verify", "MERGE_HEAD"); err == nil {
		addCheck("合并提交完成度 (MERGE_HEAD)", false, "MERGE_HEAD 仍然存在，AI 尚未完成 git commit")
	} else {
		addCheck("合并提交完成度 (MERGE_HEAD)", true, "MERGE_HEAD 已清理，合并已提交")
	}

	// Check 2: Zero unmerged files
	unmerged, _ := gitops.RunGit(ctx, repoPath, "diff", "--name-only", "--diff-filter=U")
	if strings.TrimSpace(unmerged) != "" {
		addCheck("未决冲突文件检查", false, "仍存在未解决冲突文件: "+strings.ReplaceAll(unmerged, "\n", ", "))
	} else {
		addCheck("未决冲突文件检查", true, "0 个未决冲突文件")
	}

	// Check 3: Clean working tree
	porcelain, _ := gitops.RunGit(ctx, repoPath, "status", "--porcelain")
	if strings.TrimSpace(porcelain) != "" {
		addCheck("工作区干净度检查", false, "工作区存在未提交改动或残留文件")
	} else {
		addCheck("工作区干净度检查", true, "git status --porcelain 完全干净")
	}

	// Check 4: Merge commit parents check ([preTarget, sourceTip])
	parentsLine, err := gitops.RunGit(ctx, repoPath, "rev-list", "--parents", "-n", "1", "HEAD")
	if err == nil {
		fields := strings.Fields(parentsLine)
		if len(fields) > 0 {
			rep.MergeCommitSHA = fields[0]
		}
		if len(fields) == 3 && fields[1] == preTarget && fields[2] == sourceTip {
			addCheck("双亲提交拓扑校验", true, fmt.Sprintf("HEAD (%s) 双亲精确匹配 [%s, %s]", shortSHA(rep.MergeCommitSHA), shortSHA(preTarget), shortSHA(sourceTip)))
		} else {
			addCheck("双亲提交拓扑校验", false, fmt.Sprintf("HEAD 双亲不匹配: got %v, expected [%s, %s]", fields[1:], shortSHA(preTarget), shortSHA(sourceTip)))
		}
	} else {
		addCheck("双亲提交拓扑校验", false, err.Error())
	}

	// Check 5: Scope boundary check (changed files must be subset of sourcechanged ∪ targetchanged)
	srcFilesOut, _ := gitops.RunGit(ctx, repoPath, "diff", "--name-only", mergeBase+".."+sourceTip)
	tgtFilesOut, _ := gitops.RunGit(ctx, repoPath, "diff", "--name-only", mergeBase+".."+preTarget)
	allowed := make(map[string]bool)
	for _, f := range strings.Split(srcFilesOut+"\n"+tgtFilesOut, "\n") {
		f = strings.TrimSpace(f)
		if f != "" {
			allowed[f] = true
		}
	}
	mergedFilesOut, _ := gitops.RunGit(ctx, repoPath, "diff", "--name-only", preTarget+"..HEAD")
	var unexpected []string
	for _, f := range strings.Split(mergedFilesOut, "\n") {
		f = strings.TrimSpace(f)
		if f != "" && !allowed[f] {
			unexpected = append(unexpected, f)
		}
	}
	if len(unexpected) > 0 {
		addCheck("变更范围越界校验", false, "AI 修改了双边分支以外的文件: "+strings.Join(unexpected, ", "))
	} else {
		addCheck("变更范围越界校验", true, "所有修改文件均严格处于双边分支变更集内")
	}

	// Check 6: Conflict marker scan in resolved files
	var leftover []string
	for _, f := range conflictFiles {
		abs := filepath.Join(repoPath, f)
		data, err := os.ReadFile(abs)
		if err != nil {
			continue
		}
		for _, line := range bytes.Split(data, []byte("\n")) {
			if conflictMarkerRe.Match(line) {
				leftover = append(leftover, f)
				break
			}
		}
	}
	if len(leftover) > 0 {
		addCheck("冲突标记残留扫描", false, "文件仍包含 <<<<<<< 冲突标记: "+strings.Join(leftover, ", "))
	} else {
		addCheck("冲突标记残留扫描", true, "未发现任何冲突标记残留")
	}

	rep.DiffStat, _ = gitops.RunGit(ctx, repoPath, "show", "--stat", "--oneline", "HEAD")
	rep.FullPatch, _ = gitops.RunGit(ctx, repoPath, "diff", preTarget+"..HEAD")
	return rep
}

// ConfirmAndPushAIMerge pushes the verified merge commit to origin/<targetBranch> and switches back to origBranch.
func ConfirmAndPushAIMerge(ctx context.Context, repoPath, targetBranch, origBranch string, pushRemote bool) error {
	if pushRemote {
		if _, err := gitops.RunGit(ctx, repoPath, "push", "origin", targetBranch); err != nil {
			return fmt.Errorf("推送到 origin/%s 失败: %w", targetBranch, err)
		}
	}
	if origBranch != "" && origBranch != targetBranch {
		_, _ = gitops.RunGit(ctx, repoPath, "checkout", origBranch)
	}
	return nil
}

// RollbackAIMerge aborts any in-progress merge, resets targetBranch back to preTargetSHA, and switches back to origBranch.
func RollbackAIMerge(ctx context.Context, repoPath, targetBranch, origBranch, preTargetSHA string) error {
	_, _ = gitops.RunGit(ctx, repoPath, "merge", "--abort")
	if preTargetSHA != "" {
		_, _ = gitops.RunGit(ctx, repoPath, "checkout", targetBranch)
		_, _ = gitops.RunGit(ctx, repoPath, "reset", "--hard", preTargetSHA)
	}
	if origBranch != "" {
		_, _ = gitops.RunGit(ctx, repoPath, "checkout", origBranch)
	}
	return nil
}

func buildConflictPrompt(ctx context.Context, workspaceDir, repoName, repoPath, sourceBranch, targetBranch, mergeBase, preTarget, sourceTip string, conflictFiles []string) string {
	statusShort, _ := gitops.RunGit(ctx, repoPath, "status", "--short")
	sourceCommits, _ := gitops.RunGit(ctx, repoPath, "log", "--oneline", mergeBase+".."+sourceTip)
	targetCommits, _ := gitops.RunGit(ctx, repoPath, "log", "--oneline", mergeBase+".."+preTarget)

	var fileList strings.Builder
	for _, f := range conflictFiles {
		fileList.WriteString("- `" + f + "`\n")
	}

	// Use workspace template if present
	tplPath := filepath.Join(workspaceDir, "scripts", "prompts", "ai-merge-conflict.md")
	if data, err := os.ReadFile(tplPath); err == nil {
		r := strings.NewReplacer(
			"{repo_name}", repoName,
			"{repo_dir}", repoPath,
			"{source_branch}", sourceBranch,
			"{target_branch}", targetBranch,
			"{conflict_file_list}", strings.TrimSpace(fileList.String()),
			"{status_short}", statusShort,
			"{source_commits}", sourceCommits,
			"{target_commits}", targetCommits,
		)
		return r.Replace(string(data))
	}

	return fmt.Sprintf(`你正在处理 Git 合并冲突。
仓库：%s (%s)
合并方向：%s -> %s (当前工作目录在 %s 分支，已处于 merge 冲突状态)

冲突文件：
%s
git status --short:
%s

源分支 (%s) 提交：
%s

目标分支 (%s) 提交：
%s

规则：
1. 仅解决上述冲突文件中的冲突标记，结合两边代码意图合并，绝不盲目丢弃任一侧代码。
2. 严禁修改非冲突文件，严禁执行 git push / git reset / git checkout。
3. 解决完所有冲突后执行 git add <冲突文件> 并执行 git commit --no-edit 完成合并提交。
`, repoName, repoPath, sourceBranch, targetBranch, targetBranch, fileList.String(), statusShort, sourceBranch, sourceCommits, targetBranch, targetCommits)
}

func shortSHA(sha string) string {
	if len(sha) > 8 {
		return sha[:8]
	}
	return sha
}
