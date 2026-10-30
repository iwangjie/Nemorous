package config

import "time"

// ChangeList is a named group of uncommitted files, as IDEA's changelists.
// Files are stored as "repoID::path" keys; files in no list belong to the
// implicit default list.
type ChangeList struct {
	Name      string   `json:"name"`
	Files     []string `json:"files"`
	UpdatedAt string   `json:"updated_at,omitempty"`
}

// ChangeListFor returns the named list holding key, or "" for the default
// list.
func (c Config) ChangeListFor(key string) string {
	for _, list := range c.ChangeLists {
		for _, f := range list.Files {
			if f == key {
				return list.Name
			}
		}
	}
	return ""
}

// AssignToChangeList moves keys into name ("" means the default list),
// removing them from any other list. An empty name merely unassigns.
func (c *Config) AssignToChangeList(name string, keys []string) {
	if len(keys) == 0 {
		return
	}
	remove := make(map[string]bool, len(keys))
	for _, k := range keys {
		remove[k] = true
	}
	for i := range c.ChangeLists {
		files := c.ChangeLists[i].Files[:0]
		for _, f := range c.ChangeLists[i].Files {
			if !remove[f] {
				files = append(files, f)
			}
		}
		c.ChangeLists[i].Files = files
		c.ChangeLists[i].UpdatedAt = time.Now().UTC().Format(time.RFC3339)
	}
	if name == "" {
		return
	}
	for i := range c.ChangeLists {
		if c.ChangeLists[i].Name != name {
			continue
		}
		c.ChangeLists[i].Files = append(c.ChangeLists[i].Files, keys...)
		c.ChangeLists[i].UpdatedAt = time.Now().UTC().Format(time.RFC3339)
		return
	}
	c.ChangeLists = append(c.ChangeLists, ChangeList{
		Name:      name,
		Files:     append([]string(nil), keys...),
		UpdatedAt: time.Now().UTC().Format(time.RFC3339),
	})
}

// SaveChangeList creates or replaces a named list.
func (c *Config) SaveChangeList(name string, keys []string) {
	name = trimSpace(name)
	if name == "" {
		return
	}
	for i := range c.ChangeLists {
		if c.ChangeLists[i].Name == name {
			c.ChangeLists[i].Files = append([]string(nil), keys...)
			c.ChangeLists[i].UpdatedAt = time.Now().UTC().Format(time.RFC3339)
			return
		}
	}
	c.ChangeLists = append(c.ChangeLists, ChangeList{
		Name:      name,
		Files:     append([]string(nil), keys...),
		UpdatedAt: time.Now().UTC().Format(time.RFC3339),
	})
}

// DeleteChangeList removes a list; its files fall back to the default list.
func (c *Config) DeleteChangeList(name string) {
	out := c.ChangeLists[:0]
	for _, list := range c.ChangeLists {
		if list.Name != name {
			out = append(out, list)
		}
	}
	c.ChangeLists = out
	if c.ActiveChangeList == name {
		c.ActiveChangeList = ""
	}
}

// PruneChangeLists removes files that no longer exist in the workspace.
func (c *Config) PruneChangeLists(valid map[string]bool) bool {
	changed := false
	for i := range c.ChangeLists {
		files := c.ChangeLists[i].Files[:0]
		for _, f := range c.ChangeLists[i].Files {
			if valid[f] {
				files = append(files, f)
			} else {
				changed = true
			}
		}
		c.ChangeLists[i].Files = files
	}
	return changed
}

// ChangeListNames returns the named lists in save order.
func (c Config) ChangeListNames() []string {
	out := make([]string, 0, len(c.ChangeLists))
	for _, list := range c.ChangeLists {
		out = append(out, list.Name)
	}
	return out
}

// ChangeListCount counts files in a named list; "" counts unassigned.
func (c Config) ChangeListCount(name string, valid map[string]bool) int {
	if name == "" {
		assigned := make(map[string]bool)
		for _, list := range c.ChangeLists {
			for _, f := range list.Files {
				assigned[f] = true
			}
		}
		n := 0
		for key := range valid {
			if !assigned[key] {
				n++
			}
		}
		return n
	}
	for _, list := range c.ChangeLists {
		if list.Name != name {
			continue
		}
		n := 0
		for _, f := range list.Files {
			if valid[f] {
				n++
			}
		}
		return n
	}
	return 0
}

func trimSpace(s string) string {
	start, end := 0, len(s)
	for start < end && (s[start] == ' ' || s[start] == '\t') {
		start++
	}
	for end > start && (s[end-1] == ' ' || s[end-1] == '\t') {
		end--
	}
	return s[start:end]
}

// RenameChangeList renames a list, keeping its files and the active filter.
func (c *Config) RenameChangeList(oldName, newName string) bool {
	newName = trimSpace(newName)
	if newName == "" || newName == oldName {
		return false
	}
	for _, list := range c.ChangeLists {
		if list.Name == newName {
			return false
		}
	}
	for i := range c.ChangeLists {
		if c.ChangeLists[i].Name == oldName {
			c.ChangeLists[i].Name = newName
			c.ChangeLists[i].UpdatedAt = time.Now().UTC().Format(time.RFC3339)
			if c.ActiveChangeList == oldName {
				c.ActiveChangeList = newName
			}
			return true
		}
	}
	return false
}
