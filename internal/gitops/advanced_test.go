package gitops

import (
	"context"
	"os"
	"path/filepath"
	"strings"
	"testing"
)

func testRepo(t *testing.T, branch string) string {
	t.Helper()
	dir := t.TempDir()
	run := func(args ...string) string {
		t.Helper()
		out, err := RunGit(context.Background(), dir, args...)
		if err != nil {
			t.Fatalf("git %v: %v (%s)", args, err, out)
		}
		return out
	}
	run("init", "-b", branch)
	run("config", "user.name", "Test")
	run("config", "user.email", "test@example.com")
	if err := os.WriteFile(filepath.Join(dir, "file.txt"), []byte("one\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	run("add", ".")
	run("commit", "-m", "init")
	return dir
}

func writeFile(t *testing.T, path, content string) {
	t.Helper()
	if err := os.WriteFile(path, []byte(content), 0o644); err != nil {
		t.Fatal(err)
	}
}

func TestStashLifecycle(t *testing.T) {
	ctx := context.Background()
	repo := testRepo(t, "main")
	writeFile(t, filepath.Join(repo, "file.txt"), "two\n")
	msg, err := StashCreate(ctx, repo, "test stash", true)
	if err != nil || !strings.Contains(msg, "Saved") {
		t.Fatalf("stash create: %q %v", msg, err)
	}
	stashes, err := ListStashes(ctx, repo)
	if err != nil || len(stashes) != 1 || !strings.Contains(stashes[0].Message, "test stash") {
		t.Fatalf("stashes: %+v %v", stashes, err)
	}
	diff, err := StashDiff(ctx, repo, stashes[0].Ref)
	if err != nil || !strings.Contains(diff, "file.txt") {
		t.Fatalf("stash diff: %q %v", diff, err)
	}
	if err := StashApply(ctx, repo, stashes[0].Ref); err != nil {
		t.Fatal(err)
	}
	if data, _ := os.ReadFile(filepath.Join(repo, "file.txt")); string(data) != "two\n" {
		t.Fatalf("apply did not restore file: %q", data)
	}
	if err := StashDrop(ctx, repo, stashes[0].Ref); err != nil {
		t.Fatal(err)
	}
	if stashes, _ := ListStashes(ctx, repo); len(stashes) != 0 {
		t.Fatalf("stash not dropped: %+v", stashes)
	}
}

func TestRebaseCherryPickRevert(t *testing.T) {
	ctx := context.Background()
	repo := testRepo(t, "main")
	if _, err := RunGit(ctx, repo, "checkout", "-b", "feature"); err != nil {
		t.Fatal(err)
	}
	writeFile(t, filepath.Join(repo, "feature.txt"), "feature\n")
	if _, err := RunGit(ctx, repo, "add", "."); err != nil {
		t.Fatal(err)
	}
	if _, err := RunGit(ctx, repo, "commit", "-m", "feature"); err != nil {
		t.Fatal(err)
	}
	featureHash, _ := RunGit(ctx, repo, "rev-parse", "HEAD")
	if _, err := RunGit(ctx, repo, "checkout", "main"); err != nil {
		t.Fatal(err)
	}
	writeFile(t, filepath.Join(repo, "main.txt"), "main\n")
	if _, err := RunGit(ctx, repo, "add", "."); err != nil {
		t.Fatal(err)
	}
	if _, err := RunGit(ctx, repo, "commit", "-m", "main"); err != nil {
		t.Fatal(err)
	}
	// Rebase feature onto main.
	if _, err := RunGit(ctx, repo, "checkout", "feature"); err != nil {
		t.Fatal(err)
	}
	if _, err := Rebase(ctx, repo, "main"); err != nil {
		t.Fatal(err)
	}
	behind, _ := RunGit(ctx, repo, "rev-list", "--count", "main..feature")
	if behind != "1" {
		t.Fatalf("rebase did not move feature on top of main: %s", behind)
	}
	// Cherry-pick feature onto a fresh branch from main.
	if _, err := RunGit(ctx, repo, "checkout", "-b", "copy", "main"); err != nil {
		t.Fatal(err)
	}
	if _, err := CherryPick(ctx, repo, featureHash); err != nil {
		t.Fatal(err)
	}
	if data, _ := os.ReadFile(filepath.Join(repo, "feature.txt")); string(data) != "feature\n" {
		t.Fatalf("cherry-pick missing file: %q", data)
	}
	if _, err := Revert(ctx, repo, featureHash, 0); err != nil {
		t.Fatal(err)
	}
	if _, err := os.Stat(filepath.Join(repo, "feature.txt")); !os.IsNotExist(err) {
		t.Fatalf("revert did not remove file: %v", err)
	}
}

func TestTagsRemotesAndSubmodules(t *testing.T) {
	ctx := context.Background()
	repo := testRepo(t, "main")
	if err := CreateTag(ctx, repo, "v1", "release one", "HEAD", false); err != nil {
		t.Fatal(err)
	}
	if err := CreateTag(ctx, repo, "light", "", "HEAD", false); err != nil {
		t.Fatal(err)
	}
	tags, err := ListTags(ctx, repo)
	if err != nil || len(tags) != 2 {
		t.Fatalf("tags: %+v %v", tags, err)
	}
	if err := DeleteTag(ctx, repo, "light"); err != nil {
		t.Fatal(err)
	}
	if err := AddRemote(ctx, repo, "origin", "https://example.com/repo.git"); err != nil {
		t.Fatal(err)
	}
	if err := SetRemoteURL(ctx, repo, "origin", "https://example.com/other.git"); err != nil {
		t.Fatal(err)
	}
	remotes, err := ListRemotes(ctx, repo)
	if err != nil || len(remotes) != 1 || remotes[0].FetchURL != "https://example.com/other.git" {
		t.Fatalf("remotes: %+v %v", remotes, err)
	}
	if err := RemoveRemote(ctx, repo, "origin"); err != nil {
		t.Fatal(err)
	}

	child := testRepo(t, "main")
	parent := testRepo(t, "main")
	if _, err := AddSubmodule(ctx, parent, "file://"+filepath.ToSlash(child), "mods/child"); err != nil {
		t.Fatal(err)
	}
	mods, err := ListSubmodules(ctx, parent)
	if err != nil || len(mods) != 1 || mods[0].Path != "mods/child" || !mods[0].Initialized {
		t.Fatalf("submodules: %+v %v", mods, err)
	}
	if _, err := runGitWithEnv(ctx, parent, "commit", "-m", "submodule"); err != nil {
		t.Fatal(err)
	}
	if _, err := SyncSubmodules(ctx, parent); err != nil {
		t.Fatal(err)
	}
	if _, err := UpdateSubmodules(ctx, parent); err != nil {
		t.Fatal(err)
	}
	if err := RemoveSubmodule(ctx, parent, "mods/child"); err != nil {
		t.Fatal(err)
	}
	if mods, err := ListSubmodules(ctx, parent); err != nil || len(mods) != 0 {
		t.Fatalf("submodule not removed: %+v %v", mods, err)
	}
}

func runGitWithEnv(ctx context.Context, repo string, args ...string) (string, error) {
	return RunGit(ctx, repo, args...)
}

func TestConflictState(t *testing.T) {
	ctx := context.Background()
	repo := testRepo(t, "main")
	if _, err := RunGit(ctx, repo, "checkout", "-b", "feature"); err != nil {
		t.Fatal(err)
	}
	writeFile(t, filepath.Join(repo, "file.txt"), "feature\n")
	_, _ = RunGit(ctx, repo, "commit", "-am", "feature")
	if _, err := RunGit(ctx, repo, "checkout", "main"); err != nil {
		t.Fatal(err)
	}
	writeFile(t, filepath.Join(repo, "file.txt"), "main\n")
	_, _ = RunGit(ctx, repo, "commit", "-am", "main")
	if _, err := RunGit(ctx, repo, "checkout", "feature"); err != nil {
		t.Fatal(err)
	}
	if _, err := Rebase(ctx, repo, "main"); err == nil {
		t.Fatal("expected rebase conflict")
	}
	st := RepoOperationState(ctx, repo)
	if !st.Rebase || len(st.Conflicts) != 1 {
		t.Fatalf("state: %+v", st)
	}
	if err := AbortOperation(ctx, repo); err != nil {
		t.Fatal(err)
	}
	if st := RepoOperationState(ctx, repo); st.Busy() {
		t.Fatalf("operation still in progress: %+v", st)
	}
}
