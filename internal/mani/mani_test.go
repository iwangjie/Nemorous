package mani

import (
	"os"
	"path/filepath"
	"strings"
	"testing"
)

func TestAddProjectPreservesExistingConfig(t *testing.T) {
	dir := t.TempDir()
	initial := "# workspace\nprojects:\n  existing:\n    path: existing # keep\n    desc: existing repo\n"
	if err := os.WriteFile(filepath.Join(dir, "mani.yaml"), []byte(initial), 0o644); err != nil {
		t.Fatal(err)
	}
	if err := AddProject(dir, "new", "nested/new", "git@github.com:x/new.git"); err != nil {
		t.Fatal(err)
	}
	data, _ := os.ReadFile(filepath.Join(dir, "mani.yaml"))
	text := string(data)
	if !strings.Contains(text, "# workspace") || !strings.Contains(text, "# keep") || !strings.Contains(text, "nested/new") || !strings.Contains(text, "git@github.com:x/new.git") {
		t.Fatalf("mani.yaml lost content:\n%s", text)
	}
	cfg, _, err := LoadManiConfig(dir)
	if err != nil || cfg.Projects["new"].Path != "nested/new" {
		t.Fatalf("new project=%+v err=%v", cfg, err)
	}
}

func TestFilterRepos(t *testing.T) {
	repos := []DiscoveredRepo{{ID: "a", RelPath: "a"}, {ID: "b", RelPath: "nested/b"}}
	out := FilterRepos(repos, []string{"a", "nested/b"})
	if len(out) != 0 {
		t.Fatalf("filtered=%+v", out)
	}
	out = FilterRepos(repos, []string{"a"})
	if len(out) != 1 || out[0].ID != "b" {
		t.Fatalf("filtered=%+v", out)
	}
}
