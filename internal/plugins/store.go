package plugins

import (
	"encoding/json"
	"errors"
	"fmt"
	"io/fs"
	"os"
	"path/filepath"
	"sort"
	"strings"
	"sync"
	"time"
)

// InstalledState is what Nemorous remembers about an installed plugin
// between launches. The package itself (plugin.json) is the source of
// truth for its version.
type InstalledState struct {
	Version     string `json:"version"`
	Enabled     bool   `json:"enabled"`
	Source      string `json:"source,omitempty"`
	InstalledAt string `json:"installedAt,omitempty"`
}

// State is state.json, kept beside installed projects.
type State struct {
	RegistryURL string                    `json:"registryUrl,omitempty"`
	Installed   map[string]InstalledState `json:"installed"`
}

// Plugin is one installed package.
type Plugin struct {
	Manifest *Manifest
	Dir      string
	DataDir  string
	State    InstalledState
}

// Store owns the installed plugin packages and state.json under one
// directory.
type Store struct {
	root       string
	appVersion string

	mu    sync.Mutex
	state State
}

// NewStore opens (and creates) a plugin store.
func NewStore(root, appVersion string) (*Store, error) {
	s := &Store{root: root, appVersion: appVersion}
	for _, dir := range []string{s.installedDir(), s.dataDir(), s.tmpDir()} {
		if err := os.MkdirAll(dir, 0o755); err != nil {
			return nil, err
		}
	}
	if err := s.load(); err != nil {
		return nil, err
	}
	return s, nil
}

func (s *Store) installedDir() string { return filepath.Join(s.root, "installed") }
func (s *Store) dataDir() string      { return filepath.Join(s.root, "data") }
func (s *Store) tmpDir() string       { return filepath.Join(s.root, "tmp") }
func (s *Store) statePath() string    { return filepath.Join(s.root, "state.json") }

// Root is the directory holding state.json and the installed packages.
func (s *Store) Root() string { return s.root }

func (s *Store) load() error {
	s.mu.Lock()
	defer s.mu.Unlock()
	s.state = State{Installed: make(map[string]InstalledState)}
	if data, err := os.ReadFile(s.statePath()); err == nil {
		_ = json.Unmarshal(data, &s.state)
	} else if !errors.Is(err, os.ErrNotExist) {
		return err
	}
	if s.state.Installed == nil {
		s.state.Installed = make(map[string]InstalledState)
	}
	return s.reconcileLocked()
}

// reconcileLocked matches state.json with what is on disk: an entry whose
// package disappeared is forgotten, and a package installed before a crash
// gets an entry.
func (s *Store) reconcileLocked() error {
	entries, err := os.ReadDir(s.installedDir())
	if err != nil && !errors.Is(err, os.ErrNotExist) {
		return err
	}
	onDisk := make(map[string]bool)
	for _, e := range entries {
		if !e.IsDir() {
			continue
		}
		id := e.Name()
		dir := filepath.Join(s.installedDir(), id)
		m, err := readManifestFile(dir, s.appVersion)
		if err != nil {
			continue
		}
		if m.ID != id {
			continue
		}
		onDisk[id] = true
		st, ok := s.state.Installed[id]
		if !ok {
			st = InstalledState{Enabled: true, Source: "recovered", InstalledAt: time.Now().UTC().Format(time.RFC3339)}
		}
		st.Version = m.Version
		s.state.Installed[id] = st
	}
	for id := range s.state.Installed {
		if !onDisk[id] {
			delete(s.state.Installed, id)
		}
	}
	return s.saveLocked()
}

func (s *Store) saveLocked() error {
	data, err := json.MarshalIndent(s.state, "", "  ")
	if err != nil {
		return err
	}
	data = append(data, '\n')
	tmp := s.statePath() + ".tmp"
	if err := os.WriteFile(tmp, data, 0o644); err != nil {
		return err
	}
	return os.Rename(tmp, s.statePath())
}

// List returns installed plugins sorted by name.
func (s *Store) List() []Plugin {
	s.mu.Lock()
	defer s.mu.Unlock()
	out := make([]Plugin, 0, len(s.state.Installed))
	for id, st := range s.state.Installed {
		dir := filepath.Join(s.installedDir(), id)
		m, err := readManifestFile(dir, s.appVersion)
		if err != nil {
			continue
		}
		out = append(out, Plugin{
			Manifest: m,
			Dir:      dir,
			DataDir:  filepath.Join(s.dataDir(), id),
			State:    st,
		})
	}
	sort.Slice(out, func(i, j int) bool {
		return strings.ToLower(out[i].Manifest.Name) < strings.ToLower(out[j].Manifest.Name)
	})
	return out
}

// Get returns one installed plugin.
func (s *Store) Get(id string) (Plugin, bool) {
	s.mu.Lock()
	defer s.mu.Unlock()
	st, ok := s.state.Installed[id]
	if !ok {
		return Plugin{}, false
	}
	dir := filepath.Join(s.installedDir(), id)
	m, err := readManifestFile(dir, s.appVersion)
	if err != nil {
		return Plugin{}, false
	}
	return Plugin{Manifest: m, Dir: dir, DataDir: filepath.Join(s.dataDir(), id), State: st}, true
}

// InstallFS installs a package from fsys (sub is its root inside fsys).
func (s *Store) InstallFS(fsys fs.FS, sub, source string) (*Manifest, error) {
	s.mu.Lock()
	defer s.mu.Unlock()
	stage, err := os.MkdirTemp(s.tmpDir(), "install-")
	if err != nil {
		return nil, err
	}
	defer os.RemoveAll(stage)
	if err := copyFS(fsys, sub, stage); err != nil {
		return nil, err
	}
	return s.installStagedLocked(stage, source)
}

// InstallZip installs a zip package.
func (s *Store) InstallZip(data []byte, source string) (*Manifest, error) {
	s.mu.Lock()
	defer s.mu.Unlock()
	stage, err := os.MkdirTemp(s.tmpDir(), "unzip-")
	if err != nil {
		return nil, err
	}
	defer os.RemoveAll(stage)
	root, err := extractZip(data, stage)
	if err != nil {
		return nil, err
	}
	return s.installStagedLocked(root, source)
}

// InstallDir installs a package directory (used by tests and local installs).
func (s *Store) InstallDir(dir, source string) (*Manifest, error) {
	s.mu.Lock()
	defer s.mu.Unlock()
	stage, err := os.MkdirTemp(s.tmpDir(), "copy-")
	if err != nil {
		return nil, err
	}
	defer os.RemoveAll(stage)
	if err := copyTree(dir, stage); err != nil {
		return nil, err
	}
	return s.installStagedLocked(stage, source)
}

// installStagedLocked validates the staged package and swaps it into place.
// s.mu is held.
func (s *Store) installStagedLocked(stage, source string) (*Manifest, error) {
	m, err := readManifestFile(stage, s.appVersion)
	if err != nil {
		return nil, err
	}
	target := filepath.Join(s.installedDir(), m.ID)
	var backup string
	if _, err := os.Stat(target); err == nil {
		backup, err = os.MkdirTemp(s.tmpDir(), "backup-")
		if err != nil {
			return nil, err
		}
		os.Remove(backup)
		if err := os.Rename(target, backup); err != nil {
			return nil, err
		}
	}
	if err := os.Rename(stage, target); err != nil {
		if backup != "" {
			_ = os.Rename(backup, target)
		}
		return nil, err
	}
	if err := os.MkdirAll(filepath.Join(s.dataDir(), m.ID), 0o755); err != nil {
		_ = os.RemoveAll(target)
		if backup != "" {
			_ = os.Rename(backup, target)
		}
		return nil, err
	}
	prev, existed := s.state.Installed[m.ID]
	enabled := true
	installedAt := time.Now().UTC().Format(time.RFC3339)
	if existed {
		enabled = prev.Enabled
		if prev.InstalledAt != "" {
			installedAt = prev.InstalledAt
		}
	}
	s.state.Installed[m.ID] = InstalledState{
		Version:     m.Version,
		Enabled:     enabled,
		Source:      source,
		InstalledAt: installedAt,
	}
	if err := s.saveLocked(); err != nil {
		_ = os.RemoveAll(target)
		if backup != "" {
			_ = os.Rename(backup, target)
		}
		return nil, err
	}
	if backup != "" {
		_ = os.RemoveAll(backup)
	}
	return m, nil
}

// Uninstall removes a plugin's package and its stored data.
func (s *Store) Uninstall(id string) error {
	s.mu.Lock()
	defer s.mu.Unlock()
	if _, ok := s.state.Installed[id]; !ok {
		return fmt.Errorf("plugin %s is not installed", id)
	}
	if err := os.RemoveAll(filepath.Join(s.installedDir(), id)); err != nil {
		return err
	}
	if err := os.RemoveAll(filepath.Join(s.dataDir(), id)); err != nil {
		return err
	}
	delete(s.state.Installed, id)
	return s.saveLocked()
}

// SetEnabled turns a plugin on or off and records it.
func (s *Store) SetEnabled(id string, enabled bool) error {
	s.mu.Lock()
	defer s.mu.Unlock()
	st, ok := s.state.Installed[id]
	if !ok {
		return fmt.Errorf("plugin %s is not installed", id)
	}
	st.Enabled = enabled
	s.state.Installed[id] = st
	return s.saveLocked()
}

// SetRegistryURL records a custom marketplace index; "" restores the
// official one.
func (s *Store) SetRegistryURL(url string) error {
	s.mu.Lock()
	defer s.mu.Unlock()
	s.state.RegistryURL = strings.TrimSpace(url)
	return s.saveLocked()
}

// RegistryURL is the configured custom index, if any.
func (s *Store) RegistryURL() string {
	s.mu.Lock()
	defer s.mu.Unlock()
	return s.state.RegistryURL
}

// copyFS copies a subtree of fsys into dst.
func copyFS(fsys fs.FS, sub, dst string) error {
	sub = strings.Trim(sub, "/")
	if sub == "" {
		sub = "."
	}
	return fs.WalkDir(fsys, sub, func(p string, d fs.DirEntry, err error) error {
		if err != nil {
			return err
		}
		rel, err := filepath.Rel(filepath.FromSlash(sub), filepath.FromSlash(p))
		if err != nil {
			return err
		}
		if rel == "." {
			return nil
		}
		target := filepath.Join(dst, rel)
		if d.IsDir() {
			return os.MkdirAll(target, 0o755)
		}
		data, err := fs.ReadFile(fsys, p)
		if err != nil {
			return err
		}
		if err := os.MkdirAll(filepath.Dir(target), 0o755); err != nil {
			return err
		}
		return os.WriteFile(target, data, 0o644)
	})
}
