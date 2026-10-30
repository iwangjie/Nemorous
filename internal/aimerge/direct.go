package aimerge

import (
	"context"
	"fmt"
	"os"
	"strconv"
	"strings"
	"time"

	"github.com/iwangjie/nemorous/internal/gitops"
	"github.com/iwangjie/nemorous/internal/mani"
)

// MergeTarget is a live merge direction for one repository. It is computed from
// refs and status only, never from a simulated merge.
type MergeTarget struct {
	Repo          mani.DiscoveredRepo
	SourceBranch  string
	TargetBranch  string
	TargetRef     string
	TargetExists  bool
	Detached      bool
	AheadCommits  int
	BehindCommits int
	Dirty         bool
	CommitPreview []string
}

// MergeOutcome carries the result of a direct merge. Conflicts leave the
// temporary worktree in place so the user can resolve them by hand.
type MergeOutcome struct {
	Result    gitops.OpResult
	Worktree  string
	Branch    string
	Conflicts []string
}

// InspectMergeTarget resolves the target ref and counts what would be merged.
func InspectMergeTarget(ctx context.Context, repo mani.DiscoveredRepo, targetBranch string) MergeTarget {
	target := MergeTarget{Repo: repo, TargetBranch: targetBranch}
	current, err := gitops.RunGit(ctx, repo.AbsPath, "branch", "--show-current")
	if err != nil || strings.TrimSpace(current) == "" {
		target.Detached = true
		return target
	}
	target.SourceBranch = strings.TrimSpace(current)
	for _, ref := range []string{"refs/heads/" + targetBranch, "refs/remotes/origin/" + targetBranch} {
		if _, err := gitops.RunGit(ctx, repo.AbsPath, "show-ref", "--verify", "--quiet", ref); err == nil {
			target.TargetRef = ref
			target.TargetExists = true
			break
		}
	}
	if !target.TargetExists {
		return target
	}
	if out, err := gitops.RunGit(ctx, repo.AbsPath, "rev-list", "--count", target.TargetRef+"..HEAD"); err == nil {
		if n, convErr := strconv.Atoi(strings.TrimSpace(out)); convErr == nil {
			target.AheadCommits = n
		}
	}
	if out, err := gitops.RunGit(ctx, repo.AbsPath, "rev-list", "--count", "HEAD.."+target.TargetRef); err == nil {
		if n, convErr := strconv.Atoi(strings.TrimSpace(out)); convErr == nil {
			target.BehindCommits = n
		}
	}
	if out, err := gitops.RunGit(ctx, repo.AbsPath, "log", "--oneline", "--no-decorate", "-n", "20", target.TargetRef+"..HEAD"); err == nil {
		for _, line := range strings.Split(out, "\n") {
			if line = strings.TrimSpace(line); line != "" {
				target.CommitPreview = append(target.CommitPreview, line)
			}
		}
	}
	if out, err := gitops.RunGit(ctx, repo.AbsPath, "status", "--porcelain"); err == nil {
		target.Dirty = strings.TrimSpace(out) != ""
	}
	return target
}

// MergeIntoBranch merges source into target in an isolated worktree and updates
// the local target branch. Conflicts keep the worktree; nothing is ever forced
// or pushed without an explicit request.
func MergeIntoBranch(ctx context.Context, target MergeTarget, pushRemote bool) MergeOutcome {
	start := time.Now()
	result := gitops.OpResult{RepoID: target.Repo.ID, RepoName: target.Repo.Name}
	finish := func(status, message string, worktree string, conflicts []string, err error) MergeOutcome {
		result.Status = status
		result.Message = message
		result.Duration = time.Since(start)
		result.Err = err
		return MergeOutcome{Result: result, Worktree: worktree, Conflicts: conflicts}
	}
	switch {
	case target.Detached:
		return finish("SKIP", "detached HEAD，无法合并", "", nil, nil)
	case !target.TargetExists:
		return finish("FAIL", fmt.Sprintf("未找到目标分支 %s", target.TargetBranch), "", nil, nil)
	case target.SourceBranch == target.TargetBranch:
		return finish("SKIP", "已在目标分支", "", nil, nil)
	}

	tmpDir, err := os.MkdirTemp("", "nemorous-merge-"+target.Repo.Name+"-*")
	if err != nil {
		return finish("FAIL", err.Error(), "", nil, err)
	}
	_ = os.RemoveAll(tmpDir)
	tempBranch := fmt.Sprintf("nemorous-merge/%s-%d", sanitizeBranch(target.TargetBranch), time.Now().UnixNano())
	removeWorktree := func() {
		_, _ = gitops.RunGit(context.Background(), target.Repo.AbsPath, "worktree", "remove", "--force", tmpDir)
		_ = os.RemoveAll(tmpDir)
	}
	cleanup := func() {
		removeWorktree()
		_, _ = gitops.RunGit(context.Background(), target.Repo.AbsPath, "branch", "-D", tempBranch)
	}
	if _, err := gitops.RunGit(ctx, target.Repo.AbsPath, "worktree", "add", "-b", tempBranch, "--quiet", tmpDir, target.TargetRef); err != nil {
		_ = os.RemoveAll(tmpDir)
		return finish("FAIL", "创建临时 worktree 失败: "+err.Error(), "", nil, err)
	}

	mergeMsg := fmt.Sprintf("Merge branch '%s' into %s", target.SourceBranch, target.TargetBranch)
	if _, err := gitops.RunGit(ctx, tmpDir, "merge", "--no-edit", "-m", mergeMsg, target.SourceBranch); err != nil {
		out, _ := gitops.RunGit(ctx, tmpDir, "diff", "--name-only", "--diff-filter=U")
		var conflicts []string
		for _, line := range strings.Split(out, "\n") {
			if line = strings.TrimSpace(line); line != "" {
				conflicts = append(conflicts, line)
			}
		}
		if len(conflicts) == 0 {
			cleanup()
			return finish("FAIL", "合并失败: "+err.Error(), "", nil, err)
		}
		return MergeOutcome{
			Result:    gitops.OpResult{RepoID: target.Repo.ID, RepoName: target.Repo.Name, Status: "CONFLICT", Message: fmt.Sprintf("检测到 %d 个冲突，已保留临时工作区与分支 %s", len(conflicts), tempBranch), Duration: time.Since(start)},
			Worktree:  tmpDir,
			Branch:    tempBranch,
			Conflicts: conflicts,
		}
	}

	mergedSHA, err := gitops.RunGit(ctx, tmpDir, "rev-parse", "HEAD")
	if err != nil || strings.TrimSpace(mergedSHA) == "" {
		cleanup()
		return finish("FAIL", "无法读取合并结果", "", nil, err)
	}
	mergedSHA = strings.TrimSpace(mergedSHA)

	current, _ := gitops.RunGit(ctx, target.Repo.AbsPath, "branch", "--show-current")
	if strings.TrimSpace(current) == target.TargetBranch {
		cleanup()
		return finish("FAIL", fmt.Sprintf("目标分支 %s 当前被检出，请先切回开发分支", target.TargetBranch), "", nil, nil)
	}
	if _, err := gitops.RunGit(ctx, target.Repo.AbsPath, "branch", "-f", target.TargetBranch, mergedSHA); err != nil {
		cleanup()
		return finish("FAIL", "更新本地 "+target.TargetBranch+" 失败: "+err.Error(), "", nil, err)
	}

	if pushRemote {
		if _, err := gitops.RunGit(ctx, tmpDir, "push", "origin", "HEAD:refs/heads/"+target.TargetBranch, "--quiet"); err != nil {
			cleanup()
			return finish("FAIL", fmt.Sprintf("已在本地合并，但推送 origin/%s 失败: %v", target.TargetBranch, err), "", nil, err)
		}
	}
	cleanup()

	message := fmt.Sprintf("已合并 %s → %s（%d 个提交）", target.SourceBranch, target.TargetBranch, target.AheadCommits)
	if target.Dirty {
		message += "；未提交改动未包含"
	}
	if pushRemote {
		message += "，已推送"
	}
	return finish("OK", message, "", nil, nil)
}

// FinishManualMerge commits any remaining resolution in the temporary worktree,
// updates the local target branch and optionally pushes it.
func FinishManualMerge(ctx context.Context, repoPath, worktree, tempBranch, targetBranch string, pushRemote bool) (gitops.OpResult, error) {
	start := time.Now()
	result := gitops.OpResult{Status: "OK"}
	fail := func(msg string, err error) (gitops.OpResult, error) {
		result.Status, result.Message, result.Duration, result.Err = "FAIL", msg, time.Since(start), err
		return result, err
	}
	unmerged, _ := gitops.RunGit(ctx, worktree, "diff", "--name-only", "--diff-filter=U")
	if strings.TrimSpace(unmerged) != "" {
		return fail("仍有未解决的冲突："+strings.Join(strings.Fields(unmerged), "、"), nil)
	}
	if _, err := gitops.RunGit(ctx, worktree, "rev-parse", "--verify", "--quiet", "MERGE_HEAD"); err == nil {
		if _, err := gitops.RunGit(ctx, worktree, "add", "-A"); err != nil {
			return fail("暂存解决结果失败: "+err.Error(), err)
		}
		if _, err := gitops.RunGit(ctx, worktree, "commit", "--no-edit"); err != nil {
			return fail("完成合并提交失败: "+err.Error(), err)
		}
	}
	sha, err := gitops.RunGit(ctx, worktree, "rev-parse", "HEAD")
	if err != nil || strings.TrimSpace(sha) == "" {
		return fail("无法读取手动合并结果", err)
	}
	sha = strings.TrimSpace(sha)
	current, _ := gitops.RunGit(ctx, repoPath, "branch", "--show-current")
	if strings.TrimSpace(current) == targetBranch {
		return fail(fmt.Sprintf("目标分支 %s 当前被检出，请先切回开发分支", targetBranch), nil)
	}
	if _, err := gitops.RunGit(ctx, repoPath, "branch", "-f", targetBranch, sha); err != nil {
		return fail("更新本地 "+targetBranch+" 失败: "+err.Error(), err)
	}
	if pushRemote {
		if _, err := gitops.RunGit(ctx, repoPath, "push", "origin", sha+":refs/heads/"+targetBranch, "--quiet"); err != nil {
			return fail("已在本地完成合并，但推送失败: "+err.Error(), err)
		}
	}
	removeWorktree := func() {
		_, _ = gitops.RunGit(context.Background(), repoPath, "worktree", "remove", "--force", worktree)
		_ = os.RemoveAll(worktree)
		_, _ = gitops.RunGit(context.Background(), repoPath, "branch", "-D", tempBranch)
	}
	removeWorktree()
	result.Message = fmt.Sprintf("已完成 %s 的手动合并", targetBranch)
	if pushRemote {
		result.Message += "并推送"
	}
	result.Duration = time.Since(start)
	return result, nil
}

// CleanupManualMerge discards a temporary merge worktree and its branch.
func CleanupManualMerge(ctx context.Context, repoPath, worktree, branch string) error {
	_, err := gitops.RunGit(ctx, repoPath, "worktree", "remove", "--force", worktree)
	_ = os.RemoveAll(worktree)
	if branch != "" {
		_, _ = gitops.RunGit(context.Background(), repoPath, "branch", "-D", branch)
	}
	return err
}

func sanitizeBranch(name string) string {
	replacer := strings.NewReplacer("/", "-", " ", "-", ":", "-")
	return replacer.Replace(name)
}
