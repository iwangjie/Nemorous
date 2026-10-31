package main

import (
	"context"
	"os"
	"path/filepath"
	"testing"
	"time"

	"github.com/egoist/mygo/ui"
	"github.com/iwangjie/nemorous/internal/plugins"
)

func TestJenkinsPluginEndToEnd(t *testing.T) {
	if os.Getenv("JENKINS_E2E") != "1" {
		t.Skip("set JENKINS_E2E=1 with JENKINS_USERNAME/JENKINS_PASSWORD")
	}
	user := os.Getenv("JENKINS_USERNAME")
	pass := os.Getenv("JENKINS_PASSWORD")
	base := os.Getenv("JENKINS_URL")
	if base == "" {
		base = "http://localhost:8080/"
	}
	if user == "" || pass == "" {
		t.Skip("Jenkins credentials not set")
	}
	t.Setenv("NEMOROUS_CONFIG", filepath.Join(t.TempDir(), "config.json"))
	app := newGapTestApp(t, t.TempDir())
	app.initPluginManager()
	t.Cleanup(app.closePluginManager)
	app.syncPluginsWorkspace(app.reposSnapshot())
	if _, err := app.pluginMgr.InstallBuiltin(context.Background(), "jenkins"); err != nil {
		t.Fatal(err)
	}
	ref := "jenkins:jenkins"
	app.openPluginPanel(ref)
	waitPluginSnapshot(t, app, ref, func(s *plugins.PanelSnapshot) bool { return s.Root != nil })
	for key, value := range map[string]any{"baseUrl": base, "username": user, "password": pass} {
		app.pluginMgr.Dispatch(ref, "setting", map[string]any{"key": key, "value": value})
	}
	app.pluginMgr.Dispatch(ref, "saveConnect", nil)
	waitPluginSnapshotTimeout(t, app, ref, 30, func(s *plugins.PanelSnapshot) bool {
		return nodeContainsText(s.Root, "已连接") && nodeContainsText(s.Root, "test")
	})
	app.pluginMgr.Dispatch(ref, "selectJob", map[string]any{"url": base + "job/dosrouter/"})
	waitPluginSnapshotTimeout(t, app, ref, 30, func(s *plugins.PanelSnapshot) bool {
		return nodeContainsText(s.Root, "构建参数") && nodeContainsText(s.Root, "NAMESPACE")
	})
	app.pluginMgr.Dispatch(ref, "selectJob", map[string]any{"url": base + "job/test/"})
	waitPluginSnapshotTimeout(t, app, ref, 30, func(s *plugins.PanelSnapshot) bool {
		return nodeContainsText(s.Root, "构建历史") && nodeContainsText(s.Root, "最近")
	})
	app.pluginMgr.Dispatch(ref, "build", nil)
	waitPluginSnapshotTimeout(t, app, ref, 90, func(s *plugins.PanelSnapshot) bool {
		return nodeContainsText(s.Root, "构建 #") && (nodeContainsText(s.Root, "成功") || nodeContainsText(s.Root, "失败"))
	})
	tt := ui.NewTester(app.BuildUI, 1440, 900)
	tt.Frame()
	if !tt.HasText("Jenkins") || !tt.HasText("构建历史") {
		t.Fatal("Jenkins panel did not render")
	}
}

func waitPluginSnapshotTimeout(t *testing.T, app *App, ref string, seconds int, cond func(*plugins.PanelSnapshot) bool) {
	t.Helper()
	deadline := time.Now().Add(time.Duration(seconds) * time.Second)
	for time.Now().Before(deadline) {
		if s := app.pluginMgr.Snapshot(ref); s != nil && cond(s) {
			return
		}
		time.Sleep(25 * time.Millisecond)
	}
	t.Fatalf("timed out waiting for %s", ref)
}
