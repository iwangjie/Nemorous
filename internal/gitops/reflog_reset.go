package gitops

import (
	"context"
	"fmt"
	"strings"
)

// ReflogEntry is one reflog record.
type ReflogEntry struct {
	Hash    string
	Ref     string
	Subject string
	Date    string
	Actor   string
}

// ListReflog returns the newest reflog entries.
func ListReflog(ctx context.Context, repoPath string, limit int) ([]ReflogEntry, error) {
	if limit <= 0 || limit > 500 {
		limit = 100
	}
	out, err := RunGit(ctx, repoPath, "reflog", "show", "--date=iso", "--format=%H%x1f%gd%x1f%gs%x1f%an%x1f%ad", "-n", fmt.Sprintf("%d", limit))
	if err != nil {
		return nil, err
	}
	var entries []ReflogEntry
	for _, line := range strings.Split(out, "\n") {
		if strings.TrimSpace(line) == "" {
			continue
		}
		parts := strings.SplitN(line, "\x1f", 5)
		if len(parts) != 5 {
			continue
		}
		entries = append(entries, ReflogEntry{Hash: parts[0], Ref: parts[1], Subject: parts[2], Actor: parts[3], Date: parts[4]})
	}
	return entries, nil
}

// ResetTo moves HEAD to ref using soft, mixed, hard or keep mode.
func ResetTo(ctx context.Context, repoPath, ref, mode string) (string, error) {
	ref = strings.TrimSpace(ref)
	if err := checkBranchOrRef(ref); err != nil {
		return "", err
	}
	switch mode {
	case "soft", "mixed", "hard", "keep":
	default:
		return "", fmt.Errorf("无效的 reset 模式：%s", mode)
	}
	return RunGit(ctx, repoPath, "reset", "--"+mode, ref)
}

// RefDiff compares two refs. Three-dot mode compares the tip of newRef with
// its merge base with oldRef, as a pull request's changes.
func RefDiff(ctx context.Context, repoPath, oldRef, newRef string, threeDot bool) (string, error) {
	if err := checkBranchOrRef(oldRef); err != nil {
		return "", err
	}
	if err := checkBranchOrRef(newRef); err != nil {
		return "", err
	}
	sep := ".."
	if threeDot {
		sep = "..."
	}
	return RunGitRaw(ctx, repoPath, "diff", "--no-color", "--no-ext-diff", oldRef+sep+newRef)
}

// MergeStrategy selects how a branch is merged.
type MergeStrategy string

const (
	MergeDefault MergeStrategy = "default"
	MergeNoFF    MergeStrategy = "no-ff"
	MergeFFOnly  MergeStrategy = "ff-only"
	MergeSquash  MergeStrategy = "squash"
)

// MergeBranch merges branch into the current branch with an explicit strategy.
func MergeBranch(ctx context.Context, repoPath, branch string, strategy MergeStrategy, message string) (string, error) {
	branch = strings.TrimSpace(branch)
	if err := checkBranchOrRef(branch); err != nil {
		return "", err
	}
	message = strings.TrimSpace(message)
	if strategy == "" {
		strategy = MergeDefault
	}
	switch strategy {
	case MergeFFOnly:
		return RunGit(ctx, repoPath, "merge", "--ff-only", branch)
	case MergeNoFF:
		if message == "" {
			message = "Merge branch '" + branch + "'"
		}
		return RunGit(ctx, repoPath, "merge", "--no-ff", "-m", message, branch)
	case MergeSquash:
		if staged, _ := RunGit(ctx, repoPath, "diff", "--cached", "--name-only"); strings.TrimSpace(staged) != "" {
			return "", fmt.Errorf("squash 合并需要干净的暂存区，避免把已有改动一并提交")
		}
		out, err := RunGit(ctx, repoPath, "merge", "--squash", branch)
		if err != nil {
			return out, err
		}
		if strings.Contains(out, "Already up to date") {
			return out, nil
		}
		if msgOut, msgErr := RunGit(ctx, repoPath, "commit", "-m", "Squash merge branch '"+branch+"'"); msgErr != nil {
			return out + "\n" + msgOut, msgErr
		}
		return out, nil
	case MergeDefault:
		return RunGit(ctx, repoPath, "merge", branch)
	default:
		return "", fmt.Errorf("无效的合并策略：%s", strategy)
	}
}

// ForcePushWithLease force-pushes the current branch only if the remote is
// still at the last fetched commit.
func ForcePushWithLease(ctx context.Context, repoPath string) (string, error) {
	branch, err := RunGit(ctx, repoPath, "branch", "--show-current")
	if err != nil || strings.TrimSpace(branch) == "" {
		return "", fmt.Errorf("detached HEAD 无法推送")
	}
	return RunGit(ctx, repoPath, "push", "--force-with-lease", "origin", branch)
}
