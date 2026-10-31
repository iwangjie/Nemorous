package commitmsg

import (
	"context"
	"os"
	"path/filepath"
	"runtime"
	"strings"
	"testing"
)

func TestCleanCommitMessage(t *testing.T) {
	raw := "\x1b[32mHere is the commit message:\x1b[0m\n```text\nfeat: add workspace onboarding\n\n- clone and init\n- import existing repos\n```\n"
	got := Clean(raw)
	want := "feat: add workspace onboarding\n\n- clone and init\n- import existing repos"
	if got != want {
		t.Fatalf("Clean=%q want %q", got, want)
	}
}

func TestBuildPromptIncludesChangesAndStyle(t *testing.T) {
	p := buildPrompt(Request{Branch: "feature", Subjects: []string{"fix: old bug"}, Changes: []Change{{Repo: "api", Path: "main.go", Status: "M", Patch: "+hello"}}})
	for _, want := range []string{"feature", "fix: old bug", "api: main.go", "+hello", "ONLY the commit message"} {
		if !strings.Contains(p, want) {
			t.Fatalf("prompt missing %q:\n%s", want, p)
		}
	}
}

func TestGenerateWithFakeAgent(t *testing.T) {
	if runtime.GOOS == "windows" {
		t.Skip("shell fake agent is Unix-only")
	}
	dir := t.TempDir()
	agent := filepath.Join(dir, "agent.sh")
	if err := os.WriteFile(agent, []byte("#!/bin/sh\nprintf 'feat: generated message\\n\\nbody\\n'\n"), 0o755); err != nil {
		t.Fatal(err)
	}
	got, err := Generate(context.Background(), "omp", agent, Request{Branch: "main", Changes: []Change{{Repo: "demo", Path: "a.go", Status: "M", Patch: "+x"}}})
	if err != nil {
		t.Fatal(err)
	}
	if got != "feat: generated message\n\nbody" {
		t.Fatalf("generated=%q", got)
	}
}
