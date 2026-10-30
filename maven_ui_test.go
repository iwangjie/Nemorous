package main

import (
	"context"
	"os"
	"path/filepath"
	"runtime"
	"strings"
	"testing"
	"time"

	"github.com/egoist/mygo/ui"
	"github.com/iwangjie/nemorous/internal/plugins"
)

func writePom(t *testing.T, dir, group, artifact, version, deps string) {
	t.Helper()
	_ = os.MkdirAll(dir, 0o755)
	pom := `<?xml version="1.0"?>
<project xmlns="http://maven.apache.org/POM/4.0.0">
  <modelVersion>4.0.0</modelVersion>
  <groupId>` + group + `</groupId>
  <artifactId>` + artifact + `</artifactId>
  <version>` + version + `</version>
  <dependencies>` + deps + `</dependencies>
</project>
`
	if err := os.WriteFile(filepath.Join(dir, "pom.xml"), []byte(pom), 0o644); err != nil {
		t.Fatal(err)
	}
}

func TestMavenDependencyGraphUI(t *testing.T) {
	t.Setenv("NEMOROUS_CONFIG", filepath.Join(t.TempDir(), "config.json"))
	ws := t.TempDir()
	writePom(t, filepath.Join(ws, "core"), "com.acme", "core", "1.2.0", "")
	writePom(t, filepath.Join(ws, "api"), "com.acme", "api", "2.0.0", `
    <dependency><groupId>com.acme</groupId><artifactId>core</artifactId><version>1.1.0</version></dependency>`)
	app := newGapTestApp(t, ws)
	app.initPluginManager()
	t.Cleanup(app.closePluginManager)
	app.syncPluginsWorkspace(app.reposSnapshot())
	if _, err := app.pluginMgr.InstallBuiltin(context.Background(), "maven"); err != nil {
		t.Fatal(err)
	}
	ref := "maven:maven"
	app.openPluginPanel(ref)
	waitPluginSnapshot(t, app, ref, func(s *plugins.PanelSnapshot) bool {
		return s.Root != nil && nodeContainsText(s.Root, "api") && nodeContainsText(s.Root, "core")
	})
	app.pluginMgr.Dispatch(ref, "view", map[string]any{"value": "graph"})
	waitPluginSnapshot(t, app, ref, func(s *plugins.PanelSnapshot) bool {
		return nodeContainsText(s.Root, "依赖关系图") && nodeContainsText(s.Root, "批次") &&
			nodeContainsText(s.Root, "api  →  core") && nodeContainsText(s.Root, "版本冲突")
	})
	tt := ui.NewTester(app.BuildUI, 1440, 900)
	tt.Frame()
	if !tt.HasText("依赖关系图") || !tt.HasText("构建批次") {
		t.Fatal("Maven graph view did not render")
	}
}

func TestMavenPluginUsesConcurrencyAndJVMSettings(t *testing.T) {
	if runtime.GOOS == "windows" {
		t.Skip("POSIX shell fixture")
	}
	if os.Getenv("SKIP_MAVEN_UI_EXEC") == "1" {
		t.Skip("skip")
	}
	t.Setenv("NEMOROUS_CONFIG", filepath.Join(t.TempDir(), "config.json"))
	ws := t.TempDir()
	writePom(t, filepath.Join(ws, "core"), "com.acme", "core", "1.0.0", "")
	logPath := filepath.Join(t.TempDir(), "mvn.log")
	script := "#!/bin/sh\nprintf '%s\\n' \"$*\" > " + logPath + "\nprintf '%s\\n' \"$MAVEN_OPTS\" >> " + logPath + "\n"
	mvn := filepath.Join(t.TempDir(), "mvn")
	if err := os.WriteFile(mvn, []byte(script), 0o755); err != nil {
		t.Fatal(err)
	}
	app := newGapTestApp(t, ws)
	app.initPluginManager()
	t.Cleanup(app.closePluginManager)
	app.syncPluginsWorkspace(app.reposSnapshot())
	if _, err := app.pluginMgr.InstallBuiltin(context.Background(), "maven"); err != nil {
		t.Fatal(err)
	}
	ref := "maven:maven"
	app.openPluginPanel(ref)
	waitPluginSnapshot(t, app, ref, func(s *plugins.PanelSnapshot) bool { return nodeContainsText(s.Root, "core") })
	app.pluginMgr.Dispatch(ref, "input", map[string]any{"key": "mvn", "value": mvn})
	app.pluginMgr.Dispatch(ref, "input", map[string]any{"key": "jvmArgs", "value": "-Xmx2g"})
	app.pluginMgr.Dispatch(ref, "saveSettings", nil)
	app.pluginMgr.Dispatch(ref, "goal", map[string]any{"goal": "validate"})
	deadline := time.Now().Add(5 * time.Second)
	for time.Now().Before(deadline) {
		data, _ := os.ReadFile(logPath)
		if strings.Contains(string(data), "-T 1C") && strings.Contains(string(data), "-Xmx2g") {
			return
		}
		time.Sleep(20 * time.Millisecond)
	}
	data, _ := os.ReadFile(logPath)
	t.Fatalf("maven args/env = %q", data)
}

func TestMavenUsesProjectWrapper(t *testing.T) {
	if runtime.GOOS == "windows" {
		t.Skip("POSIX shell fixture")
	}
	t.Setenv("NEMOROUS_CONFIG", filepath.Join(t.TempDir(), "config.json"))
	ws := t.TempDir()
	writePom(t, filepath.Join(ws, "demo"), "com.acme", "demo", "1.0.0", "")
	logPath := filepath.Join(t.TempDir(), "wrapper.log")
	wrapper := "#!/bin/sh\nprintf '%s\\n' \"$*\" > " + logPath + "\n"
	if err := os.WriteFile(filepath.Join(ws, "mvnw"), []byte(wrapper), 0o755); err != nil {
		t.Fatal(err)
	}
	app := newGapTestApp(t, ws)
	app.initPluginManager()
	t.Cleanup(app.closePluginManager)
	app.syncPluginsWorkspace(app.reposSnapshot())
	if _, err := app.pluginMgr.InstallBuiltin(context.Background(), "maven"); err != nil {
		t.Fatal(err)
	}
	ref := "maven:maven"
	app.openPluginPanel(ref)
	waitPluginSnapshot(t, app, ref, func(s *plugins.PanelSnapshot) bool { return s.Root != nil && nodeContainsText(s.Root, "demo") })
	app.pluginMgr.Dispatch(ref, "goal", map[string]any{"goal": "validate"})
	deadline := time.Now().Add(5 * time.Second)
	for time.Now().Before(deadline) {
		data, _ := os.ReadFile(logPath)
		if strings.Contains(string(data), "validate") {
			return
		}
		time.Sleep(20 * time.Millisecond)
	}
	data, _ := os.ReadFile(logPath)
	t.Fatalf("project wrapper was not used: %q", data)
}

func TestMavenBuildChainRunsFromReactorRoot(t *testing.T) {
	if runtime.GOOS == "windows" {
		t.Skip("POSIX shell fixture")
	}
	t.Setenv("NEMOROUS_CONFIG", filepath.Join(t.TempDir(), "config.json"))
	ws := t.TempDir()
	parentPom := `<?xml version="1.0"?><project xmlns="http://maven.apache.org/POM/4.0.0">
  <modelVersion>4.0.0</modelVersion>
  <groupId>com.acme</groupId><artifactId>demo-parent</artifactId><version>1.0.0</version>
  <packaging>pom</packaging><modules><module>core</module></modules>
</project>`
	if err := os.WriteFile(filepath.Join(ws, "pom.xml"), []byte(parentPom), 0o644); err != nil {
		t.Fatal(err)
	}
	writePom(t, filepath.Join(ws, "core"), "com.acme", "core", "1.0.0", "")
	logPath := filepath.Join(t.TempDir(), "wrapper.log")
	wrapper := "#!/bin/sh\nprintf '%s\\n' \"$*\" > " + logPath + "\n"
	if err := os.WriteFile(filepath.Join(ws, "mvnw"), []byte(wrapper), 0o755); err != nil {
		t.Fatal(err)
	}
	app := newGapTestApp(t, ws)
	app.initPluginManager()
	t.Cleanup(app.closePluginManager)
	app.syncPluginsWorkspace(app.reposSnapshot())
	if _, err := app.pluginMgr.InstallBuiltin(context.Background(), "maven"); err != nil {
		t.Fatal(err)
	}
	ref := "maven:maven"
	app.openPluginPanel(ref)
	waitPluginSnapshot(t, app, ref, func(s *plugins.PanelSnapshot) bool { return s.Root != nil && nodeContainsText(s.Root, "core") })
	app.pluginMgr.Dispatch(ref, "select", map[string]any{"path": filepath.Join(ws, "core", "pom.xml")})
	app.pluginMgr.Dispatch(ref, "buildChain", map[string]any{"goal": "package"})
	deadline := time.Now().Add(5 * time.Second)
	for time.Now().Before(deadline) {
		data, _ := os.ReadFile(logPath)
		if strings.Contains(string(data), "-pl :core -am") {
			return
		}
		time.Sleep(20 * time.Millisecond)
	}
	final, _ := os.ReadFile(logPath)
	t.Fatalf("reactor build args = %q", final)
}
