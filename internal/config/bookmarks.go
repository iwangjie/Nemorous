package config

// Bookmark is a saved line in a file.
type Bookmark struct {
	RepoID  string `json:"repo_id"`
	RelPath string `json:"rel_path"`
	Line    int    `json:"line"`
	Note    string `json:"note,omitempty"`
}

// ToggleBookmark adds or removes line's bookmark and reports whether it
// now exists.
func (c *Config) ToggleBookmark(repoID, relPath string, line int) bool {
	for i, bm := range c.Bookmarks {
		if bm.RepoID == repoID && bm.RelPath == relPath && bm.Line == line {
			c.Bookmarks = append(c.Bookmarks[:i], c.Bookmarks[i+1:]...)
			return false
		}
	}
	c.Bookmarks = append(c.Bookmarks, Bookmark{RepoID: repoID, RelPath: relPath, Line: line})
	return true
}

// HasBookmark reports whether line is bookmarked.
func (c Config) HasBookmark(repoID, relPath string, line int) bool {
	for _, bm := range c.Bookmarks {
		if bm.RepoID == repoID && bm.RelPath == relPath && bm.Line == line {
			return true
		}
	}
	return false
}

// LinesFor returns the bookmarked lines of one file.
func (c Config) LinesFor(repoID, relPath string) []int {
	var out []int
	for _, bm := range c.Bookmarks {
		if bm.RepoID == repoID && bm.RelPath == relPath {
			out = append(out, bm.Line)
		}
	}
	return out
}

// PruneBookmarks drops bookmarks for files no longer in the workspace.
func (c *Config) PruneBookmarks(valid map[string]bool) bool {
	out := c.Bookmarks[:0]
	changed := false
	for _, bm := range c.Bookmarks {
		if valid[bm.RepoID+"::"+bm.RelPath] {
			out = append(out, bm)
		} else {
			changed = true
		}
	}
	c.Bookmarks = out
	return changed
}
