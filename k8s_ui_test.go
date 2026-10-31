package main

import (
	"context"
	"os"
	"path/filepath"
	"strings"
	"testing"

	"github.com/egoist/mygo/ui"
	"github.com/iwangjie/nemorous/internal/plugins"
)

func TestKubernetesPluginEndToEnd(t *testing.T) {
	if os.Getenv("K8S_E2E") != "1" {
		t.Skip("set K8S_E2E=1 with TEST_K8S_TOKEN")
	}
	token := os.Getenv("TEST_K8S_TOKEN")
	if token == "" {
		t.Skip("TEST_K8S_TOKEN not set")
	}
	base := os.Getenv("K8S_URL")
	if base == "" {
		base = "https://localhost:6443"
	}
	t.Setenv("NEMOROUS_CONFIG", filepath.Join(t.TempDir(), "config.json"))
	app := newGapTestApp(t, t.TempDir())
	app.initPluginManager()
	t.Cleanup(app.closePluginManager)
	app.syncPluginsWorkspace(app.reposSnapshot())
	if _, err := app.pluginMgr.InstallBuiltin(context.Background(), "k8s"); err != nil {
		t.Fatal(err)
	}
	ref := "k8s:k8s"
	app.openPluginPanel(ref)
	waitPluginSnapshot(t, app, ref, func(s *plugins.PanelSnapshot) bool { return s.Root != nil })
	for key, value := range map[string]any{"baseUrl": base, "token": token, "namespace": "test"} {
		app.pluginMgr.Dispatch(ref, "setting", map[string]any{"key": key, "value": value})
	}
	app.pluginMgr.Dispatch(ref, "settingBool", map[string]any{"key": "autoRefresh", "checked": false})
	app.pluginMgr.Dispatch(ref, "saveConnect", nil)
	waitPluginSnapshotTimeout(t, app, ref, 40, func(s *plugins.PanelSnapshot) bool {
		return nodeContainsText(s.Root, "在线") && nodeContainsText(s.Root, "test")
	})
	app.pluginMgr.Dispatch(ref, "selectNS", map[string]any{"value": "test"})
	waitPluginSnapshotTimeout(t, app, ref, 40, func(s *plugins.PanelSnapshot) bool {
		return nodeContainsText(s.Root, "agent-api") && nodeContainsText(s.Root, "工作负载")
	})
	app.pluginMgr.Dispatch(ref, "tab", map[string]any{"value": "pods"})
	waitPluginSnapshotTimeout(t, app, ref, 20, func(s *plugins.PanelSnapshot) bool {
		return nodeContainsText(s.Root, "agent-api") && nodeContainsText(s.Root, "Pods")
	})
	// Pick the first pod from the rendered list and load its logs.
	var podName string
	var walk func(*plugins.Node)
	walk = func(n *plugins.Node) {
		if n == nil || podName != "" {
			return
		}
		for _, it := range n.Items {
			if strings.HasPrefix(it.Text, "agent-api-") {
				if p, ok := it.Payload.(map[string]any); ok {
					if v, ok := p["name"].(string); ok {
						podName = v
						return
					}
				}
			}
		}
		for _, c := range n.Children {
			walk(c)
		}
	}
	walk(app.pluginMgr.Snapshot(ref).Root)
	if podName == "" {
		t.Fatal("no pod found in Kubernetes list")
	}
	app.pluginMgr.Dispatch(ref, "selectPod", map[string]any{"name": podName, "namespace": "test"})
	waitPluginSnapshotTimeout(t, app, ref, 30, func(s *plugins.PanelSnapshot) bool {
		return nodeContainsText(s.Root, "容器日志")
	})
	app.pluginMgr.Dispatch(ref, "tab", map[string]any{"value": "workloads"})
	waitPluginSnapshotTimeout(t, app, ref, 20, func(s *plugins.PanelSnapshot) bool {
		return nodeContainsText(s.Root, "Deployment /") && nodeContainsText(s.Root, "工作负载")
	})
	app.pluginMgr.Dispatch(ref, "tab", map[string]any{"value": "network"})
	waitPluginSnapshotTimeout(t, app, ref, 20, func(s *plugins.PanelSnapshot) bool {
		return nodeContainsText(s.Root, "Services") && nodeContainsText(s.Root, "Ingress")
	})
	app.pluginMgr.Dispatch(ref, "tab", map[string]any{"value": "events"})
	waitPluginSnapshotTimeout(t, app, ref, 20, func(s *plugins.PanelSnapshot) bool {
		return nodeContainsText(s.Root, "事件")
	})
	tt := ui.NewTester(app.BuildUI, 1440, 900)
	tt.Frame()
	if !tt.HasText("Kubernetes") || !tt.HasText("事件") {
		t.Fatal("Kubernetes panel did not render")
	}
}
