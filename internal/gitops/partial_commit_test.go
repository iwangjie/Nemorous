package gitops

import (
	"path/filepath"
	"strings"
	"testing"

	"github.com/iwangjie/nemorous/internal/mani"
)

func TestPartialCommitPreservesOtherIndexEntriesAndWorktree(t *testing.T) {
	repo := testRepo(t, "main")
	ctx := t.Context()
	git := func(args ...string) string {
		t.Helper()
		result, err := RunGitRaw(ctx, repo, args...)
		if err != nil {
			t.Fatalf("git %v: %s %v", args, result, err)
		}
		return result
	}
	base, current := "unchanged\nold one\ncontext\nold two\n", "unchanged\nnew one\ncontext\nnew two\n"
	writeFile(t, filepath.Join(repo, "file.txt"), base)
	git("add", "file.txt")
	git("commit", "-m", "baseline")
	writeFile(t, filepath.Join(repo, "file.txt"), current)
	writeFile(t, filepath.Join(repo, "keep.txt"), "staged separately\n")
	git("add", "keep.txt")
	target := BatchCommitTarget{
		Repo: mani.DiscoveredRepo{AbsPath: repo},
		Partial: []PartialCommitFile{{
			File: ChangedFile{RepoPath: repo, Path: "file.txt", Unstaged: true},
			Old:  ReviewTextVersion(base), New: ReviewTextVersion(current),
			Included: []ChangeRange{{1, 2, 1, 2}},
		}},
	}
	if result := BatchCommit(ctx, []BatchCommitTarget{target}, "selected change", false, 1)[0]; result.Err != nil {
		t.Fatalf("partial commit failed: %v", result.Err)
	}
	if result := git("show", "HEAD:file.txt"); result != "unchanged\nnew one\ncontext\nold two\n" {
		t.Fatalf("wrong text in commit: %q", result)
	}
	if result := git("diff", "--cached", "--name-only"); result != "keep.txt\n" {
		t.Fatalf("staged entries were overwritten: %q", result)
	}
	if result := git("diff", "--", "file.txt"); !strings.Contains(result, "+new two") || strings.Contains(result, "+new one") {
		t.Fatalf("remaining worktree changes are incorrect: %q", result)
	}
	previous := git("rev-parse", "HEAD")
	result := BatchCommit(ctx, []BatchCommitTarget{target}, "stale selection", false, 1)[0]
	if result.Err == nil || git("rev-parse", "HEAD") != previous || git("show", ":keep.txt") != "staged separately\n" {
		t.Fatalf("stale selection changed repository state: %v", result.Err)
	}
}

func TestBuildSelectedFileByExactLines(t *testing.T) {
	base := "a\r\nold\r\nkeep\r\nlast"
	current := " a \r\nnew\r\nkeep\r\ninsert\r\nlast"
	got, err := buildSelectedFile(base, current, []ChangeRange{{1, 2, 1, 2}, {3, 3, 3, 4}})
	if err != nil || got != "a\r\nnew\r\nkeep\r\ninsert\r\nlast" {
		t.Fatalf("selected ranges did not preserve untouched bytes and line endings: %q %v", got, err)
	}
}
