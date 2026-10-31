// Package plugins implements Nemorous's plugin engine: plugin packages,
// the marketplace registry, installation and removal, and a sandboxed
// JavaScript runtime that draws plugin panels with a declarative UI tree.
//
// A plugin is a directory with a plugin.json manifest and a JavaScript
// entry file. It runs in an embedded goja VM with no I/O of its own: the
// host exposes workspace file reads, process execution and per-plugin
// storage, each gated by the permissions the manifest declares.
package plugins

import (
	"encoding/json"
	"fmt"
	"os"
	"path"
	"path/filepath"
	"regexp"
	"strconv"
	"strings"
)

// Permission names a host capability a plugin may use.
const (
	PermissionWorkspaceRead  = "workspace.read"
	PermissionWorkspaceWrite = "workspace.write"
	PermissionProcessExec    = "process.exec"
	PermissionNetworkHTTP    = "network.http"
	PermissionEditorRead     = "editor.read"
	PermissionTerminalOpen   = "terminal.open"
	PermissionUIOpen         = "ui.open"
	PermissionDatabase       = "database"
)

var allowedPermissions = map[string]bool{
	PermissionWorkspaceRead:  true,
	PermissionWorkspaceWrite: true,
	PermissionProcessExec:    true,
	PermissionNetworkHTTP:    true,
	PermissionEditorRead:     true,
	PermissionTerminalOpen:   true,
	PermissionUIOpen:         true,
	PermissionDatabase:       true,
}

var (
	idPattern      = regexp.MustCompile(`^[a-z0-9][a-z0-9-]{0,63}$`)
	commandPattern = regexp.MustCompile(`^[a-z0-9][a-z0-9.-]{0,63}$`)
	versionPattern = regexp.MustCompile(`^(\d+)\.(\d+)\.(\d+)(?:-([0-9A-Za-z.-]+))?$`)
)

// Manifest is a plugin.json.
type Manifest struct {
	ID            string        `json:"id"`
	Name          string        `json:"name"`
	Version       string        `json:"version"`
	Description   string        `json:"description,omitempty"`
	Author        string        `json:"author,omitempty"`
	Homepage      string        `json:"homepage,omitempty"`
	MinAppVersion string        `json:"minAppVersion,omitempty"`
	Main          string        `json:"main"`
	Icon          string        `json:"icon,omitempty"`
	Permissions   []string      `json:"permissions,omitempty"`
	Contributes   Contributions `json:"contributes"`
}

// Contributions are the UI the plugin adds to the app.
type Contributions struct {
	Panels   []PanelContribution   `json:"panels,omitempty"`
	Editors  []EditorContribution  `json:"editors,omitempty"`
	Commands []CommandContribution `json:"commands,omitempty"`
}

// EditorContribution is an editor replacement for matching files, such as a
// Markdown preview or a CSV table. Patterns are file-name globs ("*.md").
type EditorContribution struct {
	ID       string   `json:"id"`
	Title    string   `json:"title,omitempty"`
	Patterns []string `json:"patterns"`
	Priority int      `json:"priority,omitempty"`
}

// PanelContribution is a tool-window panel the plugin draws.
type PanelContribution struct {
	ID    string `json:"id"`
	Title string `json:"title"`
	Icon  string `json:"icon,omitempty"`
}

// CommandContribution is an entry in the command palette.
type CommandContribution struct {
	ID     string `json:"id"`
	Title  string `json:"title"`
	Action string `json:"action,omitempty"`
}

// ParseManifest decodes and structurally validates a manifest. appVersion
// gates minAppVersion; "" or "dev" accepts every plugin.
func ParseManifest(data []byte, appVersion string) (*Manifest, error) {
	var m Manifest
	dec := json.NewDecoder(strings.NewReader(string(data)))
	dec.DisallowUnknownFields()
	if err := dec.Decode(&m); err != nil {
		return nil, fmt.Errorf("plugin.json: %w", err)
	}
	if err := m.Validate(appVersion); err != nil {
		return nil, err
	}
	return &m, nil
}

// Validate checks the manifest's identity, version, permissions and UI
// contributions. It does not look at the files in the package.
func (m *Manifest) Validate(appVersion string) error {
	if !idPattern.MatchString(m.ID) {
		return fmt.Errorf("plugin.json: invalid id %q (lowercase letters, digits and dashes)", m.ID)
	}
	if strings.TrimSpace(m.Name) == "" {
		return fmt.Errorf("plugin.json: name is required")
	}
	if !versionPattern.MatchString(m.Version) {
		return fmt.Errorf("plugin.json: version %q is not x.y.z", m.Version)
	}
	if m.Main == "" {
		return fmt.Errorf("plugin.json: main is required")
	}
	clean := path.Clean(m.Main)
	if clean != m.Main || strings.HasPrefix(clean, "../") || strings.HasPrefix(clean, "/") || !strings.HasSuffix(clean, ".js") {
		return fmt.Errorf("plugin.json: main must be a .js path inside the package")
	}
	if m.Icon != "" {
		cleanIcon := path.Clean(m.Icon)
		if cleanIcon != m.Icon || strings.HasPrefix(cleanIcon, "../") || strings.HasPrefix(cleanIcon, "/") {
			return fmt.Errorf("plugin.json: icon must be a path inside the package")
		}
	}
	if m.MinAppVersion != "" && !versionPattern.MatchString(m.MinAppVersion) {
		return fmt.Errorf("plugin.json: minAppVersion %q is not x.y.z", m.MinAppVersion)
	}
	if !appCompatible(appVersion, m.MinAppVersion) {
		return fmt.Errorf("plugin %s %s needs Nemorous %s or newer", m.Name, m.Version, m.MinAppVersion)
	}
	seen := make(map[string]bool, len(m.Permissions))
	for _, perm := range m.Permissions {
		if !allowedPermissions[perm] {
			return fmt.Errorf("plugin.json: unknown permission %q", perm)
		}
		if seen[perm] {
			return fmt.Errorf("plugin.json: permission %q repeated", perm)
		}
		seen[perm] = true
	}
	if len(m.Contributes.Panels) > 8 {
		return fmt.Errorf("plugin.json: at most 8 panels")
	}
	panelIDs := make(map[string]bool, len(m.Contributes.Panels))
	for _, p := range m.Contributes.Panels {
		if !idPattern.MatchString(p.ID) {
			return fmt.Errorf("plugin.json: invalid panel id %q", p.ID)
		}
		if panelIDs[p.ID] {
			return fmt.Errorf("plugin.json: panel %q repeated", p.ID)
		}
		panelIDs[p.ID] = true
		if strings.TrimSpace(p.Title) == "" {
			return fmt.Errorf("plugin.json: panel %q needs a title", p.ID)
		}
		if len([]rune(p.Title)) > 60 {
			return fmt.Errorf("plugin.json: panel %q title is too long", p.ID)
		}
	}
	if len(m.Contributes.Commands) > 32 {
		return fmt.Errorf("plugin.json: at most 32 commands")
	}
	for _, cmd := range m.Contributes.Commands {
		if !commandPattern.MatchString(cmd.ID) {
			return fmt.Errorf("plugin.json: invalid command id %q", cmd.ID)
		}
		if strings.TrimSpace(cmd.Title) == "" {
			return fmt.Errorf("plugin.json: command %q needs a title", cmd.ID)
		}
	}
	return nil
}

// HasPermission reports whether the manifest declares perm.
func (m *Manifest) HasPermission(perm string) bool {
	for _, p := range m.Permissions {
		if p == perm {
			return true
		}
	}
	return false
}

// appCompatible reports whether appVersion satisfies minAppVersion. Dev
// builds ("" or "dev") accept everything so working trees can run plugins
// that need the version being developed.
func appCompatible(appVersion, minAppVersion string) bool {
	if minAppVersion == "" {
		return true
	}
	if appVersion == "" || appVersion == "dev" || strings.HasSuffix(appVersion, "-dev") {
		return true
	}
	return CompareVersions(appVersion, minAppVersion) >= 0
}

// CompareVersions compares x.y.z versions, with a dash suffix ordering
// before the release. It returns -1, 0 or 1.
func CompareVersions(a, b string) int {
	am, an, ap, apre := parseVersion(a)
	bm, bn, bp, bpre := parseVersion(b)
	if c := compareInt(am, bm); c != 0 {
		return c
	}
	if c := compareInt(an, bn); c != 0 {
		return c
	}
	if c := compareInt(ap, bp); c != 0 {
		return c
	}
	if apre == bpre {
		return 0
	}
	if apre == "" {
		return 1
	}
	if bpre == "" {
		return -1
	}
	return strings.Compare(apre, bpre)
}

func parseVersion(v string) (major, minor, patch int, pre string) {
	m := versionPattern.FindStringSubmatch(strings.TrimSpace(v))
	if m == nil {
		// Unknown versions sort first, so an invalid release never
		// shadows a real one.
		return -1, -1, -1, ""
	}
	major, _ = strconv.Atoi(m[1])
	minor, _ = strconv.Atoi(m[2])
	patch, _ = strconv.Atoi(m[3])
	return major, minor, patch, m[4]
}

func compareInt(a, b int) int {
	switch {
	case a < b:
		return -1
	case a > b:
		return 1
	default:
		return 0
	}
}

// readManifestFile reads and validates plugin.json from a package directory.
func readManifestFile(dir string, appVersion string) (*Manifest, error) {
	data, err := os.ReadFile(filepath.Join(dir, "plugin.json"))
	if err != nil {
		return nil, err
	}
	m, err := ParseManifest(data, appVersion)
	if err != nil {
		return nil, err
	}
	info, err := os.Stat(filepath.Join(dir, m.Main))
	if err != nil {
		return nil, fmt.Errorf("plugin.json: main %q not found", m.Main)
	}
	if info.IsDir() {
		return nil, fmt.Errorf("plugin.json: main %q is a directory", m.Main)
	}
	if m.Icon != "" {
		if info, err := os.Stat(filepath.Join(dir, m.Icon)); err != nil || info.IsDir() {
			return nil, fmt.Errorf("plugin.json: icon %q not found", m.Icon)
		}
	}
	return m, nil
}
