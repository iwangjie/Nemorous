package conflict

import (
	"context"
	"os"
	"path/filepath"
	"strings"
	"testing"

	"github.com/iwangjie/nemorous/internal/gitops"
)

func TestParseAndRenderPreservesOrder(t *testing.T) {
	content := "before\n" +
		"<<<<<<< HEAD\nours1\n=======\ntheirs1\n>>>>>>> branch\n" +
		"middle\n" +
		"<<<<<<< HEAD\nours2\n=======\ntheirs2\n>>>>>>> branch\n" +
		"after\n"
	doc, err := Parse(content)
	if err != nil {
		t.Fatal(err)
	}
	if len(doc.Hunks) != 2 || doc.Prefix != "before\n" || doc.Suffix != "after\n" {
		t.Fatalf("doc = %+v hunks=%d", doc, len(doc.Hunks))
	}
	got, err := doc.Resolve(Ours)
	if err != nil {
		t.Fatal(err)
	}
	want := "before\nours1\nmiddle\nours2\nafter\n"
	if got != want {
		t.Fatalf("ours render:\n%q\nwant\n%q", got, want)
	}
	got, _ = doc.Resolve(Both)
	want = "before\nours1\ntheirs1\nmiddle\nours2\ntheirs2\nafter\n"
	if got != want {
		t.Fatalf("both render: %q", got)
	}
}

func TestDiff3AndApplyTo(t *testing.T) {
	content := "a\n<<<<<<< HEAD\nours\n||||||| base\nbase\n=======\ntheirs\n>>>>>>> other\nb\n"
	doc, err := Parse(content)
	if err != nil {
		t.Fatal(err)
	}
	if !doc.Hunks[0].HasBase || doc.Hunks[0].Base != "base\n" {
		t.Fatalf("base not parsed: %+v", doc.Hunks[0])
	}
	out, err := ApplyTo(content, doc.Hunks[0], Theirs)
	if err != nil {
		t.Fatal(err)
	}
	if out != "a\ntheirs\nb\n" {
		t.Fatalf("apply: %q", out)
	}
}

func TestNoMarkers(t *testing.T) {
	if _, err := Parse("plain\n"); err != ErrNoMarkers {
		t.Fatalf("err = %v", err)
	}
	if _, err := Parse("<<<<<<< HEAD\nours\n"); err == nil {
		t.Fatal("unterminated hunk accepted")
	}
}

// TestRebaseConflictEndToEnd resolves a real rebase conflict through the
// same functions the conflict editor uses.
func TestRebaseConflictEndToEnd(t *testing.T) {
	ctx := context.Background()
	repo := t.TempDir()
	run := func(args ...string) string {
		t.Helper()
		out, err := gitops.RunGit(ctx, repo, args...)
		if err != nil {
			t.Fatalf("git %v: %v (%s)", args, err, out)
		}
		return out
	}
	run("init", "-b", "main")
	run("config", "user.name", "Test")
	run("config", "user.email", "test@example.com")
	path := filepath.Join(repo, "file.txt")
	if err := os.WriteFile(path, []byte("base\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	run("add", ".")
	run("commit", "-m", "base")
	run("checkout", "-b", "feature")
	if err := os.WriteFile(path, []byte("feature\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	run("commit", "-am", "feature")
	run("checkout", "main")
	if err := os.WriteFile(path, []byte("main\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	run("commit", "-am", "main")
	run("checkout", "feature")
	if _, err := gitops.Rebase(ctx, repo, "main"); err == nil {
		t.Fatal("expected conflict")
	}
	files, err := UnresolvedFiles(ctx, repo)
	if err != nil || len(files) != 1 {
		t.Fatalf("unresolved: %v %v", files, err)
	}
	content, err := ReadFile(repo, files[0])
	if err != nil {
		t.Fatal(err)
	}
	base, ours, theirs := Versions(ctx, repo, files[0])
	if base != "base\n" || ours != "main\n" || theirs != "feature\n" {
		t.Fatalf("versions: %q %q %q", base, ours, theirs)
	}
	doc, err := Parse(content)
	if err != nil {
		t.Fatal(err)
	}
	resolved, err := doc.Resolve(Ours)
	if err != nil {
		t.Fatal(err)
	}
	if err := WriteAndStage(ctx, repo, files[0], resolved); err != nil {
		t.Fatal(err)
	}
	if files, _ := UnresolvedFiles(ctx, repo); len(files) != 0 {
		t.Fatalf("still unresolved: %v", files)
	}
	if _, err := gitops.ContinueOperation(ctx, repo); err != nil {
		t.Fatal(err)
	}
	data, _ := os.ReadFile(path)
	if strings.TrimSpace(string(data)) != "main" {
		t.Fatalf("resolved file = %q", data)
	}
	if st := gitops.RepoOperationState(ctx, repo); st.Busy() {
		t.Fatalf("rebase still in progress: %+v", st)
	}
}

func TestSmartResolveAndRanges(t *testing.T) {
	content := "before\n" +
		"<<<<<<< HEAD\nsame\n||||||| base\nbase1\n=======\nsame\n>>>>>>> other\n" +
		"middle\n" +
		"<<<<<<< HEAD\nlocal\n||||||| base\nbase2\n=======\nincoming\n>>>>>>> other\n" +
		"after\n"
	doc, err := Parse(content)
	if err != nil {
		t.Fatal(err)
	}
	if doc.Hunks[0].OursStart != 1 || doc.Hunks[0].BaseStart != 1 || doc.Hunks[0].TheirsStart != 1 {
		t.Fatalf("ranges: %+v", doc.Hunks[0])
	}
	out, resolved, remaining := doc.SmartResolve()
	if resolved != 1 || remaining != 1 {
		t.Fatalf("resolved=%d remaining=%d", resolved, remaining)
	}
	if !strings.Contains(out, "same") || !strings.Contains(out, "<<<<<<<") {
		t.Fatalf("smart resolve output = %q", out)
	}
}

func TestMergeFileBuildsDiff3Markers(t *testing.T) {
	ctx := context.Background()
	repo := t.TempDir()
	run := func(args ...string) string {
		t.Helper()
		out, err := gitops.RunGit(ctx, repo, args...)
		if err != nil {
			t.Fatalf("git %v: %v (%s)", args, err, out)
		}
		return out
	}
	run("init", "-b", "main")
	run("config", "user.name", "Test")
	run("config", "user.email", "test@example.com")
	path := filepath.Join(repo, "f.txt")
	_ = os.WriteFile(path, []byte("base\n"), 0o644)
	run("add", ".")
	run("commit", "-m", "base")
	run("checkout", "-b", "feat")
	_ = os.WriteFile(path, []byte("feat\n"), 0o644)
	run("commit", "-am", "feat")
	run("checkout", "main")
	_ = os.WriteFile(path, []byte("main\n"), 0o644)
	run("commit", "-am", "main")
	run("checkout", "feat")
	_, _ = gitops.Rebase(ctx, repo, "main")
	merged, err := MergeFile(ctx, repo, "f.txt")
	if err != nil {
		t.Fatal(err)
	}
	doc, err := Parse(merged)
	if err != nil {
		t.Fatal(err)
	}
	if !doc.Hunks[0].HasBase {
		t.Fatalf("diff3 markers missing: %q", merged)
	}
}

func TestCRLFPreserved(t *testing.T) {
	content := "a\r\n<<<<<<< HEAD\r\nours\r\n=======\r\ntheirs\r\n>>>>>>> other\r\nb\r\n"
	doc, err := Parse(content)
	if err != nil {
		t.Fatal(err)
	}
	out, err := doc.Resolve(Theirs)
	if err != nil {
		t.Fatal(err)
	}
	if out != "a\r\ntheirs\r\nb\r\n" {
		t.Fatalf("CRLF resolution = %q", out)
	}
}
