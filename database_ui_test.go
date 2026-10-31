package main

import (
	"context"
	"database/sql"
	"os"
	"path/filepath"
	"testing"

	"github.com/iwangjie/nemorous/internal/plugins"
)

func TestDatabasePluginSQLiteWorkflow(t *testing.T) {
	t.Setenv("NEMOROUS_CONFIG", filepath.Join(t.TempDir(), "config.json"))
	ws := t.TempDir()
	dbPath := filepath.Join(ws, "demo.sqlite")
	db, err := sql.Open("sqlite", dbPath)
	if err != nil {
		t.Fatal(err)
	}
	if _, err := db.Exec(`CREATE TABLE users (id INTEGER PRIMARY KEY, name TEXT NOT NULL, email TEXT); INSERT INTO users (name,email) VALUES ('Alice','alice@example.com'), ('Bob','bob@example.com'); CREATE INDEX idx_users_name ON users(name);`); err != nil {
		t.Fatal(err)
	}
	_ = db.Close()

	app := newGapTestApp(t, ws)
	app.initPluginManager()
	t.Cleanup(app.closePluginManager)
	app.syncPluginsWorkspace(app.reposSnapshot())
	if _, err := app.pluginMgr.InstallBuiltin(context.Background(), "database"); err != nil {
		t.Fatal(err)
	}
	ref := "database:database"
	app.openPluginPanel(ref)
	waitPluginSnapshot(t, app, ref, func(s *plugins.PanelSnapshot) bool { return s.Root != nil })
	app.pluginMgr.Dispatch(ref, "newConnection", nil)
	app.pluginMgr.Dispatch(ref, "connField", map[string]any{"key": "conn:name:x", "value": "Demo SQLite"})
	app.pluginMgr.Dispatch(ref, "connField", map[string]any{"key": "conn:path:x", "value": dbPath})
	app.pluginMgr.Dispatch(ref, "saveConnection", nil)
	waitPluginSnapshot(t, app, ref, func(s *plugins.PanelSnapshot) bool { return nodeContainsText(s.Root, "Demo SQLite") })
	app.pluginMgr.Dispatch(ref, "testConnection", nil)
	waitPluginSnapshot(t, app, ref, func(s *plugins.PanelSnapshot) bool { return nodeContainsText(s.Root, "连接成功") })
	app.pluginMgr.Dispatch(ref, "sqlInput", map[string]any{"value": "SELECT id, name, email FROM users ORDER BY id"})
	app.pluginMgr.Dispatch(ref, "runQuery", nil)
	waitPluginSnapshotTimeout(t, app, ref, 20, func(s *plugins.PanelSnapshot) bool {
		return nodeContainsText(s.Root, "Alice") && nodeContainsText(s.Root, "alice@example.com")
	})
	app.pluginMgr.Dispatch(ref, "view", map[string]any{"value": "schema"})
	waitPluginSnapshotTimeout(t, app, ref, 20, func(s *plugins.PanelSnapshot) bool { return nodeContainsText(s.Root, "users") })
	app.pluginMgr.Dispatch(ref, "selectTable", map[string]any{"name": "users"})
	waitPluginSnapshotTimeout(t, app, ref, 20, func(s *plugins.PanelSnapshot) bool {
		return nodeContainsText(s.Root, "email") && nodeContainsText(s.Root, "idx_users_name")
	})
}

func TestDatabasePluginDoesNotShipCredentials(t *testing.T) {
	path := filepath.Join("plugins", "database")
	entries, err := os.ReadDir(path)
	if err != nil {
		t.Fatal(err)
	}
	for _, e := range entries {
		if e.IsDir() {
			continue
		}
		data, err := os.ReadFile(filepath.Join(path, e.Name()))
		if err != nil {
			continue
		}
		if len(data) > 0 && containsCredentialPattern(string(data)) {
			t.Fatalf("credential-like content in %s", e.Name())
		}
	}
}

func containsCredentialPattern(s string) bool {
	for _, marker := range []string{"BEGIN PRIVATE KEY", "password=", "PASSWORD="} {
		if len(marker) > 0 && len(s) >= len(marker) {
			for i := 0; i+len(marker) <= len(s); i++ {
				if s[i:i+len(marker)] == marker {
					return true
				}
			}
		}
	}
	return false
}
