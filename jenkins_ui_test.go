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
		return nodeContainsText(s.Root, "在线") && nodeContainsText(s.Root, "test")
	})
	app.pluginMgr.Dispatch(ref, "selectJob", map[string]any{"url": base + "job/dosrouter/"})
	waitPluginSnapshotTimeout(t, app, ref, 30, func(s *plugins.PanelSnapshot) bool {
		return nodeContainsText(s.Root, "/job/dosrouter/") && nodeContainsText(s.Root, "构建参数") && nodeContainsText(s.Root, "NAMESPACE")
	})
	app.pluginMgr.Dispatch(ref, "selectJob", map[string]any{"url": base + "job/test/"})
	waitPluginSnapshotTimeout(t, app, ref, 30, func(s *plugins.PanelSnapshot) bool {
		return nodeContainsText(s.Root, "/job/test/") && nodeContainsText(s.Root, "最近构建") && nodeContainsText(s.Root, "查看构建")
	})
	app.pluginMgr.Dispatch(ref, "build", nil)
	waitPluginSnapshotTimeout(t, app, ref, 90, func(s *plugins.PanelSnapshot) bool {
		return nodeContainsText(s.Root, "构建历史") && nodeContainsText(s.Root, "控制台输出") && (nodeContainsText(s.Root, "成功") || nodeContainsText(s.Root, "失败"))
	})
	if s := app.pluginMgr.Snapshot(ref); s != nil {
		t.Logf("ERR=%q", s.Err)
		dumpJenkinsText(t, s.Root, 0)
	}
	tt := ui.NewTester(app.BuildUI, 1440, 900)
	tt.Frame()
	tt.Frame()
	if !tt.HasText("Jenkins") || !tt.HasText("构建历史") {
		t.Fatal("Jenkins panel did not render")
	}
}

func dumpJenkinsText(t *testing.T, n *plugins.Node, depth int) {
	if n == nil || depth > 4 {
		return
	}
	if n.Text != "" {
		t.Logf("%*s%s %q", depth*2, "", n.Type, n.Text)
	}
	for _, it := range n.Items {
		t.Logf("%*sitem %q %q", (depth+1)*2, "", it.Text, it.Detail)
	}
	for _, c := range n.Children {
		dumpJenkinsText(t, c, depth+1)
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
	if snap := app.pluginMgr.Snapshot(ref); snap != nil {
		t.Logf("snapshot error: %q", snap.Err)
		dumpJenkinsText(t, snap.Root, 0)
	}
	t.Fatalf("timed out waiting for %s", ref)
}
