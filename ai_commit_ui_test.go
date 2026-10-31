package main

import (
	"os"
	"path/filepath"
	"runtime"
	"strings"
	"testing"

	"github.com/egoist/mygo/ui"
)

func TestAICommitMessageButton(t *testing.T) {
	if runtime.GOOS == "windows" {
		t.Skip("shell fake agent is Unix-only")
	}
	ws := makeP0Repo(t, 1)
	if err := os.WriteFile(filepath.Join(ws, "f.txt"), []byte("line\nnew value\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	agent := filepath.Join(t.TempDir(), "agent.sh")
	if err := os.WriteFile(agent, []byte("#!/bin/sh\nprintf 'feat: generated from ui\\n\\nbody\\n'\n"), 0o755); err != nil {
		t.Fatal(err)
	}
	app := newGapTestApp(t, ws)
	app.aiAgentName = "omp"
	app.aiAgentBin = agent
	app.checkedChanges = map[string]bool{}
	for _, f := range app.statuses[0].Files {
		app.checkedChanges[f.Key()] = true
	}
	app.activeTab = TabChanges
	tt := ui.NewTester(app.BuildUI, 1440, 900)
	tt.Frame()
	if !tt.HasText("AI 生成") {
		t.Fatal("AI generate button missing")
	}
	if err := tt.Click("AI 生成"); err != nil {
		t.Fatalf("click AI generate: %v", err)
	}
	waitApp(t, "AI commit message", func() bool {
		return strings.Contains(app.commitMessage, "generated from ui")
	})
}
