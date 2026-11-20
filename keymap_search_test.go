package main

import (
	"context"
	"os"
	"path/filepath"
	"slices"
	"strings"
	"testing"
	"time"

	"github.com/egoist/mygo/nemorouseditor"
	"github.com/egoist/mygo/ui"
	"github.com/iwangjie/nemorous/internal/config"
	"github.com/iwangjie/nemorous/internal/gitops"
)

func TestKeymapRebindSwitcherAndPersist(t *testing.T) {
	t.Setenv("NEMOROUS_CONFIG", filepath.Join(t.TempDir(), "config.json"))
	app := newReviewTestApp(t)
	tt := ui.NewTester(app.BuildUI, 1040, 660)
	tt.Frame()
	tt.Key(ui.Cmd, ui.KeyComma)
	click := func(label string) {
		t.Helper()
		if err := tt.Click(label); err != nil {
			t.Fatal(err)
		}
	}
	click("配置快捷键…")
	click("添加键盘快捷键")
	tt.Key(ui.Alt, ui.KeyF12)
	click("确定")
	if app.keymap == nil {
		t.Fatal("adding a shortcut should return to the keymap")
	}
	key := stroke(ui.Alt, ui.KeyF12)
	if !slices.Contains(app.keymap.shortcuts(keySwitcher), key) {
		t.Fatalf("shortcut did not enter the draft: %v", app.keymap.shortcuts(keySwitcher))
	}
	click("确定")
	if app.keymap != nil || !slices.Contains(config.Load().KeyboardShortcuts[string(keySwitcher)], key) {
		t.Fatal("applying the keymap must persist it")
	}
	tt.Key(0, ui.KeyEscape)

	for _, file := range []string{"a.txt", "b.txt"} {
		app.openReviewLocation(reviewLocation{file: roundFourFile(t, app, file), pos: editor.Pos{Line: -1}})
		tt.Frame()
	}
	tt.HoldModifiers(ui.Alt)
	tt.Key(ui.Alt, ui.KeyF12)
	if app.reviewSwitcher == nil || app.reviewSwitcher.tool {
		t.Fatal("remapped shortcut should open the file side of Switcher")
	}
	tt.Key(ui.Alt, ui.KeyLeft)
	if app.reviewSwitcher == nil || !app.reviewSwitcher.tool {
		t.Fatal("the new modifier should navigate to the tool side")
	}
	tt.HoldModifiers(0)
	if app.reviewSwitcher != nil || app.activeTab != TabChanges {
		t.Fatal("releasing the new modifier should activate the Commit tool window")
	}

	tt.Key(ui.Cmd, ui.KeyComma)
	click("配置快捷键…")
	click("恢复默认快捷键")
	click("确定")
	if _, found := config.Load().KeyboardShortcuts[string(keySwitcher)]; found {
		t.Fatal("restoring the default must remove the override on disk")
	}
}

func TestSwitcherWithoutModifier(t *testing.T) {
	app := newReviewTestApp(t)
	app.cfg.KeyboardShortcuts = map[string][]config.KeyStroke{
		string(keySwitcher): {stroke(0, ui.KeyF6)},
	}
	tt := ui.NewTester(app.BuildUI, 1040, 660)
	tt.Frame()
	app.openReviewLocation(reviewLocation{file: roundFourFile(t, app, "a.txt"), pos: editor.Pos{Line: -1}})
	tt.Frame()
	tt.Key(0, ui.KeyF6)
	if app.reviewSwitcher == nil || app.reviewSwitcher.tool {
		t.Fatal("F6 should open and keep Switcher on the file side")
	}
	tt.Key(0, ui.KeyLeft)
	tt.Key(0, ui.KeyDown)
	if app.reviewSwitcher == nil || !app.reviewSwitcher.tool || app.reviewSwitcher.index != 1 {
		t.Fatal("the arrows should move to the second tool")
	}
	tt.Key(0, ui.KeyEnter)
	if app.reviewSwitcher != nil || app.activeTab != TabCode {
		t.Fatal("Enter should activate Project and close Switcher")
	}
}

func TestKeymapConflictsAndDisabledBinding(t *testing.T) {
	t.Setenv("NEMOROUS_CONFIG", filepath.Join(t.TempDir(), "config.json"))
	app := newReviewTestApp(t)
	app.openKeymapSettings()
	draft := app.keymap
	key := stroke(ui.Cmd, ui.Key1)
	conflicts := draft.conflicts(keyGitWindow, key)
	if len(conflicts) != 1 || conflicts[0].id != keyProject {
		t.Fatalf("expected conflict with Project: %+v", conflicts)
	}
	draft.add(keyGitWindow, key, conflicts)
	if !slices.Contains(draft.shortcuts(keyGitWindow), key) || len(draft.shortcuts(keyProject)) != 0 {
		t.Fatal("choosing to remove the conflict must move the binding")
	}
	if err := app.applyKeymap(draft.draft); err != nil {
		t.Fatal(err)
	}
	app.keymap = nil
	if keys, exists := config.Load().KeyboardShortcuts[string(keyProject)]; !exists || len(keys) != 0 {
		t.Fatal("an intentionally unbound action must stay unbound after reloading")
	}
	tt := ui.NewTester(app.BuildUI, 1040, 660)
	tt.Frame()
	tt.Key(ui.Cmd, ui.Key1)
	if !app.showGitPanel {
		t.Fatal("the reassigned key did not open Git")
	}
}

func TestSearchStartsWithEditorSelection(t *testing.T) {
	app := newReviewTestApp(t)
	file := roundFourFile(t, app, "a.txt")
	const contents = "target.txt\nfirst phrase\nsecond phrase\n"
	if err := os.WriteFile(file.AbsPath, []byte(contents), 0o644); err != nil {
		t.Fatal(err)
	}
	app.openReviewLocation(reviewLocation{file: file, pos: editor.Pos{}})
	tt := ui.NewTester(app.BuildUI, 1040, 660)
	tt.Frame()
	state := app.activeCodeEditor()
	state.Select(0, len("target.txt"))
	tt.Key(ui.Cmd|ui.Shift, ui.KeyO)
	if app.reviewPickerQuery != "target.txt" {
		t.Fatalf("Go to File query = %q", app.reviewPickerQuery)
	}
	tt.Type("a.txt")
	if app.reviewPickerQuery != "a.txt" {
		t.Fatalf("typing should replace the seeded query, got %q", app.reviewPickerQuery)
	}
	tt.Key(0, ui.KeyEscape)
	if app.reviewPickerOpen {
		t.Fatal("Escape did not close Go to File")
	}
	tt.Frame()
	state.Select(0, len("target.txt"))
	for range 2 {
		tt.CodeDown(ui.Shift, ui.CodeShiftLeft)
		tt.Advance(20 * time.Millisecond)
		tt.CodeUp(0, ui.CodeShiftLeft)
	}
	if !app.reviewPickerOpen || app.reviewPickerMode != reviewSearchEverywhere || app.reviewPickerQuery != "target.txt" {
		t.Fatalf("Search Everywhere open=%t mode=%d, query=%q, tracked=%+v, editor focused=%t selected=%q", app.reviewPickerOpen, app.reviewPickerMode, app.reviewPickerQuery, app.searchGesture, state.Focused(), state.SelectedText())
	}
	tt.Key(0, ui.KeyEscape)

	const selected = "first phrase\nsecond phrase"
	from := strings.Index(contents, selected)
	state.Select(from, from+len(selected))
	tt.Key(ui.Cmd|ui.Shift, ui.KeyO)
	if app.reviewPickerQuery != "" {
		t.Fatalf("file-name search should ignore a multiline selection, got %q", app.reviewPickerQuery)
	}
	tt.Key(0, ui.KeyEscape)
	state.Select(from, from+len(selected))
	tt.Key(ui.Cmd|ui.Shift, ui.KeyF)
	if app.globalSearchQuery != selected {
		t.Fatalf("project search query = %q, want %q", app.globalSearchQuery, selected)
	}
	waitAppUI(t, app, "multiline selection search", func() bool { return !app.globalSearchRunning })
	if app.globalSearchTotal != 1 || app.globalSearchGroups[0].Hits[0].Line != 2 {
		t.Fatalf("expected a multiline hit at line 2, got %d: %+v", app.globalSearchTotal, app.globalSearchGroups)
	}
	tt.Frame()
	tt.Key(0, ui.KeyEscape)
	state.Select(0, 0)
	tt.Key(ui.Cmd|ui.Shift, ui.KeyF)
	if app.globalSearchQuery != "" {
		t.Fatalf("a caret without a selection should not seed project search: %q", app.globalSearchQuery)
	}
}

func TestSearchStartsWithDiffSelection(t *testing.T) {
	app := newReviewTestApp(t)
	tt := ui.NewTester(app.BuildUI, 1440, 900)
	tt.Frame()
	toolbar, ok := tt.Find("修改 (F4)")
	if !ok {
		t.Fatal("missing Diff toolbar")
	}
	tt.ClickAt(545, toolbar.Y+toolbar.H+35)
	diff := app.diffState
	if diff.ActiveSide() != editor.DiffOld {
		t.Fatal("test must focus the old Diff side")
	}
	diff.Run(editor.SelectAll)
	want := diff.SelectedText()
	if want == "" {
		t.Fatal("test must select old-side text")
	}
	tt.Key(ui.Cmd|ui.Shift, ui.KeyF)
	if app.globalSearchQuery != want {
		t.Fatalf("search from the old Diff side used %q, want %q", app.globalSearchQuery, want)
	}
	tt.Key(0, ui.KeyEscape)
}

func TestMultilineContentSearchBackends(t *testing.T) {
	dir := t.TempDir()
	file := filepath.Join(dir, "mixed.txt")
	content := "same\r\n  one[]\r\ntwo\r\nsame\r\n  one[]\r\ntwo\r\n"
	if err := os.WriteFile(file, []byte(content), 0o644); err != nil {
		t.Fatal(err)
	}
	query := "one[]\ntwo"
	opts := globalSearchOptions{}
	check := func(out globalSearchOutcome, err error) {
		t.Helper()
		if err != nil || len(out.Hits) != 2 {
			t.Fatalf("wanted two matches, got %+v, %v", out, err)
		}
		for i, hit := range out.Hits {
			if hit.Line != 2+i*3 || hit.Column != 3 || hit.Snippet != "  one[]" {
				t.Fatalf("bad match span: %+v", hit)
			}
		}
	}
	check(goSearch(context.Background(), []gitops.WorkspaceFile{{AbsPath: file}}, query, opts))
	check(searchWorkspace(context.Background(), dir, []gitops.WorkspaceFile{{AbsPath: file}}, query, opts))
}
