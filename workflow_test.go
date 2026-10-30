package main

import (
	"path/filepath"
	"strings"
	"testing"
	"time"

	"github.com/egoist/mygo"
	"github.com/iwangjie/nemorous/internal/aimerge"
	"github.com/iwangjie/nemorous/internal/config"
	"github.com/iwangjie/nemorous/internal/gitops"
	"github.com/iwangjie/nemorous/internal/mani"
)

func TestBranchTreeHierarchyRecentFirst(t *testing.T) {
	root := buildBranchTree([]gitops.BranchInfo{
		{Name: "main", CommitUnix: 100},
		{Name: "feature/aaa/login", CommitUnix: 300},
		{Name: "feature/aaa/cart", CommitUnix: 200},
		{Name: "feature/zzz", CommitUnix: 150},
		{Name: "legacy/old", CommitUnix: 50},
		{Name: "origin/main", CommitUnix: 999, IsRemote: true},
	})

	if got := branchNames(root); strings.Join(got, ",") != "feature,main,legacy" {
		t.Fatalf("expected groups ordered by newest commit, got %v", got)
	}
	feature := root.children[0]
	if got := branchNames(feature); strings.Join(got, ",") != "aaa,zzz" {
		t.Fatalf("expected feature subgroups by recency, got %v", got)
	}
	aaa := feature.children[0]
	if got := branchNames(aaa); strings.Join(got, ",") != "login,cart" {
		t.Fatalf("expected leaves by recency, got %v", got)
	}
	if !aaa.children[0].leaf || aaa.children[0].full != "feature/aaa/login" {
		t.Fatalf("expected leaf to keep its full branch name, got %+v", aaa.children[0])
	}
	for _, child := range root.children {
		if child.name == "origin" || child.full == "origin/main" {
			t.Fatal("remote-tracking branches must not appear in the switch menu")
		}
	}
}

func branchNames(n *branchNode) []string {
	names := make([]string, 0, len(n.children))
	for _, child := range n.children {
		names = append(names, child.name)
	}
	return names
}

func TestBranchPickerOrdersMineThenRecentThenRest(t *testing.T) {
	now := time.Now().Unix()
	day := int64(24 * time.Hour / time.Second)
	branches := []gitops.BranchInfo{
		{Name: "mine-old", CommitUnix: now - 30*day, Mine: true},
		{Name: "other-old", CommitUnix: now - 20*day},
		{Name: "other-recent", CommitUnix: now - 2*day},
		{Name: "mine-recent", CommitUnix: now - 3*day, Mine: true},
	}
	sortBranchesForPicker(branches)

	got := make([]string, 0, len(branches))
	for _, bi := range branches {
		got = append(got, bi.Name)
	}
	want := "mine-recent,other-recent,other-old,mine-old"
	if strings.Join(got, ",") != want {
		t.Fatalf("expected picker order %s, got %s", want, strings.Join(got, ","))
	}
}

func TestBranchTreePickerTierOrder(t *testing.T) {
	now := time.Now().Unix()
	day := int64(24 * time.Hour / time.Second)
	root := buildBranchTree([]gitops.BranchInfo{
		{Name: "team/old", CommitUnix: now - 30*day},
		{Name: "team/recent", CommitUnix: now - int64(time.Hour/time.Second)},
		{Name: "mine/recent", CommitUnix: now - 2*3600, Mine: true},
	})

	if got := branchNames(root); strings.Join(got, ",") != "mine,team" {
		t.Fatalf("expected my branch group first, got %v", got)
	}
	if !root.children[0].children[0].mine {
		t.Fatal("expected the branch authored by me to carry the me marker")
	}
	team := root.children[1]
	if got := branchNames(team); strings.Join(got, ",") != "recent,old" {
		t.Fatalf("expected recent teammate branch before stale one, got %v", got)
	}
}

func TestMergeSelectionDefaultsAndPreservesChoices(t *testing.T) {
	app := NewApp(config.Default())
	app.mergeCheckedRepos["beta"] = false

	targets := []aimerge.MergeTarget{
		{Repo: mani.DiscoveredRepo{ID: "alpha"}, TargetExists: true, SourceBranch: "feat", TargetBranch: "test", AheadCommits: 2},
		{Repo: mani.DiscoveredRepo{ID: "beta"}, TargetExists: true, SourceBranch: "feat", TargetBranch: "test", AheadCommits: 2},
		{Repo: mani.DiscoveredRepo{ID: "gamma"}, TargetExists: true, SourceBranch: "feat", TargetBranch: "test", AheadCommits: 0},
	}
	app.syncMergeSelection(targets)
	if !app.mergeRepoChecked("alpha") {
		t.Fatal("mergeable repositories should be selected by default")
	}
	if app.mergeRepoChecked("beta") {
		t.Fatal("an explicit deselection must survive a rescan")
	}
	if app.mergeRepoChecked("gamma") {
		t.Fatal("repositories with no new commits should stay unselected")
	}

	app.syncMergeSelection(targets[:1])
	if _, ok := app.mergeCheckedRepos["beta"]; ok {
		t.Fatal("repositories missing from the scan should be dropped")
	}
}

func TestCommitTargetsFollowScope(t *testing.T) {
	app := NewApp(config.Default())
	app.statuses = []gitops.RepoStatus{
		{
			Repo: mani.DiscoveredRepo{ID: "alpha", Name: "alpha"},
			Files: []gitops.ChangedFile{
				{RepoID: "alpha", RepoName: "alpha", Path: "a.go", Kind: "M", Staged: true},
				{RepoID: "alpha", RepoName: "alpha", Path: "b.go", Kind: "A"},
			},
		},
		{
			Repo:  mani.DiscoveredRepo{ID: "beta", Name: "beta"},
			Files: []gitops.ChangedFile{{RepoID: "beta", RepoName: "beta", Path: "c.go", Kind: "D"}},
		},
	}
	app.statusByID = map[string]gitops.RepoStatus{}
	for _, st := range app.statuses {
		app.statusByID[st.Repo.ID] = st
	}
	app.checkedChanges = map[string]bool{"alpha::a.go": true, "beta::c.go": true}

	targets := app.commitTargets()
	if len(targets) != 2 {
		t.Fatalf("expected one target per repository, got %+v", targets)
	}
	if len(targets[0].Files) != 1 || targets[0].Files[0] != "a.go" {
		t.Fatalf("checked files should be committed, got %+v", targets[0].Files)
	}
}

func TestMergePresetPersistence(t *testing.T) {
	t.Setenv("NEMOROUS_CONFIG", filepath.Join(t.TempDir(), "config.json"))
	cfg := config.Default()
	cfg.SaveMergePreset("合并到 test", "test", []string{"alpha", "beta", "alpha"})
	if len(cfg.MergePresets) != 1 {
		t.Fatalf("expected one merge preset, got %+v", cfg.MergePresets)
	}
	preset := cfg.MergePresets[0]
	if preset.TargetBranch != "test" || strings.Join(preset.RepoIDs, ",") != "alpha,beta" {
		t.Fatalf("merge preset should keep target and deduplicated repositories, got %+v", preset)
	}
	if err := cfg.Save(); err != nil {
		t.Fatal(err)
	}
	loaded := config.Load()
	if len(loaded.MergePresets) != 1 || loaded.MergePresets[0].Name != "合并到 test" {
		t.Fatalf("merge presets should persist, got %+v", loaded.MergePresets)
	}
	loaded.DeleteMergePreset("合并到 test")
	if len(loaded.MergePresets) != 0 {
		t.Fatalf("merge preset should be deletable, got %+v", loaded.MergePresets)
	}
}

func TestCommitAvailabilityRequiresCheckedFiles(t *testing.T) {
	app := NewApp(config.Default())
	if ok, hint := app.commitAvailability(0); ok || !strings.Contains(hint, "勾选") {
		t.Fatalf("no checked files must be unavailable with a hint, got %v %q", ok, hint)
	}
	if ok, _ := app.commitAvailability(3); !ok {
		t.Fatal("checked files must be committable")
	}
}

func TestShiftRangeSelection(t *testing.T) {
	app := NewApp(config.Default())
	ordered := []gitops.ChangedFile{
		{RepoID: "r", Path: "a.go"},
		{RepoID: "r", Path: "b.go"},
		{RepoID: "r", Path: "c.go"},
	}
	app.checkedChanges = map[string]bool{"r::a.go": true, "r::b.go": false, "r::c.go": false}
	app.lastCheckedKey = "r::a.go"

	app.toggleChangeRange(ordered, "r::c.go")
	for _, key := range []string{"r::a.go", "r::b.go", "r::c.go"} {
		if !app.checkedChanges[key] {
			t.Fatalf("shift range should check every file in between, %s is unchecked", key)
		}
	}

	app.toggleChangeRange(ordered, "r::a.go")
	for _, key := range []string{"r::a.go", "r::b.go", "r::c.go"} {
		if app.checkedChanges[key] {
			t.Fatalf("reverse shift range should uncheck every file, %s is checked", key)
		}
	}
}

func TestBranchCoverageCountsRepositories(t *testing.T) {
	app := NewApp(config.Default())
	app.statuses = []gitops.RepoStatus{
		{Repo: mani.DiscoveredRepo{ID: "a"}, BranchInfos: []gitops.BranchInfo{{Name: "feat/login"}, {Name: "main"}}},
		{Repo: mani.DiscoveredRepo{ID: "b"}, BranchInfos: []gitops.BranchInfo{{Name: "feat/login"}}},
		{Repo: mani.DiscoveredRepo{ID: "c"}, BranchInfos: []gitops.BranchInfo{{Name: "main"}}},
	}
	if got := app.branchCoverage("feat/login"); got != "2/3" {
		t.Fatalf("expected 2/3 coverage, got %q", got)
	}
	if got := app.branchCoverage("missing"); got != "0/3" {
		t.Fatalf("expected 0/3 coverage, got %q", got)
	}
}

func TestRenderedChangeFilesSkipsCollapsedDirs(t *testing.T) {
	app := NewApp(config.Default())
	tree := buildChangeTree([]gitops.ChangedFile{
		{RepoID: "r", Path: "src/main.go"},
		{RepoID: "r", Path: "README.md"},
	})
	app.collapsedChangeDirs["r::src"] = true
	visible := app.renderedChangeFiles("r", tree)
	if len(visible) != 1 || visible[0].Path != "README.md" {
		t.Fatalf("collapsed folders must not contribute to ranges or arrow navigation, got %+v", visible)
	}
	delete(app.collapsedChangeDirs, "r::src")
	if visible := app.renderedChangeFiles("r", tree); len(visible) != 2 {
		t.Fatalf("expanded folders should expose all files, got %+v", visible)
	}
}

func TestClassifyWatchHints(t *testing.T) {
	app := NewApp(config.Default())
	app.repos = []mani.DiscoveredRepo{
		{ID: "alpha", RelPath: "alpha"},
		{ID: "beta", RelPath: "beta"},
	}
	ids, listFiles, workspaceLevel := app.classifyWatchHints([]mygo.FileEvent{{Path: "alpha/src", Op: mygo.FileHint}})
	if workspaceLevel || !listFiles || len(ids) != 1 || !ids["alpha"] {
		t.Fatalf("source hint should refresh one repo and its files, got ids=%v files=%v ws=%v", ids, listFiles, workspaceLevel)
	}
	ids, listFiles, workspaceLevel = app.classifyWatchHints([]mygo.FileEvent{{Path: "beta/.git", Op: mygo.FileHint}})
	if workspaceLevel || listFiles || len(ids) != 1 || !ids["beta"] {
		t.Fatalf("git hint should refresh status only, got ids=%v files=%v ws=%v", ids, listFiles, workspaceLevel)
	}
	if _, _, workspaceLevel = app.classifyWatchHints([]mygo.FileEvent{{Path: ".", Op: mygo.FileHint}}); !workspaceLevel {
		t.Fatal("workspace root hints need a full rediscovery")
	}
	if _, _, workspaceLevel = app.classifyWatchHints([]mygo.FileEvent{{Path: "unknown", Op: mygo.FileHint}}); !workspaceLevel {
		t.Fatal("unknown paths need a full rediscovery")
	}
}
