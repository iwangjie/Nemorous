package config

import (
	"encoding/json"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"time"
)

// BranchPreset stores a named multi-repository branch combination (e.g. "AAA 需求").
type BranchPreset struct {
	Name         string            `json:"name"`          // e.g. "AAA 需求"
	RepoBranches map[string]string `json:"repo_branches"` // repoID -> targetBranch
	UpdatedAt    string            `json:"updated_at"`
}

// MergePreset stores a named merge plan: target branch plus the repositories to merge into it.
type MergePreset struct {
	Name         string   `json:"name"`          // e.g. "合并到 test"
	TargetBranch string   `json:"target_branch"` // e.g. "test"
	RepoIDs      []string `json:"repo_ids"`      // repositories to include
	UpdatedAt    string   `json:"updated_at"`
}

// Config holds persistent application configuration for Nemorous.
type Config struct {
	WorkspacePath        string           `json:"workspace_path"`
	RecentWorkspaces     []string         `json:"recent_workspaces"`
	Theme                string           `json:"theme"`                 // "light", "dark", "darcula", "system"
	DefaultTargetBranch  string           `json:"default_target_branch"` // e.g. "test", "main"
	AIAgent              string           `json:"ai_agent"`              // "auto", "omp", "pi", "claude", "codex"
	Concurrency          int              `json:"concurrency"`           // parallel git workers (default 8)
	DiffSideBySide       bool             `json:"diff_side_by_side"`
	DiffInlineMode       string           `json:"diff_inline_mode"` // "word", "char", "none"
	EditorFontSize       float32          `json:"editor_font_size"`
	EditorSoftWrap       bool             `json:"editor_soft_wrap"`
	Zoom                 float32          `json:"zoom,omitempty"` // global UI zoom scaling factor (default 1.0)
	NavShowLabels        bool             `json:"nav_show_labels"`
	IncludeRootRepo      bool             `json:"include_root_repo"`
	ManiEnabled          bool             `json:"mani_enabled"`
	AutoUpdate           bool             `json:"auto_update"`          // check GitHub Releases for signed updates
	AutoUpdateIdle       bool             `json:"auto_update_idle"`     // install while the app is in the background
	UpdateCheckMinutes   int              `json:"update_check_minutes"` // foreground check interval
	UpdateMirror         string           `json:"update_mirror"`        // GitHub proxy prefix for faster downloads
	SkippedUpdateVersion string           `json:"skipped_update_version,omitempty"`
	ProtectedBranches    []string         `json:"protected_branches"`
	BranchPresets        []BranchPreset   `json:"branch_presets,omitempty"`
	MergePresets         []MergePreset    `json:"merge_presets,omitempty"`
	ChangeLists          []ChangeList     `json:"change_lists,omitempty"`
	ActiveChangeList     string           `json:"active_change_list,omitempty"`
	FormatOnSave         bool             `json:"format_on_save"`
	FormatCommand        string           `json:"format_command,omitempty"`
	RunConfigs           []RunConfig      `json:"run_configs,omitempty"`
	RunPanelHeight       float32          `json:"run_panel_height,omitempty"`
	DebugAdapter         string           `json:"debug_adapter,omitempty"`
	DebugAdapterArgs     string           `json:"debug_adapter_args,omitempty"`
	Bookmarks            []Bookmark       `json:"bookmarks,omitempty"`
	Breakpoints          map[string][]int `json:"breakpoints,omitempty"`
}

// Default returns a sensible default configuration.
func Default() Config {
	ws := detectDefaultWorkspace()
	var recents []string
	if ws != "" {
		recents = []string{ws}
	}
	return Config{
		WorkspacePath:       ws,
		RecentWorkspaces:    recents,
		Theme:               "light",
		DefaultTargetBranch: "test",
		AIAgent:             "auto",
		Concurrency:         8,
		DiffSideBySide:      true,
		DiffInlineMode:      "word",
		EditorFontSize:      14,
		EditorSoftWrap:      true,
		Zoom:                1.0,
		IncludeRootRepo:     true,
		ManiEnabled:         true,
		AutoUpdate:          true,
		AutoUpdateIdle:      true,
		UpdateCheckMinutes:  30,
		UpdateMirror:        "https://ghfast.top",
		ProtectedBranches:   []string{"main", "master", "test", "dev", "prod", "production"},
	}
}

func detectDefaultWorkspace() string {
	if env := os.Getenv("NEMOROUS_WORKSPACE"); env != "" {
		if info, err := os.Stat(env); err == nil && info.IsDir() {
			return env
		}
	}
	home, _ := os.UserHomeDir()
	if home != "" {
		candidate := filepath.Join(home, "work", "workspace")
		if info, err := os.Stat(candidate); err == nil && info.IsDir() {
			return candidate
		}
	}
	cwd, err := os.Getwd()
	if err == nil {
		return cwd
	}
	return ""
}

// ConfigPath returns the path to the JSON config file.
func ConfigPath() string {
	if custom := os.Getenv("NEMOROUS_CONFIG"); custom != "" {
		return custom
	}
	dir, err := os.UserConfigDir()
	if err != nil || dir == "" {
		home, _ := os.UserHomeDir()
		dir = filepath.Join(home, ".config")
	}
	return filepath.Join(dir, "nemorous", "config.json")
}

// Load reads config from disk or returns Default() if missing/invalid.
func Load() Config {
	cfg := Default()
	data, err := os.ReadFile(ConfigPath())
	if err == nil {
		_ = json.Unmarshal(data, &cfg)
	}
	if envWs := os.Getenv("NEMOROUS_WORKSPACE"); envWs != "" {
		cfg.WorkspacePath = envWs
	}
	if cfg.WorkspacePath == "" {
		cfg.WorkspacePath = detectDefaultWorkspace()
	} else if info, statErr := os.Stat(cfg.WorkspacePath); statErr != nil || !info.IsDir() {
		cfg.WorkspacePath = detectDefaultWorkspace()
	}
	if cfg.Concurrency <= 0 {
		cfg.Concurrency = 8
	}
	if cfg.EditorFontSize < 10 || cfg.EditorFontSize > 28 {
		cfg.EditorFontSize = 14
	}
	if cfg.Zoom < 0.5 || cfg.Zoom > 3.0 {
		cfg.Zoom = 1.0
	}
	if cfg.Theme == "" {
		cfg.Theme = "light"
	}
	if cfg.DefaultTargetBranch == "" {
		cfg.DefaultTargetBranch = "test"
	}
	if cfg.AIAgent == "" {
		cfg.AIAgent = "auto"
	}
	if cfg.DiffInlineMode == "" {
		cfg.DiffInlineMode = "word"
	}
	if cfg.UpdateCheckMinutes <= 0 {
		cfg.UpdateCheckMinutes = 30
	}
	if cfg.UpdateCheckMinutes < 5 {
		cfg.UpdateCheckMinutes = 5
	}
	if cfg.UpdateCheckMinutes > 24*60 {
		cfg.UpdateCheckMinutes = 24 * 60
	}
	cfg.UpdateMirror = strings.TrimSpace(cfg.UpdateMirror)
	if len(cfg.ProtectedBranches) == 0 {
		cfg.ProtectedBranches = []string{"main", "master", "test", "dev", "prod", "production"}
	}
	return cfg
}

// Save writes the configuration to disk atomically.
func (c *Config) Save() error {
	path := ConfigPath()
	if err := os.MkdirAll(filepath.Dir(path), 0o755); err != nil {
		return err
	}
	data, err := json.MarshalIndent(c, "", "  ")
	if err != nil {
		return err
	}
	tmp := path + ".tmp"
	if err := os.WriteFile(tmp, data, 0o644); err != nil {
		return err
	}
	return os.Rename(tmp, path)
}

// AddRecentWorkspace prepends ws to RecentWorkspaces (deduplicated, max 10).
func (c *Config) AddRecentWorkspace(ws string) {
	ws = filepath.Clean(ws)
	if ws == "" {
		return
	}
	c.WorkspacePath = ws
	next := []string{ws}
	for _, r := range c.RecentWorkspaces {
		if filepath.Clean(r) != ws && len(next) < 10 {
			next = append(next, filepath.Clean(r))
		}
	}
	c.RecentWorkspaces = next
}

// SaveBranchPreset adds or updates a named multi-repo branch preset.
func (c *Config) SaveBranchPreset(name string, repoBranches map[string]string) {
	name = strings.TrimSpace(name)
	if name == "" || len(repoBranches) == 0 {
		return
	}
	copied := make(map[string]string, len(repoBranches))
	for k, v := range repoBranches {
		v = strings.TrimSpace(v)
		if k != "" && v != "" {
			copied[k] = v
		}
	}
	if len(copied) == 0 {
		return
	}
	preset := BranchPreset{
		Name:         name,
		RepoBranches: copied,
		UpdatedAt:    time.Now().Format("01-02 15:04"),
	}
	for i := range c.BranchPresets {
		if c.BranchPresets[i].Name == name {
			c.BranchPresets[i] = preset
			return
		}
	}
	c.BranchPresets = append([]BranchPreset{preset}, c.BranchPresets...)
}

// DeleteBranchPreset removes a saved branch preset by name.
func (c *Config) DeleteBranchPreset(name string) {
	next := make([]BranchPreset, 0, len(c.BranchPresets))
	for _, p := range c.BranchPresets {
		if p.Name != name {
			next = append(next, p)
		}
	}
	c.BranchPresets = next
}

// SaveMergePreset adds or updates a named merge plan (target branch + repository set).
func (c *Config) SaveMergePreset(name, targetBranch string, repoIDs []string) {
	name = strings.TrimSpace(name)
	targetBranch = strings.TrimSpace(targetBranch)
	if name == "" || targetBranch == "" || len(repoIDs) == 0 {
		return
	}
	seen := make(map[string]bool, len(repoIDs))
	ids := make([]string, 0, len(repoIDs))
	for _, id := range repoIDs {
		id = strings.TrimSpace(id)
		if id != "" && !seen[id] {
			seen[id] = true
			ids = append(ids, id)
		}
	}
	if len(ids) == 0 {
		return
	}
	preset := MergePreset{
		Name:         name,
		TargetBranch: targetBranch,
		RepoIDs:      ids,
		UpdatedAt:    time.Now().Format("01-02 15:04"),
	}
	for i := range c.MergePresets {
		if c.MergePresets[i].Name == name {
			c.MergePresets[i] = preset
			return
		}
	}
	c.MergePresets = append([]MergePreset{preset}, c.MergePresets...)
}

// DeleteMergePreset removes a saved merge plan by name.
func (c *Config) DeleteMergePreset(name string) {
	next := make([]MergePreset, 0, len(c.MergePresets))
	for _, p := range c.MergePresets {
		if p.Name != name {
			next = append(next, p)
		}
	}
	c.MergePresets = next
}

// ResolvedAIAgent returns the detected AI agent binary name and path.
func (c *Config) ResolvedAIAgent() (name string, binPath string) {
	candidates := []string{"omp", "pi", "claude", "codex"}
	if c.AIAgent != "" && c.AIAgent != "auto" {
		candidates = append([]string{c.AIAgent}, candidates...)
	}
	home, _ := os.UserHomeDir()
	extraDirs := []string{
		filepath.Join(home, ".bun", "bin"),
		filepath.Join(home, ".local", "bin"),
		filepath.Join(home, ".local", "share", "mise", "installs", "node", "24", "bin"),
		"/opt/homebrew/bin",
		"/usr/local/bin",
	}
	for _, cand := range candidates {
		if p, err := exec.LookPath(cand); err == nil {
			return cand, p
		}
		for _, dir := range extraDirs {
			full := filepath.Join(dir, cand)
			if info, err := os.Stat(full); err == nil && !info.IsDir() {
				return cand, full
			}
		}
	}
	return "omp", ""
}
