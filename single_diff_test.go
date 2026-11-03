package main

import (
	"os"
	"path/filepath"
	"strings"
	"testing"
	"time"

	"github.com/iwangjie/nemorous/internal/config"
	"github.com/iwangjie/nemorous/internal/gitops"
)

func testDiffTabsRepo(t *testing.T) string {
	t.Helper()
	t.Setenv("NEMOROUS_CONFIG", filepath.Join(t.TempDir(), "config.json"))
	ws := t.TempDir()
	repo := filepath.Join(ws, "demo")
	if err := os.MkdirAll(repo, 0o755); err != nil {
		t.Fatal(err)
	}
	for _, args := range [][]string{
		{"init", "-b", "main"},
		{"config", "user.name", "Test"},
		{"config", "user.email", "test@example.com"},
	} {
		if _, err := gitops.RunGit(t.Context(), repo, args...); err != nil {
			t.Fatal(err)
		}
	}
	for _, name := range []string{"a.txt", "b.txt"} {
		if err := os.WriteFile(filepath.Join(repo, name), []byte(name+"\n"), 0o644); err != nil {
			t.Fatal(err)
		}
	}
	if _, err := gitops.RunGit(t.Context(), repo, "add", "-A"); err != nil {
		t.Fatal(err)
	}
	if _, err := gitops.RunGit(t.Context(), repo, "commit", "-m", "init"); err != nil {
		t.Fatal(err)
	}
	for _, name := range []string{"a.txt", "b.txt"} {
		if err := os.WriteFile(filepath.Join(repo, name), []byte(name+"\nchanged\n"), 0o644); err != nil {
			t.Fatal(err)
		}
	}
	return ws
}

func waitCommitDiff(t *testing.T, app *App) {
	t.Helper()
	deadline := time.Now().Add(5 * time.Second)
	for time.Now().Before(deadline) {
		if !app.gitDiffLoading {
			return
		}
		time.Sleep(10 * time.Millisecond)
	}
	t.Fatal("timed out waiting for commit diff")
}

func TestWorkingDiffIsSingleAndEditable(t *testing.T) {
	ws := testDiffTabsRepo(t)
	cfg := config.Default()
	cfg.WorkspacePath = ws
	cfg.IncludeRootRepo = false
	app := NewApp(cfg)
	app.LoadWorkspaceSync(ws)

	var aFile, bFile gitops.ChangedFile
	for _, st := range app.statuses {
		for _, f := range st.Files {
			switch f.Path {
			case "a.txt":
				aFile = f
			case "b.txt":
				bFile = f
			}
		}
	}
	if aFile.Path == "" || bFile.Path == "" {
		t.Fatal("expected both changed files")
	}
	app.selectChangedFile(aFile)
	if !app.diffEditable || app.diffState == nil || !app.diffState.Editable {
		t.Fatal("working-tree diff should be directly editable")
	}
	if !strings.Contains(app.diffState.New.String(), "a.txt") {
		t.Fatal("expected a.txt content")
	}
	app.diffState.New.Set(app.diffState.New.String() + "edited\n")
	app.selectChangedFile(bFile)
	if app.diffState == nil || !strings.Contains(app.diffState.New.String(), "b.txt") {
		t.Fatal("selecting another file should reuse the single diff view")
	}
	onDisk, err := os.ReadFile(filepath.Join(ws, "demo", "a.txt"))
	if err != nil {
		t.Fatal(err)
	}
	if !strings.Contains(string(onDisk), "edited") {
		t.Fatal("switching files should save the previous working-tree diff edit")
	}
	if app.selectedChangeKey != bFile.Key() {
		t.Fatalf("expected b.txt selected, got %q", app.selectedChangeKey)
	}
}

func TestStagedDiffIsReadOnly(t *testing.T) {
	ws := testDiffTabsRepo(t)
	repo := filepath.Join(ws, "demo")
	if _, err := gitops.RunGit(t.Context(), repo, "add", "a.txt"); err != nil {
		t.Fatal(err)
	}
	cfg := config.Default()
	cfg.WorkspacePath = ws
	cfg.IncludeRootRepo = false
	app := NewApp(cfg)
	app.LoadWorkspaceSync(ws)
	app.diffStagedOnly = true
	for _, st := range app.statuses {
		for _, f := range st.Files {
			if f.Path == "a.txt" {
				app.selectChangedFile(f)
			}
		}
	}
	if app.selectedChangeKey == "" {
		t.Fatal("expected a staged file")
	}
	if app.diffEditable || app.diffState == nil || app.diffState.Editable {
		t.Fatal("staged diff should be read-only")
	}
}

func TestCommitDiffIsSingle(t *testing.T) {
	ws := testDiffTabsRepo(t)
	repo := filepath.Join(ws, "demo")
	out, err := gitops.RunGit(t.Context(), repo, "rev-parse", "HEAD")
	if err != nil {
		t.Fatal(err)
	}
	hash := strings.TrimSpace(out)
	cfg := config.Default()
	cfg.WorkspacePath = ws
	cfg.IncludeRootRepo = false
	app := NewApp(cfg)
	app.LoadWorkspaceSync(ws)
	detail := gitops.CommitDetail{
		CommitEntry: gitops.CommitEntry{RepoName: "demo", RepoPath: repo, Hash: hash},
		Files:       []gitops.CommitFile{{Path: "a.txt", Status: "M"}, {Path: "b.txt", Status: "M"}},
	}
	app.TriggerGitFileDiff(detail, detail.Files[0])
	waitCommitDiff(t, app)
	if app.gitDiffPath != "a.txt" || app.gitDetailSelectedPath != "a.txt" {
		t.Fatalf("expected a.txt selected, got %q / %q", app.gitDiffPath, app.gitDetailSelectedPath)
	}
	app.TriggerGitFileDiff(detail, detail.Files[1])
	waitCommitDiff(t, app)
	if app.gitDiffPath != "b.txt" || app.gitDetailSelectedPath != "b.txt" {
		t.Fatalf("expected b.txt selected, got %q / %q", app.gitDiffPath, app.gitDetailSelectedPath)
	}
	if app.gitCommitDiff == nil || !strings.Contains(app.gitCommitDiff.New.String(), "b.txt") {
		t.Fatal("commit diff should show the selected file's committed content")
	}
}
