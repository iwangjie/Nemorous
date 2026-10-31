package main

import (
	"testing"

	"github.com/egoist/mygo/ui"
)

func TestDiscoverabilityEntrypoints(t *testing.T) {
	ws := makeP0Repo(t, 2)
	app := newGapTestApp(t, ws)
	tt := ui.NewTester(app.BuildUI, 1440, 900)

	app.activeTab = TabChanges
	tt.Frame()
	if !tt.HasText("修正上次提交") || !tt.HasText("撤销上次提交…") {
		t.Fatal("amend/undo entrypoints missing from commit box")
	}

	app.showGitPanel = true
	app.gitSubTab = "history"
	tt.Frame()
	if !tt.HasText("Git 操作") {
		t.Fatal("visible Git operations entry missing")
	}

	app.showGitPanel = false
	app.openBranchSwitchModal()
	tt.Frame()
	if !tt.HasText("还没有分支组合") {
		t.Fatal("branch preset empty state missing")
	}
	app.showBranchModal = false

	app.openWorkspaceManager()
	app.wsAddMode = "clone"
	tt.Frame()
	if !tt.HasText("配置凭据…") {
		t.Fatal("clone form does not link to credentials")
	}
	app.showWorkspaceManager = false

	app.commitMsgHistory = []string{"feat: previous message", "fix: another message"}
	app.showCommitMsgHistory = true
	tt.Frame()
	if !tt.HasText("提交信息历史") || !tt.HasText("feat: previous message") {
		t.Fatal("commit message history modal did not render")
	}
	if err := tt.Click("feat: previous message"); err != nil {
		t.Fatalf("select commit message: %v", err)
	}
	if app.commitMessage != "feat: previous message" {
		t.Fatalf("commit message=%q", app.commitMessage)
	}
}
