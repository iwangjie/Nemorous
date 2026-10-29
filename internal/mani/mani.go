package mani

import (
	"bytes"
	"context"
	"fmt"
	"os"
	"os/exec"
	"path/filepath"
	"sort"
	"strings"
	"sync"
	"time"

	"gopkg.in/yaml.v3"
)

// Config represents a parsed mani.yaml configuration.
type Config struct {
	Projects map[string]ProjectSpec `yaml:"projects"`
	Specs    map[string]SpecDef     `yaml:"specs"`
	Tasks    map[string]TaskSpec    `yaml:"tasks"`
}

// ProjectSpec represents a project entry in mani.yaml.
type ProjectSpec struct {
	Path string   `yaml:"path"`
	Desc string   `yaml:"desc"`
	Tags []string `yaml:"tags"`
	URL  string   `yaml:"url"`
}

// SpecDef represents an execution spec in mani.yaml.
type SpecDef struct {
	Output   string `yaml:"output"`
	Parallel bool   `yaml:"parallel"`
	Forks    int    `yaml:"forks"`
	OmitEmpty bool  `yaml:"omit_empty"`
}

// TaskSpec represents a task entry in mani.yaml.
type TaskSpec struct {
	Desc   string `yaml:"desc"`
	Spec   any    `yaml:"spec"`
	Target any    `yaml:"target"`
	Cmd    string `yaml:"cmd"`
	Env    any    `yaml:"env"`
}

// EnvPairs returns the default environment variables declared on the task as KEY=VAL strings.
func (t TaskSpec) EnvPairs() []string {
	var out []string
	switch v := t.Env.(type) {
	case []any:
		for _, item := range v {
			if s, ok := item.(string); ok && s != "" {
				out = append(out, s)
			}
		}
	case []string:
		out = append(out, v...)
	case map[string]any:
		for k, val := range v {
			out = append(out, fmt.Sprintf("%s=%v", k, val))
		}
	case map[string]string:
		for k, val := range v {
			out = append(out, fmt.Sprintf("%s=%s", k, val))
		}
	}
	return out
}

// IsCwdTarget reports whether the task targets only the workspace root directory (cwd: true).
func (t TaskSpec) IsCwdTarget() bool {
	switch v := t.Target.(type) {
	case string:
		return v == "cwd"
	case map[string]any:
		if cwd, ok := v["cwd"].(bool); ok && cwd {
			return true
		}
	}
	return false
}

// DiscoveredRepo represents a Git repository discovered in the workspace.
type DiscoveredRepo struct {
	ID          string // e.g., "cloud-caring", "workspace"
	Name        string // display name
	RelPath     string // "." or child folder name
	AbsPath     string // absolute filesystem path
	Description string // from mani.yaml or AGENTS.md
	InMani      bool   // whether listed in mani.yaml projects
	IsRoot      bool   // whether this is the workspace root repository
	Monogram    string // 2-letter code e.g. "cc", "gw", "mo"
	ColorIndex  int    // 0..7 deterministic color index
}

// TaskResult represents the result of running a mani task on one project.
type TaskResult struct {
	RepoID   string
	RepoName string
	Status   string // "OK", "SKIP", "FAIL", "INFO"
	Output   string
	Duration time.Duration
	Err      error
}

// LoadManiConfig loads mani.yaml or mani.yml from workspaceDir if present.
func LoadManiConfig(workspaceDir string) (*Config, string, error) {
	candidates := []string{"mani.yaml", "mani.yml", ".mani.yaml", ".mani.yml"}
	for _, name := range candidates {
		full := filepath.Join(workspaceDir, name)
		data, err := os.ReadFile(full)
		if err != nil {
			continue
		}
		var cfg Config
		if err := yaml.Unmarshal(data, &cfg); err != nil {
			return nil, full, fmt.Errorf("parse %s: %w", name, err)
		}
		return &cfg, full, nil
	}
	return nil, "", os.ErrNotExist
}

// ParseAgentsDescriptions extracts repository descriptions from AGENTS.md markdown tables and bullet lists.
func ParseAgentsDescriptions(workspaceDir string) map[string]string {
	descs := make(map[string]string)
	data, err := os.ReadFile(filepath.Join(workspaceDir, "AGENTS.md"))
	if err != nil {
		return descs
	}
	lines := strings.Split(string(data), "\n")
	for _, line := range lines {
		line = strings.TrimSpace(line)
		if strings.HasPrefix(line, "- ") || strings.HasPrefix(line, "* ") {
			item := strings.TrimSpace(line[2:])
			if idx := strings.IndexAny(item, ":："); idx > 0 {
				key := strings.Trim(strings.TrimSpace(item[:idx]), "`* ")
				val := strings.TrimSpace(strings.TrimPrefix(strings.TrimPrefix(item[idx:], ":"), "："))
				if key != "" && val != "" && !strings.ContainsAny(key, " \t/") && len(val) < 80 {
					descs[key] = val
				}
			}
			continue
		}
		if !strings.HasPrefix(line, "|") || !strings.HasSuffix(line, "|") {
			continue
		}
		parts := strings.Split(line, "|")
		if len(parts) < 4 {
			continue
		}
		col1 := strings.Trim(strings.TrimSpace(parts[1]), "`* ")
		col2 := strings.TrimSpace(parts[2])
		if col1 == "" || col1 == "项目" || strings.HasPrefix(col1, "-") || strings.HasPrefix(col2, "-") {
			continue
		}
		descs[col1] = col2
	}
	return descs
}

// DiscoverRepos finds all Git repositories in workspaceDir by combining mani.yaml and filesystem scan.
func DiscoverRepos(workspaceDir string, includeRoot bool) ([]DiscoveredRepo, *Config) {
	workspaceDir = filepath.Clean(workspaceDir)
	maniCfg, _, _ := LoadManiConfig(workspaceDir)
	agentDescs := ParseAgentsDescriptions(workspaceDir)

	byRelPath := make(map[string]*DiscoveredRepo)

	// 1. Add projects from mani.yaml if available
	if maniCfg != nil {
		for name, proj := range maniCfg.Projects {
			rel := filepath.Clean(proj.Path)
			if rel == "" {
				rel = name
			}
			isRoot := rel == "."
			if isRoot && !includeRoot {
				continue
			}
			abs := filepath.Join(workspaceDir, rel)
			if !isGitRepo(abs) {
				continue
			}
			desc := proj.Desc
			if d, ok := agentDescs[name]; ok && d != "" {
				desc = d
			} else if d, ok := agentDescs[rel]; ok && d != "" {
				desc = d
			}
			byRelPath[rel] = &DiscoveredRepo{
				ID:          name,
				Name:        name,
				RelPath:     rel,
				AbsPath:     abs,
				Description: desc,
				InMani:      true,
				IsRoot:      isRoot,
			}
		}
	}

	// 2. Scan root directory itself if includeRoot is true
	if includeRoot && byRelPath["."] == nil && isGitRepo(workspaceDir) {
		rootName := filepath.Base(workspaceDir)
		desc := "工作区根仓库 (Workspace Root)"
		if d, ok := agentDescs["workspace"]; ok && d != "" {
			desc = d
		}
		byRelPath["."] = &DiscoveredRepo{
			ID:          rootName,
			Name:        rootName,
			RelPath:     ".",
			AbsPath:     workspaceDir,
			Description: desc,
			InMani:      false,
			IsRoot:      true,
		}
	}

	// 3. Scan direct child directories for .git
	entries, err := os.ReadDir(workspaceDir)
	if err == nil {
		for _, ent := range entries {
			if !ent.IsDir() || strings.HasPrefix(ent.Name(), ".") {
				continue
			}
			rel := ent.Name()
			if byRelPath[rel] != nil {
				continue
			}
			abs := filepath.Join(workspaceDir, rel)
			if !isGitRepo(abs) {
				continue
			}
			desc := agentDescs[rel]
			byRelPath[rel] = &DiscoveredRepo{
				ID:          rel,
				Name:        rel,
				RelPath:     rel,
				AbsPath:     abs,
				Description: desc,
				InMani:      false,
				IsRoot:      false,
			}
		}
	}

	repos := make([]DiscoveredRepo, 0, len(byRelPath))
	for _, r := range byRelPath {
		repos = append(repos, *r)
	}

	// Sort: child repos alphabetically first, then root workspace repo (or child repos in consistent order)
	sort.Slice(repos, func(i, j int) bool {
		if repos[i].IsRoot != repos[j].IsRoot {
			return !repos[i].IsRoot // child service repos first, workspace root last
		}
		return repos[i].Name < repos[j].Name
	})

	for i := range repos {
		repos[i].Monogram = computeMonogram(repos[i].Name)
		repos[i].ColorIndex = i % 8
	}

	return repos, maniCfg
}

func isGitRepo(dir string) bool {
	_, err := os.Stat(filepath.Join(dir, ".git"))
	return err == nil
}

func computeMonogram(name string) string {
	clean := strings.ToLower(strings.TrimSpace(name))
	if clean == "" {
		return "rp"
	}
	parts := strings.FieldsFunc(clean, func(r rune) bool {
		return r == '-' || r == '_' || r == '.' || r == ' '
	})
	if len(parts) >= 2 && len(parts[0]) > 0 && len(parts[1]) > 0 {
		return string([]byte{parts[0][0], parts[1][0]})
	}
	if len(clean) >= 2 {
		return clean[:2]
	}
	return clean + "r"
}

// RunTask executes a mani.yaml task across the target repositories in parallel.
func RunTask(ctx context.Context, workspaceDir string, taskName string, task TaskSpec, repos []DiscoveredRepo, extraEnv map[string]string, concurrency int) []TaskResult {
	if concurrency <= 0 {
		concurrency = 8
	}
	// Filter repos if task targets only mani projects or cwd
	var targets []DiscoveredRepo
	if task.IsCwdTarget() {
		targets = []DiscoveredRepo{{
			ID:      "workspace",
			Name:    "workspace (cwd)",
			RelPath: ".",
			AbsPath: workspaceDir,
			IsRoot:  true,
		}}
	} else {
		for _, r := range repos {
			if r.InMani || len(repos) > 0 {
				// If mani projects exist, mani tasks normally run on mani projects; include all discovered or mani projects
				if r.InMani {
					targets = append(targets, r)
				}
			}
		}
		if len(targets) == 0 {
			targets = repos
		}
	}

	results := make([]TaskResult, len(targets))
	sem := make(chan struct{}, concurrency)
	var wg sync.WaitGroup

	for i, repo := range targets {
		wg.Add(1)
		go func(idx int, r DiscoveredRepo) {
			defer wg.Done()
			sem <- struct{}{}
			defer func() { <-sem }()

			start := time.Now()
			cmd := exec.CommandContext(ctx, "bash", "-c", task.Cmd)
			cmd.Dir = r.AbsPath
			env := os.Environ()
			env = append(env, "MANI_PROJECT_NAME="+r.Name, "MANI_PROJECT_PATH="+r.AbsPath)
			env = append(env, task.EnvPairs()...)
			for k, v := range extraEnv {
				env = append(env, fmt.Sprintf("%s=%s", k, v))
			}
			cmd.Env = env

			var out bytes.Buffer
			cmd.Stdout = &out
			cmd.Stderr = &out
			err := cmd.Run()
			raw := strings.TrimSpace(out.String())
			status := classifyTaskOutput(raw, err)
			results[idx] = TaskResult{
				RepoID:   r.ID,
				RepoName: r.Name,
				Status:   status,
				Output:   raw,
				Duration: time.Since(start),
				Err:      err,
			}
		}(i, repo)
	}
	wg.Wait()
	return results
}

func classifyTaskOutput(out string, err error) string {
	if err != nil {
		return "FAIL"
	}
	upper := strings.ToUpper(out)
	if strings.HasPrefix(upper, "FAIL") || strings.Contains(upper, "\nFAIL") {
		return "FAIL"
	}
	if strings.HasPrefix(upper, "SKIP") {
		return "SKIP"
	}
	if strings.HasPrefix(upper, "OK") {
		return "OK"
	}
	return "OK"
}
