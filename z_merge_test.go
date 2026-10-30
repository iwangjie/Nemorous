package main

import (
	"image/png"
	"os"
	"path/filepath"
	"testing"

	"github.com/egoist/mygo/ui"
	"github.com/iwangjie/nemorous/internal/aimerge"
	"github.com/iwangjie/nemorous/internal/config"
)

func TestMergeShots(t *testing.T) {
	t.Setenv("NEMOROUS_CONFIG", filepath.Join(t.TempDir(), "config.json"))
	ws := setupUITestWorkspace(t)
	cfg := config.Default()
	cfg.WorkspacePath = ws
	cfg.IncludeRootRepo = false
	cfg.Theme = "dark"
	app := NewApp(cfg)
	app.LoadWorkspaceSync(ws)
	app.mergeTargets = []aimerge.MergeTarget{
		{Repo: app.repos[0], SourceBranch: "feat/aaa-login", TargetBranch: "test", TargetRef: "refs/heads/test", TargetExists: true, AheadCommits: 2, CommitPreview: []string{"abc123 feat: login", "def456 fix: token"}},
		{Repo: app.repos[1], SourceBranch: "feat/aaa-login", TargetBranch: "test", TargetRef: "refs/heads/test", TargetExists: true, AheadCommits: 1, Dirty: true},
	}
	app.syncMergeSelection(app.mergeTargets)
	app.selectedMergeRepoID = app.repos[1].ID
	app.manualMerges[app.repos[1].ID] = ManualMerge{RepoPath: app.repos[1].AbsPath, Path: "/tmp/nemorous-manual-demo", Conflicts: []string{"main.go"}}
	app.activeTab = TabMerge
	app.mergeScanAt = app.lastRefresh
	tt := ui.NewTester(app.BuildUI, 1440, 900)
	tt.Frame()
	f, _ := os.Create("/tmp/merge-direct.png")
	_ = png.Encode(f, tt.Image())
	_ = f.Close()
}
