package github

import (
	"context"
	"os"
	"path/filepath"
	"runtime"
	"strings"
	"testing"
)

func TestParseRemote(t *testing.T) {
	cases := map[string]Repo{
		"git@github.com:acme/app.git":       {Owner: "acme", Name: "app"},
		"https://github.com/acme/app.git":   {Owner: "acme", Name: "app"},
		"https://github.com/acme/app":       {Owner: "acme", Name: "app"},
		"ssh://git@github.com/acme/app.git": {Owner: "acme", Name: "app"},
	}
	for remote, want := range cases {
		got, err := ParseRemote(remote)
		if err != nil || got != want {
			t.Errorf("ParseRemote(%q) = %+v, %v; want %+v", remote, got, err, want)
		}
	}
	if _, err := ParseRemote("https://gitlab.com/a/b.git"); err == nil {
		t.Fatal("non-GitHub remote accepted")
	}
}

func TestCheckBucket(t *testing.T) {
	if CheckBucket("SUCCESS") != "success" || CheckBucket("FAILURE") != "failure" || CheckBucket("IN_PROGRESS") != "pending" {
		t.Fatal("bucket normalization is wrong")
	}
}

func TestGHCommands(t *testing.T) {
	if runtime.GOOS == "windows" {
		t.Skip("fake gh shell script")
	}
	bin := t.TempDir()
	outPath := filepath.Join(t.TempDir(), "args.txt")
	script := `#!/bin/sh
echo "$@" >> ` + outPath + `
case "$1 $2" in
  "pr list") echo '[{"number":7,"title":"Fix it","state":"OPEN","url":"https://github.com/acme/app/pull/7","headRefName":"fix","baseRefName":"main","isDraft":false,"updatedAt":"2026-01-01T00:00:00Z","reviewDecision":"APPROVED","author":{"login":"alice"}}]' ;;
  "pr view") echo '{"number":7,"title":"Fix it","state":"OPEN","url":"https://github.com/acme/app/pull/7","headRefName":"fix","baseRefName":"main","body":"Body","author":{"login":"alice"},"statusCheckRollup":[{"name":"build","status":"COMPLETED","conclusion":"SUCCESS","detailsUrl":"https://ci/1"},{"name":"test","status":"COMPLETED","conclusion":"FAILURE"}],"comments":[{"author":{"login":"bob"},"body":"LGTM?","createdAt":"2026-01-01T00:00:00Z"}],"reviews":[{"author":{"login":"carol"},"body":"Please fix","createdAt":"2026-01-01T00:00:00Z"}]}' ;;
  "pr comment") ;;
  "pr create") echo 'https://github.com/acme/app/pull/8' ;;
esac
`
	if err := os.WriteFile(filepath.Join(bin, "gh"), []byte(script), 0o755); err != nil {
		t.Fatal(err)
	}
	t.Setenv("PATH", bin+string(os.PathListSeparator)+os.Getenv("PATH"))
	ctx := context.Background()
	repo := t.TempDir()
	prs, err := ListPRs(ctx, repo, "")
	if err != nil || len(prs) != 1 || prs[0].Author != "alice" || prs[0].Number != 7 {
		t.Fatalf("prs = %+v err=%v", prs, err)
	}
	details, err := ViewPR(ctx, repo, 7)
	if err != nil {
		t.Fatal(err)
	}
	if len(details.Checks) != 2 || details.Checks[0].Bucket != "success" || details.Checks[1].Bucket != "failure" {
		t.Fatalf("checks = %+v", details.Checks)
	}
	if len(details.Comments) != 2 {
		t.Fatalf("comments = %+v", details.Comments)
	}
	if err := AddComment(ctx, repo, 7, "hello"); err != nil {
		t.Fatal(err)
	}
	url, err := CreatePR(ctx, repo, "Title", "Body", "main", true)
	if err != nil || !strings.Contains(url, "/pull/8") {
		t.Fatalf("create pr = %q err=%v", url, err)
	}
	log, _ := os.ReadFile(outPath)
	if !strings.Contains(string(log), "pr comment 7 --body hello") || !strings.Contains(string(log), "pr create --title Title") {
		t.Fatalf("gh args = %s", log)
	}
}
