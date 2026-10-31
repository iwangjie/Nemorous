package main

import (
	"context"
	"path/filepath"
	"testing"

	"github.com/iwangjie/nemorous/internal/plugins"
)

func TestJenkinsAndKubernetesPluginsRenderSettings(t *testing.T) {
	t.Setenv("NEMOROUS_CONFIG", filepath.Join(t.TempDir(), "config.json"))
	app := newGapTestApp(t, t.TempDir())
	app.initPluginManager()
	t.Cleanup(app.closePluginManager)
	app.syncPluginsWorkspace(app.reposSnapshot())
	cases := []struct{ id, ref, text string }{
		{"jenkins", "jenkins:jenkins", "Jenkins 连接"},
		{"k8s", "k8s:k8s", "Kubernetes 连接"},
	}
	for _, tc := range cases {
		if _, err := app.pluginMgr.InstallBuiltin(context.Background(), tc.id); err != nil {
			t.Fatal(err)
		}
		app.openPluginPanel(tc.ref)
		waitPluginSnapshot(t, app, tc.ref, func(s *plugins.PanelSnapshot) bool {
			return s.Root != nil && nodeContainsText(s.Root, tc.text)
		})
	}
}
