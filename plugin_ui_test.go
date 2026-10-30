package main

import (
	"context"
	"os"
	"path/filepath"
	"testing"
	"time"

	"github.com/egoist/mygo/ui"
	"github.com/iwangjie/nemorous/internal/config"
	"github.com/iwangjie/nemorous/internal/plugins"
)

func pluginInstalled(mgr *plugins.Manager, id string) bool {
	for _, p := range mgr.Installed() {
		if p.Manifest.ID == id {
			return true
		}
	}
	return false
}

func waitPluginSnapshot(t *testing.T, app *App, ref string, cond func(*plugins.PanelSnapshot) bool) {
	t.Helper()
	deadline := time.Now().Add(10 * time.Second)
	for time.Now().Before(deadline) {
		if app.pluginMgr != nil {
			if snap := app.pluginMgr.Snapshot(ref); snap != nil && cond(snap) {
				return
			}
		}
		time.Sleep(10 * time.Millisecond)
	}
	t.Fatalf("timed out waiting for plugin snapshot %s", ref)
}

func TestPluginPanelAndMarketplaceUI(t *testing.T) {
	t.Setenv("NEMOROUS_CONFIG", filepath.Join(t.TempDir(), "config.json"))
	ws := setupUITestWorkspace(t)
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

	if _, err := app.pluginMgr.InstallBuiltin(context.Background(), "maven"); err != nil {
		t.Fatal(err)
	}
	ref := "maven:maven"
	waitPluginSnapshot(t, app, ref, func(s *plugins.PanelSnapshot) bool { return s.Root != nil })

	tt := ui.NewTester(app.BuildUI, 1440, 900)
	tt.Frame()
	if err := tt.Click("Maven"); err != nil {
		t.Fatalf("Maven rail icon should be clickable: %v", err)
	}
	tt.Frame()
	if app.pluginPanel != ref {
		t.Fatalf("expected Maven panel open, got %q", app.pluginPanel)
	}
	if !tt.HasText("项目") || !tt.HasText("依赖") || !tt.HasText("POM") || !tt.HasText("设置") {
		t.Fatal("Maven plugin tabs should render in the tool window")
	}
	if !tt.HasText("刷新") {
		t.Fatal("Maven plugin refresh control should render")
	}

	// The marketplace skips a network refresh while one is marked loading.
	app.pluginRegistryLoading = true
	app.openPluginMarket()
	tt.Frame()
	if !tt.HasText("插件市场") || !tt.HasText("Maven") || !tt.HasText("已安装") {
		t.Fatal("marketplace should list the installed Maven plugin")
	}
	if !tt.HasText("卸载") || !tt.HasText("禁用") {
		t.Fatal("installed plugin should offer disable/uninstall actions")
	}
	app.showPluginMarket = false
	tt.Frame()

	// Uninstalling closes the panel that belonged to the plugin.
	if err := app.pluginMgr.Uninstall("maven"); err != nil {
		t.Fatal(err)
	}
	app.syncPluginPanelsAfterInstall()
	if app.pluginPanel != "" {
		t.Fatalf("panel should close after uninstall, got %q", app.pluginPanel)
	}
	if _, err := os.Stat(filepath.Join(app.pluginMgr.Root(), "installed", "maven")); !os.IsNotExist(err) {
		t.Fatal("maven package directory still exists after uninstall")
	}
	tt.Frame()
	if err := tt.Click("Maven"); err == nil {
		t.Fatal("rail should no longer show the uninstalled plugin")
	}
}

func TestPluginMarketplaceInstallUninstallFlow(t *testing.T) {
	t.Setenv("NEMOROUS_CONFIG", filepath.Join(t.TempDir(), "config.json"))
	ws := setupUITestWorkspace(t)
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

	tt := ui.NewTester(app.BuildUI, 1440, 900)
	app.pluginRegistryLoading = true // no network in tests
	app.openPluginMarket()
	app.pluginMarketQuery = "Maven"
	tt.Frame()
	if err := tt.Click("安装"); err != nil {
		t.Fatalf("install button should be clickable: %v", err)
	}
	deadline := time.Now().Add(10 * time.Second)
	for {
		if pluginInstalled(app.pluginMgr, "maven") {
			break
		}
		if time.Now().After(deadline) {
			t.Fatal("marketplace install did not finish")
		}
		time.Sleep(20 * time.Millisecond)
	}
	waitPluginSnapshot(t, app, "maven:maven", func(s *plugins.PanelSnapshot) bool { return s.Root != nil })
	app.showPluginMarket = false
	tt.Frame()
	if err := tt.Click("Maven"); err != nil {
		t.Fatalf("installed plugin should appear on the rail: %v", err)
	}
	tt.Frame()
	if app.pluginPanel != "maven:maven" {
		t.Fatalf("Maven panel should be open, got %q", app.pluginPanel)
	}

	app.openPluginMarket()
	app.pluginMarketQuery = "Maven"
	tt.Frame()
	if err := tt.Click("卸载"); err != nil {
		t.Fatalf("uninstall should be offered: %v", err)
	}
	tt.Frame()
	if err := tt.Click("确认卸载"); err != nil {
		t.Fatalf("confirmation should be offered: %v", err)
	}
	deadline = time.Now().Add(10 * time.Second)
	for {
		if !pluginInstalled(app.pluginMgr, "maven") {
			break
		}
		if time.Now().After(deadline) {
			t.Fatal("marketplace uninstall did not finish")
		}
		time.Sleep(20 * time.Millisecond)
	}
	app.syncPluginPanelsAfterInstall()
	if app.pluginPanel != "" {
		t.Fatalf("panel should close after uninstall, got %q", app.pluginPanel)
	}
}
