package gitops

import (
	"context"
	"fmt"
	"regexp"
	"slices"
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
	Files        []CommitFile
	BaseRevision string // empty for a regular commit against its first parent
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
	return changedRevisionFiles(ctx, repoPath, []string{"show", "--format="}, hash)
}

// CompareCommitFiles lists the changed paths between two committed trees.
func CompareCommitFiles(ctx context.Context, repoPath, before, after string) ([]CommitFile, error) {
	return changedRevisionFiles(ctx, repoPath, []string{"diff"}, before, after)
}

func changedRevisionFiles(ctx context.Context, repoPath string, command []string, revisions ...string) ([]CommitFile, error) {
	read := func(format string) (string, error) {
		args := append(slices.Clone(command), "--no-ext-diff", "--find-renames", "-z", format)
		args = append(args, revisions...)
		return RunGitRaw(ctx, repoPath, append(args, "--")...)
	}
	out, err := read("--name-status")
	if err != nil {
		return nil, err
	}
	files := parseRevisionPaths(out)
	index := make(map[string]int)
	for i, file := range files {
		index[file.Path] = i
	}
	if stats, statErr := read("--numstat"); statErr == nil {
		records := strings.Split(stats, "\x00")
		for i := 0; i < len(records); i++ {
			parts := strings.SplitN(records[i], "\t", 3)
			if len(parts) < 3 {
				continue
			}
			path := parts[2]
			if path == "" && i+2 < len(records) {
				i += 2 // An empty path is followed by the old and new names.
				path = records[i]
			}
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

func parseRevisionPaths(out string) []CommitFile {
	var files []CommitFile
	fields := strings.Split(strings.Trim(out, "\x00"), "\x00")
	for i := 0; i+1 < len(fields); {
		status := strings.TrimSpace(fields[i])
		path := fields[i+1]
		i += 2
		if status == "" || path == "" {
			continue
		}
		file := CommitFile{Path: path, Status: status[:1]}
		if (file.Status == "R" || file.Status == "C") && i < len(fields) {
			file.OldPath, file.Path = path, fields[i]
			i++
		}
		files = append(files, file)
	}
	return files
}

type FileRevision struct {
	CommitInfo
	File CommitFile
}

// FileHistoryAtRevision follows the file backwards from the selected commit.
// Its paths are the paths in those commits, including renames.
func FileHistoryAtRevision(ctx context.Context, repoPath, revision, path string) ([]FileRevision, error) {
	const format = "%x1e%H%x1f%s%x1f%cr%x1f%an%x00"
	out, err := RunGitRaw(ctx, repoPath, "log", "--follow", "--find-renames", "-n", "100", "-z", "--name-status", "--pretty=format:"+format, revision, "--", ":(literal)"+path)
	if err != nil {
		return nil, err
	}
	var entries []FileRevision
	for _, record := range strings.Split(out, "\x1e") {
		meta, names, _ := strings.Cut(record, "\x00")
		parts := strings.SplitN(meta, "\x1f", 4)
		if len(parts) != 4 {
			continue
		}
		for _, file := range parseRevisionPaths(names) {
			entries = append(entries, FileRevision{CommitInfo: CommitInfo{Hash: parts[0], Subject: parts[1], RelTime: parts[2], Author: parts[3]}, File: file})
			break
		}
	}
	return entries, nil
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
	return RevisionFileVersions(ctx, repoPath, hash+"^", hash, CommitFile{Path: path, OldPath: oldPath, Status: status})
}

func RevisionFileVersions(ctx context.Context, repoPath, before, after string, file CommitFile) (string, string, error) {
	path, oldPath, status := file.Path, file.OldPath, file.Status
	var oldContent string
	if status != "A" {
		if oldPath == "" {
			oldPath = path
		}
		var err error
		oldContent, err = readReviewBlob(ctx, repoPath, before+":"+oldPath, status != "D")
		if err != nil {
			return "", "", err
		}
	}
	if status == "D" {
		return oldContent, "", nil
	}
	newContent, err := readReviewBlob(ctx, repoPath, after+":"+path, false)
	return oldContent, newContent, err
}

func ReadFileRevision(ctx context.Context, repoPath, revision, path string) (string, error) {
	return readReviewBlob(ctx, repoPath, revision+":"+path, false)
}
