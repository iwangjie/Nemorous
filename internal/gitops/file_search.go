package gitops

import (
	"bufio"
	"context"
	"os/exec"
	"path/filepath"
	"strings"
	"time"

	"github.com/iwangjie/nemorous/internal/mani"
)

const WorkspaceFilePreviewLimit = 2500

const (
	FilePathPartial = 1
	FilePathExact   = 2
)

// SearchWorkspaceFiles walks the Git file list on demand, without keeping the
// full list in memory or running an indexer when the app is idle.
func SearchWorkspaceFiles(ctx context.Context, repos []mani.DiscoveredRepo, query string, limit int) []WorkspaceFile {
	if query == "" || limit <= 0 {
		return nil
	}
	var exact, partial []WorkspaceFile
	for _, repo := range repos {
		visitRepositoryFiles(ctx, repo, repos, func(path string) bool {
			file := workspaceFile(repo, path)
			switch WorkspacePathMatch(file, query) {
			case FilePathExact:
				exact = append(exact, file)
			case FilePathPartial:
				if len(partial) < limit {
					partial = append(partial, file)
				}
			}
			return len(exact) < limit
		})
		if ctx.Err() != nil || len(exact) == limit {
			break
		}
	}
	return append(exact, partial[:min(len(partial), limit-len(exact))]...)
}

func visitRepositoryFiles(ctx context.Context, repo mani.DiscoveredRepo, repos []mani.DiscoveredRepo, visit func(string) bool) {
	var childPaths []string
	if repo.IsRoot {
		childPaths = childRepoPaths(repo, repos)
	}
	ctx, cancel := context.WithTimeout(ctx, 10*time.Second)
	defer cancel()
	cmd := exec.CommandContext(ctx, "git", "ls-files", "-z", "--cached", "--others", "--exclude-standard")
	cmd.Dir = repo.AbsPath
	stdout, err := cmd.StdoutPipe()
	if err != nil || cmd.Start() != nil {
		return
	}
	reader := bufio.NewReader(stdout)
	previous := ""
	for {
		entry, err := reader.ReadString(0)
		if err != nil {
			break
		}
		path := strings.TrimSuffix(entry, "\x00")
		if path != previous && !isChildRepoPath(path, childPaths) && !visit(path) {
			_ = cmd.Process.Kill()
			break
		}
		previous = path
	}
	_ = cmd.Wait()
}

func WorkspacePathMatch(file WorkspaceFile, query string) int {
	rel := strings.ToLower(filepath.ToSlash(file.RelPath))
	path := strings.ToLower(file.RepoName) + "/" + rel
	if query == "" || query == rel || query == path || query == filepath.Base(rel) || strings.EqualFold(query, filepath.ToSlash(file.AbsPath)) {
		return FilePathExact
	}
	if strings.Contains(path, query) {
		return FilePathPartial
	}
	return 0
}

func workspaceFile(repo mani.DiscoveredRepo, path string) WorkspaceFile {
	return WorkspaceFile{
		RepoID: repo.ID, RepoName: repo.Name, RepoPath: repo.AbsPath, IsRoot: repo.IsRoot,
		Monogram: repo.Monogram, ColorIdx: repo.ColorIndex, RelPath: path, AbsPath: filepath.Join(repo.AbsPath, path),
	}
}

func childRepoPaths(root mani.DiscoveredRepo, repos []mani.DiscoveredRepo) []string {
	var paths []string
	for _, repo := range repos {
		if repo.ID == root.ID || repo.IsRoot {
			continue
		}
		rel, err := filepath.Rel(root.AbsPath, repo.AbsPath)
		if err == nil && rel != "." && filepath.IsLocal(rel) {
			paths = append(paths, filepath.ToSlash(rel))
		}
	}
	return paths
}

func isChildRepoPath(path string, childPaths []string) bool {
	for _, child := range childPaths {
		if path == child || strings.HasPrefix(path, child+"/") {
			return true
		}
	}
	return false
}
