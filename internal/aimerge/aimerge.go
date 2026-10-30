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
	"strings"

	"github.com/iwangjie/nemorous/internal/gitops"
	"github.com/iwangjie/nemorous/internal/mani"
)

// RepoMergePlan describes one controlled merge for the AI conflict resolver.
type RepoMergePlan struct {
	Repo         mani.DiscoveredRepo
	SourceBranch string
	TargetBranch string
	TargetRef    string
}

// VerificationReport holds the machine verification checks after an AI merge resolution.
type VerificationReport struct {
	Passed         bool
	Checks         []VerifyCheck
	PreTargetSHA   string
	SourceTipSHA   string
	MergeCommitSHA string
	OrigBranch     string
	DiffStat       string
	FullPatch      string
	FailureReason  string
}

// VerifyCheck represents one machine safety assertion.
type VerifyCheck struct {
	Name   string
	Passed bool
	Detail string
}

var conflictMarkerRe = regexp.MustCompile(`^(<{7}|={7}|>{7}|\|{7})( |$)`)

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
