package main

import (
	"os"
	"path/filepath"
	"runtime"
	"strings"
	"testing"
	"time"

	"github.com/egoist/mygo/nemorouseditor"
	"github.com/egoist/mygo/ui"
	"github.com/iwangjie/nemorous/internal/gitops"
)

func TestCommittedDiffCopiesTextAndActivePosition(t *testing.T) {
	app := newReviewTestApp(t)
	repo := app.repos[0]
	write := func(path, content string) {
		t.Helper()
		if err := os.WriteFile(filepath.Join(repo.AbsPath, path), []byte(content), 0o644); err != nil {
			t.Fatal(err)
		}
	}
	git := func(args ...string) string {
		t.Helper()
		out, err := gitops.RunGit(t.Context(), repo.AbsPath, args...)
		if err != nil {
			t.Fatal(err)
		}
		return strings.TrimSpace(out)
	}
	const oldText = "first\nold selection\nthird\n"
	const newText = "intro\nfirst\nnew selection\nthird\n"
	write("a.txt", oldText)
	git("add", "a.txt")
	git("commit", "-m", "before the rename")
	git("mv", "a.txt", "renamed.txt")
	write("renamed.txt", newText)
	git("add", "renamed.txt")
	git("commit", "-m", "rename the file")
	file := gitops.CommitFile{Path: "renamed.txt", OldPath: "a.txt", Status: "R"}
	detail := gitops.CommitDetail{
		CommitEntry: gitops.CommitEntry{RepoID: repo.ID, RepoName: repo.Name, RepoPath: repo.AbsPath, Hash: git("rev-parse", "HEAD")},
		Files:       []gitops.CommitFile{file},
	}
	app.TriggerGitFileDiff(detail, file)
	waitCommitDiff(t, app)
	tt := ui.NewTester(app.BuildUI, 1440, 900)
	tt.Frame()
	toolbar, ok := tt.Find("修改 (F4)")
	if !ok {
		t.Fatal("missing history diff")
	}
	y := toolbar.Y + toolbar.H + 35
	for _, tc := range []struct {
		x         float32
		side      editor.DiffSide
		text      string
		location  string
		lineInput string
		pos       editor.Pos
	}{
		{550, editor.DiffOld, oldText, "demo/a.txt:2", "2:4", editor.Pos{Line: 1, Col: 3}},
		{1100, editor.DiffNew, newText, "demo/renamed.txt:3", "3:4", editor.Pos{Line: 2, Col: 3}},
	} {
		tt.ClickAt(tc.x, y)
		if app.gitCommitDiff.ActiveSide() != tc.side {
			t.Fatalf("could not focus historical side %v", tc.side)
		}
		tt.Press(tc.x, y)
		tt.Move(tc.x, y+18)
		tt.Release(tc.x, y+18)
		tt.Key(ui.Cmd, ui.KeyC)
		if got := tt.Clipboard(); !strings.Contains(got, "\n") || got == tc.text || !strings.Contains(tc.text, got) {
			t.Fatalf("dragging in side %v must copy only the selected fragment, got %q", tc.side, got)
		}
		tt.Key(ui.Cmd, ui.KeyA)
		tt.Key(ui.Cmd, ui.KeyC)
		if got := tt.Clipboard(); got != tc.text {
			t.Fatalf("side %v copied %q, want %q", tc.side, got, tc.text)
		}
		if runtime.GOOS == "darwin" {
			tt.SetClipboard("")
			tt.Command("copy")
			if got := tt.Clipboard(); got != tc.text {
				t.Fatalf("native Copy on side %v returned %q", tc.side, got)
			}
		}
		tt.Key(ui.Cmd, reviewEditorGoToLineKey())
		tt.Key(ui.Cmd, ui.KeyA)
		tt.Type(tc.lineInput)
		tt.Key(0, ui.KeyEnter)
		if pos := app.gitCommitDiff.CursorPos(); pos != tc.pos {
			t.Fatalf("side %v jumped to %+v, want %+v", tc.side, pos, tc.pos)
		}
		tt.Key(ui.Cmd|ui.Alt|ui.Shift, ui.KeyC)
		tt.Frame()
		if got := tt.Clipboard(); got != tc.location {
			t.Fatalf("side %v copied location %q, want %q", tc.side, got, tc.location)
		}
		tt.Type("immutable")
		if app.gitCommitDiff.Old.String() != oldText || app.gitCommitDiff.New.String() != newText {
			t.Fatal("both committed versions must stay read-only")
		}
	}
}

func TestCommittedDiffReviewFromGitLog(t *testing.T) {
	app := newReviewTestApp(t)
	repo := app.repos[0]
	git := func(args ...string) string {
		t.Helper()
		out, err := gitops.RunGit(t.Context(), repo.AbsPath, args...)
		if err != nil {
			t.Fatal(err)
		}
		return strings.TrimSpace(out)
	}
	write := func(path, content string) {
		t.Helper()
		if err := os.WriteFile(filepath.Join(repo.AbsPath, path), []byte(content), 0o644); err != nil {
			t.Fatal(err)
		}
	}
	const path = "renamed 文档.txt"
	old := "original\n" + strings.Repeat("context retained across revisions\n", 6)
	committed := strings.Replace(old, "original", "committed", 1)
	local := strings.Replace(committed, "committed", "local", 1)
	write("a.txt", old)
	git("add", "a.txt")
	git("commit", "-m", "before rename")
	base := git("rev-parse", "HEAD")
	write("b.txt", "changed in an intervening commit\n")
	git("add", "b.txt")
	git("commit", "-m", "intervening file change")
	parent := git("rev-parse", "HEAD")
	git("mv", "a.txt", path)
	write(path, committed)
	git("add", path)
	const historyAuthor = "History Reviewer"
	const historyDate = "2024-04-05T06:07:08Z"
	git("commit", "-m", "rename and update", "--author", historyAuthor+" <history@example.test>", "--date", historyDate)
	head := git("rev-parse", "HEAD")
	write(path, local)
	files, err := gitops.CommitFiles(t.Context(), repo.AbsPath, head)
	if err != nil || len(files) != 1 || files[0].Status != "R" || files[0].OldPath != "a.txt" || files[0].Path != path {
		t.Fatalf("unexpected changed file data %+v (%v)", files, err)
	}
	newer := gitops.CommitEntry{Hash: head, ShortHash: head[:8], Parents: []string{parent}, RepoID: repo.ID, RepoName: repo.Name, RepoPath: repo.AbsPath, Subject: "rename and update", AuthorUnix: 2}
	older := newer
	older.Hash, older.ShortHash, older.Parents, older.Subject = base, base[:8], nil, "before rename"
	detail := &gitops.CommitDetail{CommitEntry: newer, Files: files}
	app.gitDetail, app.gitSelectedCommit = detail, repo.ID+"|"+head
	app.gitUnifiedRows, app.gitUnifiedMaxLanes = buildGitRows([]gitops.CommitEntry{newer, older})
	app.gitLogTargets = app.statuses
	app.gitLogLoaded, app.gitAuthorsLoaded, app.showGitPanel = true, true, true
	app.gitEditorPct = 43
	tt := ui.NewTester(app.BuildUI, 1440, 900)
	tt.Frame()
	click := func(label string) {
		t.Helper()
		if err := tt.Click(label); err != nil {
			t.Fatal(err)
		}
	}
	item := func(label string) {
		t.Helper()
		if err := tt.RightClick("变更文件 " + path); err != nil {
			t.Fatal(err)
		}
		click(label)
	}
	click("变更文件 " + path)
	if app.gitCommitDiff != nil || app.gitDetailSelectedPath != path {
		t.Fatal("single click must select a changed file without opening its diff")
	}
	tt.Key(ui.Cmd, ui.KeyD)
	diff := app.gitCommitDiff
	if diff == nil || diff.Old.String() != old || diff.New.String() != committed {
		t.Fatal("Cmd+D must open the selected changes against their parent")
	}
	view, ok := tt.Find("提交差异")
	if !ok {
		t.Fatal("missing commit diff surface")
	}
	tt.ClickAt(view.X+90, view.Y+12)
	tt.Key(ui.Cmd, reviewEditorGoToLineKey())
	tt.Key(ui.Cmd, ui.KeyA)
	tt.Type("2:4")
	tt.Key(0, ui.KeyEnter)
	pos := diff.CursorPos()
	scroll := diff.ScrollY()
	tt.Key(0, ui.KeyF4)
	if app.editorTab != "file" || app.activeCodeTab().file.RelPath != path || app.activeCodeTab().state.CursorPos() != pos {
		t.Fatal("F4 must open the working file at the matching position")
	}
	tt.Key(ui.Cmd, ui.KeyBracketLeft)
	if app.editorTab != "commit" || app.gitCommitDiff != diff || diff.ActiveSide() != editor.DiffOld || diff.CursorPos() != pos || diff.ScrollY() != scroll {
		t.Fatal("Back must restore the historical diff and caret")
	}
	view, _ = tt.Find("提交差异")
	tt.RightClickAt(view.X+8, view.Y+12)
	click("使用 Git Blame 注解")
	blame := app.gitDiffBlame[0]
	if blame == nil || len(blame.lines) == 0 || blame.lines[0].Hash != base || !tt.HasText("Git Blame") {
		t.Fatal("blame on the old gutter must be calculated for the old path and revision")
	}
	tt.Key(ui.Cmd, ui.KeyW)
	tt.Key(ui.Cmd, ui.KeyC)
	tt.Frame()
	if got := tt.Clipboard(); got != path || app.gitCommitDiff != nil {
		t.Fatalf("closing the diff should return focus to its file, copied %q", got)
	}
	item("打开仓库版本")
	snapshot := app.activeCodeTab()
	if snapshot == nil || snapshot.revision != head || !snapshot.state.ReadOnly || snapshot.state.Buffer.String() != committed {
		t.Fatal("Open Repository Version must open an immutable committed file")
	}
	app.refreshVisibleCodeFile(nil)
	if snapshot.state.Buffer.String() != committed {
		t.Fatal("refreshing working files must keep the repository version intact")
	}
	item("与本地版本比较")
	if d := app.gitCommitDiff; d == nil || d.Old.String() != committed || d.New.String() != local {
		t.Fatal("Compare with Local must compare the committed and current disk texts")
	}
	item("查看截至此处的历史")
	history := app.gitRevisionHistory
	if history == nil || app.gitSubTab != "file-history" || len(history.entries) < 2 || history.entries[0].Hash != head || history.entries[1].File.Path != "a.txt" {
		t.Fatalf("History Up to Here must follow the rename: %+v", history)
	}
	when, err := time.Parse(time.RFC3339, historyDate)
	if err != nil {
		t.Fatal(err)
	}
	if revision := history.entries[0]; revision.Author != historyAuthor || revision.AuthorUnix != when.Unix() {
		t.Fatalf("file history did not retain the Git author and author date: %+v", revision)
	}
	narrowHistory := ui.NewTester(func(c *ui.Context) { app.buildRevisionHistory(c, app.palette(), history) }, 375, 160)
	narrowHistory.Frame()
	for _, label := range []string{"作者", "时间", historyAuthor, when.Local().Format("2006-01-02 15:04")} {
		cell, ok := narrowHistory.Find(label)
		if !ok || cell.W < 90 || cell.X+cell.W > 375 {
			t.Fatalf("history column %q is clipped at a narrow width: %+v (found %v)", label, cell, ok)
		}
	}
	click("提交图")
	click(older.Subject)
	if err := tt.ClickWith(ui.Cmd, newer.Subject); err != nil {
		t.Fatal(err)
	}
	if err := tt.RightClick(older.Subject); err != nil {
		t.Fatal(err)
	}
	click("比较版本")
	if d := app.gitDetail; d == nil || d.BaseRevision != base || d.Hash != head || len(d.Files) != 2 || d.Files[0].Path != "b.txt" || d.Files[1].Path != path {
		t.Fatalf("Compare Versions must list the changes between selected commits: %+v", app.gitDetail)
	}
	click("变更文件 " + path)
	tt.Key(ui.Cmd, ui.KeyD)
	if d := app.gitCommitDiff; d == nil || d.Old.String() != old || d.New.String() != committed {
		t.Fatal("opening a file from Compare Versions must use both selected revisions")
	}
	git("rm", "-f", path)
	git("commit", "-m", "delete the renamed file")
	deleted := newer
	deleted.Hash, deleted.Parents = git("rev-parse", "HEAD"), []string{head}
	app.openRepositoryVersion(gitops.CommitDetail{CommitEntry: deleted}, gitops.CommitFile{Path: path, Status: "D"})
	if tab := app.activeCodeTab(); tab.revision != head || !tab.state.ReadOnly || tab.state.Buffer.String() != committed {
		t.Fatal("opening a deleted file must show its last committed content")
	}
}
