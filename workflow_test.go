package main

import (
	"path/filepath"
	"strings"
	"testing"

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

func TestBuildCommitMessagesPerRepo(t *testing.T) {
	files := []gitops.ChangedFile{
		{RepoID: "alpha", RepoName: "alpha", Path: "src/a.go", Kind: "M", AddedLines: 2, DeletedLines: 1},
		{RepoID: "alpha", RepoName: "alpha", Path: "src/b.go", Kind: "A"},
		{RepoID: "beta", RepoName: "beta", Path: "README.md", Kind: "D", DeletedLines: 4},
	}
	set := buildCommitMessages(files)

	for _, want := range []string{"更新 alpha、beta", "2 个仓库", "3 个文件", "新增 1", "修改 1", "删除 1"} {
		if !strings.Contains(set.root, want) {
			t.Fatalf("root message missing %q:\n%s", want, set.root)
		}
	}
	alpha := set.byRepo["alpha"]
	if !strings.Contains(alpha, "更新 alpha 的 2 个文件") || !strings.Contains(alpha, "src/a.go") {
		t.Fatalf("alpha message should list its own files:\n%s", alpha)
	}
	if strings.Contains(alpha, "README.md") {
		t.Fatalf("alpha message must not mention another repository's files:\n%s", alpha)
	}
	if beta := set.byRepo["beta"]; beta != "更新 beta：README.md" {
		t.Fatalf("single-file repository message should name the file, got %q", beta)
	}
}

func TestMergeSelectionDefaultsAndPreservesChoices(t *testing.T) {
	app := NewApp(config.Default())
	app.statuses = []gitops.RepoStatus{
		{Repo: mani.DiscoveredRepo{ID: "alpha"}},
		{Repo: mani.DiscoveredRepo{ID: "beta"}},
		{Repo: mani.DiscoveredRepo{ID: "gamma"}},
	}
	app.mergeCheckedRepos["beta"] = false

	app.syncMergeSelection([]aimerge.RepoMergePlan{
		{Repo: mani.DiscoveredRepo{ID: "alpha"}, Kind: aimerge.PlanClean},
		{Repo: mani.DiscoveredRepo{ID: "beta"}, Kind: aimerge.PlanClean},
		{Repo: mani.DiscoveredRepo{ID: "gamma"}, Kind: aimerge.PlanSkipUpToDate},
	})
	if !app.mergePlanChecked("alpha") {
		t.Fatal("clean plans should be selected by default")
	}
	if app.mergePlanChecked("beta") {
		t.Fatal("an explicit deselection must survive a preflight")
	}
	if app.mergePlanChecked("gamma") {
		t.Fatal("plans without work should stay unselected")
	}

	// Stale repositories disappear from the selection after a new preflight.
	app.syncMergeSelection([]aimerge.RepoMergePlan{{Repo: mani.DiscoveredRepo{ID: "alpha"}, Kind: aimerge.PlanClean}})
	if _, ok := app.mergeCheckedRepos["beta"]; ok {
		t.Fatal("repositories missing from the preflight should be dropped")
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
		t.Fatalf("checked scope should stage only the checked file, got %+v", targets[0].Files)
	}

	app.commitScope = "staged"
	targets = app.commitTargets()
	if len(targets) != 1 || targets[0].Repo.ID != "alpha" {
		t.Fatalf("staged scope should include only repositories with staged files, got %+v", targets)
	}
	if len(targets[0].Files) != 0 {
		t.Fatalf("staged scope must commit the index as-is, got %+v", targets[0].Files)
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

func TestCommitAvailabilityByScope(t *testing.T) {
	app := NewApp(config.Default())
	app.commitScope = "checked"
	if ok, _ := app.commitAvailability(0, 0); ok {
		t.Fatal("checked scope with no files must be unavailable")
	}
	if ok, _ := app.commitAvailability(3, 0); !ok {
		t.Fatal("checked scope with files must be available")
	}
	app.commitScope = "staged"
	if ok, hint := app.commitAvailability(3, 0); ok || !strings.Contains(hint, "暂存") {
		t.Fatalf("staged scope without staged files must explain why, got %v %q", ok, hint)
	}
	if ok, _ := app.commitAvailability(3, 1); !ok {
		t.Fatal("staged scope with staged files must be available")
	}
	app.commitScope = "current"
	if ok, _ := app.commitAvailability(3, 1); ok {
		t.Fatal("current-repository scope needs a selected file")
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
