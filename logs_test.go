package main

import (
	"strings"
	"testing"
	"time"

	"github.com/iwangjie/nemorous/internal/config"
	"github.com/iwangjie/nemorous/internal/gitops"
)

func TestLogsSeparateCommandsAndRuntime(t *testing.T) {
	app := NewApp(config.Default())
	app.clearLog("command")
	app.clearLog("runtime")

	app.logCommand("RUN", "正在拉取更新...", "")
	app.setBatchResults([]gitops.OpResult{
		{RepoName: "api", Status: "OK", Message: "拉取成功", Duration: 12 * time.Millisecond},
		{RepoName: "web", Status: "FAIL", Message: "网络错误", Duration: 8 * time.Millisecond},
	})
	app.showToast("err", "拉取失败")

	commands := app.logEntries("command")
	if len(commands) != 3 {
		t.Fatalf("expected RUN plus two result rows, got %+v", commands)
	}
	if commands[0].Level != logRun || !strings.Contains(commands[1].Message, "api") || commands[2].Level != logFail {
		t.Fatalf("unexpected command log: %+v", commands)
	}
	runtime := app.logEntries("runtime")
	if len(runtime) != 1 || runtime[0].Level != logError || runtime[0].Message != "拉取失败" {
		t.Fatalf("unexpected runtime log: %+v", runtime)
	}

	app.clearLog("command")
	if len(app.logEntries("command")) != 0 || len(app.logEntries("runtime")) != 1 {
		t.Fatal("clear must only affect the selected tab")
	}
}

func TestRuntimeLogDeduplicatesRepeatedErrors(t *testing.T) {
	app := NewApp(config.Default())
	app.clearLog("runtime")
	app.logRuntime("err", "仓库读取失败")
	app.logRuntime("err", "仓库读取失败")
	if got := app.logEntries("runtime"); len(got) != 1 {
		t.Fatalf("expected deduplicated runtime log, got %+v", got)
	}
}
