package main

import (
	"context"
	"os"
	"path/filepath"
	"testing"

	"github.com/egoist/mygo/ui"
	"github.com/iwangjie/nemorous/internal/plugins"
)

func TestJavaPluginScansMainsAndTests(t *testing.T) {
	t.Setenv("NEMOROUS_CONFIG", filepath.Join(t.TempDir(), "config.json"))
	ws := t.TempDir()
	write := func(name, body string) {
		p := filepath.Join(ws, filepath.FromSlash(name))
		_ = os.MkdirAll(filepath.Dir(p), 0o755)
		if err := os.WriteFile(p, []byte(body), 0o644); err != nil {
			t.Fatal(err)
		}
	}
	write("demo/pom.xml", `<project><groupId>com.acme</groupId><artifactId>demo</artifactId><version>1.0.0</version></project>`)
	write("demo/src/main/java/com/acme/DemoApplication.java", `package com.acme;
public class DemoApplication {
  public static void main(String[] args) { System.out.println("hi"); }
}`)
	write("demo/src/test/java/com/acme/DemoApplicationTest.java", `package com.acme;
import org.junit.Test;
public class DemoApplicationTest {
  @Test public void boots() {}
}`)
	app := newGapTestApp(t, ws)
	app.initPluginManager()
	t.Cleanup(app.closePluginManager)
	app.syncPluginsWorkspace(app.reposSnapshot())
	if _, err := app.pluginMgr.InstallBuiltin(context.Background(), "java"); err != nil {
		t.Fatal(err)
	}
	ref := "java:java"
	app.openPluginPanel(ref)
	waitPluginSnapshot(t, app, ref, func(s *plugins.PanelSnapshot) bool {
		return s.Root != nil && nodeContainsText(s.Root, "DemoApplication") && nodeContainsText(s.Root, "主类")
	})
	tt := ui.NewTester(app.BuildUI, 1440, 900)
	tt.Frame()
	if !tt.HasText("Java") || !tt.HasText("DemoApplication") {
		t.Fatal("Java panel did not render project classes")
	}
	app.pluginMgr.Dispatch(ref, "view", map[string]any{"value": "run"})
	waitPluginSnapshot(t, app, ref, func(s *plugins.PanelSnapshot) bool {
		return nodeContainsText(s.Root, "运行") && nodeContainsText(s.Root, "命令")
	})
}

func TestJavaPluginRunsMain(t *testing.T) {
	t.Setenv("NEMOROUS_CONFIG", filepath.Join(t.TempDir(), "config.json"))
	ws := t.TempDir()
	write := func(name, body string) {
		p := filepath.Join(ws, filepath.FromSlash(name))
		_ = os.MkdirAll(filepath.Dir(p), 0o755)
		if err := os.WriteFile(p, []byte(body), 0o644); err != nil {
			t.Fatal(err)
		}
	}
	write("demo/src/main/java/com/acme/Hello.java", `package com.acme;
public class Hello { public static void main(String[] args) { System.out.println("java-plugin-ok"); } }`)
	app := newGapTestApp(t, ws)
	app.initPluginManager()
	t.Cleanup(app.closePluginManager)
	app.syncPluginsWorkspace(app.reposSnapshot())
	if _, err := app.pluginMgr.InstallBuiltin(context.Background(), "java"); err != nil {
		t.Fatal(err)
	}
	ref := "java:java"
	app.openPluginPanel(ref)
	waitPluginSnapshot(t, app, ref, func(s *plugins.PanelSnapshot) bool {
		return nodeContainsText(s.Root, "Hello") && !nodeContainsText(s.Root, "正在检测 JDK")
	})
	java := filepath.Join(t.TempDir(), "java")
	if err := os.WriteFile(java, []byte("#!/bin/sh\nprintf 'java-plugin-ok\n'\n"), 0o755); err != nil {
		t.Fatal(err)
	}
	app.pluginMgr.Dispatch(ref, "input", map[string]any{"key": "java", "value": java})
	app.pluginMgr.Dispatch(ref, "runMain", nil)
	waitPluginSnapshot(t, app, ref, func(s *plugins.PanelSnapshot) bool {
		return nodeContainsText(s.Root, "java-plugin-ok")
	})
}
