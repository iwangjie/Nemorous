package plugins

import (
	"encoding/json"
	"fmt"
	"net/http"
	"net/http/httptest"
	"os"
	"path/filepath"
	"strings"
	"testing"
)

func pluginWithManifest(t *testing.T, id, permissions, script string) string {
	t.Helper()
	dir := t.TempDir()
	manifest := fmt.Sprintf(`{"id":%q,"name":"Test","version":"1.0.0","main":"main.js","permissions":%s,"contributes":{"panels":[{"id":"main","title":"Test"}]}}`, id, permissions)
	if err := os.WriteFile(filepath.Join(dir, "plugin.json"), []byte(manifest), 0o644); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(filepath.Join(dir, "main.js"), []byte(script), 0o644); err != nil {
		t.Fatal(err)
	}
	return dir
}

func TestDecodePreviewNodes(t *testing.T) {
	markdown := decodeTestNode(t, map[string]any{
		"type": "markdown", "text": "# Hello", "basePath": "/tmp/repo",
	})
	if markdown.Text != "# Hello" || markdown.BasePath != "/tmp/repo" {
		t.Fatalf("markdown node = %+v", markdown)
	}
	table := decodeTestNode(t, map[string]any{
		"type": "table", "headers": []string{"a", "b"}, "rows": [][]string{{"1", "2"}},
	})
	if len(table.Headers) != 2 || len(table.Rows) != 1 || table.Rows[0][1] != "2" {
		t.Fatalf("table node = %+v", table)
	}
	area := decodeTestNode(t, map[string]any{
		"type": "textarea", "value": "body", "key": "src", "widthPercent": 50, "heightPercent": 100,
	})
	if area.Value != "body" || area.WidthPercent != 50 || area.HeightPercent != 100 {
		t.Fatalf("textarea node = %+v", area)
	}
	bar := decodeTestNode(t, map[string]any{"type": "bar", "text": "Go", "fraction": 0.4})
	if bar.Text != "Go" || bar.Fraction != 0.4 {
		t.Fatalf("bar node = %+v", bar)
	}
	if _, err := decodeNodeJSON(`{"type":"table","rows":[["x"]],"headers":["a"],"fraction":9}`); err != nil {
		t.Fatal(err)
	}
}

func decodeTestNode(t *testing.T, value map[string]any) *Node {
	t.Helper()
	data, err := json.Marshal(value)
	if err != nil {
		t.Fatal(err)
	}
	n, err := decodeNodeJSON(string(data))
	if err != nil {
		t.Fatal(err)
	}
	return n
}

func decodeNodeJSON(raw string) (*Node, error) {
	var v any
	if err := json.Unmarshal([]byte(raw), &v); err != nil {
		return nil, err
	}
	return decodeNode(v)
}

func TestRuntimeFSWriteAndHTTP(t *testing.T) {
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if r.Header.Get("X-Test") != "yes" {
			http.Error(w, "missing header", http.StatusForbidden)
			return
		}
		_, _ = w.Write([]byte("remote body"))
	}))
	defer server.Close()

	workspace := t.TempDir()
	m := newTestManager(t)
	m.SetWorkspace(workspace, nil)
	dir := pluginWithManifest(t, "ext", `["workspace.read","workspace.write","network.http"]`, fmt.Sprintf(`
var body = "";
function render(panel) { return {type:"text", text: body}; }
function handleAction(action) {
  if (action === "go") {
    nemorous.fs.write(%q, "written");
    nemorous.http.request({
      url: %q,
      headers: { "X-Test": "yes" },
      onResponse: function (r) { body = r.body; nemorous.invalidate(); },
      onError: function (e) { body = "error " + e; nemorous.invalidate(); }
    });
  }
}
`, filepath.Join(workspace, "out.txt"), server.URL))
	if _, err := m.store.InstallDir(dir, "test"); err != nil {
		t.Fatal(err)
	}
	m.Load()
	waitSnapshot(t, m, "ext:main", func(s *PanelSnapshot) bool { return s.Root != nil })
	m.Dispatch("ext:main", "go", nil)
	waitSnapshot(t, m, "ext:main", func(s *PanelSnapshot) bool { return strings.Contains(snapshotText(s), "remote body") })
	data, err := os.ReadFile(filepath.Join(workspace, "out.txt"))
	if err != nil || string(data) != "written" {
		t.Fatalf("write = %q, %v", data, err)
	}
}

func TestRuntimeActiveFileAndTerminalAPI(t *testing.T) {
	var gotCommand []string
	active := &ActiveFile{RepoID: "r", RepoName: "repo", Path: "/tmp/a.md", RelPath: "a.md", Name: "a.md", Content: "# A"}
	m, err := NewManager(Options{
		Root: filepath.Join(t.TempDir(), "plugins"), AppVersion: "0.5.0",
		Invalidate: func() {}, Notify: func(string, string) {},
		ActiveFile: func() *ActiveFile { return active },
		OpenTerminal: func(command []string, dir, title string) error {
			gotCommand = append([]string(nil), command...)
			return nil
		},
		OpenURL: func(string) error { return nil },
	})
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(m.Close)
	m.SetWorkspace(t.TempDir(), nil)
	dir := pluginWithManifest(t, "shell", `["editor.read","terminal.open"]`, `
var out = "";
function render(panel) { return {type:"text", text: out}; }
function handleAction(action) {
  if (action === "go") {
    var f = nemorous.editor.activeFile();
    out = f ? f.relPath + ":" + f.content : "none";
    nemorous.terminal.open({ command: ["ssh", "host"], cwd: nemorous.workspace().path, title: "SSH" });
  }
}
`)
	if _, err := m.store.InstallDir(dir, "test"); err != nil {
		t.Fatal(err)
	}
	m.Load()
	waitSnapshot(t, m, "shell:main", func(s *PanelSnapshot) bool { return s.Root != nil })
	m.Dispatch("shell:main", "go", nil)
	waitSnapshot(t, m, "shell:main", func(s *PanelSnapshot) bool { return strings.Contains(snapshotText(s), "a.md:# A") })
	if len(gotCommand) != 2 || gotCommand[0] != "ssh" {
		t.Fatalf("command = %v", gotCommand)
	}
}

func TestDefaultEditorPluginsInstallOnceAndStayDismissed(t *testing.T) {
	root := filepath.Join(t.TempDir(), "plugins")
	newMgr := func() *Manager {
		m, err := NewManager(Options{
			Root: root, AppVersion: "0.5.0", AutoInstallDefaults: true,
			Invalidate: func() {}, Notify: func(string, string) {},
		})
		if err != nil {
			t.Fatal(err)
		}
		m.Load()
		t.Cleanup(m.Close)
		return m
	}
	m := newMgr()
	waitFor(t, "default plugins", func() bool {
		_, markdown := m.store.Get("markdown")
		_, csv := m.store.Get("csv")
		return markdown && csv
	})
	if err := m.Uninstall("markdown"); err != nil {
		t.Fatal(err)
	}
	m.Close()
	m = newMgr()
	if _, ok := m.store.Get("markdown"); ok {
		t.Fatal("dismissed Markdown plugin was reinstalled")
	}
	if _, ok := m.store.Get("csv"); !ok {
		t.Fatal("CSV default should stay installed")
	}
}
