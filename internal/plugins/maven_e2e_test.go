package plugins

import (
	"context"
	"os"
	"os/exec"
	"path/filepath"
	"runtime"
	"strings"
	"testing"
	"time"
)

// TestMavenPluginEndToEnd installs the official Maven plugin from the
// binary, opens its tool window, discovers a real multi-module project,
// runs the package lifecycle, and reads the dependency tree and effective
// POM back through the declarative UI tree. It needs Maven on PATH and
// skips where it is not installed.
func TestMavenPluginEndToEnd(t *testing.T) {
	if testing.Short() {
		t.Skip("maven end-to-end test")
	}
	if runtime.GOOS == "windows" {
		t.Skip("maven end-to-end test runs on macOS and Linux CI")
	}
	if _, err := exec.LookPath("mvn"); err != nil {
		t.Skip("mvn not found")
	}
	ws := t.TempDir()
	writeMavenFixture(t, ws)

	m := newTestManager(t)
	m.SetWorkspace(ws, []Repo{{ID: "demo", Name: "demo", Path: ws}})
	if _, err := m.InstallBuiltin(context.Background(), "maven"); err != nil {
		t.Fatal(err)
	}
	ref := "maven:maven"
	waitSnapshot(t, m, ref, func(s *PanelSnapshot) bool { return s.Root != nil })
	waitForLong(t, "maven plugin load", func() bool {
		s := m.Snapshot(ref)
		return s != nil && !strings.Contains(snapshotText(s), "加载插件失败")
	})

	m.Dispatch(ref, "open", nil)
	waitForLong(t, "pom discovery", func() bool {
		txt := snapshotText(m.Snapshot(ref))
		return strings.Contains(txt, "demo") && strings.Contains(txt, "core") && strings.Contains(txt, "app")
	})

	m.Dispatch(ref, "goal", map[string]any{"goal": "package"})
	waitForLong(t, "maven package", func() bool {
		s := m.Snapshot(ref)
		if s == nil {
			return false
		}
		txt := snapshotText(s)
		return strings.Contains(txt, "构建成功") || strings.Contains(txt, "构建失败")
	})
	final := snapshotText(m.Snapshot(ref))
	if strings.Contains(final, "构建失败") {
		t.Fatalf("maven package failed:\n%s", final)
	}
	if _, err := os.Stat(filepath.Join(ws, "app", "target", "app-1.0.0.jar")); err != nil {
		t.Fatalf("package did not produce the app jar: %v", err)
	}

	m.Dispatch(ref, "openDeps", nil)
	waitForLong(t, "dependency tree", func() bool {
		s := m.Snapshot(ref)
		if s == nil {
			return false
		}
		return hasItem(s.Root, "junit:junit", 1) && hasItem(s.Root, "hamcrest-core", 2)
	})

	m.Dispatch(ref, "view", map[string]any{"value": "pom"})
	waitForLong(t, "effective pom", func() bool {
		s := m.Snapshot(ref)
		return s != nil && strings.Contains(snapshotText(s), "<project")
	})

	m.Dispatch(ref, "view", map[string]any{"value": "settings"})
	if err := m.Uninstall("maven"); err != nil {
		t.Fatal(err)
	}
	if _, ok := m.store.Get("maven"); ok {
		t.Fatal("maven still installed after uninstall")
	}
	if _, err := os.Stat(filepath.Join(m.Root(), "installed", "maven")); !os.IsNotExist(err) {
		t.Fatal("maven package directory still present after uninstall")
	}
}

func hasItem(n *Node, want string, depth int) bool {
	if n == nil {
		return false
	}
	for _, item := range n.Items {
		if strings.Contains(item.Text, want) && item.Depth == depth {
			return true
		}
	}
	for _, child := range n.Children {
		if hasItem(child, want, depth) {
			return true
		}
	}
	return false
}

func waitForLong(t *testing.T, what string, cond func() bool) {
	t.Helper()
	deadline := time.Now().Add(2 * time.Minute)
	for time.Now().Before(deadline) {
		if cond() {
			return
		}
		time.Sleep(20 * time.Millisecond)
	}
	t.Fatalf("timed out waiting for %s", what)
}

func writeMavenFixture(t *testing.T, ws string) {
	t.Helper()
	files := map[string]string{
		"pom.xml": `<project xmlns="http://maven.apache.org/POM/4.0.0">
  <modelVersion>4.0.0</modelVersion>
  <groupId>com.example</groupId>
  <artifactId>demo</artifactId>
  <version>1.0.0</version>
  <packaging>pom</packaging>
  <name>Demo Parent</name>
  <modules><module>core</module><module>app</module></modules>
  <profiles>
    <profile><id>fast</id></profile>
    <profile><id>ci</id></profile>
  </profiles>
  <build>
    <pluginManagement>
      <plugins>
        <plugin><groupId>org.apache.maven.plugins</groupId><artifactId>maven-resources-plugin</artifactId><version>3.3.1</version></plugin>
        <plugin><groupId>org.apache.maven.plugins</groupId><artifactId>maven-compiler-plugin</artifactId><version>3.11.0</version></plugin>
        <plugin><groupId>org.apache.maven.plugins</groupId><artifactId>maven-surefire-plugin</artifactId><version>3.2.5</version></plugin>
      </plugins>
    </pluginManagement>
  </build>
</project>`,
		"core/pom.xml": `<project xmlns="http://maven.apache.org/POM/4.0.0">
  <modelVersion>4.0.0</modelVersion>
  <parent><groupId>com.example</groupId><artifactId>demo</artifactId><version>1.0.0</version></parent>
  <artifactId>core</artifactId>
  <name>Core</name>
</project>`,
		"app/pom.xml": `<project xmlns="http://maven.apache.org/POM/4.0.0">
  <modelVersion>4.0.0</modelVersion>
  <parent><groupId>com.example</groupId><artifactId>demo</artifactId><version>1.0.0</version></parent>
  <artifactId>app</artifactId>
  <dependencies><dependency><groupId>junit</groupId><artifactId>junit</artifactId><version>4.13.2</version><scope>test</scope></dependency></dependencies>
</project>`,
		"core/src/main/java/com/example/core/Core.java": `package com.example.core;
public class Core { public static String hi() { return "hi"; } }
`,
		"app/src/main/java/com/example/app/App.java": `package com.example.app;
public class App { public static void main(String[] args) { System.out.println("app"); } }
`,
	}
	for name, content := range files {
		path := filepath.Join(ws, filepath.FromSlash(name))
		if err := os.MkdirAll(filepath.Dir(path), 0o755); err != nil {
			t.Fatal(err)
		}
		if err := os.WriteFile(path, []byte(content), 0o644); err != nil {
			t.Fatal(err)
		}
	}
}
