// Package session persists the window's working set between launches.
package session

import (
	"encoding/json"
	"os"
	"path/filepath"
)

// File is one restored code tab.
type File struct {
	RepoID  string `json:"repoId"`
	RelPath string `json:"path"`
	Line    int    `json:"line,omitempty"`
}

// State is the last session's layout and open files.
type State struct {
	Workspace         string  `json:"workspace,omitempty"`
	OpenFiles         []File  `json:"openFiles,omitempty"`
	ActiveFile        int     `json:"activeFile,omitempty"`
	ActiveTab         string  `json:"activeTab,omitempty"`
	ShowGitPanel      bool    `json:"showGitPanel,omitempty"`
	ShowTerminalPanel bool    `json:"showTerminalPanel,omitempty"`
	ShowLogPanel      bool    `json:"showLogPanel,omitempty"`
	ShowManiPanel     bool    `json:"showManiPanel,omitempty"`
	PluginPanel       string  `json:"pluginPanel,omitempty"`
	ChangesSplit      float32 `json:"changesSplit,omitempty"`
	CodeSplit         float32 `json:"codeSplit,omitempty"`
	GitEditorPct      float32 `json:"gitEditorPct,omitempty"`
}

// Load reads a session; a missing or broken file yields an empty state.
func Load(path string) State {
	var st State
	data, err := os.ReadFile(path)
	if err != nil {
		return State{}
	}
	_ = json.Unmarshal(data, &st)
	if len(st.OpenFiles) > 20 {
		st.OpenFiles = st.OpenFiles[:20]
	}
	return st
}

// Save writes a session atomically.
func Save(path string, st State) error {
	if len(st.OpenFiles) > 20 {
		st.OpenFiles = st.OpenFiles[:20]
	}
	data, err := json.MarshalIndent(st, "", "  ")
	if err != nil {
		return err
	}
	if err := os.MkdirAll(filepath.Dir(path), 0o755); err != nil {
		return err
	}
	tmp := path + ".tmp"
	if err := os.WriteFile(tmp, append(data, '\n'), 0o644); err != nil {
		return err
	}
	return os.Rename(tmp, path)
}
