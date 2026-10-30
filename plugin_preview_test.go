package main

import (
	"context"
	"net/http"
	"net/http/httptest"
	"os"
	"path/filepath"
	"strings"
	"testing"
	"time"

	"github.com/egoist/mygo/ui"
	"github.com/iwangjie/nemorous/internal/config"
	"github.com/iwangjie/nemorous/internal/plugins"
)

func nodeHasType(n *plugins.Node, typ string) bool {
	if n == nil {
		return false
	}
	if n.Type == typ {
		return true
	}
	for _, child := range n.Children {
		if nodeHasType(child, typ) {
			return true
		}
	}
	return false
}

func nodeContainsText(n *plugins.Node, text string) bool {
	if n == nil {
		return false
	}
	if strings.Contains(n.Text, text) || strings.Contains(n.Detail, text) {
		return true
	}
	for _, item := range n.Items {
		if strings.Contains(item.Text, text) || strings.Contains(item.Detail, text) {
			return true
		}
	}
	for _, cell := range n.Headers {
		if strings.Contains(cell, text) {
			return true
		}
	}
	for _, row := range n.Rows {
		for _, cell := range row {
			if strings.Contains(cell, text) {
				return true
			}
		}
	}
	for _, child := range n.Children {
		if nodeContainsText(child, text) {
			return true
		}
	}
	return false
}

func TestOfficialPreviewPluginsRender(t *testing.T) {
	t.Setenv("NEMOROUS_CONFIG", filepath.Join(t.TempDir(), "config.json"))
	ws := setupUITestWorkspace(t)
	_ = os.WriteFile(filepath.Join(ws, "cloud-caring", "README.md"), []byte("# Payments API\n\nA short **Markdown** preview.\n\n```go\nfunc main() {}\n```\n"), 0o644)
	_ = os.WriteFile(filepath.Join(ws, "cloud-caring", "data.csv"), []byte("name,amount\nAlice,42\nBob,7\n"), 0o644)
	cfg := config.Default()
	cfg.WorkspacePath = ws
	cfg.IncludeRootRepo = false
	app := NewApp(cfg)
	app.LoadWorkspaceSync(ws)
	app.initPluginManager()
	if app.pluginMgr == nil {
		t.Fatal("plugin manager not initialized")
	}
	t.Cleanup(app.closePluginManager)
	app.syncPluginsWorkspace(app.reposSnapshot())
	for _, id := range []string{"markdown", "csv", "gitlab", "code-stats", "ssh"} {
		if _, err := app.pluginMgr.InstallBuiltin(context.Background(), id); err != nil {
			t.Fatalf("install %s: %v", id, err)
		}
	}
	md, ok := app.workspaceFileByKey("cloud-caring", "README.md")
	if !ok {
		t.Fatal("README.md missing")
	}
	app.selectCodeFile(md)
	app.activeTab = TabCode
	tt := ui.NewTester(app.BuildUI, 1440, 900)
	tt.Frame()
	time.Sleep(300 * time.Millisecond)
	waitPluginSnapshot(t, app, "markdown:preview", func(s *plugins.PanelSnapshot) bool {
		return s.Err == "" && nodeHasType(s.Root, "markdown") && nodeContainsText(s.Root, "Payments")
	})
	tt.Frame()
	if !tt.HasText("Payments") {
		t.Fatal("Markdown editor view did not render")
	}

	csv, ok := app.workspaceFileByKey("cloud-caring", "data.csv")
	if !ok {
		t.Fatal("data.csv missing")
	}
	app.selectCodeFile(csv)
	app.activeTab = TabCode
	tt.Frame()
	waitPluginSnapshot(t, app, "csv:preview", func(s *plugins.PanelSnapshot) bool {
		return s.Err == "" && nodeHasType(s.Root, "table") && nodeContainsText(s.Root, "Alice")
	})
	tt.Frame()
	if !tt.HasText("Alice") {
		t.Fatal("CSV editor view did not render")
	}

	for _, tc := range []struct{ ref, node, text string }{
		{"gitlab:gitlab", "input", "Token"},
		{"code-stats:code-stats", "bar", "Go"},
		{"ssh:ssh", "input", "主机"},
	} {
		app.openPluginPanel(tc.ref)
		waitPluginSnapshot(t, app, tc.ref, func(s *plugins.PanelSnapshot) bool {
			return s.Err == "" && (nodeHasType(s.Root, tc.node) || nodeContainsText(s.Root, tc.text))
		})
		tt.Frame()
		if !tt.HasText(tc.text) {
			t.Fatalf("%s did not render %q", tc.ref, tc.text)
		}
	}
}

func TestGitLabPluginPrivateServerFlow(t *testing.T) {
	t.Setenv("NEMOROUS_CONFIG", filepath.Join(t.TempDir(), "config.json"))
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if r.Header.Get("PRIVATE-TOKEN") != "tok" {
			http.Error(w, `{"message":"unauthorized"}`, http.StatusUnauthorized)
			return
		}
		w.Header().Set("Content-Type", "application/json")
		if strings.HasSuffix(r.URL.Path, "/approve") {
			_, _ = w.Write([]byte(`{}`))
			return
		}
		if strings.HasSuffix(r.URL.Path, "/notes") {
			if r.Method == http.MethodPost {
				_, _ = w.Write([]byte(`{}`))
				return
			}
			_, _ = w.Write([]byte(`[{"author":{"username":"bob"},"body":"LGTM","created_at":"2026-10-30T08:00:00Z"}]`))
			return
		}
		if strings.HasSuffix(r.URL.Path, "/merge_requests") {
			_, _ = w.Write([]byte(`[{"id":11,"iid":7,"title":"Fix retry","state":"opened","source_branch":"fix/retry","target_branch":"main","web_url":"https://gitlab.example.com/group/project/-/merge_requests/7","author":{"username":"alice"}}]`))
			return
		}
		http.NotFound(w, r)
	}))
	defer server.Close()
	ws := setupUITestWorkspace(t)
	cfg := config.Default()
	cfg.WorkspacePath = ws
	cfg.IncludeRootRepo = false
	app := NewApp(cfg)
	app.LoadWorkspaceSync(ws)
	app.initPluginManager()
	t.Cleanup(app.closePluginManager)
	app.syncPluginsWorkspace(app.reposSnapshot())
	if _, err := app.pluginMgr.InstallBuiltin(context.Background(), "gitlab"); err != nil {
		t.Fatal(err)
	}
	ref := "gitlab:gitlab"
	app.openPluginPanel(ref)
	waitPluginSnapshot(t, app, ref, func(s *plugins.PanelSnapshot) bool { return s.Root != nil })
	for key, value := range map[string]any{
		"gl-url:0": server.URL, "gl-token:0": "tok", "gl-project:0": "group/project",
	} {
		app.pluginMgr.Dispatch(ref, "setting", map[string]any{"key": key, "value": value})
	}
	app.pluginMgr.Dispatch(ref, "save-settings", nil)
	waitPluginSnapshot(t, app, ref, func(s *plugins.PanelSnapshot) bool { return nodeContainsText(s.Root, "Fix retry") })
	tt := ui.NewTester(app.BuildUI, 1440, 900)
	tt.Frame()
	if err := tt.Click("!7 Fix retry"); err != nil {
		t.Fatalf("select MR: %v", err)
	}
	waitPluginSnapshot(t, app, ref, func(s *plugins.PanelSnapshot) bool { return nodeContainsText(s.Root, "LGTM") })
	app.pluginMgr.Dispatch(ref, "approve", nil)
	waitPluginSnapshot(t, app, ref, func(s *plugins.PanelSnapshot) bool { return nodeContainsText(s.Root, "已批准") })
	app.pluginMgr.Dispatch(ref, "comment-input", map[string]any{"key": "gl-comment", "value": "Looks good"})
	app.pluginMgr.Dispatch(ref, "comment", nil)
	waitPluginSnapshot(t, app, ref, func(s *plugins.PanelSnapshot) bool { return nodeContainsText(s.Root, "评论已发送") })
}

func TestMarkdownPluginLivePreviewAndSave(t *testing.T) {
	t.Setenv("NEMOROUS_CONFIG", filepath.Join(t.TempDir(), "config.json"))
	ws := setupUITestWorkspace(t)
	path := filepath.Join(ws, "cloud-caring", "README.md")
	_ = os.WriteFile(path, []byte("# Draft\n\nold text\n"), 0o644)
	cfg := config.Default()
	cfg.WorkspacePath = ws
	cfg.IncludeRootRepo = false
	app := NewApp(cfg)
	app.LoadWorkspaceSync(ws)
	app.initPluginManager()
	t.Cleanup(app.closePluginManager)
	app.syncPluginsWorkspace(app.reposSnapshot())
	if _, err := app.pluginMgr.InstallBuiltin(context.Background(), "markdown"); err != nil {
		t.Fatal(err)
	}
	wf, ok := app.workspaceFileByKey("cloud-caring", "README.md")
	if !ok {
		t.Fatal("README.md missing")
	}
	app.selectCodeFile(wf)
	app.activeTab = TabCode
	tt := ui.NewTester(app.BuildUI, 1440, 900)
	tt.Frame()
	ref := "markdown:preview"
	waitPluginSnapshot(t, app, ref, func(s *plugins.PanelSnapshot) bool { return nodeContainsText(s.Root, "old text") })
	changed := false
	for _, rec := range app.pluginCodeEditors {
		rec.state.Buffer.Set("# Draft\n\nnew **live** text\n")
		changed = true
	}
	if !changed {
		t.Fatal("Markdown code editor was not created")
	}
	tt.Frame()
	waitPluginSnapshot(t, app, ref, func(s *plugins.PanelSnapshot) bool { return nodeContainsText(s.Root, "live") })
	app.pluginMgr.Dispatch(ref, "save", nil)
	deadline := time.Now().Add(5 * time.Second)
	for time.Now().Before(deadline) {
		data, _ := os.ReadFile(path)
		if strings.Contains(string(data), "new **live** text") {
			return
		}
		time.Sleep(20 * time.Millisecond)
	}
	t.Fatal("markdown save did not reach disk")
}
