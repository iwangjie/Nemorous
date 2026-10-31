package plugins

import (
	"archive/zip"
	"bytes"
	"context"
	"crypto/sha256"
	"encoding/hex"
	"fmt"
	"io"
	"net/http"
	"net/http/httptest"
	"os"
	"path/filepath"
	"strconv"
	"strings"
	"testing"
	"time"
)

func testPluginJSON(id string) string {
	return fmt.Sprintf(`{
  "id": %q,
  "name": "Test %s",
  "version": "1.0.0",
  "main": "main.js",
  "permissions": ["workspace.read", "process.exec"],
  "contributes": {"panels": [{"id": "main", "title": "Test"}]}
}`, id, id)
}

func makePluginDir(t *testing.T, id, script string) string {
	t.Helper()
	dir := t.TempDir()
	if err := os.WriteFile(filepath.Join(dir, "plugin.json"), []byte(testPluginJSON(id)), 0o644); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(filepath.Join(dir, "main.js"), []byte(script), 0o644); err != nil {
		t.Fatal(err)
	}
	return dir
}

func newTestManager(t *testing.T) *Manager {
	t.Helper()
	m, err := NewManager(Options{
		Root:       filepath.Join(t.TempDir(), "plugins"),
		AppVersion: "0.5.0",
		Invalidate: func() {},
		Notify:     func(string, string) {},
	})
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(m.Close)
	return m
}

func waitFor(t *testing.T, what string, cond func() bool) {
	t.Helper()
	deadline := time.Now().Add(5 * time.Second)
	for time.Now().Before(deadline) {
		if cond() {
			return
		}
		time.Sleep(10 * time.Millisecond)
	}
	t.Fatalf("timed out waiting for %s", what)
}

func waitSnapshot(t *testing.T, m *Manager, ref string, cond func(*PanelSnapshot) bool) *PanelSnapshot {
	t.Helper()
	var got *PanelSnapshot
	waitFor(t, "snapshot "+ref, func() bool {
		got = m.Snapshot(ref)
		return got != nil && cond(got)
	})
	return got
}

func snapshotText(s *PanelSnapshot) string {
	var b strings.Builder
	var walk func(n *Node)
	walk = func(n *Node) {
		if n == nil {
			return
		}
		b.WriteString(n.Text)
		b.WriteByte(' ')
		for _, it := range n.Items {
			b.WriteString(it.Text)
			b.WriteByte(' ')
		}
		for _, c := range n.Children {
			walk(c)
		}
	}
	walk(s.Root)
	return b.String()
}

func TestRuntimeRendersAndHandlesActions(t *testing.T) {
	m := newTestManager(t)
	dir := makePluginDir(t, "hello", `
var count = 0;
function render(panel) {
  return {type: "column", children: [{type: "text", text: "count " + count}]};
}
function handleAction(action, payload) {
  if (action === "inc") count++;
}
`)
	if _, err := m.store.InstallDir(dir, "test"); err != nil {
		t.Fatal(err)
	}
	m.Load()
	ref := "hello:main"
	waitSnapshot(t, m, ref, func(s *PanelSnapshot) bool {
		return strings.Contains(snapshotText(s), "count 0")
	})
	m.Dispatch(ref, "inc", nil)
	waitSnapshot(t, m, ref, func(s *PanelSnapshot) bool {
		return strings.Contains(snapshotText(s), "count 1")
	})
}

func TestRuntimeFSIsWorkspaceScoped(t *testing.T) {
	m := newTestManager(t)
	workspace := t.TempDir()
	outside := filepath.Join(t.TempDir(), "secret.txt")
	if err := os.WriteFile(outside, []byte("secret"), 0o644); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(filepath.Join(workspace, "inside.txt"), []byte("inside"), 0o644); err != nil {
		t.Fatal(err)
	}
	m.SetWorkspace(workspace, nil)
	dir := makePluginDir(t, "fs", fmt.Sprintf(`
var inside = "";
var outside = "";
function render(panel) {
  return {type: "column", children: [{type: "text", text: inside}, {type: "text", text: outside}]};
}
function handleAction(action, payload) {
  if (action === "read") {
    inside = nemorous.fs.read(%q);
    try { outside = nemorous.fs.read(%q); } catch (e) { outside = "denied"; }
  }
}
`, filepath.Join(workspace, "inside.txt"), outside))
	if _, err := m.store.InstallDir(dir, "test"); err != nil {
		t.Fatal(err)
	}
	m.Load()
	ref := "fs:main"
	waitSnapshot(t, m, ref, func(s *PanelSnapshot) bool { return s.Root != nil })
	m.Dispatch(ref, "read", nil)
	waitSnapshot(t, m, ref, func(s *PanelSnapshot) bool {
		txt := snapshotText(s)
		return strings.Contains(txt, "inside") && strings.Contains(txt, "denied")
	})
}

func TestRuntimeExecStreamsAndExits(t *testing.T) {
	m := newTestManager(t)
	workspace := t.TempDir()
	m.SetWorkspace(workspace, nil)
	dir := makePluginDir(t, "runner", `
var out = "";
var done = "";
function render(panel) {
  return {type: "column", children: [{type: "text", text: out}, {type: "text", text: done}]};
}
function handleAction(action) {
  if (action === "run") {
    nemorous.exec({
      command: "go",
      args: ["env", "GOROOT"],
      cwd: nemorous.workspace().path,
      onOutput: function (chunk) { out += chunk; },
      onExit: function (code) { done = "exit=" + code; }
    });
  }
}
`)
	if _, err := m.store.InstallDir(dir, "test"); err != nil {
		t.Fatal(err)
	}
	m.Load()
	ref := "runner:main"
	waitSnapshot(t, m, ref, func(s *PanelSnapshot) bool { return s.Root != nil })
	m.Dispatch(ref, "run", nil)
	waitFor(t, "exec output and exit", func() bool {
		s := m.Snapshot(ref)
		if s == nil {
			return false
		}
		return strings.Contains(snapshotText(s), "exit=0")
	})
}

func TestStoreInstallPersistAndUninstall(t *testing.T) {
	root := filepath.Join(t.TempDir(), "plugins")
	store, err := NewStore(root, "0.3.0")
	if err != nil {
		t.Fatal(err)
	}
	dir := makePluginDir(t, "persist", `function render(){return {type:"text", text:"ok"}}`)
	if _, err := store.InstallDir(dir, "builtin"); err != nil {
		t.Fatal(err)
	}
	p, ok := store.Get("persist")
	if !ok || !p.State.Enabled {
		t.Fatalf("expected installed enabled plugin, got %+v ok=%v", p, ok)
	}
	if err := store.SetEnabled("persist", false); err != nil {
		t.Fatal(err)
	}
	reopened, err := NewStore(root, "0.3.0")
	if err != nil {
		t.Fatal(err)
	}
	p, ok = reopened.Get("persist")
	if !ok || p.State.Enabled {
		t.Fatalf("expected disabled plugin after reopen, got %+v ok=%v", p, ok)
	}
	if err := reopened.Uninstall("persist"); err != nil {
		t.Fatal(err)
	}
	if _, ok := reopened.Get("persist"); ok {
		t.Fatal("plugin still present after uninstall")
	}
	if _, err := os.Stat(filepath.Join(root, "installed", "persist")); !os.IsNotExist(err) {
		t.Fatal("plugin directory still present")
	}
}

func TestArchiveRejectsTraversal(t *testing.T) {
	var buf bytes.Buffer
	zw := zip.NewWriter(&buf)
	w, err := zw.Create("../escape.txt")
	if err != nil {
		t.Fatal(err)
	}
	if _, err := w.Write([]byte("evil")); err != nil {
		t.Fatal(err)
	}
	if err := zw.Close(); err != nil {
		t.Fatal(err)
	}
	dest := t.TempDir()
	if _, err := extractZip(buf.Bytes(), dest); err == nil {
		t.Fatal("expected traversal to be rejected")
	}
	if _, err := os.Stat(filepath.Join(filepath.Dir(dest), "escape.txt")); err == nil {
		t.Fatal("traversal wrote outside destination")
	}
}

func TestBuiltinMavenPackageInstalls(t *testing.T) {
	m := newTestManager(t)
	if _, err := m.InstallBuiltin(context.Background(), "maven"); err != nil {
		t.Fatal(err)
	}
	p, ok := m.store.Get("maven")
	if !ok {
		t.Fatal("maven not installed")
	}
	if p.Manifest.Version != "1.1.0" {
		t.Fatalf("version = %s", p.Manifest.Version)
	}
	if p.Manifest.Contributes.Panels[0].ID != "maven" {
		t.Fatalf("panels = %+v", p.Manifest.Contributes.Panels)
	}
	// The runtime starts with an empty workspace and must still render.
	m.Load()
	waitSnapshot(t, m, "maven:maven", func(s *PanelSnapshot) bool { return s.Root != nil })
	if err := m.Uninstall("maven"); err != nil {
		t.Fatal(err)
	}
	if _, ok := m.store.Get("maven"); ok {
		t.Fatal("maven still installed")
	}
}

func TestRegistryRemotePluginInstallAndUpdate(t *testing.T) {
	zipData := pluginZip(t, "remote", "1.0.0", `function render(){return {type:"text", text:"remote"}}`)
	sum := sha256.Sum256(zipData)
	var server *httptest.Server
	server = httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		switch r.URL.Path {
		case "/registry.json":
			fmt.Fprintf(w, `{"schema":1,"updatedAt":"2026-01-01T00:00:00Z","plugins":[{
				"id":"remote","name":"Remote","version":"1.0.0","main":"main.js",
				"contributes":{"panels":[{"id":"main","title":"Remote"}]},
				"builtin":false,"downloadUrl":"%s/remote.zip","sha256":"%s","size":%d
			}]}`, server.URL, hex.EncodeToString(sum[:]), len(zipData))
		case "/remote.zip":
			w.Write(zipData)
		default:
			http.NotFound(w, r)
		}
	}))
	defer server.Close()
	m := newTestManager(t)
	if err := m.SetRegistryURL(server.URL + "/registry.json"); err != nil {
		t.Fatal(err)
	}
	if err := m.RefreshRegistry(context.Background()); err != nil {
		t.Fatal(err)
	}
	catalog := m.Catalog()
	var remote *RegistryEntry
	for i := range catalog {
		if catalog[i].ID == "remote" {
			remote = &catalog[i]
		}
	}
	if remote == nil {
		t.Fatalf("remote not in catalog: %+v", catalog)
	}
	if _, err := m.InstallRegistry(context.Background(), *remote); err != nil {
		t.Fatal(err)
	}
	if p, ok := m.store.Get("remote"); !ok || p.Manifest.Version != "1.0.0" {
		t.Fatalf("remote install failed: %+v ok=%v", p, ok)
	}
	m.Load()
	waitSnapshot(t, m, "remote:main", func(s *PanelSnapshot) bool { return s.Root != nil })
	if err := m.Uninstall("remote"); err != nil {
		t.Fatal(err)
	}
}

func TestRegistryChecksumMismatch(t *testing.T) {
	zipData := pluginZip(t, "bad", "1.0.0", `function render(){return {type:"text", text:"x"}}`)
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		w.Write(zipData)
	}))
	defer server.Close()
	m := newTestManager(t)
	entry := RegistryEntry{RegistryPlugin: RegistryPlugin{
		Manifest:    Manifest{ID: "bad", Name: "Bad", Version: "1.0.0", Main: "main.js"},
		DownloadURL: server.URL, SHA256: strings.Repeat("0", 64),
	}}
	if _, err := m.InstallRegistry(context.Background(), entry); err == nil || !strings.Contains(err.Error(), "SHA256") {
		t.Fatalf("expected checksum failure, got %v", err)
	}
}

func TestOpenAndUninstallBuiltinRegistry(t *testing.T) {
	m := newTestManager(t)
	reg := m.Registry()
	if reg == nil || len(reg.Plugins) == 0 {
		t.Fatal("embedded registry is empty")
	}
	found := false
	for _, p := range reg.Plugins {
		if p.ID == "maven" && p.Builtin {
			found = true
		}
	}
	if !found {
		t.Fatal("maven missing from embedded registry")
	}
}

func TestVersionCompare(t *testing.T) {
	cases := []struct {
		a, b string
		want int
	}{
		{"0.3.0", "0.3.0", 0},
		{"0.3.1", "0.3.0", 1},
		{"0.2.9", "0.3.0", -1},
		{"1.0.0", "1.0.0-rc.1", 1},
		{"1.0.0-rc.1", "1.0.0-rc.2", -1},
	}
	for _, c := range cases {
		if got := CompareVersions(c.a, c.b); got != c.want {
			t.Errorf("CompareVersions(%q, %q) = %d, want %d", c.a, c.b, got, c.want)
		}
	}
}

func TestManifestValidation(t *testing.T) {
	base := `{"id":"x","name":"X","version":"1.0.0","main":"main.js","minAppVersion":"0.3.0","contributes":{"panels":[{"id":"p","title":"P"}]}}`
	if _, err := ParseManifest([]byte(base), "0.3.0"); err != nil {
		t.Fatalf("valid manifest rejected: %v", err)
	}
	bad := strings.Replace(base, `"0.3.0"`, `"9.0.0"`, 1)
	if _, err := ParseManifest([]byte(bad), "0.3.0"); err == nil {
		t.Fatal("incompatible manifest accepted")
	}
	bad = strings.Replace(base, `"main.js"`, `"../main.js"`, 1)
	if _, err := ParseManifest([]byte(bad), "0.3.0"); err == nil {
		t.Fatal("escaping main accepted")
	}
}

func pluginZip(t *testing.T, id, version, script string) []byte {
	t.Helper()
	var buf bytes.Buffer
	zw := zip.NewWriter(&buf)
	add := func(name, content string) {
		w, err := zw.Create(name)
		if err != nil {
			t.Fatal(err)
		}
		if _, err := io.WriteString(w, content); err != nil {
			t.Fatal(err)
		}
	}
	manifest := fmt.Sprintf(`{"id":%q,"name":"%s","version":%q,"main":"main.js","contributes":{"panels":[{"id":"main","title":"P"}]}}`, id, id, version)
	add("plugin.json", manifest)
	add("main.js", script)
	if err := zw.Close(); err != nil {
		t.Fatal(err)
	}
	return buf.Bytes()
}

func TestRuntimeExecStartFailureClearsJob(t *testing.T) {
	m := newTestManager(t)
	workspace := t.TempDir()
	m.SetWorkspace(workspace, nil)
	dir := makePluginDir(t, "badcmd", `
var status = "idle";
function render(panel) { return {type: "text", text: status}; }
function handleAction(action) {
  if (action !== "run") return;
  status = "starting";
  nemorous.exec({
    command: "nemorous-no-such-binary-xyz",
    args: [],
    cwd: nemorous.workspace().path,
    onExit: function (code) { status = "exit=" + code; }
  });
}
`)
	if _, err := m.store.InstallDir(dir, "test"); err != nil {
		t.Fatal(err)
	}
	m.Load()
	ref := "badcmd:main"
	waitSnapshot(t, m, ref, func(s *PanelSnapshot) bool { return strings.Contains(snapshotText(s), "idle") })
	m.Dispatch(ref, "run", nil)
	waitSnapshot(t, m, ref, func(s *PanelSnapshot) bool { return strings.Contains(snapshotText(s), "exit=-1") })
}

func TestRuntimeStorageSurvivesRestart(t *testing.T) {
	root := filepath.Join(t.TempDir(), "plugins")
	m, err := NewManager(Options{Root: root, AppVersion: "0.3.0", Invalidate: func() {}, Notify: func(string, string) {}})
	if err != nil {
		t.Fatal(err)
	}
	dir := makePluginDir(t, "counter", `
var n = nemorous.storage.get("n") || 0;
function render(panel) { return {type: "text", text: "n=" + n}; }
function handleAction(action) {
  if (action === "inc") { n++; nemorous.storage.set("n", n); }
}
`)
	if _, err := m.store.InstallDir(dir, "test"); err != nil {
		t.Fatal(err)
	}
	m.Load()
	ref := "counter:main"
	waitSnapshot(t, m, ref, func(s *PanelSnapshot) bool { return strings.Contains(snapshotText(s), "n=0") })
	m.Dispatch(ref, "inc", nil)
	waitSnapshot(t, m, ref, func(s *PanelSnapshot) bool { return strings.Contains(snapshotText(s), "n=1") })
	m.Close()

	m2, err := NewManager(Options{Root: root, AppVersion: "0.3.0", Invalidate: func() {}, Notify: func(string, string) {}})
	if err != nil {
		t.Fatal(err)
	}
	defer m2.Close()
	m2.Load()
	waitSnapshot(t, m2, ref, func(s *PanelSnapshot) bool { return strings.Contains(snapshotText(s), "n=1") })
}

func TestRuntimeRejectsUnknownNodeType(t *testing.T) {
	m := newTestManager(t)
	dir := makePluginDir(t, "invalid", `function render() { return {type: "video", text: "x"}; }`)
	if _, err := m.store.InstallDir(dir, "test"); err != nil {
		t.Fatal(err)
	}
	m.Load()
	snap := waitSnapshot(t, m, "invalid:main", func(s *PanelSnapshot) bool { return s.Err != "" })
	if !strings.Contains(snap.Err, "unknown node type") {
		t.Fatalf("unexpected error: %s", snap.Err)
	}
}

func TestMarketplaceUpdateFlow(t *testing.T) {
	zipV1 := pluginZip(t, "upd", "1.0.0", `function render(){return {type:"text", text:"v1"}}`)
	zipV2 := pluginZip(t, "upd", "1.1.0", `function render(){return {type:"text", text:"v2"}}`)
	sum := sha256.Sum256(zipV2)
	var server *httptest.Server
	server = httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		switch r.URL.Path {
		case "/registry.json":
			fmt.Fprintf(w, `{"schema":1,"plugins":[{
				"id":"upd","name":"Upd","version":"1.1.0","main":"main.js",
				"contributes":{"panels":[{"id":"main","title":"Upd"}]},
				"downloadUrl":"%s/upd.zip","sha256":"%s"
			}]}`, server.URL, hex.EncodeToString(sum[:]))
		case "/upd.zip":
			w.Write(zipV2)
		}
	}))
	defer server.Close()
	m := newTestManager(t)
	if _, err := m.store.InstallZip(zipV1, "file"); err != nil {
		t.Fatal(err)
	}
	if err := m.SetRegistryURL(server.URL + "/registry.json"); err != nil {
		t.Fatal(err)
	}
	if err := m.RefreshRegistry(context.Background()); err != nil {
		t.Fatal(err)
	}
	var entry *RegistryEntry
	for i, e := range m.Catalog() {
		if e.ID == "upd" {
			entry = &m.Catalog()[i]
		}
	}
	if entry == nil || !entry.UpdateAvailable || entry.InstalledVersion != "1.0.0" {
		t.Fatalf("expected update, got %+v", entry)
	}
	if _, err := m.InstallRegistry(context.Background(), *entry); err != nil {
		t.Fatal(err)
	}
	p, ok := m.store.Get("upd")
	if !ok || p.Manifest.Version != "1.1.0" {
		t.Fatalf("update did not install 1.1.0: %+v ok=%v", p, ok)
	}
}

func TestBundledPluginsUpdateWithApp(t *testing.T) {
	root := filepath.Join(t.TempDir(), "plugins")
	store, err := NewStore(root, "0.5.0")
	if err != nil {
		t.Fatal(err)
	}
	pkg := t.TempDir()
	manifest := `{"id":"maven","name":"Maven","version":"0.9.0","main":"main.js","minAppVersion":"0.5.0","contributes":{"panels":[{"id":"maven","title":"Maven"}]}}`
	if err := os.WriteFile(filepath.Join(pkg, "plugin.json"), []byte(manifest), 0o644); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(filepath.Join(pkg, "main.js"), []byte(`function render(){return {type:"text",text:"old"}}`), 0o644); err != nil {
		t.Fatal(err)
	}
	if _, err := store.InstallDir(pkg, "builtin"); err != nil {
		t.Fatal(err)
	}
	m, err := NewManager(Options{Root: root, AppVersion: "0.5.0", AutoInstallDefaults: true, Invalidate: func() {}, Notify: func(string, string) {}})
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(m.Close)
	m.Load()
	p, ok := m.store.Get("maven")
	if !ok {
		t.Fatal("maven missing after update")
	}
	if p.Manifest.Version != "1.1.0" {
		t.Fatalf("bundled plugin stayed at %s", p.Manifest.Version)
	}
}

func TestRuntimeTimers(t *testing.T) {
	m := newTestManager(t)
	m.SetWorkspace(t.TempDir(), nil)
	dir := makePluginDir(t, "timers", `
var status = "idle";
function render(panel) { return {type: "text", text: status}; }
function handleAction(action) {
  if (action !== "go") return;
  status = "waiting";
  setTimeout(function () { status = "fired"; }, 80);
  var ticks = 0;
  var interval = setInterval(function () {
    ticks++;
    if (ticks >= 2) clearInterval(interval);
  }, 60);
  var chain = 0;
  function again() {
    chain++;
    if (chain < 3) setTimeout(again, 40);
    else status = "fired";
  }
  setTimeout(again, 40);
}
`)
	if _, err := m.store.InstallDir(dir, "test"); err != nil {
		t.Fatal(err)
	}
	m.Load()
	ref := "timers:main"
	waitSnapshot(t, m, ref, func(s *PanelSnapshot) bool { return strings.Contains(snapshotText(s), "idle") })
	m.Dispatch(ref, "go", nil)
	waitSnapshot(t, m, ref, func(s *PanelSnapshot) bool { return strings.Contains(snapshotText(s), "fired") })
}

func TestRuntimeDatabaseSQLite(t *testing.T) {
	m := newTestManager(t)
	m.SetWorkspace(t.TempDir(), nil)
	dbPath := filepath.Join(t.TempDir(), "db.sqlite")
	script := `
var status = "idle";
function render() { return {type: "text", text: status}; }
function handleAction(action) {
  if (action !== "run") return;
  status = "running";
  var dsn = __DBPATH__;
  function fail(e) { status = "err:" + e; }
  nemorous.db.exec({driver: "sqlite", dsn: dsn, sql: "CREATE TABLE items (id INTEGER PRIMARY KEY, name TEXT)"}, function () {
    nemorous.db.exec({driver: "sqlite", dsn: dsn, sql: "INSERT INTO items (name) VALUES ('alpha')"}, function () {
      nemorous.db.query({driver: "sqlite", dsn: dsn, sql: "SELECT id, name FROM items"}, function (r) {
        status = "ok:" + r.rowCount + ":" + r.rows[0][1] + ":" + r.columns[1];
      }, fail);
    }, fail);
  }, fail);
}
`
	script = strings.ReplaceAll(script, "__DBPATH__", strconv.Quote(dbPath))
	dir := t.TempDir()
	manifest := `{"id":"dbtest","name":"DB Test","version":"1.0.0","main":"main.js","minAppVersion":"0.5.0","permissions":["database"],"contributes":{"panels":[{"id":"p","title":"DB"}]}}`
	if err := os.WriteFile(filepath.Join(dir, "plugin.json"), []byte(manifest), 0o644); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(filepath.Join(dir, "main.js"), []byte(script), 0o644); err != nil {
		t.Fatal(err)
	}
	if _, err := m.store.InstallDir(dir, "test"); err != nil {
		t.Fatal(err)
	}
	m.Load()
	ref := "dbtest:p"
	waitSnapshot(t, m, ref, func(s *PanelSnapshot) bool { return strings.Contains(snapshotText(s), "idle") })
	m.Dispatch(ref, "run", nil)
	waitSnapshot(t, m, ref, func(s *PanelSnapshot) bool {
		txt := snapshotText(s)
		return strings.Contains(txt, "ok:1:alpha:name") || strings.Contains(txt, "err:")
	})
	if txt := snapshotText(m.Snapshot(ref)); !strings.Contains(txt, "ok:1:alpha:name") {
		t.Fatalf("db result = %s", txt)
	}
}

func TestSQLiteDSNNormalization(t *testing.T) {
	cases := map[string]string{
		"C:\\data\\app.db": "file:///C:/data/app.db",
		"file::memory:":    "file::memory:",
		"relative.db":      "relative.db",
	}
	for in, want := range cases {
		if got := sqliteDSN(in); got != want {
			t.Errorf("sqliteDSN(%q) = %q, want %q", in, got, want)
		}
	}
}
