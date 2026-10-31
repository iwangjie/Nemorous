package gitops_test

import (
	"context"
	"os"
	"path/filepath"
	"strings"
	"testing"

	"github.com/iwangjie/nemorous/internal/gitops"
	"github.com/iwangjie/nemorous/internal/mani"
)

func readFile(t *testing.T, path string) string {
	t.Helper()
	data, err := os.ReadFile(path)
	if err != nil {
		t.Fatal(err)
	}
	return string(data)
}

func TestWorkspaceCloneInitImportAndMani(t *testing.T) {
	ctx := context.Background()
	ws := t.TempDir()
	dest, err := gitops.InitRepo(ctx, ws, "fresh")
	if err != nil {
		t.Fatal(err)
	}
	if _, err := os.Stat(filepath.Join(dest, ".git")); err != nil {
		t.Fatalf("init did not create .git: %v", err)
	}
	maniCfg, _, err := mani.LoadManiConfig(ws)
	if err != nil || maniCfg.Projects["fresh"].Path != "fresh" {
		t.Fatalf("mani project missing: %+v, %v", maniCfg, err)
	}

	src := filepath.Join(t.TempDir(), "source")
	initTestRepo(t, src, "main", map[string]string{"README.md": "hello\n"})
	bare := filepath.Join(t.TempDir(), "source.git")
	if _, err := gitops.RunGit(ctx, src, "clone", "--bare", src, bare); err != nil {
		t.Fatal(err)
	}
	cloned, err := gitops.CloneRepo(ctx, ws, bare, "cloned")
	if err != nil {
		t.Fatal(err)
	}
	if !strings.Contains(readFile(t, filepath.Join(cloned, "README.md")), "hello") {
		t.Fatal("clone content missing")
	}
	nested := filepath.Join(ws, "nested", "repo")
	initTestRepo(t, nested, "main", map[string]string{"a.txt": "a\n"})
	name, rel, err := gitops.ImportRepo(ctx, ws, nested)
	if err != nil {
		t.Fatal(err)
	}
	if name != "repo" || rel != "nested/repo" {
		t.Fatalf("import = %q %q", name, rel)
	}
	cfg, _, err := mani.LoadManiConfig(ws)
	if err != nil || cfg.Projects["repo"].Path != "nested/repo" {
		t.Fatalf("nested project missing: %+v", cfg)
	}
	repos, _ := mani.DiscoverRepos(ws, false)
	filtered := mani.FilterRepos(repos, []string{"cloned"})
	for _, repo := range filtered {
		if repo.ID == "cloned" {
			t.Fatal("hidden repo still discovered")
		}
	}
}

func TestStageHunksAndUnstage(t *testing.T) {
	ctx := context.Background()
	repo := t.TempDir()
	lines := strings.Join([]string{"l1", "l2", "l3", "l4", "l5", "l6", "l7", "l8", "l9", "l10"}, "\n") + "\n"
	initTestRepo(t, repo, "main", map[string]string{"f.txt": lines})
	changed := strings.Replace(lines, "l2", "L2", 1)
	changed = strings.Replace(changed, "l10", "L10", 1)
	if err := os.WriteFile(filepath.Join(repo, "f.txt"), []byte(changed), 0o644); err != nil {
		t.Fatal(err)
	}
	if err := gitops.StageHunks(ctx, repo, "f.txt", []int{0}); err != nil {
		t.Fatal(err)
	}
	cached, _ := gitops.RunGit(ctx, repo, "show", ":f.txt")
	if !strings.Contains(cached, "L2") || strings.Contains(cached, "L10") {
		t.Fatalf("stage hunk 0 cached=%q", cached)
	}
	work, _ := gitops.RunGit(ctx, repo, "show", "HEAD:f.txt")
	if strings.Contains(work, "L2") {
		t.Fatal("worktree unexpectedly changed")
	}
	if err := gitops.UnstageHunks(ctx, repo, "f.txt", []int{0}); err != nil {
		t.Fatal(err)
	}
	cached, _ = gitops.RunGit(ctx, repo, "show", ":f.txt")
	if strings.Contains(cached, "L2") {
		t.Fatal("unstage hunk 0 did not restore index")
	}
}

func TestStageSelectedAddedLines(t *testing.T) {
	ctx := context.Background()
	repo := t.TempDir()
	base := "a\nb\nc\nd\ne\n"
	initTestRepo(t, repo, "main", map[string]string{"f.txt": base})
	if err := os.WriteFile(filepath.Join(repo, "f.txt"), []byte(base+"new1\nnew2\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	// new1 is line 6, new2 is line 7; stage only new2.
	if err := gitops.StageLines(ctx, repo, "f.txt", false, 7, 7); err != nil {
		t.Fatal(err)
	}
	cached, _ := gitops.RunGit(ctx, repo, "show", ":f.txt")
	if !strings.Contains(cached, "new2") || strings.Contains(cached, "new1") {
		t.Fatalf("line staging cached=%q", cached)
	}
	work := readFile(t, filepath.Join(repo, "f.txt"))
	if !strings.Contains(work, "new1") || !strings.Contains(work, "new2") {
		t.Fatalf("worktree changed: %q", work)
	}
}

func TestStageSelectedDeletedLine(t *testing.T) {
	ctx := context.Background()
	repo := t.TempDir()
	base := "a\nb\nc\nd\ne\n"
	initTestRepo(t, repo, "main", map[string]string{"f.txt": base})
	if err := os.WriteFile(filepath.Join(repo, "f.txt"), []byte("a\nc\nd\ne\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	// Deleted old line 2 (b); stage that one deletion.
	if err := gitops.StageLines(ctx, repo, "f.txt", true, 2, 2); err != nil {
		t.Fatal(err)
	}
	cached, _ := gitops.RunGit(ctx, repo, "show", ":f.txt")
	if strings.Contains(cached, "\nb\n") {
		t.Fatalf("deleted line still staged: %q", cached)
	}
}

func TestInteractiveRebaseDropRewordAndSquash(t *testing.T) {
	ctx := context.Background()
	repo := t.TempDir()
	initTestRepo(t, repo, "main", map[string]string{"a.txt": "a\n"})
	if _, err := gitops.RunGit(ctx, repo, "commit", "--allow-empty", "-m", "A"); err != nil {
		t.Fatal(err)
	}
	if _, err := gitops.RunGit(ctx, repo, "commit", "--allow-empty", "-m", "B"); err != nil {
		t.Fatal(err)
	}
	if _, err := gitops.RunGit(ctx, repo, "commit", "--allow-empty", "-m", "C"); err != nil {
		t.Fatal(err)
	}
	commits, err := gitops.ListRebaseCommits(ctx, repo, "HEAD~3")
	if err != nil || len(commits) != 3 {
		t.Fatalf("commits=%v err=%v", commits, err)
	}
	if commits[0].Subject != "A" || commits[2].Subject != "C" {
		t.Fatalf("unexpected order: %+v", commits)
	}
	steps := []gitops.RebaseStep{
		{Hash: commits[0].Hash, Action: "pick"},
		{Hash: commits[1].Hash, Action: "reword", Message: "B reworded"},
		{Hash: commits[2].Hash, Action: "drop"},
	}
	if _, err := gitops.InteractiveRebase(ctx, repo, "HEAD~3", steps); err != nil {
		t.Fatal(err)
	}
	subjects, _ := gitops.RunGit(ctx, repo, "log", "--format=%s")
	if strings.Contains(subjects, "C") || !strings.Contains(subjects, "B reworded") {
		t.Fatalf("subjects=%q", subjects)
	}

	// Squash two commits in a fresh history.
	repo2 := t.TempDir()
	initTestRepo(t, repo2, "main", map[string]string{"a.txt": "a\n"})
	for _, msg := range []string{"X", "Y"} {
		if _, err := gitops.RunGit(ctx, repo2, "commit", "--allow-empty", "-m", msg); err != nil {
			t.Fatal(err)
		}
	}
	commits, err = gitops.ListRebaseCommits(ctx, repo2, "HEAD~2")
	if err != nil {
		t.Fatal(err)
	}
	if _, err := gitops.InteractiveRebase(ctx, repo2, "HEAD~2", []gitops.RebaseStep{{Hash: commits[0].Hash, Action: "pick"}, {Hash: commits[1].Hash, Action: "squash"}}); err != nil {
		t.Fatal(err)
	}
	count, _ := gitops.RunGit(ctx, repo2, "rev-list", "--count", "HEAD")
	if strings.TrimSpace(count) != "2" { // initial + squashed
		t.Fatalf("squash commit count=%q", count)
	}
	if _, err := gitops.InteractiveRebase(ctx, repo2, "HEAD~2", []gitops.RebaseStep{{Hash: commits[0].Hash, Action: "reword", Message: "x"}, {Hash: commits[1].Hash, Action: "squash"}}); err == nil {
		t.Fatal("expected reword+squash validation error")
	}
}

func TestReflogResetMergeAndCompare(t *testing.T) {
	ctx := context.Background()
	repo := t.TempDir()
	initTestRepo(t, repo, "main", map[string]string{"f.txt": "base\n"})
	if _, err := gitops.RunGit(ctx, repo, "checkout", "-b", "feature"); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(filepath.Join(repo, "f.txt"), []byte("feature\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	if _, err := gitops.RunGit(ctx, repo, "commit", "-am", "feature change"); err != nil {
		t.Fatal(err)
	}
	if _, err := gitops.RunGit(ctx, repo, "checkout", "main"); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(filepath.Join(repo, "main.txt"), []byte("main only\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	if _, err := gitops.RunGit(ctx, repo, "add", "main.txt"); err != nil {
		t.Fatal(err)
	}
	if _, err := gitops.RunGit(ctx, repo, "commit", "-m", "main only"); err != nil {
		t.Fatal(err)
	}
	if _, err := gitops.MergeBranch(ctx, repo, "feature", gitops.MergeNoFF, "merge feature"); err != nil {
		t.Fatal(err)
	}
	if out, _ := gitops.RunGit(ctx, repo, "log", "--format=%s"); !strings.Contains(out, "merge feature") {
		t.Fatalf("merge commit missing: %q", out)
	}
	entries, err := gitops.ListReflog(ctx, repo, 20)
	if err != nil || len(entries) == 0 || entries[0].Hash == "" {
		t.Fatalf("reflog=%v err=%v", entries, err)
	}
	diff, err := gitops.RefDiff(ctx, repo, "feature", "main", true)
	if err != nil || !strings.Contains(diff, "main only") {
		t.Fatalf("ref diff=%q err=%v", diff, err)
	}
	if _, err := gitops.ResetTo(ctx, repo, entries[0].Hash, "hard"); err != nil {
		t.Fatal(err)
	}
}

func TestWorktreeSparseSigningAndLFS(t *testing.T) {
	ctx := context.Background()
	repo := t.TempDir()
	initTestRepo(t, repo, "main", map[string]string{"src/a.txt": "a\n", "docs/b.txt": "b\n"})
	wt := filepath.Join(t.TempDir(), "wt")
	if _, err := gitops.AddWorktree(ctx, repo, wt, ""); err != nil {
		t.Fatal(err)
	}
	list, err := gitops.ListWorktrees(ctx, repo)
	if err != nil || len(list) < 2 {
		t.Fatalf("worktrees=%v err=%v", list, err)
	}
	if _, err := gitops.RemoveWorktree(ctx, repo, wt, false); err != nil {
		t.Fatal(err)
	}

	if _, err := gitops.SetSparse(ctx, repo, []string{"src/"}); err != nil {
		t.Fatal(err)
	}
	sparse := gitops.InspectSparse(ctx, repo)
	if !sparse.Enabled || len(sparse.Patterns) == 0 {
		t.Fatalf("sparse=%+v", sparse)
	}
	if _, err := gitops.DisableSparse(ctx, repo); err != nil {
		t.Fatal(err)
	}

	if _, err := gitops.SetSigning(ctx, repo, true, "ssh", "/tmp/key.pub"); err != nil {
		t.Fatal(err)
	}
	signing := gitops.InspectSigning(ctx, repo)
	if !signing.Enabled || signing.Format != "ssh" || signing.Key != "/tmp/key.pub" {
		t.Fatalf("signing=%+v", signing)
	}
	if _, err := gitops.SetSigning(ctx, repo, false, "ssh", ""); err != nil {
		t.Fatal(err)
	}

	lfs := gitops.InspectLFS(ctx, repo)
	if lfs.Installed {
		if _, err := gitops.LFSInstall(ctx, repo); err != nil {
			t.Fatal(err)
		}
		if _, err := gitops.LFSTrack(ctx, repo, "*.bin"); err != nil {
			t.Fatal(err)
		}
	}
}

func TestStageSelectedReplacementLine(t *testing.T) {
	ctx := context.Background()
	repo := t.TempDir()
	base := "a\nb\nc\nd\ne\n"
	initTestRepo(t, repo, "main", map[string]string{"f.txt": base})
	if err := os.WriteFile(filepath.Join(repo, "f.txt"), []byte("a\nB\nC\nd\ne\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	// Lines 2 and 3 both change; stage only the replacement on line 2.
	if err := gitops.StageLines(ctx, repo, "f.txt", false, 2, 2); err != nil {
		t.Fatal(err)
	}
	cached, _ := gitops.RunGit(ctx, repo, "show", ":f.txt")
	if !strings.Contains(cached, "\nB\n") || !strings.Contains(cached, "\nc\n") || strings.Contains(cached, "\nC\n") {
		t.Fatalf("partial replacement cached=%q", cached)
	}
	work := readFile(t, filepath.Join(repo, "f.txt"))
	if !strings.Contains(work, "B") || !strings.Contains(work, "C") {
		t.Fatalf("worktree changed: %q", work)
	}
}
