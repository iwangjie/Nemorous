package main

import (
	"context"
	"os"
	"path/filepath"
	"strings"
	"testing"
	"unicode/utf8"

	"github.com/iwangjie/nemorous/internal/gitops"
)

func TestSearchSnippetKeepsUTF8AndOffsets(t *testing.T) {
	line := strings.Repeat("前置内容", 20) + "needle" + strings.Repeat("后缀内容", 20)
	start := strings.Index(line, "needle")
	snippet, from, to := searchSnippet(line, start, start+len("needle"))
	if !strings.HasPrefix(snippet, "…") || !strings.HasSuffix(snippet, "…") {
		t.Fatalf("expected trimmed snippet, got %q", snippet)
	}
	if snippet[from:to] != "needle" {
		t.Fatalf("match offsets point at %q", snippet[from:to])
	}
	if !utf8.ValidString(snippet) {
		t.Fatalf("snippet split a rune: %q", snippet)
	}
}

func TestGoSearchFindsContentWithoutRipgrep(t *testing.T) {
	root := t.TempDir()
	fileA := filepath.Join(root, "a.go")
	fileB := filepath.Join(root, "nested", "b.txt")
	if err := os.MkdirAll(filepath.Dir(fileB), 0o755); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(fileA, []byte("package main\nfunc Needle() {}\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(fileB, []byte("first\nneedle here\nlast\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	files := []gitops.WorkspaceFile{
		{RepoID: "r", RepoName: "r", RepoPath: root, RelPath: "a.go", AbsPath: fileA},
		{RepoID: "r", RepoName: "r", RepoPath: root, RelPath: "nested/b.txt", AbsPath: fileB},
	}
	t.Setenv("PATH", t.TempDir()) // force the portable scanner
	out, err := searchWorkspace(context.Background(), root, files, "needle", globalSearchOptions{})
	if err != nil {
		t.Fatal(err)
	}
	if len(out.Hits) != 2 {
		t.Fatalf("expected two case-insensitive hits, got %d: %+v", len(out.Hits), out.Hits)
	}
	byName := make(map[string]globalSearchHit)
	for _, hit := range out.Hits {
		byName[filepath.Base(hit.AbsPath)] = hit
	}
	if a, ok := byName["a.go"]; !ok || a.Line != 2 || a.Snippet != "func Needle() {}" {
		t.Fatalf("unexpected a.go hit: %+v", a)
	}
	if b, ok := byName["b.txt"]; !ok || b.Line != 2 {
		t.Fatalf("unexpected b.txt hit: %+v", b)
	}

	out, err = searchWorkspace(context.Background(), root, files, `func\s+Needle`, globalSearchOptions{Regex: true, CaseSensitive: true})
	if err != nil {
		t.Fatal(err)
	}
	if len(out.Hits) != 1 || out.Hits[0].AbsPath != fileA {
		t.Fatalf("expected one regex hit, got %+v", out.Hits)
	}

	if _, err := searchWorkspace(context.Background(), root, files, "(", globalSearchOptions{Regex: true}); err == nil {
		t.Fatal("expected invalid regex error")
	}
}
