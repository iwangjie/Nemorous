package gitops

import (
	"context"
	"os"
	"path/filepath"
	"strings"
	"testing"
)

func TestFileBlameReportsEveryLine(t *testing.T) {
	dir := t.TempDir()
	ctx := context.Background()
	if _, err := RunGit(ctx, dir, "init", "-b", "main"); err != nil {
		t.Fatal(err)
	}
	if _, err := RunGit(ctx, dir, "config", "user.name", "Blame Tester"); err != nil {
		t.Fatal(err)
	}
	if _, err := RunGit(ctx, dir, "config", "user.email", "blame@example.com"); err != nil {
		t.Fatal(err)
	}
	path := "src/app.go"
	if err := os.MkdirAll(filepath.Join(dir, "src"), 0o755); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(filepath.Join(dir, path), []byte("package app\n\nfunc Run() {}\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	if _, err := RunGit(ctx, dir, "add", "-A"); err != nil {
		t.Fatal(err)
	}
	if _, err := RunGit(ctx, dir, "commit", "-m", "add app"); err != nil {
		t.Fatal(err)
	}
	lines, err := FileBlame(ctx, dir, path)
	if err != nil {
		t.Fatal(err)
	}
	if len(lines) != 3 {
		t.Fatalf("expected three blame lines, got %+v", lines)
	}
	for _, line := range lines {
		if line.Author != "Blame Tester" || len(line.Hash) != 40 || line.Summary != "add app" {
			t.Fatalf("unexpected blame line: %+v", line)
		}
	}
	if got := BlameDisplay(lines[0]); !strings.Contains(got, "Blame Tester") || !strings.Contains(got, "Blame Tester · ") {
		t.Fatalf("display = %q", got)
	}
}
