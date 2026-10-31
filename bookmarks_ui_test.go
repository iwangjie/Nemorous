package main

import (
	"context"
	"encoding/base64"
	"net/http"
	"net/http/httptest"
	"path/filepath"
	"strings"
	"testing"

	"github.com/iwangjie/nemorous/internal/plugins"
)

func TestBookmarksPluginAddFetchSearchAndCategory(t *testing.T) {
	png, _ := base64.StdEncoding.DecodeString("iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mNk+A8AAQUBAScY42YAAAAASUVORK5CYII=")
	mux := http.NewServeMux()
	mux.HandleFunc("/", func(w http.ResponseWriter, r *http.Request) {
		w.Header().Set("Content-Type", "text/html")
		_, _ = w.Write([]byte(`<html><head><link rel="icon" href="/icon.png"></head><body>demo</body></html>`))
	})
	mux.HandleFunc("/icon.png", func(w http.ResponseWriter, r *http.Request) {
		w.Header().Set("Content-Type", "image/png")
		_, _ = w.Write(png)
	})
	srv := httptest.NewServer(mux)
	defer srv.Close()

	t.Setenv("NEMOROUS_CONFIG", filepath.Join(t.TempDir(), "config.json"))
	app := newGapTestApp(t, t.TempDir())
	app.initPluginManager()
	t.Cleanup(app.closePluginManager)
	app.syncPluginsWorkspace(app.reposSnapshot())
	if _, err := app.pluginMgr.InstallBuiltin(context.Background(), "bookmarks"); err != nil {
		t.Fatal(err)
	}
	ref := "bookmarks:bookmarks"
	app.openPluginPanel(ref)
	waitPluginSnapshot(t, app, ref, func(s *plugins.PanelSnapshot) bool { return s.Root != nil })
	app.pluginMgr.Dispatch(ref, "addBookmark", nil)
	for key, value := range map[string]string{
		"bm-title:1": "Example", "bm-url:1": srv.URL, "bm-category:1": "Test", "bm-desc:1": "A demo bookmark",
	} {
		app.pluginMgr.Dispatch(ref, "field", map[string]any{"key": key, "value": value})
	}
	app.pluginMgr.Dispatch(ref, "saveBookmark", nil)
	waitPluginSnapshot(t, app, ref, func(s *plugins.PanelSnapshot) bool {
		return nodeContainsText(s.Root, "Example") && nodeContainsText(s.Root, "Test")
	})
	waitPluginSnapshotTimeout(t, app, ref, 10, func(s *plugins.PanelSnapshot) bool {
		return hasImageSrc(s.Root, "data:image/png;base64,")
	})
	app.pluginMgr.Dispatch(ref, "refreshIcons", nil)
	waitPluginSnapshotTimeout(t, app, ref, 10, func(s *plugins.PanelSnapshot) bool {
		return nodeContainsText(s.Root, "图标刷新完成")
	})
	app.pluginMgr.Dispatch(ref, "filter", map[string]any{"value": "demo bookmark"})
	waitPluginSnapshot(t, app, ref, func(s *plugins.PanelSnapshot) bool { return nodeContainsText(s.Root, "Example") })
	app.pluginMgr.Dispatch(ref, "category", map[string]any{"value": "Test"})
	waitPluginSnapshot(t, app, ref, func(s *plugins.PanelSnapshot) bool { return nodeContainsText(s.Root, "Example") })

	// Reopen the plugin runtime from the same store: the bookmark must survive.
	app.closePluginManager()
	app.initPluginManager()
	app.syncPluginsWorkspace(app.reposSnapshot())
	app.openPluginPanel(ref)
	waitPluginSnapshot(t, app, ref, func(s *plugins.PanelSnapshot) bool { return nodeContainsText(s.Root, "Example") })
}

func hasImageSrc(n *plugins.Node, prefix string) bool {
	if n == nil {
		return false
	}
	if n.Type == "image" && strings.HasPrefix(n.Src, prefix) {
		return true
	}
	for _, c := range n.Children {
		if hasImageSrc(c, prefix) {
			return true
		}
	}
	return false
}
