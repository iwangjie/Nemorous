package main

import (
	"os"
	"path/filepath"
	"strings"
	"testing"

	"github.com/egoist/mygo/nemorouseditor"
	"github.com/egoist/mygo/ui"
	"github.com/iwangjie/nemorous/internal/gitops"
)

func writeReviewFixture(t *testing.T, repo, path, value string) {
	t.Helper()
	if err := os.WriteFile(filepath.Join(repo, path), []byte(value), 0o644); err != nil {
		t.Fatal(err)
	}
}

func reviewFixtureFile(t *testing.T, app *App, name string) gitops.WorkspaceFile {
	t.Helper()
	for _, file := range app.workspaceFiles {
		if file.RelPath == name {
			return file
		}
	}
	t.Fatalf("missing project file %s", name)
	return gitops.WorkspaceFile{}
}

func TestReviewCompareProjectFilesAndWhitespace(t *testing.T) {
	repo := makeP0Repo(t, 1)
	old, next := "  a b\nkeep\nold\n", "ab\nkeep\nnew\n"
	writeReviewFixture(t, repo, "first.txt", old)
	writeReviewFixture(t, repo, "second.txt", next)
	app := newGapTestApp(t, repo)
	app.activeTab = TabCode
	app.codeRootCollapsed = false
	tt := ui.NewTester(app.BuildUI, 1440, 900)
	tt.Frame()
	if err := tt.Click("first.txt"); err != nil {
		t.Fatal(err)
	}
	if err := tt.ClickWith(ui.Cmd, "second.txt"); err != nil {
		t.Fatal(err)
	}
	tt.Key(ui.Cmd, ui.KeyD)
	tt.Frame()
	comparison := app.comparison
	if comparison == nil || comparison.title != "first.txt - second.txt" || len(comparison.diff.Hunks()) != 2 {
		t.Fatalf("two-file Diff did not open with both changes: %+v", comparison)
	}
	tt.Key(ui.Cmd|ui.Shift, ui.KeyD)
	tt.Frame()
	if !tt.HasText("忽略空白和空行") {
		t.Fatal("whitespace choices did not open from the Diff shortcut")
	}
	tt.Key(0, ui.KeyDown)
	tt.Key(0, ui.KeyDown)
	tt.Key(0, ui.KeyEnter)
	tt.Frame()
	if comparison.diff.Ignore != editor.DiffIgnoreWhitespace || len(comparison.diff.Hunks()) != 1 {
		t.Fatalf("whitespace policy did not filter the hunk: %v %+v", comparison.diff.Ignore, comparison.diff.Hunks())
	}
	if comparison.diff.Old.String() != old || comparison.diff.New.String() != next {
		t.Fatal("the whitespace policy rewrote a displayed file")
	}
}

func TestReviewRollbackSelectedBlockAndCompareClipboard(t *testing.T) {
	repo := makeP0Repo(t, 1)
	before := "old one\nold two\ncontext\nold three\n"
	after := "new one\nnew two\ncontext\nnew three\n"
	writeReviewFixture(t, repo, "f.txt", before)
	gitRun(t, repo, "add", "--", "f.txt")
	gitRun(t, repo, "commit", "-m", "baseline")
	writeReviewFixture(t, repo, "f.txt", after)
	app := newGapTestApp(t, repo)
	app.selectCodeFile(reviewFixtureFile(t, app, "f.txt"))
	tab := app.activeCodeTab()
	tt := ui.NewTester(app.BuildUI, 1440, 900)
	tt.Frame()
	tab.viewHandle.Focus()
	tab.state.Select(0, len("new one"))
	tt.Frame()
	tt.Key(ui.Cmd|ui.Alt, ui.KeyZ)
	tt.Frame()
	if got := tab.state.Buffer.String(); got != "old one\nold two\ncontext\nnew three\n" {
		t.Fatalf("Roll Back Lines changed the wrong block: %q", got)
	}
	tt.Key(ui.Cmd, ui.KeyZ)
	tt.Frame()
	if got := tab.state.Buffer.String(); got != after {
		t.Fatalf("undo did not restore the buffer: %q", got)
	}
	tab.state.Select(0, len("new one"))
	tt.SetClipboard("old one")
	tt.Frame()
	area, ok := tt.Find("Code editor")
	if !ok {
		t.Fatal("code editor not visible")
	}
	tt.RightClickAt(area.X+55, area.Y+9)
	if err := tt.ChooseMenuItem("与剪贴板比较"); err != nil {
		t.Fatal(err)
	}
	tt.Frame()
	if comp := app.comparison; comp == nil || comp.diff.Old.String() != "old one" || comp.diff.New.String() != "new one" {
		t.Fatalf("clipboard comparison did not preserve the exact two selections: %+v", comp)
	}
}

func TestReviewSearchMaskAndReplaceSingleMatch(t *testing.T) {
	repo := makeP0Repo(t, 1)
	writeReviewFixture(t, repo, "a.txt", "needle needle\r\n")
	writeReviewFixture(t, repo, "skip.txt", "needle\n")
	writeReviewFixture(t, repo, "a.md", "needle\n")
	app := newGapTestApp(t, repo)
	tt := ui.NewTester(app.BuildUI, 1440, 900)
	tt.Frame()
	tt.Key(ui.Cmd|ui.Shift, ui.KeyR)
	tt.Frame()
	if !app.globalSearchReplaceMode || !tt.HasText("在文件中替换") {
		t.Fatal("replace in files shortcut did not open the dialog")
	}
	if err := tt.Click("文件掩码"); err != nil {
		t.Fatal(err)
	}
	app.globalSearchMask = "*.txt, !skip*"
	app.globalSearchQuery = "needle"
	app.globalSearchReplacement = "other"
	app.scheduleGlobalSearch()
	waitAppUI(t, app, "masked matches", func() bool { return !app.globalSearchRunning && app.globalSearchTotal == 2 })
	tt.Frame()
	if len(app.globalSearchGroups) != 1 || app.globalSearchGroups[0].RelPath != "a.txt" {
		t.Fatalf("wrong files passed the mask: %+v", app.globalSearchGroups)
	}
	if err := tt.Click("替换"); err != nil {
		t.Fatal(err)
	}
	waitAppUI(t, app, "single replacement", func() bool { return !app.globalReplaceRunning })
	if got, _ := os.ReadFile(filepath.Join(repo, "a.txt")); string(got) != "other needle\r\n" {
		t.Fatalf("replace should change one hit and preserve CRLF: %q", got)
	}
	for _, path := range []string{"skip.txt", "a.md"} {
		if got, _ := os.ReadFile(filepath.Join(repo, path)); string(got) != "needle\n" {
			t.Fatalf("excluded file changed: %s %q", path, got)
		}
	}
	waitAppUI(t, app, "remaining masked match", func() bool { return !app.globalSearchRunning && app.globalSearchTotal == 1 })
	tt.Frame()
	tt.Key(ui.Shift|ui.Alt, ui.KeyEnter)
	waitAppUI(t, app, "replace-all confirmation", func() bool { return app.globalReplacePlan != nil })
	tt.Frame()
	if !tt.HasText("将 1 个文件中的 1 处") {
		t.Fatal("replace all did not present an exact count before writing")
	}
	if got, _ := os.ReadFile(filepath.Join(repo, "a.txt")); string(got) != "other needle\r\n" {
		t.Fatalf("replace all wrote before confirmation: %q", got)
	}
	tt.Key(0, ui.KeyEnter)
	waitAppUI(t, app, "confirmed replacement", func() bool { return !app.globalReplaceRunning && !app.showGlobalSearch })
	if got, _ := os.ReadFile(filepath.Join(repo, "a.txt")); string(got) != "other other\r\n" {
		t.Fatalf("confirmed replacement did not preserve CRLF: %q", got)
	}
}

func TestReviewGitLogDiffPreviewFollowsSelection(t *testing.T) {
	repo := makeP0Repo(t, 1)
	writeReviewFixture(t, repo, "a.txt", "first\n")
	writeReviewFixture(t, repo, "b.txt", "second\n")
	gitRun(t, repo, "add", ".")
	gitRun(t, repo, "commit", "-m", "two files")
	app := newGapTestApp(t, repo)
	entry := gitops.CommitEntry{Hash: gitRun(t, repo, "rev-parse", "HEAD"), RepoPath: repo, RepoName: "review"}
	files, err := gitops.CommitFiles(t.Context(), repo, entry.Hash)
	if err != nil {
		t.Fatal(err)
	}
	detail := gitops.CommitDetail{CommitEntry: entry, Files: files}
	app.cfg.GitLogDiffPreview = true
	tt := ui.NewTester(func(c *ui.Context) { app.buildGitChangedFiles(c, app.palette(), detail) }, 540, 300)
	tt.Frame()
	if err := tt.Click("变更文件 a.txt"); err != nil {
		t.Fatal(err)
	}
	tt.Frame()
	if app.gitLogPreview == nil || app.gitLogPreview.file.Path != "a.txt" || app.gitCommitDiff != nil || !tt.HasText("视图选项") {
		t.Fatal("Git Log file selection did not open the preview")
	}
	tt.Key(0, ui.KeyDown)
	tt.Frame()
	if app.gitDetailSelectedPath != "b.txt" || app.gitLogPreview.diff == nil || app.gitLogPreview.diff.New.String() != "second\n" {
		t.Fatal("Git Log keyboard selection did not advance the preview")
	}
	tt.Key(ui.Cmd, ui.KeyD)
	tt.Frame()
	if app.gitCommitDiff == nil || app.gitCommitDiff.New.String() != "second\n" || app.gitLogPreview.file.Path != "b.txt" {
		t.Fatal("opening a separate committed Diff replaced the docked preview")
	}
}

func TestReviewThreeWayAndLocalHistoryControls(t *testing.T) {
	repo := makeP0Repo(t, 1)
	writeReviewFixture(t, repo, "f.txt", "staged\nline\nvalue a\n")
	gitRun(t, repo, "add", "--", "f.txt")
	writeReviewFixture(t, repo, "f.txt", "staged\nlocal\nvalue a\n")
	app := newGapTestApp(t, repo)
	file := app.statuses[0].Files[0]
	app.openStagedReview(file)
	tt := ui.NewTester(app.BuildUI, 1440, 900)
	tt.Frame()
	view := app.stagedReview
	if view == nil || view.state == nil || !tt.HasText("暂存版本") || !tt.HasText("本地版本") {
		t.Fatal("three-way comparison did not display all versions")
	}
	index := view.state.Versions[1]
	if index.ReadOnly || !view.state.Versions[0].ReadOnly || !view.state.Versions[2].ReadOnly {
		t.Fatal("editability does not belong exclusively to staged version")
	}
	index.Buffer.Edit(editor.Edit{Start: 0, End: 6, Text: "index"})
	tt.Frame()
	if err := tt.Click("保存暂存版本 (⌘S)"); err != nil {
		t.Fatal(err)
	}
	waitAppUI(t, app, "staged edit", func() bool { return !view.saving && !view.dirty() })
	if got := gitRun(t, repo, "show", ":f.txt"); !strings.HasPrefix(got, "index\nline\n") {
		t.Fatalf("index was not saved independently: %q", got)
	}
	if got, _ := os.ReadFile(filepath.Join(repo, "f.txt")); string(got) != "staged\nlocal\nvalue a\n" {
		t.Fatalf("staged edit modified worktree file: %q", got)
	}

	index.Buffer.Edit(editor.Edit{Start: 0, End: 5, Text: "saved on close"})
	tt.Frame()
	tt.Key(ui.Cmd, ui.KeyW)
	waitAppUI(t, app, "save staged editor on close", func() bool { return app.stagedReview == nil })
	if got := gitRun(t, repo, "show", ":f.txt"); !strings.HasPrefix(got, "saved on close\n") {
		t.Fatalf("closing the staging comparison did not save the edited index: %q", got)
	}
	app.selectCodeFile(reviewFixtureFile(t, app, "f.txt"))
	tab := app.activeCodeTab()
	store := app.historyStore()
	if err := store.Record(file.RepoID, file.Path, "staged\nlocal\nprevious\n", "snapshot"); err != nil {
		t.Fatal(err)
	}
	app.openLocalHistory(file.RepoID, repo, file.Path)
	waitAppUI(t, app, "local history Diff", func() bool { return app.historyDiff != nil })
	tt.Frame()
	if err := tt.Click("恢复此变更块"); err != nil {
		t.Fatal(err)
	}
	if got := tab.state.Buffer.String(); got != "staged\nlocal\nprevious\n" {
		t.Fatalf("restoring from local history modified the wrong lines: %q", got)
	}
	if !tab.state.Buffer.Undo() || tab.state.Buffer.String() != "staged\nlocal\nvalue a\n" {
		t.Fatal("restoring one hunk did not preserve Undo")
	}
}

func TestReviewCommitCheckboxMapsBackToOriginalHunk(t *testing.T) {
	repo := makeP0Repo(t, 1)
	before := "space\nold one\ncontext\nold two\n"
	after := "  space\nnew one\ncontext\nnew two\n"
	writeReviewFixture(t, repo, "f.txt", before)
	gitRun(t, repo, "add", "--", "f.txt")
	gitRun(t, repo, "commit", "-m", "baseline")
	writeReviewFixture(t, repo, "f.txt", after)
	app := newGapTestApp(t, repo)
	file := app.statuses[0].Files[0]
	app.setChangeChecked(file.Key(), false)
	app.selectChangedFile(file)
	app.cfg.DiffIgnoreMode = uint8(editor.DiffIgnoreWhitespace)
	tt := ui.NewTester(app.BuildUI, 1440, 900)
	tt.Frame()
	if err := tt.Click("纳入提交"); err != nil {
		t.Fatal(err)
	}
	targets := app.commitTargets()
	if len(targets) != 1 || len(targets[0].Partial) != 1 {
		t.Fatalf("hunk selection did not produce a partial commit: %+v", targets)
	}
	part := targets[0].Partial[0]
	if len(part.Included) != 1 || part.Included[0] != (gitops.ChangeRange{OldStart: 0, OldEnd: 2, NewStart: 0, NewEnd: 2}) {
		t.Fatalf("ignored whitespace split the selected original commit hunk: %+v", part.Included)
	}
}
