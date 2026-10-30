package plugins

import (
	"encoding/json"
	"fmt"
	"io"
	"net/http"
	"sort"
	"strings"
	"time"
)

// DefaultRegistryURL is the official marketplace index. The app also ships
// the same index embedded, so the marketplace works offline and the remote
// copy can add plugins without an app release.
const DefaultRegistryURL = "https://raw.githubusercontent.com/iwangjie/Nemorous/main/plugins/registry.json"

// Registry is the marketplace's plugins/registry.json.
type Registry struct {
	Schema    int              `json:"schema"`
	UpdatedAt string           `json:"updatedAt"`
	Plugins   []RegistryPlugin `json:"plugins"`
}

// RegistryPlugin is one marketplace listing. The manifest fields are
// flattened into the entry; the rest says where to get the package.
type RegistryPlugin struct {
	Manifest
	Tags        []string `json:"tags,omitempty"`
	Official    bool     `json:"official,omitempty"`
	Builtin     bool     `json:"builtin,omitempty"`
	DownloadURL string   `json:"downloadUrl,omitempty"`
	SHA256      string   `json:"sha256,omitempty"`
	Size        int64    `json:"size,omitempty"`
}

// RegistryEntry is a listing merged with what is installed locally.
type RegistryEntry struct {
	RegistryPlugin
	InstalledVersion string
	Enabled          bool
	Installed        bool
	Compatible       bool
	IncompatibleWhy  string
	// UpdateAvailable is set when a newer version than the installed one
	// is listed.
	UpdateAvailable bool
}

func parseRegistry(data []byte) (*Registry, error) {
	var r Registry
	dec := json.NewDecoder(strings.NewReader(string(data)))
	dec.DisallowUnknownFields()
	if err := dec.Decode(&r); err != nil {
		return nil, fmt.Errorf("registry: %w", err)
	}
	if r.Schema != 0 && r.Schema != 1 {
		return nil, fmt.Errorf("registry: unsupported schema %d", r.Schema)
	}
	seen := make(map[string]bool, len(r.Plugins))
	for i := range r.Plugins {
		p := &r.Plugins[i]
		p.Builtin = p.Builtin || (p.DownloadURL == "" && p.SHA256 == "")
		if !p.Builtin && p.SHA256 == "" {
			return nil, fmt.Errorf("registry: plugin %s needs a sha256", p.ID)
		}
		if p.Builtin && p.DownloadURL != "" {
			return nil, fmt.Errorf("registry: plugin %s is builtin and has a downloadUrl", p.ID)
		}
		if err := p.Manifest.Validate(""); err != nil {
			return nil, fmt.Errorf("registry: %s: %w", p.ID, err)
		}
		if seen[p.ID] {
			return nil, fmt.Errorf("registry: plugin %s listed twice", p.ID)
		}
		seen[p.ID] = true
	}
	sort.SliceStable(r.Plugins, func(i, j int) bool {
		if r.Plugins[i].Official != r.Plugins[j].Official {
			return r.Plugins[i].Official
		}
		return strings.ToLower(r.Plugins[i].Name) < strings.ToLower(r.Plugins[j].Name)
	})
	return &r, nil
}

// mergeRegistries overlays remote entries on the embedded ones, so a
// remote listing can publish a newer version or a new plugin while the
// embedded copy keeps the marketplace useful offline.
func mergeRegistries(base, remote *Registry) *Registry {
	out := &Registry{Schema: 1}
	byID := make(map[string]RegistryPlugin)
	order := make([]string, 0)
	add := func(p RegistryPlugin) {
		if _, ok := byID[p.ID]; !ok {
			order = append(order, p.ID)
		}
		byID[p.ID] = p
	}
	if base != nil {
		out.UpdatedAt = base.UpdatedAt
		for _, p := range base.Plugins {
			add(p)
		}
	}
	if remote != nil {
		if remote.UpdatedAt != "" {
			out.UpdatedAt = remote.UpdatedAt
		}
		for _, p := range remote.Plugins {
			add(p)
		}
	}
	for _, id := range order {
		out.Plugins = append(out.Plugins, byID[id])
	}
	sort.SliceStable(out.Plugins, func(i, j int) bool {
		if out.Plugins[i].Official != out.Plugins[j].Official {
			return out.Plugins[i].Official
		}
		return strings.ToLower(out.Plugins[i].Name) < strings.ToLower(out.Plugins[j].Name)
	})
	return out
}

const maxRegistrySize = 2 << 20

func fetchRegistry(client *http.Client, url string) (*Registry, error) {
	req, err := http.NewRequest(http.MethodGet, url, nil)
	if err != nil {
		return nil, err
	}
	req.Header.Set("User-Agent", "Nemorous-plugin-market")
	resp, err := client.Do(req)
	if err != nil {
		return nil, err
	}
	defer resp.Body.Close()
	if resp.StatusCode != http.StatusOK {
		return nil, fmt.Errorf("registry: HTTP %s", resp.Status)
	}
	data, err := io.ReadAll(io.LimitReader(resp.Body, maxRegistrySize+1))
	if err != nil {
		return nil, err
	}
	if len(data) > maxRegistrySize {
		return nil, fmt.Errorf("registry: index is too large")
	}
	return parseRegistry(data)
}

func newHTTPClient() *http.Client {
	return &http.Client{Timeout: 25 * time.Second}
}
