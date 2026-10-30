package plugins

import (
	"context"
	"crypto/sha256"
	"encoding/hex"
	"fmt"
	"io"
	"io/fs"
	"net/http"
	"os"
	"path/filepath"
	"sort"
	"strings"
	"sync"
	"time"

	builtinplugins "github.com/iwangjie/nemorous/plugins"
)

// Repo is one workspace repository as plugins see it.
type Repo struct {
	ID   string `json:"id"`
	Name string `json:"name"`
	Path string `json:"path"`
}

// Workspace is the plugin-facing snapshot of the loaded workspace.
type Workspace struct {
	Path  string `json:"path"`
	Repos []Repo `json:"repos"`
}

// ActiveFile is the editor file a plugin may inspect with editor.read.
type ActiveFile struct {
	RepoID   string `json:"repoId"`
	RepoName string `json:"repoName"`
	Path     string `json:"path"`
	RelPath  string `json:"relPath"`
	Name     string `json:"name"`
	Content  string `json:"content"`
}

// PanelInfo identifies one plugin tool window.
type PanelInfo struct {
	Ref        string
	PluginID   string
	PluginName string
	PanelID    string
	Title      string
	Icon       string
	IconData   []byte
	Version    string
}

// CommandInfo is one plugin command in the palette.
type CommandInfo struct {
	PluginID   string
	PluginName string
	CommandID  string
	Title      string
	Action     string
}

// EditorInfo identifies one plugin editor contribution.
type EditorInfo struct {
	Ref        string
	PluginID   string
	PluginName string
	EditorID   string
	Title      string
	Patterns   []string
	Priority   int
}

// PanelSnapshot is the latest render() of one panel. It is immutable
// once published.
type PanelSnapshot struct {
	Ref       string
	PluginID  string
	PanelID   string
	Title     string
	Version   uint64
	Root      *Node
	Err       string
	UpdatedAt time.Time
}

// Options configures a Manager.
type Options struct {
	Root                string
	AppVersion          string
	Invalidate          func()
	Notify              func(kind, message string)
	AutoInstallDefaults bool
	ActiveFile          func() *ActiveFile
	OpenURL             func(url string) error
	OpenTerminal        func(command []string, dir, title string) error
	CloseTerminal       func() error
	FileWritten         func(path string)
}

// Manager owns installed plugins, their runtimes and the marketplace.
type Manager struct {
	appVersion string
	store      *Store
	opts       Options
	http       *http.Client

	mu        sync.RWMutex
	runtimes  map[string]*Runtime
	snapshots map[string]*PanelSnapshot
	inputs    map[string]*string
	workspace Workspace
	registry  *Registry
	remote    *Registry
	regErr    string
	regAt     time.Time
	closed    bool

	opMu           sync.Mutex
	runtimeMu      sync.Mutex
	refreshMu      sync.Mutex
	builtinOnce    sync.Once
	builtinReg     *Registry
	builtinErr     error
	defaultInstall bool
}

// NewManager opens the store under opts.Root.
func NewManager(opts Options) (*Manager, error) {
	store, err := NewStore(opts.Root, opts.AppVersion)
	if err != nil {
		return nil, err
	}
	return &Manager{
		appVersion: opts.AppVersion,
		store:      store,
		opts:       opts,
		http:       newHTTPClient(),
		runtimes:   make(map[string]*Runtime),
		snapshots:  make(map[string]*PanelSnapshot),
		inputs:     make(map[string]*string),
	}, nil
}

// AppVersion is the version compatibility checks use.
func (m *Manager) AppVersion() string { return m.appVersion }

// Root is the plugin store directory.
func (m *Manager) Root() string { return m.store.Root() }

// Load starts every enabled plugin that is not running yet.
func (m *Manager) Load() {
	if m.opts.AutoInstallDefaults {
		m.installDefaults()
	}
	for _, p := range m.store.List() {
		if !p.State.Enabled {
			continue
		}
		m.mu.RLock()
		running := m.runtimes[p.Manifest.ID] != nil
		m.mu.RUnlock()
		if !running {
			m.startRuntime(p)
		}
	}
}

// installDefaults installs editor plugins that must feel native on first run.
// A plugin the user explicitly uninstalled stays gone.
func (m *Manager) installDefaults() {
	reg := m.builtinRegistry()
	if reg == nil {
		return
	}
	for _, entry := range reg.Plugins {
		if !entry.Default || !entry.Builtin {
			continue
		}
		if _, installed := m.store.Get(entry.ID); installed || m.store.Dismissed(entry.ID) {
			continue
		}
		ctx, cancel := context.WithTimeout(context.Background(), 15*time.Second)
		m.defaultInstall = true
		_, err := m.InstallBuiltin(ctx, entry.ID)
		m.defaultInstall = false
		cancel()
		if err != nil {
			continue
		}
		_ = m.store.Undismiss(entry.ID)
	}
}

// Close stops all runtimes.
func (m *Manager) Close() {
	m.mu.Lock()
	if m.closed {
		m.mu.Unlock()
		return
	}
	m.closed = true
	runtimes := make([]*Runtime, 0, len(m.runtimes))
	for _, rt := range m.runtimes {
		runtimes = append(runtimes, rt)
	}
	m.runtimes = make(map[string]*Runtime)
	m.mu.Unlock()
	for _, rt := range runtimes {
		rt.stop()
	}
}

// SetWorkspace publishes a workspace snapshot to the plugins.
func (m *Manager) SetWorkspace(path string, repos []Repo) {
	copied := append([]Repo(nil), repos...)
	m.mu.Lock()
	changed := m.workspace.Path != path || !sameRepos(m.workspace.Repos, copied)
	m.workspace = Workspace{Path: path, Repos: copied}
	runtimes := make([]*Runtime, 0, len(m.runtimes))
	for _, rt := range m.runtimes {
		runtimes = append(runtimes, rt)
	}
	m.mu.Unlock()
	if !changed {
		return
	}
	for _, rt := range runtimes {
		rt.workspaceChanged()
	}
	m.invalidate()
}

// ActiveFile returns the current editor file, if any.
func (m *Manager) ActiveFile() *ActiveFile {
	if m.opts.ActiveFile == nil {
		return nil
	}
	return m.opts.ActiveFile()
}

// OpenURL opens an external URL after validating its scheme.
func (m *Manager) OpenURL(url string) error {
	if m.opts.OpenURL == nil {
		return fmt.Errorf("openURL: host does not support opening URLs")
	}
	return m.opts.OpenURL(url)
}

// OpenTerminal asks the host to open the embedded terminal with a command.
func (m *Manager) OpenTerminal(command []string, dir, title string) error {
	if m.opts.OpenTerminal == nil {
		return fmt.Errorf("terminal.open: host does not support terminals")
	}
	return m.opts.OpenTerminal(command, dir, title)
}

// CloseTerminal asks the host to close the embedded terminal.
func (m *Manager) CloseTerminal() error {
	if m.opts.CloseTerminal == nil {
		return fmt.Errorf("terminal.close: host does not support terminals")
	}
	return m.opts.CloseTerminal()
}

// FileWritten tells the host that a plugin changed a file, so an open editor
// can reload it instead of keeping a stale buffer.
func (m *Manager) FileWritten(path string) {
	if m.opts.FileWritten != nil {
		m.opts.FileWritten(path)
	}
}

// PathAllowed reports whether p stays inside the workspace or its repos.
func (m *Manager) PathAllowed(p string) bool {
	return m.pathAllowed(p, "")
}

// Workspace returns the current snapshot.
func (m *Manager) Workspace() Workspace {
	m.mu.RLock()
	defer m.mu.RUnlock()
	return Workspace{Path: m.workspace.Path, Repos: append([]Repo(nil), m.workspace.Repos...)}
}

func sameRepos(a, b []Repo) bool {
	if len(a) != len(b) {
		return false
	}
	for i := range a {
		if a[i] != b[i] {
			return false
		}
	}
	return true
}

// EditorForFile returns the enabled editor contribution with the highest
// priority whose pattern matches file.
func (m *Manager) EditorForFile(file string) (EditorInfo, bool) {
	base := filepath.Base(file)
	slashed := filepath.ToSlash(file)
	var best EditorInfo
	found := false
	for _, p := range m.store.List() {
		if !p.State.Enabled {
			continue
		}
		for _, e := range p.Manifest.Contributes.Editors {
			matched := false
			for _, pattern := range e.Patterns {
				if ok, _ := filepath.Match(pattern, base); ok {
					matched = true
					break
				}
				if ok, _ := filepath.Match(pattern, slashed); ok {
					matched = true
					break
				}
			}
			if !matched {
				continue
			}
			if found && e.Priority <= best.Priority {
				continue
			}
			best = EditorInfo{
				Ref:        panelRef(p.Manifest.ID, e.ID),
				PluginID:   p.Manifest.ID,
				PluginName: p.Manifest.Name,
				EditorID:   e.ID,
				Title:      e.Title,
				Patterns:   append([]string(nil), e.Patterns...),
				Priority:   e.Priority,
			}
			if best.Title == "" {
				best.Title = p.Manifest.Name
			}
			found = true
		}
	}
	return best, found
}

// Panels lists the tool windows of enabled plugins.
func (m *Manager) Panels() []PanelInfo {
	var out []PanelInfo
	for _, p := range m.store.List() {
		if !p.State.Enabled {
			continue
		}
		var iconData []byte
		if p.Manifest.Icon != "" {
			if data, err := os.ReadFile(filepath.Join(p.Dir, p.Manifest.Icon)); err == nil && len(data) <= 64<<10 {
				iconData = data
			}
		}
		for _, panel := range p.Manifest.Contributes.Panels {
			out = append(out, PanelInfo{
				Ref:        panelRef(p.Manifest.ID, panel.ID),
				PluginID:   p.Manifest.ID,
				PluginName: p.Manifest.Name,
				PanelID:    panel.ID,
				Title:      panel.Title,
				Icon:       panel.Icon,
				IconData:   iconData,
				Version:    p.Manifest.Version,
			})
		}
	}
	sort.Slice(out, func(i, j int) bool {
		if out[i].PluginName != out[j].PluginName {
			return out[i].PluginName < out[j].PluginName
		}
		return out[i].Title < out[j].Title
	})
	return out
}

// Commands lists plugin commands for the command palette.
func (m *Manager) Commands() []CommandInfo {
	var out []CommandInfo
	for _, p := range m.store.List() {
		if !p.State.Enabled {
			continue
		}
		for _, cmd := range p.Manifest.Contributes.Commands {
			action := cmd.Action
			if action == "" {
				action = cmd.ID
			}
			out = append(out, CommandInfo{
				PluginID:   p.Manifest.ID,
				PluginName: p.Manifest.Name,
				CommandID:  cmd.ID,
				Title:      cmd.Title,
				Action:     action,
			})
		}
	}
	return out
}

// Panel returns one panel's contribution.
func (m *Manager) Panel(ref string) (PanelInfo, bool) {
	pluginID, panelID := splitRef(ref)
	for _, p := range m.Panels() {
		if p.PluginID == pluginID && p.PanelID == panelID {
			return p, true
		}
	}
	return PanelInfo{}, false
}

func panelRef(pluginID, panelID string) string { return pluginID + ":" + panelID }

func splitRef(ref string) (pluginID, panelID string) {
	if i := strings.IndexByte(ref, ':'); i >= 0 {
		return ref[:i], ref[i+1:]
	}
	return ref, ""
}

// Snapshot returns the latest render of a panel.
func (m *Manager) Snapshot(ref string) *PanelSnapshot {
	m.mu.RLock()
	defer m.mu.RUnlock()
	return m.snapshots[ref]
}

// Input returns the stable value behind a declarative text input.
func (m *Manager) Input(ref, key, initial string) *string {
	m.mu.Lock()
	defer m.mu.Unlock()
	k := ref + "\x00" + key
	if v, ok := m.inputs[k]; ok {
		return v
	}
	v := initial
	m.inputs[k] = &v
	return &v
}

// Dispatch runs a panel action in its plugin.
func (m *Manager) Dispatch(ref, action string, payload any) {
	if action == "" {
		return
	}
	pluginID, panelID := splitRef(ref)
	m.mu.RLock()
	rt := m.runtimes[pluginID]
	m.mu.RUnlock()
	if rt == nil {
		return
	}
	rt.action(panelID, action, payload)
}

// DispatchCommand runs a plugin command.
func (m *Manager) DispatchCommand(pluginID, action string) {
	if action == "" {
		return
	}
	m.mu.RLock()
	rt := m.runtimes[pluginID]
	m.mu.RUnlock()
	if rt == nil {
		return
	}
	rt.action("", action, nil)
}

func (m *Manager) startRuntime(p Plugin) {
	m.runtimeMu.Lock()
	defer m.runtimeMu.Unlock()
	rt := newRuntime(m, p)
	m.mu.Lock()
	if m.closed {
		m.mu.Unlock()
		return
	}
	old := m.runtimes[p.Manifest.ID]
	if old != nil {
		delete(m.runtimes, p.Manifest.ID)
	}
	m.mu.Unlock()
	if old != nil {
		old.stop()
	}
	m.mu.Lock()
	if m.closed {
		m.mu.Unlock()
		return
	}
	m.runtimes[p.Manifest.ID] = rt
	m.mu.Unlock()
	go rt.run()
}

func (m *Manager) stopRuntime(id string) {
	m.runtimeMu.Lock()
	defer m.runtimeMu.Unlock()
	m.mu.Lock()
	rt := m.runtimes[id]
	delete(m.runtimes, id)
	m.mu.Unlock()
	if rt != nil {
		rt.stop()
	}
	m.mu.Lock()
	for ref := range m.snapshots {
		if strings.HasPrefix(ref, id+":") {
			delete(m.snapshots, ref)
		}
	}
	for key := range m.inputs {
		if strings.HasPrefix(key, id+":") {
			delete(m.inputs, key)
		}
	}
	m.mu.Unlock()
}

func (m *Manager) publish(snap *PanelSnapshot) {
	m.mu.Lock()
	if m.closed {
		m.mu.Unlock()
		return
	}
	m.snapshots[snap.Ref] = snap
	m.mu.Unlock()
	m.invalidate()
}

func (m *Manager) invalidate() {
	if m.opts.Invalidate != nil {
		m.opts.Invalidate()
	}
}

func (m *Manager) notify(kind, message string) {
	if m.opts.Notify != nil {
		m.opts.Notify(kind, message)
	}
}

// RegistryEntry describes a marketplace listing and its local state.
func (m *Manager) Catalog() []RegistryEntry {
	reg := m.Registry()
	if reg == nil {
		return nil
	}
	installed := make(map[string]Plugin)
	for _, p := range m.store.List() {
		installed[p.Manifest.ID] = p
	}
	out := make([]RegistryEntry, 0, len(reg.Plugins))
	for _, rp := range reg.Plugins {
		entry := RegistryEntry{RegistryPlugin: rp, Compatible: appCompatible(m.appVersion, rp.MinAppVersion)}
		if !entry.Compatible {
			entry.IncompatibleWhy = "需要 Nemorous " + rp.MinAppVersion + " 或更高版本"
		}
		if p, ok := installed[rp.ID]; ok {
			entry.Installed = true
			entry.InstalledVersion = p.Manifest.Version
			entry.Enabled = p.State.Enabled
			entry.UpdateAvailable = entry.Compatible && CompareVersions(rp.Version, p.Manifest.Version) > 0
			if !entry.Compatible {
				entry.UpdateAvailable = false
			}
		}
		out = append(out, entry)
	}
	// Installed plugins that the registry no longer lists stay manageable.
	listed := make(map[string]bool, len(out))
	for _, e := range out {
		listed[e.ID] = true
	}
	for _, p := range m.store.List() {
		if listed[p.Manifest.ID] {
			continue
		}
		out = append(out, RegistryEntry{
			RegistryPlugin:   RegistryPlugin{Manifest: *p.Manifest, Official: false},
			Installed:        true,
			InstalledVersion: p.Manifest.Version,
			Enabled:          p.State.Enabled,
			Compatible:       true,
		})
	}
	return out
}

// Installed returns every installed plugin.
func (m *Manager) Installed() []Plugin { return m.store.List() }

// Registry is the merged embedded/remote marketplace index.
func (m *Manager) Registry() *Registry {
	m.mu.RLock()
	defer m.mu.RUnlock()
	if m.registry != nil {
		return m.registry
	}
	return m.builtinRegistry()
}

func (m *Manager) builtinRegistry() *Registry {
	m.builtinOnce.Do(func() {
		data, err := fs.ReadFile(builtinplugins.Builtin, "registry.json")
		if err != nil {
			m.builtinErr = err
			return
		}
		reg, err := parseRegistry(data)
		if err != nil {
			m.builtinErr = err
			return
		}
		m.builtinReg = reg
	})
	return m.builtinReg
}

// RegistryError is the last remote fetch failure, if the embedded index
// is being used instead.
func (m *Manager) RegistryError() string {
	m.mu.RLock()
	defer m.mu.RUnlock()
	return m.regErr
}

// RegistryURL is the configured remote index.
func (m *Manager) RegistryURL() string {
	if u := m.store.RegistryURL(); u != "" {
		return u
	}
	return DefaultRegistryURL
}

// SetRegistryURL saves a custom index; "" restores the official one.
func (m *Manager) SetRegistryURL(url string) error {
	return m.store.SetRegistryURL(url)
}

// RefreshRegistry fetches the remote index and merges it over the
// embedded one. A failure keeps the embedded index usable and is
// reported through RegistryError.
func (m *Manager) RefreshRegistry(ctx context.Context) error {
	m.refreshMu.Lock()
	defer m.refreshMu.Unlock()
	url := m.RegistryURL()
	reg, err := fetchRegistry(m.http, url)
	base := m.builtinRegistry()
	if err != nil {
		m.mu.Lock()
		m.regErr = err.Error()
		if m.registry == nil && base != nil {
			m.registry = base
		}
		m.mu.Unlock()
		m.invalidate()
		return err
	}
	merged := mergeRegistries(base, reg)
	m.mu.Lock()
	m.remote = reg
	m.registry = merged
	m.regErr = ""
	m.regAt = time.Now()
	m.mu.Unlock()
	m.invalidate()
	return nil
}

// RegistryUpdatedAt reports when the remote index was fetched.
func (m *Manager) RegistryUpdatedAt() time.Time {
	m.mu.RLock()
	defer m.mu.RUnlock()
	return m.regAt
}

// InstallBuiltin installs an official plugin shipped in the binary.
func (m *Manager) InstallBuiltin(ctx context.Context, id string) (*Manifest, error) {
	reg := m.builtinRegistry()
	if reg == nil {
		return nil, fmt.Errorf("内置插件清单不可用")
	}
	var entry *RegistryPlugin
	for i := range reg.Plugins {
		if reg.Plugins[i].ID == id && reg.Plugins[i].Builtin {
			entry = &reg.Plugins[i]
			break
		}
	}
	if entry == nil {
		return nil, fmt.Errorf("没有内置插件 %s", id)
	}
	sub := "builtin/" + id
	if _, err := fs.Stat(builtinplugins.Builtin, sub); err != nil {
		// Official packages live at plugins/<id> in the repository.
		sub = id
	}
	return m.install(func() (*Manifest, error) {
		m.stopRuntime(id)
		return m.store.InstallFS(builtinplugins.Builtin, sub, "builtin")
	})
}

// InstallRegistry installs or updates one marketplace entry.
func (m *Manager) InstallRegistry(ctx context.Context, entry RegistryEntry) (*Manifest, error) {
	if entry.Builtin {
		return m.InstallBuiltin(ctx, entry.ID)
	}
	if entry.DownloadURL == "" {
		return nil, fmt.Errorf("插件 %s 缺少下载地址", entry.ID)
	}
	data, err := m.download(ctx, entry.DownloadURL, entry.SHA256, entry.Size)
	if err != nil {
		return nil, err
	}
	manifest, err := m.install(func() (*Manifest, error) {
		m.stopRuntime(entry.ID)
		return m.store.InstallZip(data, "registry")
	})
	if err != nil {
		return nil, err
	}
	if manifest.ID != entry.ID {
		_ = m.Uninstall(manifest.ID)
		return nil, fmt.Errorf("插件包 id %s 与市场条目 %s 不一致", manifest.ID, entry.ID)
	}
	return manifest, nil
}

// InstallFile installs a plugin zip the user picked.
func (m *Manager) InstallFile(ctx context.Context, path string) (*Manifest, error) {
	info, err := os.Stat(path)
	if err != nil {
		return nil, err
	}
	if info.Size() > maxArchiveTotal {
		return nil, fmt.Errorf("插件包超过 32 MB")
	}
	data, err := os.ReadFile(path)
	if err != nil {
		return nil, err
	}
	return m.install(func() (*Manifest, error) {
		return m.store.InstallZip(data, "file")
	})
}

func (m *Manager) install(fn func() (*Manifest, error)) (*Manifest, error) {
	m.opMu.Lock()
	defer m.opMu.Unlock()
	manifest, err := fn()
	if err != nil {
		return nil, err
	}
	p, ok := m.store.Get(manifest.ID)
	if !ok {
		return nil, fmt.Errorf("插件安装后无法读取")
	}
	// A plugin already running must reload the new files.
	m.stopRuntime(p.Manifest.ID)
	if p.State.Enabled {
		m.startRuntime(p)
	}
	if !m.defaultInstall {
		m.notify("ok", fmt.Sprintf("已安装插件 %s %s", manifest.Name, manifest.Version))
	}
	m.invalidate()
	return manifest, nil
}

// Uninstall removes a plugin and its data.
func (m *Manager) Uninstall(id string) error {
	m.opMu.Lock()
	defer m.opMu.Unlock()
	name := id
	if p, ok := m.store.Get(id); ok {
		name = p.Manifest.Name
	}
	p, had := m.store.Get(id)
	m.stopRuntime(id)
	if err := m.store.Uninstall(id); err != nil {
		return err
	}
	if had && p.State.Source == "builtin" {
		_ = m.store.Dismiss(id)
	}
	m.notify("ok", "已卸载插件 "+name)
	m.invalidate()
	return nil
}

// SetEnabled turns a plugin on or off, starting or stopping its runtime.
func (m *Manager) SetEnabled(id string, enabled bool) error {
	m.opMu.Lock()
	defer m.opMu.Unlock()
	if err := m.store.SetEnabled(id, enabled); err != nil {
		return err
	}
	name := id
	if p, ok := m.store.Get(id); ok {
		name = p.Manifest.Name
	}
	if enabled {
		if p, ok := m.store.Get(id); ok {
			m.startRuntime(p)
		}
		m.notify("ok", "已启用插件 "+name)
	} else {
		m.stopRuntime(id)
		m.notify("ok", "已禁用插件 "+name)
	}
	m.invalidate()
	return nil
}

func (m *Manager) download(ctx context.Context, url, wantSHA string, wantSize int64) ([]byte, error) {
	req, err := http.NewRequestWithContext(ctx, http.MethodGet, url, nil)
	if err != nil {
		return nil, err
	}
	req.Header.Set("User-Agent", "Nemorous/"+m.appVersion)
	resp, err := m.http.Do(req)
	if err != nil {
		return nil, err
	}
	defer resp.Body.Close()
	if resp.StatusCode != http.StatusOK {
		return nil, fmt.Errorf("下载失败：HTTP %s", resp.Status)
	}
	data, err := io.ReadAll(io.LimitReader(resp.Body, maxArchiveTotal+1))
	if err != nil {
		return nil, err
	}
	if len(data) > maxArchiveTotal {
		return nil, fmt.Errorf("插件包超过 32 MB")
	}
	if wantSize > 0 && int64(len(data)) != wantSize {
		return nil, fmt.Errorf("插件包大小不符")
	}
	if wantSHA != "" {
		sum := sha256.Sum256(data)
		got := hex.EncodeToString(sum[:])
		if !strings.EqualFold(got, strings.TrimSpace(wantSHA)) {
			return nil, fmt.Errorf("插件包 SHA256 校验失败")
		}
	}
	return data, nil
}

// installZipForTest is used by tests to install a zip without a file.
func (m *Manager) installZipForTest(data []byte, source string) (*Manifest, error) {
	return m.install(func() (*Manifest, error) {
		return m.store.InstallZip(data, source)
	})
}
