package main

import (
	"errors"
	"fmt"
	"os"
	"path/filepath"
	"runtime"
	"strings"
	"testing"
	"time"

	"github.com/egoist/mygo/nemorouseditor"
	"github.com/egoist/mygo/ui"
	"github.com/iwangjie/nemorous/internal/config"
	"github.com/iwangjie/nemorous/internal/gitops"
)

func newReviewTestApp(t *testing.T) *App {
	t.Helper()
	ws := testDiffTabsRepo(t)
	cfg := config.Default()
	cfg.WorkspacePath = ws
	cfg.IncludeRootRepo = false
	app := NewApp(cfg)
	app.LoadWorkspaceSync(ws)
	return app
}

func choosePathMenuItem(t *testing.T, tt *ui.Tester, target, label, want string) {
	t.Helper()
	if err := tt.RightClick(target); err != nil {
		t.Fatal(err)
	}
	menu, ok := tt.Find(copyPathMenu)
	if !ok {
		t.Fatalf("%s menu missing for %s", copyPathMenu, target)
	}
	tt.Move(menu.X+menu.W/2, menu.Y+menu.H/2)
	if err := tt.Click(label); err != nil {
		t.Fatal(err)
	}
	tt.Frame()
	if got := tt.Clipboard(); got != want {
		t.Fatalf("%s > %s copied %q, want %q", target, label, got, want)
	}
}

func TestReviewPathMenuAndSourceReturn(t *testing.T) {
	app := newReviewTestApp(t)
	tt := ui.NewTester(app.BuildUI, 1440, 900)
	tt.Frame()
	repo := app.selectedChangeFile.RepoPath
	choosePathMenuItem(t, tt, "Diff: a.txt", "文件名", "a.txt")
	choosePathMenuItem(t, tt, "Diff: a.txt", "所在目录", repo)
	choosePathMenuItem(t, tt, "Diff: a.txt", "相对工作区路径", "demo/a.txt")
	if err := tt.RightClick("Diff: a.txt"); err != nil {
		t.Fatal(err)
	}
	if err := tt.Click("在文件树中定位"); err != nil {
		t.Fatal(err)
	}
	if app.activeTab != TabCode || app.editorTab != "changes" || app.codeLocatedKey != app.selectedChangeKey {
		t.Fatal("locating the file must keep the diff active and show the explorer row")
	}
	choosePathMenuItem(t, tt, "demo", "目录名", "demo")
	choosePathMenuItem(t, tt, "b.txt", "绝对路径", filepath.Join(repo, "b.txt"))
	diff := app.diffState
	diff.GoToHunk(0)
	tt.Key(0, ui.KeyF4)
	if app.editorTab != "file" || app.activeCodeTab().file.RelPath != "a.txt" || app.activeCodeTab().state.CursorPos().Line != 1 {
		t.Fatal("F4 must open a.txt at the changed line")
	}
	tt.Key(ui.Cmd|ui.Alt|ui.Shift, ui.KeyC)
	tt.Frame()
	if got := tt.Clipboard(); got != "demo/a.txt:2" {
		t.Fatalf("copied code reference %q", got)
	}
	if err := tt.Click("返回差异"); err != nil {
		t.Fatal(err)
	}
	if app.editorTab != "changes" || app.diffState != diff || app.diffState.CursorPos().Line != 1 {
		t.Fatal("return must restore the original diff and line")
	}
}

func TestReviewCopyFollowsTreeAndEditorFocus(t *testing.T) {
	app := newReviewTestApp(t)
	repo := app.repos[0]
	if err := os.Mkdir(filepath.Join(repo.AbsPath, "scripts"), 0o755); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(filepath.Join(repo.AbsPath, "scripts", "example.sh"), []byte("echo example\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	app.applyRepoUpdates(gitops.InspectRepos(t.Context(), app.reposSnapshot(), 1), map[string][]gitops.WorkspaceFile{
		repo.ID: gitops.ListWorkspaceFiles(t.Context(), app.reposSnapshot(), 0),
	})
	tt := ui.NewTester(app.BuildUI, 1440, 900)
	tt.Frame()
	wantCopy := func(want string) {
		t.Helper()
		tt.Key(ui.Cmd, ui.KeyC)
		tt.Frame()
		if got := tt.Clipboard(); got != want {
			t.Fatalf("copied %q, want %q", got, want)
		}
	}
	click := func(label string) {
		t.Helper()
		if err := tt.Click(label); err != nil {
			t.Fatal(err)
		}
	}
	click("b.txt")
	wantCopy("b.txt")
	tt.Key(ui.Cmd|ui.Shift, ui.KeyC)
	tt.Frame()
	if got, want := tt.Clipboard(), filepath.Join(repo.AbsPath, "b.txt"); got != want {
		t.Fatalf("absolute shortcut copied %q, want %q", got, want)
	}
	tt.Key(0, ui.KeyDown)
	change := app.changeTreeSelection.file
	if change.Untracked {
		wantCopy(displayPath(filepath.Join(repo.AbsPath, change.Path)))
	} else {
		wantCopy(filepath.Base(change.Path))
	}
	click("scripts")
	wantCopy(displayPath(filepath.Join(repo.AbsPath, "scripts")))
	if _, visible := tt.Find("example.sh"); !visible {
		tt.Key(0, ui.KeyRight)
	}
	click("example.sh")
	want := displayPath(filepath.Join(repo.AbsPath, "scripts", "example.sh"))
	wantCopy(want)
	if runtime.GOOS == "darwin" {
		tt.SetClipboard("")
		tt.Command("copy")
		tt.Frame()
		if got := tt.Clipboard(); got != want {
			t.Fatalf("macOS Edit > Copy returned %q, want %q", got, want)
		}
	}
	app.setPrimaryTab(TabCode)
	app.expandedCodeRepos[repo.ID] = true
	tt.Frame()
	click("scripts")
	wantCopy("scripts")
	if app.expandedCodeDirs[repo.ID+"::scripts"] {
		t.Fatal("selecting a folder must leave it collapsed")
	}
	tt.Key(0, ui.KeyRight)
	click("example.sh")
	wantCopy("example.sh")
	if runtime.GOOS == "darwin" {
		tt.SetClipboard("")
		tt.Command("copy")
		tt.Frame()
		if got := tt.Clipboard(); got != "example.sh" {
			t.Fatalf("macOS Edit > Copy returned %q", got)
		}
	}
	if app.activeCodeTab() != nil {
		t.Fatal("single-clicking a file must leave the editor unchanged")
	}
	tt.Key(0, ui.KeyEnter)
	if app.activeCodeTab() == nil || app.activeCodeTab().file.RelPath != "scripts/example.sh" {
		t.Fatal("Enter must open the selected file")
	}
	app.activeCodeTab().viewHandle.Focus()
	tt.Frame()
	tt.Key(ui.Cmd, ui.KeyA)
	wantCopy("echo example\n")
}

func TestReviewCommitFocusAndDiffActivation(t *testing.T) {
	app := newReviewTestApp(t)
	diff := app.diffState
	tt := ui.NewTester(app.BuildUI, 1440, 900)
	tt.Frame()
	click := func(label string) {
		t.Helper()
		if err := tt.Click(label); err != nil {
			t.Fatal(err)
		}
	}
	click("b.txt")
	if app.diffState != diff || app.changeTreeSelection.file.Path != "b.txt" {
		t.Fatal("single click must only select the Commit row")
	}
	tt.Key(ui.Cmd, ui.KeyD)
	if app.diffState == diff || app.selectedChangeFile.Path != "b.txt" {
		t.Fatal("Cmd+D must open the selected Commit diff")
	}
	tt.Key(ui.Cmd, ui.KeyW)
	if app.diffState != nil || app.changeTreeSelection.file.Path != "b.txt" {
		t.Fatal("closing a diff must keep the selected Commit row")
	}
	tt.Advance(time.Second)
	click("a.txt")
	click("a.txt")
	if app.diffState == nil || app.selectedChangeFile.Path != "a.txt" {
		t.Fatal("double click must open the selected Commit diff")
	}
	app.cfg.ChangeDiffPreview = true
	click("b.txt")
	if app.selectedChangeFile.Path != "b.txt" {
		t.Fatal("single click must open a diff when preview is enabled")
	}
}

func TestReviewNavigationShortcutsKeepEditorAndRouteCopy(t *testing.T) {
	app := newReviewTestApp(t)
	tt := ui.NewTester(app.BuildUI, 1440, 900)
	tt.Frame()
	diff := app.diffState
	diff.GoToHunk(0)
	tt.Key(0, ui.KeyF4)
	source := app.activeCodeTab()
	pos := editor.Pos{Line: 1}
	if app.editorTab != "file" || source == nil || source.state.CursorPos() != pos {
		t.Fatal("F4 must open the source at the changed line")
	}
	tt.Key(ui.Cmd, ui.KeyBracketLeft)
	if app.editorTab != "changes" || app.diffState != diff || diff.CursorPos() != pos {
		t.Fatal("Cmd+[ must restore the diff at its original position")
	}
	tt.Key(ui.Cmd, ui.KeyBracketRight)
	if app.editorTab != "file" || app.activeCodeTab() != source || source.state.CursorPos() != pos {
		t.Fatal("Cmd+] must return to the same source position")
	}
	tt.Key(ui.Alt, ui.KeyF1)
	if app.selectInFile == nil || !tt.HasText("Project") {
		t.Fatal("Option+F1 must open Select In")
	}
	if err := tt.Click("Project"); err != nil {
		t.Fatal(err)
	}
	wantCopy := func(want string) {
		t.Helper()
		tt.Key(ui.Cmd, ui.KeyC)
		tt.Frame()
		if got := tt.Clipboard(); got != want {
			t.Fatalf("copy routed to %q, want %q", got, want)
		}
	}
	wantCopy("a.txt")
	tt.Key(ui.Cmd, ui.Key0)
	if app.activeTab != TabChanges {
		t.Fatal("Cmd+0 must show Commit")
	}
	wantCopy("a.txt")
	tt.Key(ui.Cmd, ui.Key1)
	if app.activeTab != TabCode {
		t.Fatal("Cmd+1 must show Project")
	}
	wantCopy("a.txt")
	if app.editorTab != "file" || app.activeCodeTab() != source || source.state.CursorPos() != pos {
		t.Fatal("activating a tool window must preserve the active source and position")
	}
}

func TestReviewGoToLineFindReplaceUndoAndSave(t *testing.T) {
	app := newReviewTestApp(t)
	disk := filepath.Join(app.selectedChangeFile.RepoPath, "a.txt")
	const original = "alpha here\nother text\nalpha again\n"
	if err := os.WriteFile(disk, []byte(original), 0o644); err != nil {
		t.Fatal(err)
	}
	tt := ui.NewTester(app.BuildUI, 1440, 900)
	tt.Frame()
	tt.Key(ui.Cmd|ui.Shift, ui.KeyO)
	tt.Type(displayPath(disk) + ":2:3")
	tt.Key(0, ui.KeyEnter)
	tab := app.activeCodeTab()
	if tab == nil || tab.file.AbsPath != disk || tab.state.CursorPos() != (editor.Pos{Line: 1, Col: 2}) {
		t.Fatal("Go to File must handle a home-relative path and its line and column")
	}
	tt.Key(ui.Cmd, reviewEditorGoToLineKey())
	tt.Key(ui.Cmd, ui.KeyA)
	tt.Type("1:2")
	tt.Key(0, ui.KeyEnter)
	if got := tab.state.CursorPos(); got != (editor.Pos{Col: 1}) {
		t.Fatalf("Go to Line moved to %+v", got)
	}
	if runtime.GOOS == "darwin" {
		tt.Key(ui.Cmd, ui.KeyF)
		tt.Type("alpha")
		tt.Key(ui.Cmd, ui.KeyG)
		first := tab.state.CursorPos()
		tt.Key(ui.Cmd, ui.KeyG)
		second := tab.state.CursorPos()
		tt.Key(ui.Cmd|ui.Shift, ui.KeyG)
		if first.Line == second.Line || (first.Line != 0 && first.Line != 2) || (second.Line != 0 && second.Line != 2) || tab.state.CursorPos() != first {
			t.Fatalf("find next/previous did not visit both matches: %+v, %+v, %+v", first, second, tab.state.CursorPos())
		}
		tt.Key(0, ui.KeyEscape)
	}
	tt.Key(ui.Cmd, ui.KeyR)
	replaceAll, ok := tt.Find("Replace All")
	if !ok {
		t.Fatal("Cmd+R must open the replacement bar")
	}
	if runtime.GOOS != "darwin" {
		tt.Type("alpha")
	}
	// The replacement input is to the left of the Replace and Replace All buttons.
	tt.ClickAt(replaceAll.X-180, replaceAll.Y+replaceAll.H/2)
	tt.Type("beta")
	if err := tt.Click("Replace All"); err != nil {
		t.Fatal(err)
	}
	if got, want := tab.state.Buffer.String(), strings.ReplaceAll(original, "alpha", "beta"); got != want {
		t.Fatalf("replace all left %q, want %q", got, want)
	}
	tt.Key(0, ui.KeyEscape)
	if tt.HasText("Replace All") {
		t.Fatal("Escape must close the replacement bar after clicking Replace All")
	}
	tt.Key(ui.Cmd, ui.KeyZ)
	if tab.state.Buffer.String() != original {
		t.Fatal("Cmd+Z must undo the replacement")
	}
	tt.Type("new ")
	app.isRefreshing = true
	tt.Key(ui.Cmd, ui.KeyS)
	got, err := os.ReadFile(disk)
	if err != nil || string(got) != tab.state.Buffer.String() || string(got) == original || tab.state.Buffer.Modified() {
		t.Fatalf("Cmd+S left %q on disk: %v", got, err)
	}
}

func TestReviewDiffFindUsesFocusedSide(t *testing.T) {
	app := newReviewTestApp(t)
	tt := ui.NewTester(app.BuildUI, 1440, 900)
	tt.Frame()
	toolbar, ok := tt.Find("修改 (F4)")
	if !ok {
		t.Fatal("missing Diff toolbar")
	}
	y := toolbar.Y + toolbar.H + 35
	tt.ClickAt(550, y)
	if app.diffState.ActiveSide() != editor.DiffOld {
		t.Fatal("the left Diff editor must take focus")
	}
	tt.Key(ui.Cmd, ui.KeyF)
	tt.Type("changed")
	if !tt.HasText("No results") {
		t.Fatal("search on the old side must not find text from the new side")
	}
	tt.Key(0, ui.KeyEscape)
	tt.ClickAt(1100, y)
	if app.diffState.ActiveSide() != editor.DiffNew {
		t.Fatal("the right Diff editor must take focus")
	}
	tt.Key(ui.Cmd, ui.KeyF)
	tt.Type("changed")
	if !tt.HasText("1 match") && !tt.HasText("1 of 1") {
		t.Fatal("search on the new side must find the added text")
	}
}

func TestReviewRefreshAndSaveConflict(t *testing.T) {
	for _, surface := range []string{"diff", "file"} {
		t.Run(surface, func(t *testing.T) {
			app := newReviewTestApp(t)
			file := changedWorkspaceFile(*app.selectedChangeFile)
			diskPath := filepath.Join(file.RepoPath, file.RelPath)
			buffer := app.diffState.New
			save := app.saveDiffMicroEdit
			if surface == "file" {
				app.selectCodeFile(file)
				buffer = app.activeCodeTab().state.Buffer
				save = app.saveCodeMicroEdit
			}
			write := func(text string) {
				t.Helper()
				if err := os.WriteFile(diskPath, []byte(text), 0o644); err != nil {
					t.Fatal(err)
				}
			}
			const afterRefresh = "agent saved a second change\n"
			write(afterRefresh)
			app.applyRepoUpdates(gitops.InspectRepos(t.Context(), app.reposSnapshot(), 1), nil)
			if buffer.String() != afterRefresh || buffer.Modified() {
				t.Fatal("refresh should update the existing clean buffer")
			}
			const local = "edited in Nemorous\n"
			const external = "agent edited the same file again\n"
			buffer.Set(local)
			write(external)
			app.applyRepoUpdates(gitops.InspectRepos(t.Context(), app.reposSnapshot(), 1), nil)
			save()
			if app.reviewSaveConflict == nil || !buffer.Modified() || buffer.String() != local {
				t.Fatal("external change should open a comparison and keep the unsaved buffer")
			}
			disk, err := os.ReadFile(diskPath)
			if err != nil || string(disk) != external {
				t.Fatalf("save changed the disk contents to %q: %v", disk, err)
			}
			tt := ui.NewTester(app.BuildUI, 1440, 900)
			tt.Frame()
			if err := tt.Click("复制我的编辑"); err != nil {
				t.Fatal(err)
			}
			if tt.Clipboard() != local {
				t.Fatalf("expected copied draft, got %q", tt.Clipboard())
			}
			if err := tt.Click("使用磁盘版本"); err != nil {
				t.Fatal(err)
			}
			if app.reviewSaveConflict != nil || buffer.Modified() || buffer.String() != external {
				t.Fatal("adopting the disk version should close the dialog and update the buffer")
			}
		})
	}
}

func TestReviewKeyboardMixedStagingAndFilePicker(t *testing.T) {
	app := newReviewTestApp(t)
	file := *app.selectedChangeFile
	if _, err := gitops.RunGit(t.Context(), file.RepoPath, "add", file.Path); err != nil {
		t.Fatal(err)
	}
	disk := filepath.Join(file.RepoPath, file.Path)
	if err := os.WriteFile(disk, []byte("a.txt\nchanged\nagent line after staging\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	app.applyRepoUpdates(gitops.InspectRepos(t.Context(), app.reposSnapshot(), 1), nil)
	file = *app.selectedChangeFile
	app.collapsedStagedSection = false
	app.stagedSectionInit = true
	app.selectChangedFileInScope(file, true)
	tt := ui.NewTester(app.BuildUI, 1440, 900)
	tt.Frame()
	tt.Key(ui.Alt, ui.KeyRight)
	if app.selectedChangeKey != file.Key() || app.diffStagedOnly || !app.diffEditable || app.diffState.Old.String() != "a.txt\nchanged\n" {
		t.Fatal("next file must visit the unstaged differences of a partially staged file")
	}
	app.diffState.GoToHunk(0)
	tt.Frame()
	tt.Key(0, ui.KeyF7)
	if app.selectedChangeFile.Key() != file.Key() || !tt.HasText("已到最后一个差异，再按 F7 查看下一个文件") {
		t.Fatal("at the boundary F7 should prompt before switching files")
	}
	tt.Key(0, ui.KeyF7)
	if app.selectedChangeFile.Path != "b.txt" {
		t.Fatal("next difference must continue into the next file")
	}
	tt.Key(ui.Cmd|ui.Shift, ui.KeyO)
	tt.Type("demo/a.txt:3:2")
	tt.Key(0, ui.KeyEnter)
	if tab := app.activeCodeTab(); tab == nil || tab.file.RelPath != "a.txt" || tab.state.CursorPos() != (editor.Pos{Line: 2, Col: 1}) {
		t.Fatalf("file picker did not open a.txt:3:2; tab %v", tab)
	}
	tt.Key(ui.Cmd, ui.KeyR)
	if !tt.HasText("Replace All") {
		t.Fatal("Cmd+R should open replacement in the active source editor")
	}
	tt.Key(0, ui.KeyEscape)
	tt.Key(ui.Cmd|ui.Shift, ui.KeyE)
	if !tt.HasText("最近位置") || !tt.HasText("第 3 行") {
		t.Fatal("recent locations should show a.txt's visited line")
	}
}

func TestReviewCheckedSaveRejectsDeletedAndKeepsLineEndings(t *testing.T) {
	app := newReviewTestApp(t)
	file := *app.selectedChangeFile
	disk := filepath.Join(file.RepoPath, file.Path)
	const crlf = "one\r\ntwo\r\n"
	if err := os.WriteFile(disk, []byte(crlf), 0o644); err != nil {
		t.Fatal(err)
	}
	app.selectCodeFile(changedWorkspaceFile(file))
	buffer := app.activeCodeTab().state.Buffer
	buffer.Set("one\r\nthree\r\n")
	app.isRefreshing = true
	if !app.saveCodeTab(app.activeCodeTab()) {
		t.Fatal("a matching file version should save")
	}
	data, err := os.ReadFile(disk)
	if err != nil || string(data) != "one\r\nthree\r\n" {
		t.Fatalf("save must preserve CRLF: %q, %v", data, err)
	}
	if err := os.Remove(disk); err != nil {
		t.Fatal(err)
	}
	app.selectChangedFile(file)
	if app.diffEditable {
		t.Fatal("a removed worktree file is read-only in the diff")
	}
	if err := gitops.SaveReviewText(file.RepoPath, file.Path, "draft", gitops.ReviewTextVersion(string(data))); !errors.Is(err, gitops.ErrReviewFileChanged) {
		t.Fatalf("save of a deleted file must report a conflict; got %v", err)
	}
	if _, err := os.Stat(disk); !errors.Is(err, os.ErrNotExist) {
		t.Fatalf("checked save recreated the deleted file; stat = %v", err)
	}
	if err := os.WriteFile(disk, []byte(strings.Repeat("x", (2<<20)+1)), 0o644); err != nil {
		t.Fatal(err)
	}
	app.selectCodeFile(changedWorkspaceFile(file))
	if !app.activeCodeTab().state.ReadOnly {
		t.Fatal("oversized source files must be read-only")
	}
}

func TestReviewFilePickerFindsBeyondPreview(t *testing.T) {
	app := newReviewTestApp(t)
	repo := app.repos[0]
	for i := range gitops.WorkspaceFilePreviewLimit {
		name := filepath.Join(repo.AbsPath, fmt.Sprintf("fixture-%04d", i))
		if err := os.WriteFile(name, nil, 0o644); err != nil {
			t.Fatal(err)
		}
	}
	target := filepath.Join(repo.AbsPath, "zz agent file.txt")
	if err := os.WriteFile(target, []byte("first\nsecond\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	if _, err := gitops.RunGit(t.Context(), repo.AbsPath, "add", "-A"); err != nil {
		t.Fatal(err)
	}
	app.workspaceFiles = gitops.ListWorkspaceFiles(t.Context(), app.repos, gitops.WorkspaceFilePreviewLimit)
	if _, ok := app.workspacePreviewFileByKey(repo.ID, filepath.Base(target)); ok {
		t.Fatal("file should be beyond the initial preview")
	}
	app.openReviewPicker(reviewGoToFile)
	if files := app.reviewPickerResults("zz agent file.txt:2"); len(files) != 0 {
		t.Fatalf("cached results should be empty, got %v", files)
	}
	waitApp(t, "on-demand file search", func() bool {
		app.uiMu.Lock()
		defer app.uiMu.Unlock()
		return app.reviewPickerCancel == nil
	})
	if files := app.reviewPickerMatches; len(files) != 1 || files[0].file.AbsPath != target || files[0].pos.Line != 1 {
		t.Fatalf("unexpected file search results: %v", files)
	}
	app.closeReviewPicker()
	app.locateFileInTree(app.reviewPickerMatches[0].file)
	if app.codeRevealKey == "" || len(app.codeExtraFile) != 1 || app.codeExtraFile[0].AbsPath != target {
		t.Fatal("a file outside the preview should still be locatable in the tree")
	}
}

func TestReviewSearchOnlySelectedDirectory(t *testing.T) {
	app := newReviewTestApp(t)
	repo := app.repos[0]
	for _, dir := range []string{"inside", "outside"} {
		path := filepath.Join(repo.AbsPath, dir)
		if err := os.Mkdir(path, 0o755); err != nil {
			t.Fatal(err)
		}
		if err := os.WriteFile(filepath.Join(path, "search.txt"), []byte("review directory needle\n"), 0o644); err != nil {
			t.Fatal(err)
		}
	}
	app.workspaceFiles = gitops.ListWorkspaceFiles(t.Context(), app.repos, 0)
	app.activeTab = TabCode
	app.expandedCodeRepos[repo.ID] = true
	tt := ui.NewTester(app.BuildUI, 1440, 900)
	tt.Frame()
	if err := tt.RightClick("inside"); err != nil {
		t.Fatal(err)
	}
	if err := tt.Click("在目录中查找"); err != nil {
		t.Fatal(err)
	}
	if app.globalSearchDirectory != filepath.Join(repo.AbsPath, "inside") {
		t.Fatal("search should be scoped to the selected folder")
	}
	tt.Type("review directory needle")
	waitApp(t, "scope search", func() bool {
		app.uiMu.Lock()
		defer app.uiMu.Unlock()
		return !app.globalSearchRunning
	})
	if app.globalSearchTotal != 1 || len(app.globalSearchGroups) != 1 || app.globalSearchGroups[0].RelPath != "inside/search.txt" {
		t.Fatalf("search was not limited to inside/: %v", app.globalSearchGroups)
	}
}

func TestReviewStagedRenameUsesOriginalPath(t *testing.T) {
	app := newReviewTestApp(t)
	repo := app.repos[0]
	if _, err := gitops.RunGit(t.Context(), repo.AbsPath, "mv", "a.txt", "renamed.txt"); err != nil {
		t.Fatal(err)
	}
	status := gitops.InspectRepo(t.Context(), repo)
	for _, file := range status.Files {
		if file.Path != "renamed.txt" {
			continue
		}
		staged, err := gitops.ReadReviewChange(t.Context(), file, true)
		if err != nil || staged.Old != "a.txt\n" || staged.New != staged.Old {
			t.Fatalf("a pure stage rename must preserve both sides, got %+v, %v", staged, err)
		}
		unstaged, err := gitops.ReadReviewChange(t.Context(), file, false)
		if err != nil || unstaged.Old != staged.New || unstaged.New != "a.txt\nchanged\n" {
			t.Fatalf("unstaged edits must compare against the renamed index path, got %+v, %v", unstaged, err)
		}
		return
	}
	t.Fatal("expected staged rename")
}
