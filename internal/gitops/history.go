package gitops

import (
	"context"
	"fmt"
	"regexp"
	"sort"
	"strconv"
	"strings"
)

// CommitEntry is one commit in a repository history.
type CommitEntry struct {
	RepoID        string
	RepoName      string
	RepoPath      string
	Hash          string
	ShortHash     string
	Subject       string
	Body          string
	Author        string
	AuthorEmail   string
	AuthorUnix    int64
	CommitterUnix int64
	Parents       []string
	Refs          []string
}

// CommitFile describes one path changed by a commit.
type CommitFile struct {
	Path    string
	OldPath string
	Status  string
	Added   int
	Deleted int
}

// CommitDetail is a commit plus its changed paths.
type CommitDetail struct {
	CommitEntry
	Files []CommitFile
}

// LogOptions narrows a history query.
type LogOptions struct {
	Branch    string
	Author    string
	AuthorMe  bool
	Path      string
	SinceDays int
	Limit     int
}

// RepoLog reads the commit graph of one repository. All refs are included when
// Branch is empty, so the history stays a true multi-branch DAG.
func RepoLog(ctx context.Context, repo RepoStatus, opts LogOptions) ([]CommitEntry, error) {
	limit := opts.Limit
	if limit <= 0 {
		limit = 300
	}
	format := "%H%x1f%h%x1f%P%x1f%an%x1f%ae%x1f%at%x1f%ct%x1f%D%x1f%s%x1f%b%x1e"
	args := []string{"log", "--date=unix", "--topo-order", "--pretty=format:" + format, "-n", strconv.Itoa(limit)}
	if opts.Branch != "" {
		args = append(args, opts.Branch)
	} else {
		args = append(args, "--all")
	}
	if opts.SinceDays > 0 {
		args = append(args, fmt.Sprintf("--since=%d days ago", opts.SinceDays))
	}
	author := strings.TrimSpace(opts.Author)
	if opts.AuthorMe {
		author = repoAuthorIdentity(ctx, repo.Repo.AbsPath)
		if author == "" {
			return nil, nil
		}
		author = regexp.QuoteMeta(author)
	}
	if author != "" {
		args = append(args, "--author="+author)
	}
	if pathSpec := strings.TrimSpace(opts.Path); pathSpec != "" {
		// rg-style path globs: translate *, ?, [] into git's glob pathspec.
		if strings.ContainsAny(pathSpec, "*?[") && !strings.HasPrefix(pathSpec, ":(") {
			// rg -g '*.go' matches basenames at any depth; Git needs **/ for that.
			if !strings.Contains(pathSpec, "/") {
				pathSpec = "**/" + pathSpec
			}
			pathSpec = ":(glob)" + pathSpec
		}
		args = append(args, "--", pathSpec)
	}
	out, err := RunGit(ctx, repo.Repo.AbsPath, args...)
	if err != nil {
		return nil, err
	}
	var commits []CommitEntry
	for _, record := range strings.Split(out, "\x1e") {
		record = strings.TrimLeft(record, "\r\n")
		if strings.TrimSpace(record) == "" {
			continue
		}
		parts := strings.Split(record, "\x1f")
		if len(parts) < 10 {
			continue
		}
		entry := CommitEntry{
			RepoID:        repo.Repo.ID,
			RepoName:      repo.Repo.Name,
			RepoPath:      repo.Repo.AbsPath,
			Hash:          parts[0],
			ShortHash:     parts[1],
			Subject:       parts[8],
			Body:          strings.TrimSpace(parts[9]),
			Author:        parts[3],
			AuthorEmail:   parts[4],
			AuthorUnix:    parseInt64(parts[5]),
			CommitterUnix: parseInt64(parts[6]),
		}
		for _, parent := range strings.Fields(parts[2]) {
			if parent != "" {
				entry.Parents = append(entry.Parents, parent)
			}
		}
		for _, ref := range strings.Split(parts[7], ",") {
			ref = strings.TrimSpace(ref)
			if ref != "" {
				entry.Refs = append(entry.Refs, ref)
			}
		}
		commits = append(commits, entry)
	}
	return commits, nil
}

// CommitFiles lists paths changed by a commit with per-file line counts.
func CommitFiles(ctx context.Context, repoPath, hash string) ([]CommitFile, error) {
	out, err := RunGit(ctx, repoPath, "show", "--format=", "--find-renames", "--name-status", hash)
	if err != nil {
		return nil, err
	}
	files := make([]CommitFile, 0, 16)
	index := make(map[string]int)
	for _, line := range strings.Split(out, "\n") {
		line = strings.TrimSpace(line)
		if line == "" {
			continue
		}
		parts := strings.Split(line, "\t")
		if len(parts) < 2 {
			continue
		}
		status := strings.TrimSpace(parts[0])
		path := strings.TrimSpace(parts[len(parts)-1])
		oldPath := ""
		if (strings.HasPrefix(status, "R") || strings.HasPrefix(status, "C")) && len(parts) >= 3 {
			oldPath = strings.TrimSpace(parts[1])
		}
		if path == "" {
			continue
		}
		index[path] = len(files)
		files = append(files, CommitFile{Path: path, OldPath: oldPath, Status: string(status[0])})
	}

	if stats, statErr := RunGit(ctx, repoPath, "show", "--format=", "--find-renames", "--numstat", hash); statErr == nil {
		for _, line := range strings.Split(stats, "\n") {
			parts := strings.Split(line, "\t")
			if len(parts) < 3 {
				continue
			}
			path := strings.TrimSpace(parts[2])
			idx, ok := index[path]
			if !ok {
				continue
			}
			files[idx].Added, _ = strconv.Atoi(strings.TrimSpace(parts[0]))
			files[idx].Deleted, _ = strconv.Atoi(strings.TrimSpace(parts[1]))
		}
	}
	sort.SliceStable(files, func(i, j int) bool { return files[i].Path < files[j].Path })
	return files, nil
}

// RepoAuthors lists distinct author names seen in a repository.
func RepoAuthors(ctx context.Context, repo RepoStatus, limit int) ([]string, error) {
	if limit <= 0 {
		limit = 500
	}
	out, err := RunGit(ctx, repo.Repo.AbsPath, "log", "--all", "--format=%an", "-n", strconv.Itoa(limit))
	if err != nil {
		return nil, err
	}
	seen := make(map[string]bool)
	names := make([]string, 0, 16)
	for _, line := range strings.Split(out, "\n") {
		name := strings.TrimSpace(line)
		if name == "" || seen[name] {
			continue
		}
		seen[name] = true
		names = append(names, name)
	}
	sort.Strings(names)
	return names, nil
}

func repoAuthorIdentity(ctx context.Context, repoPath string) string {
	for _, key := range []string{"user.email", "user.name"} {
		if v, err := RunGit(ctx, repoPath, "config", key); err == nil && strings.TrimSpace(v) != "" {
			return strings.TrimSpace(v)
		}
	}
	for _, key := range []string{"user.email", "user.name"} {
		if v, err := RunGit(ctx, repoPath, "config", "--global", key); err == nil && strings.TrimSpace(v) != "" {
			return strings.TrimSpace(v)
		}
	}
	return ""
}

func parseInt64(s string) int64 {
	v, _ := strconv.ParseInt(strings.TrimSpace(s), 10, 64)
	return v
}

// CommitFileVersions returns the old and new file contents for a commit diff,
// mirroring the buffers the worktree diff viewer consumes.
func CommitFileVersions(ctx context.Context, repoPath, hash, path, oldPath, status string) (string, string, error) {
	oldRev := hash + "^"
	oldContent := ""
	var err error
	switch status {
	case "A":
		oldContent = ""
	case "R", "C":
		if oldPath == "" {
			oldPath = path
		}
		oldContent, err = showFileAt(ctx, repoPath, oldRev, oldPath)
		if err != nil {
			oldContent = ""
		}
	case "D":
		oldContent, err = showFileAt(ctx, repoPath, oldRev, path)
		if err != nil {
			return "", "", err
		}
		return oldContent, "", nil
	default:
		oldContent, err = showFileAt(ctx, repoPath, oldRev, path)
		if err != nil {
			oldContent = ""
		}
	}
	newContent, err := showFileAt(ctx, repoPath, hash, path)
	if err != nil {
		return oldContent, "", err
	}
	return oldContent, newContent, nil
}

func showFileAt(ctx context.Context, repoPath, rev, path string) (string, error) {
	if rev == "" || rev == "^" {
		return "", nil
	}
	out, err := RunGitRaw(ctx, repoPath, "show", rev+":"+path)
	if err != nil {
		return "", err
	}
	if strings.ContainsRune(out, '\x00') {
		return "// 二进制文件，无法以文本形式显示", nil
	}
	return out, nil
}
