package main

import (
	"testing"
	"time"

	"github.com/egoist/mygo/ui"
	"github.com/iwangjie/nemorous/internal/config"
	"github.com/iwangjie/nemorous/internal/gitops"
)

func TestGitDateGroupingRendersHeaders(t *testing.T) {
	t.Setenv("NEMOROUS_CONFIG", t.TempDir()+"/config.json")
	app := NewApp(config.Default())
	app.showGitPanel = true
	app.gitLogLoaded = true
	app.gitGroupMode = "date"
	app.gitLogTargets = []gitops.RepoStatus{{}}
	now := time.Now()
	app.gitUnifiedRows = []gitCommitRow{
		{Entry: gitops.CommitEntry{RepoID: "r", RepoName: "demo", Hash: "h1", Subject: "today", Author: "A", AuthorUnix: now.Unix()}},
		{Entry: gitops.CommitEntry{RepoID: "r", RepoName: "demo", Hash: "h2", Subject: "yesterday", Author: "A", AuthorUnix: now.AddDate(0, 0, -1).Unix()}},
	}
	tt := ui.NewTester(app.BuildUI, 1440, 900)
	tt.Frame()
	if _, ok := tt.Find(gitDayLabel(now.Unix())); !ok {
		t.Fatalf("missing today header %q", gitDayLabel(now.Unix()))
	}
	if _, ok := tt.Find(gitDayLabel(now.AddDate(0, 0, -1).Unix())); !ok {
		t.Fatalf("missing yesterday header %q", gitDayLabel(now.AddDate(0, 0, -1).Unix()))
	}
	app.gitGroupMode = "repo"
	app.gitUnifiedRows = []gitCommitRow{
		{Entry: gitops.CommitEntry{RepoID: "r1", RepoName: "alpha", Hash: "h1", Subject: "one", AuthorUnix: now.Unix()}},
		{Entry: gitops.CommitEntry{RepoID: "r2", RepoName: "beta", Hash: "h2", Subject: "two", AuthorUnix: now.Add(-time.Hour).Unix()}},
	}
	tt.Frame()
	for _, repo := range []string{"alpha", "beta"} {
		if _, ok := tt.Find(repo); !ok {
			t.Fatalf("missing repo section %q", repo)
		}
	}
}
