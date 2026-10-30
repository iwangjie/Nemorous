package main

import (
	"bufio"
	"context"
	"encoding/json"
	"fmt"
	"io"
	"os"
	"path/filepath"
	"strconv"
	"strings"
	"testing"

	"github.com/egoist/mygo/ui"
	"github.com/iwangjie/nemorous/internal/config"
	"github.com/iwangjie/nemorous/internal/gitops"
)

func TestRunPanelRunsConfiguration(t *testing.T) {
	ws := setupUITestWorkspace(t)
	cfg := config.Default()
	cfg.WorkspacePath = ws
	cfg.IncludeRootRepo = false
	cfg.RunConfigs = []config.RunConfig{{
		ID: "echo", Name: "GOROOT", Kind: "run", Command: "go", Args: "env GOROOT", Cwd: ".",
	}}
	t.Setenv("NEMOROUS_CONFIG", filepath.Join(t.TempDir(), "config.json"))
	app := NewApp(cfg)
	app.LoadWorkspaceSync(ws)
	app.runConfigID = "echo"
	app.showRunPanel = true
	app.launchSelectedConfig()
	waitApp(t, "run finish", func() bool { return !app.runRunning && app.runStatus != "" })
	if app.runExit != 0 || app.runStatus != "成功" {
		t.Fatalf("run result = exit %d status %q output %v", app.runExit, app.runStatus, app.runOutput)
	}
	if len(app.runOutput) == 0 {
		t.Fatal("run produced no output")
	}
	tt := ui.NewTester(app.BuildUI, 1440, 900)
	tt.Frame()
	if !tt.HasText("运行 / 调试 / 测试") || !tt.HasText("成功") {
		t.Fatal("run panel did not render result")
	}
}

func TestGitHubModalShowsPRChecksAndComments(t *testing.T) {
	ws := setupUITestWorkspace(t)
	repoPath := filepath.Join(ws, "cloud-caring")
	if _, err := gitops.RunGit(context.Background(), repoPath, "remote", "add", "origin", "git@github.com:acme/app.git"); err != nil {
		t.Fatal(err)
	}
	bin := t.TempDir()
	script := `#!/bin/sh
case "$1 $2" in
  "pr list") echo '[{"number":7,"title":"Fix it","state":"OPEN","url":"https://github.com/acme/app/pull/7","headRefName":"fix","baseRefName":"main","isDraft":false,"updatedAt":"2026-01-01T00:00:00Z","reviewDecision":"APPROVED","author":{"login":"alice"}}]' ;;
  "pr view") echo '{"number":7,"title":"Fix it","state":"OPEN","url":"https://github.com/acme/app/pull/7","headRefName":"fix","baseRefName":"main","body":"Body","author":{"login":"alice"},"statusCheckRollup":[{"name":"build","status":"COMPLETED","conclusion":"SUCCESS"}],"comments":[{"author":{"login":"bob"},"body":"LGTM?","createdAt":"2026-01-01T00:00:00Z"}],"reviews":[]}' ;;
  "pr comment") ;;
  "api --paginate") echo '[{"user":{"login":"carol"},"body":"inline note","created_at":"2026-01-01T00:00:00Z","path":"main.go","line":4}]' ;;
esac
`
	if err := os.WriteFile(filepath.Join(bin, "gh"), []byte(script), 0o755); err != nil {
		t.Fatal(err)
	}
	t.Setenv("PATH", bin+string(os.PathListSeparator)+os.Getenv("PATH"))
	app := newGapTestApp(t, ws)
	app.githubRepo = "cloud-caring"
	app.openGitHub()
	waitApp(t, "github prs", func() bool { return len(app.githubPRs) > 0 })
	waitApp(t, "github details", func() bool { return app.githubDetails != nil })
	tt := ui.NewTester(app.BuildUI, 1440, 900)
	tt.Frame()
	if !tt.HasText("GitHub PR / CI") || !tt.HasText("Fix it") || !tt.HasText("build") || !tt.HasText("LGTM?") || !tt.HasText("inline note") {
		t.Fatal("GitHub modal did not render PR, checks and review comments")
	}
	app.githubComment = "hello"
	app.postGitHubComment()
	waitApp(t, "comment posted", func() bool { return app.githubDetails != nil })
}

// TestMainDAPHelper speaks a minimal DAP session over stdio.
func TestMainDAPHelper(t *testing.T) {
	if os.Getenv("NEMOROUS_MAIN_DAP_HELPER") != "1" {
		return
	}
	logPath := os.Getenv("NEMOROUS_MAIN_DAP_LOG")
	logf, _ := os.OpenFile(logPath, os.O_CREATE|os.O_APPEND|os.O_WRONLY, 0o644)
	reader := bufio.NewReader(os.Stdin)
	write := func(v any) {
		data, _ := json.Marshal(v)
		fmt.Fprintf(os.Stdout, "Content-Length: %d\r\n\r\n%s", len(data), data)
	}
	seq := 0
	sendEvent := func(name string, body any) {
		seq++
		write(map[string]any{"seq": seq, "type": "event", "event": name, "body": body})
	}
	for {
		length := -1
		for {
			line, err := reader.ReadString('\n')
			if err != nil {
				os.Exit(0)
			}
			line = strings.TrimRight(line, "\r\n")
			if line == "" {
				break
			}
			if strings.HasPrefix(strings.ToLower(line), "content-length:") {
				length, _ = strconv.Atoi(strings.TrimSpace(line[len("content-length:"):]))
			}
		}
		if length < 0 {
			os.Exit(1)
		}
		data := make([]byte, length)
		if _, err := io.ReadFull(reader, data); err != nil {
			os.Exit(0)
		}
		var req struct {
			Seq     int    `json:"seq"`
			Command string `json:"command"`
		}
		_ = json.Unmarshal(data, &req)
		if logf != nil {
			fmt.Fprintf(logf, "%s\n", req.Command)
		}
		seq++
		resp := map[string]any{"seq": seq, "type": "response", "request_seq": req.Seq, "command": req.Command, "success": true}
		switch req.Command {
		case "initialize":
			resp["body"] = map[string]any{"supportsConfigurationDoneRequest": true}
		case "configurationDone":
			write(resp)
			sendEvent("stopped", map[string]any{"threadId": 1, "reason": "breakpoint"})
			continue
		case "threads":
			resp["body"] = map[string]any{"threads": []map[string]any{{"id": 1, "name": "main"}}}
		case "stackTrace":
			resp["body"] = map[string]any{"stackFrames": []map[string]any{{"id": 100, "name": "main", "line": 12, "source": map[string]any{"path": "/tmp/main.go"}}}}
		case "scopes":
			resp["body"] = map[string]any{"scopes": []map[string]any{{"name": "Locals", "variablesReference": 1}}}
		case "variables":
			resp["body"] = map[string]any{"variables": []map[string]any{{"name": "x", "value": "42", "variablesReference": 0}}}
		case "continue":
			write(resp)
			sendEvent("continued", map[string]any{"threadId": 1})
			sendEvent("terminated", map[string]any{})
			continue
		case "disconnect":
			write(resp)
			os.Exit(0)
		}
		write(resp)
	}
}

func TestDAPDebugPanel(t *testing.T) {
	ws := setupUITestWorkspace(t)
	dapLog := filepath.Join(t.TempDir(), "dap.log")
	cfg := config.Default()
	cfg.WorkspacePath = ws
	cfg.IncludeRootRepo = false
	cfg.RunConfigs = []config.RunConfig{{
		ID: "dbg", Name: "Mock Debug", Kind: "debug",
		Command: "go", Adapter: os.Args[0],
		AdapterArgs: "-test.run=TestMainDAPHelper",
		Env:         map[string]string{"NEMOROUS_MAIN_DAP_HELPER": "1", "NEMOROUS_MAIN_DAP_LOG": dapLog},
		Program:     ".",
	}}
	t.Setenv("NEMOROUS_CONFIG", filepath.Join(t.TempDir(), "config.json"))
	app := NewApp(cfg)
	app.LoadWorkspaceSync(ws)
	app.runConfigID = "dbg"
	app.showRunPanel = true
	app.breakpoints["cloud-caring::main.go"] = []int{3}
	app.startDebug(cfg.RunConfigs[0])
	waitApp(t, "debug stopped", func() bool {
		return app.debugClient != nil && app.debugStopped && len(app.debugFrames) > 0
	})
	if app.debugFrames[0].Line != 12 {
		t.Fatalf("frame = %+v", app.debugFrames[0])
	}
	waitApp(t, "debug variables", func() bool { return len(app.debugVars) > 0 })
	data, _ := os.ReadFile(dapLog)
	if !strings.Contains(string(data), "setBreakpoints") {
		t.Fatalf("breakpoints were not sent: %s", data)
	}
	tt := ui.NewTester(app.BuildUI, 1440, 900)
	tt.Frame()
	if !tt.HasText("调用栈") || !tt.HasText("变量") || !tt.HasText("main") {
		t.Fatal("debug panel did not render stack and variables")
	}
	app.debugCommand("continue")
	waitApp(t, "debug terminated", func() bool { return app.runStatus == "调试结束" })
	app.stopDebug()
}
