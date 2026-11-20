package main

import (
	"runtime"
	"strings"
	"testing"
	"time"

	"github.com/egoist/mygo/nemorouseditor"
	"github.com/egoist/mygo/ui"
	"github.com/iwangjie/nemorous/internal/gitops"
)

func roundFourFile(t *testing.T, app *App, path string) gitops.WorkspaceFile {
	t.Helper()
	for _, file := range app.workspaceFiles {
		if file.RelPath == path {
			return file
		}
	}
	t.Fatalf("missing fixture file %s", path)
	return gitops.WorkspaceFile{}
}

func TestRoundFourSearchEverywhereAndSwitcher(t *testing.T) {
	app := newReviewTestApp(t)
	tt := ui.NewTester(app.BuildUI, 1220, 780)
	tt.Frame()
	tap := func() {
		tt.CodeDown(ui.Shift, ui.CodeShiftLeft)
		tt.Advance(30 * time.Millisecond)
		tt.CodeUp(0, ui.CodeShiftLeft)
	}
	tap()
	if app.reviewPickerOpen {
		t.Fatal("one Shift tap should leave the current view alone")
	}
	tt.Advance(80 * time.Millisecond)
	tap()
	if !app.reviewPickerOpen || app.reviewPickerMode != reviewSearchEverywhere || app.searchOnlyFiles {
		t.Fatal("two Shift taps should open Search Everywhere on All")
	}
	tt.Type("a.txt")
	tt.Key(0, ui.KeyTab)
	if !app.searchOnlyFiles || app.reviewPickerQuery != "a.txt" {
		t.Fatal("Tab should switch to Files while preserving the query")
	}
	tt.Key(0, ui.KeyEnter)
	if tab := app.activeCodeTab(); app.reviewPickerOpen || tab == nil || tab.file.RelPath != "a.txt" {
		t.Fatal("Enter should open the matched file")
	}
	tt.CodeDown(ui.Shift, ui.CodeShiftLeft)
	tt.Key(ui.Shift, ui.KeyRight)
	tt.CodeUp(0, ui.CodeShiftLeft)
	tap()
	if app.reviewPickerOpen {
		t.Fatal("Shift used to select text should not count as a tap")
	}
	tt.Advance(time.Second)
	if err := tt.Click("随处搜索 (双击 Shift)"); err != nil || app.reviewPickerMode != reviewSearchEverywhere || !app.reviewPickerOpen {
		t.Fatalf("the magnifier should open the same search: %v", err)
	}
	tt.Key(0, ui.KeyEscape)
	app.openReviewLocation(reviewLocation{file: roundFourFile(t, app, "b.txt"), pos: editor.Pos{Line: -1}})
	tt.Frame()
	tt.HoldModifiers(ui.Ctrl)
	tt.Key(ui.Ctrl, ui.KeyTab)
	if app.reviewSwitcher == nil || !tt.HasText("Switcher") {
		t.Fatal("Ctrl+Tab should show Switcher while Ctrl is held")
	}
	tt.Key(ui.Ctrl, ui.KeyTab)
	if app.reviewSwitcher == nil || app.reviewSwitcher.index != 0 {
		t.Fatal("another Ctrl+Tab should cycle to the next item")
	}
	tt.Key(ui.Ctrl, ui.KeyTab)
	tt.HoldModifiers(0)
	if app.reviewSwitcher != nil || app.activeCodeTab().file.RelPath != "a.txt" {
		t.Fatal("releasing Ctrl should activate the selected recent file")
	}
	tt.CodeDown(ui.Ctrl, ui.CodeControlLeft)
	tt.CodeUp(ui.Ctrl, ui.CodeTab)
	if app.reviewSwitcher == nil {
		t.Fatal("Switcher must open when macOS delivers only Ctrl+Tab's release")
	}
	tt.CodeUp(0, ui.CodeControlLeft)
	if app.reviewSwitcher != nil || app.activeCodeTab().file.RelPath != "b.txt" {
		t.Fatal("releasing Control after Tab up should activate the selected file")
	}
}

func TestRoundFourFindResultsPersist(t *testing.T) {
	app := newReviewTestApp(t)
	tt := ui.NewTester(app.BuildUI, 1220, 780)
	tt.Frame()
	tt.Key(ui.Cmd|ui.Shift, ui.KeyF)
	tt.Type("changed")
	waitAppUI(t, app, "round four file matches", func() bool { return !app.globalSearchRunning && app.globalSearchTotal == 2 })
	tt.Frame()
	if err := tt.Click("在查找窗口中打开"); err != nil {
		t.Fatal(err)
	}
	if app.showGlobalSearch || !app.showFindPanel || app.findResults == nil || len(app.findResults.groups) != 2 {
		t.Fatal("Find in Files results should move to the docked Find window")
	}
	if err := tt.Click("changed"); err != nil {
		t.Fatal(err)
	}
	tt.Key(0, ui.KeyEnter)
	if tab := app.activeCodeTab(); tab == nil || tab.file.RelPath != "a.txt" || !app.showFindPanel || !tt.HasText("查找：changed") {
		t.Fatal("opening a result should leave the results available")
	}
}

func TestRoundFourSharedSplitAndProjectFollow(t *testing.T) {
	app := newReviewTestApp(t)
	aFile, bFile := roundFourFile(t, app, "a.txt"), roundFourFile(t, app, "b.txt")
	app.openReviewLocation(reviewLocation{file: aFile, pos: editor.Pos{}})
	tt := ui.NewTester(app.BuildUI, 1220, 780)
	tt.Frame()
	app.splitCodeEditorRight(app.codeTabIdx)
	tt.Frame()
	split := app.codeEditorSplit
	if split == nil || split.sharedView == nil || split.left.state.Buffer != split.sharedView.Buffer {
		t.Fatal("both editor groups should share one buffer for a file")
	}
	split.sharedHandle.Focus()
	tt.Frame()
	app.openReviewLocation(reviewLocation{file: bFile, pos: editor.Pos{}})
	tt.Frame()
	if split.left.file.RelPath != "a.txt" || split.right.file.RelPath != "b.txt" {
		t.Fatal("opening a different file in the right group should keep the left file")
	}
	app.unsplitCodeEditor()
	tt.Frame()
	if app.codeEditorSplit != nil || app.activeCodeTab().file.RelPath != "b.txt" || len(app.codeTabs) != 2 {
		t.Fatal("Unsplit All should retain the open tabs and active file")
	}
	if err := tt.Click("Project 选项"); err != nil {
		t.Fatal(err)
	}
	behavior, ok := tt.Find("行为")
	if !ok {
		t.Fatal("missing Project Behavior menu")
	}
	tt.Move(behavior.X+behavior.W/2, behavior.Y+behavior.H/2)
	if err := tt.Click("始终选中打开的文件"); err != nil {
		t.Fatal(err)
	}
	app.activateCodeTab(0)
	tt.Frame()
	if !app.cfg.ProjectFollowEditor || app.codeLocatedKey != aFile.RepoID+"::a.txt" {
		t.Fatal("Project should select the file opened in the editor")
	}
}

func TestRoundFourRevisionPathsAndWhitespaces(t *testing.T) {
	app := newReviewTestApp(t)
	wf := roundFourFile(t, app, "a.txt")
	app.openReviewLocation(reviewLocation{file: wf, pos: editor.Pos{}})
	app.showWorkingFileHistory(wf)
	tt := ui.NewTester(app.BuildUI, 1220, 780)
	tt.Frame()
	if app.gitSubTab != "file-history" || !tt.HasText("历史：a.txt") {
		t.Fatal("Show History should open the split Git file history")
	}
	if err := tt.Click("显示所有受影响的文件"); err != nil {
		t.Fatal(err)
	}
	if !tt.HasText("变更文件 a.txt") || !tt.HasText("变更文件 b.txt") {
		t.Fatal("affected paths should include both files in the historical commit")
	}
	if err := tt.Click("关闭受影响的文件"); err != nil {
		t.Fatal(err)
	}
	if err := tt.Click("差异设置"); err != nil {
		t.Fatal(err)
	}
	if err := tt.Click("显示空白字符"); err != nil {
		t.Fatal(err)
	}
	preview := app.gitRevisionHistory.preview.diff
	if !preview.ShowWhitespaces || preview.Ignore != editor.DiffDoNotIgnore {
		t.Fatal("visible whitespaces must keep the ignore policy independent")
	}
	app.activeCodeTab().state.Buffer.Set("a.txt\nunsaved current content\n")
	app.openFileRevisionPicker(wf)
	tt.Frame()
	tt.Key(0, ui.KeyEnter)
	if d := app.gitCommitDiff; d == nil || d.Old.String() != "a.txt\n" || d.New.String() != "a.txt\nunsaved current content\n" {
		t.Fatalf("Compare with Revision should compare against the current editor buffer, picker=%v error=%q diff=%v", app.fileRevisionPicker != nil, app.gitDiffErr, app.gitCommitDiff != nil)
	}
}

func TestRoundFourIdeaNextOccurrence(t *testing.T) {
	if runtime.GOOS != "darwin" {
		t.Skip("IDEA macOS keymap")
	}
	app := newReviewTestApp(t)
	app.openReviewLocation(reviewLocation{file: roundFourFile(t, app, "a.txt"), pos: editor.Pos{}})
	tab := app.activeCodeTab()
	tab.state.Buffer.Set("same same\n")
	tab.state.GoTo(editor.Pos{Col: 1})
	tt := ui.NewTester(app.BuildUI, 1220, 780)
	tt.Frame()
	tt.Key(ui.Ctrl, ui.KeyG)
	tt.Key(ui.Ctrl, ui.KeyG)
	if selections := tab.state.Selections(); len(selections) != 2 {
		t.Fatalf("Ctrl+G should add the next occurrence: %v", selections)
	}
	tt.Type("updated")
	if got := tab.state.Buffer.String(); strings.TrimSpace(got) != "updated updated" {
		t.Fatalf("editing the two selections produced %q", got)
	}
}
