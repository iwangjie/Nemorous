// Package localhistory keeps bounded snapshots of files Nemorous edits,
// as IntelliJ IDEA's Local History does, so a discarded change can be
// recovered without a commit.
package localhistory

import (
	"crypto/sha1"
	"encoding/hex"
	"encoding/json"
	"errors"
	"fmt"
	"os"
	"path/filepath"
	"sort"
	"strings"
	"sync"
	"time"
)

const (
	defaultMaxPerFile = 30
	maxSnapshotBytes  = 1 << 20
)

// Entry is one stored revision.
type Entry struct {
	ID      string    `json:"id"`
	RepoID  string    `json:"repoId"`
	Path    string    `json:"path"`
	At      time.Time `json:"at"`
	Reason  string    `json:"reason,omitempty"`
	Size    int       `json:"size"`
	Content string    `json:"-"`
}

// Store is a per-user local history directory.
type Store struct {
	root       string
	maxPerFile int

	mu sync.Mutex
}

// New opens (and creates) a local history store.
func New(root string) (*Store, error) {
	if err := os.MkdirAll(root, 0o700); err != nil {
		return nil, err
	}
	return &Store{root: root, maxPerFile: defaultMaxPerFile}, nil
}

// Record saves content as a revision of relPath in repoID. A revision
// identical to the newest one is not stored twice.
func (s *Store) Record(repoID, relPath, content, reason string) error {
	if len(content) > maxSnapshotBytes {
		return nil // binary or huge files are not worth keeping
	}
	s.mu.Lock()
	defer s.mu.Unlock()
	dir := s.fileDir(repoID, relPath)
	if err := os.MkdirAll(dir, 0o700); err != nil {
		return err
	}
	entries, err := readDirEntries(dir)
	if err != nil {
		return err
	}
	if len(entries) > 0 {
		if latest, err := readEntryContent(dir, entries[0].ID); err == nil && latest == content {
			return nil
		}
	}
	now := time.Now()
	entry := Entry{
		ID:     fmt.Sprintf("%020d", now.UnixNano()),
		RepoID: repoID,
		Path:   relPath,
		At:     now,
		Reason: reason,
		Size:   len(content),
	}
	contentPath := filepath.Join(dir, entry.ID+".txt")
	contentTmp := contentPath + ".tmp"
	if err := os.WriteFile(contentTmp, []byte(content), 0o600); err != nil {
		return err
	}
	if err := os.Rename(contentTmp, contentPath); err != nil {
		return err
	}
	data, err := json.Marshal(entry)
	if err != nil {
		return err
	}
	target := filepath.Join(dir, entry.ID+".json")
	tmp := target + ".tmp"
	if err := os.WriteFile(tmp, data, 0o600); err != nil {
		return err
	}
	if err := os.Rename(tmp, target); err != nil {
		return err
	}
	if len(entries) >= s.maxPerFile {
		for _, old := range entries[s.maxPerFile-1:] {
			_ = os.Remove(filepath.Join(dir, old.ID+".json"))
			_ = os.Remove(filepath.Join(dir, old.ID+".txt"))
		}
	}
	return nil
}

// List returns revisions of relPath, newest first.
func (s *Store) List(repoID, relPath string) ([]Entry, error) {
	s.mu.Lock()
	defer s.mu.Unlock()
	entries, err := readDirEntries(s.fileDir(repoID, relPath))
	if err != nil {
		if errors.Is(err, os.ErrNotExist) {
			return nil, nil
		}
		return nil, err
	}
	return entries, nil
}

// Read returns one revision with its content.
func (s *Store) Read(repoID, relPath, id string) (Entry, error) {
	s.mu.Lock()
	defer s.mu.Unlock()
	dir := s.fileDir(repoID, relPath)
	entry, err := readEntry(filepath.Join(dir, id+".json"))
	if err != nil {
		return Entry{}, err
	}
	content, err := readEntryContent(dir, id)
	if err != nil {
		return Entry{}, err
	}
	entry.Content = content
	return entry, nil
}

func readEntryContent(dir, id string) (string, error) {
	data, err := os.ReadFile(filepath.Join(dir, id+".txt"))
	if err != nil {
		return "", err
	}
	return string(data), nil
}

// Restore writes a revision back to repoPath/relPath.
func (s *Store) Restore(repoID, repoPath, relPath, id string) error {
	entry, err := s.Read(repoID, relPath, id)
	if err != nil {
		return err
	}
	target := filepath.Join(repoPath, filepath.FromSlash(relPath))
	if !within(repoPath, target) {
		return fmt.Errorf("路径不在仓库内")
	}
	mode := os.FileMode(0o644)
	if info, err := os.Stat(target); err == nil {
		mode = info.Mode().Perm()
	}
	if err := os.MkdirAll(filepath.Dir(target), 0o755); err != nil {
		return err
	}
	tmp := target + ".nemorous-restore"
	if err := os.WriteFile(tmp, []byte(entry.Content), mode); err != nil {
		return err
	}
	return os.Rename(tmp, target)
}

func (s *Store) fileDir(repoID, relPath string) string {
	return filepath.Join(s.root, hash(repoID), hash(filepath.ToSlash(relPath)))
}

func hash(s string) string {
	sum := sha1.Sum([]byte(s))
	return hex.EncodeToString(sum[:])
}

func readDirEntries(dir string) ([]Entry, error) {
	files, err := os.ReadDir(dir)
	if err != nil {
		return nil, err
	}
	var entries []Entry
	for _, f := range files {
		if f.IsDir() || !strings.HasSuffix(f.Name(), ".json") {
			continue
		}
		entry, err := readEntry(filepath.Join(dir, f.Name()))
		if err != nil {
			continue
		}
		entries = append(entries, entry)
	}
	sort.Slice(entries, func(i, j int) bool { return entries[i].At.After(entries[j].At) })
	return entries, nil
}

func readEntry(path string) (Entry, error) {
	data, err := os.ReadFile(path)
	if err != nil {
		return Entry{}, err
	}
	var entry Entry
	if err := json.Unmarshal(data, &entry); err != nil {
		return Entry{}, err
	}
	return entry, nil
}

func within(root, p string) bool {
	rel, err := filepath.Rel(root, p)
	if err != nil {
		return false
	}
	return rel == "." || (rel != ".." && !strings.HasPrefix(rel, ".."+string(filepath.Separator)))
}

// RecordFile snapshots a file already on disk.
func (s *Store) RecordFile(repoID, repoPath, relPath, reason string) error {
	data, err := os.ReadFile(filepath.Join(repoPath, filepath.FromSlash(relPath)))
	if err != nil {
		return err
	}
	return s.Record(repoID, relPath, string(data), reason)
}
