package gitops

import (
	"context"
	"fmt"
	"os"
	"path/filepath"
	"regexp"
	"strings"

	"github.com/iwangjie/nemorous/internal/mani"
)

var repoSegmentRe = regexp.MustCompile(`[^A-Za-z0-9._-]+`)

// CloneRepo clones url into workspace/name and records the project in
// mani.yaml when present. It never overwrites an existing directory.
func CloneRepo(ctx context.Context, workspace, url, name string) (string, error) {
	url = strings.TrimSpace(url)
	if url == "" {
		return "", fmt.Errorf("仓库地址不能为空")
	}
	workspace = filepath.Clean(workspace)
	if name = strings.TrimSpace(name); name == "" {
		name = RepoNameFromURL(url)
	}
	name = repoSegmentRe.ReplaceAllString(strings.TrimSuffix(name, ".git"), "-")
	name = strings.Trim(name, "-.")
	if name == "" {
		return "", fmt.Errorf("无法从仓库地址推导目录名")
	}
	dest := filepath.Join(workspace, name)
	if rel, err := filepath.Rel(workspace, dest); err != nil || rel == ".." || strings.HasPrefix(rel, ".."+string(filepath.Separator)) {
		return "", fmt.Errorf("克隆目录必须位于工作区内")
	}
	if _, err := os.Stat(dest); err == nil {
		return "", fmt.Errorf("目标目录已存在：%s", dest)
	}
	if out, err := RunGitLong(ctx, workspace, "clone", "--", url, dest); err != nil {
		msg := strings.TrimSpace(out)
		if msg == "" {
			msg = err.Error()
		}
		return "", fmt.Errorf("克隆失败：%s", msg)
	}
	if _, err := os.Stat(filepath.Join(dest, ".git")); err != nil {
		return "", fmt.Errorf("克隆完成但目录不是 Git 仓库：%s", dest)
	}
	_ = mani.AddProject(workspace, name, name, url)
	return dest, nil
}

// RepoNameFromURL derives a repository directory name from an HTTPS or SSH URL.
func RepoNameFromURL(raw string) string {
	raw = strings.TrimSpace(strings.TrimSuffix(raw, "/"))
	raw = strings.TrimSuffix(raw, ".git")
	if i := strings.LastIndexAny(raw, "/:"); i >= 0 {
		raw = raw[i+1:]
	}
	return raw
}

// InitRepo creates a repository inside the workspace with main as its first
// branch, then records it in mani.yaml.
func InitRepo(ctx context.Context, workspace, name string) (string, error) {
	name = repoSegmentRe.ReplaceAllString(strings.Trim(strings.TrimSpace(name), "-."), "-")
	if name == "" {
		return "", fmt.Errorf("目录名不能为空")
	}
	dest := filepath.Join(filepath.Clean(workspace), name)
	if rel, err := filepath.Rel(workspace, dest); err != nil || rel == ".." || strings.HasPrefix(rel, ".."+string(filepath.Separator)) {
		return "", fmt.Errorf("仓库目录必须位于工作区内")
	}
	if _, err := os.Stat(dest); err == nil {
		return "", fmt.Errorf("目标目录已存在：%s", dest)
	}
	if out, err := RunGit(ctx, workspace, "init", "-b", "main", dest); err != nil {
		return "", fmt.Errorf("初始化失败：%s", strings.TrimSpace(out))
	}
	_ = mani.AddProject(workspace, name, name, "")
	return dest, nil
}

// ImportRepo validates an existing Git repository inside the workspace and
// records nested repositories in mani.yaml.
func ImportRepo(ctx context.Context, workspace, path string) (string, string, error) {
	workspace = filepath.Clean(workspace)
	path = filepath.Clean(strings.TrimSpace(path))
	if !filepath.IsAbs(path) {
		path = filepath.Join(workspace, path)
	}
	rel, err := filepath.Rel(workspace, path)
	if err != nil || rel == "." || rel == ".." || strings.HasPrefix(rel, ".."+string(filepath.Separator)) {
		return "", "", fmt.Errorf("仓库必须位于工作区内的子目录")
	}
	if info, err := os.Stat(path); err != nil || !info.IsDir() {
		return "", "", fmt.Errorf("目录不存在：%s", path)
	}
	out, err := RunGit(ctx, path, "rev-parse", "--show-toplevel")
	if err != nil {
		return "", "", fmt.Errorf("不是 Git 仓库：%s", path)
	}
	top := filepath.Clean(strings.TrimSpace(out))
	if top != path {
		// macOS /var is a symlink of /private/var; compare resolved paths.
		if resolvedPath, e1 := filepath.EvalSymlinks(path); e1 == nil {
			if resolvedTop, e2 := filepath.EvalSymlinks(top); e2 != nil || resolvedTop != resolvedPath {
				return "", "", fmt.Errorf("请选择仓库根目录：%s", top)
			}
		} else {
			return "", "", fmt.Errorf("请选择仓库根目录：%s", top)
		}
	}
	rel = filepath.ToSlash(rel)
	name := filepath.Base(path)
	_ = mani.AddProject(workspace, name, rel, "")
	return name, rel, nil
}

// LFSState reports whether Git LFS is installed and enabled for a repository.
type LFSState struct {
	Installed bool
	Enabled   bool
	Version   string
	Patterns  []string
}

// InspectLFS reports Git LFS availability and local tracking patterns.
func InspectLFS(ctx context.Context, repoPath string) LFSState {
	var st LFSState
	if out, err := RunGit(ctx, repoPath, "lfs", "version"); err == nil {
		st.Installed = true
		st.Version = firstLine(out)
	}
	if _, err := RunGit(ctx, repoPath, "config", "--local", "--get", "filter.lfs.clean"); err == nil {
		st.Enabled = true
	}
	if out, err := RunGit(ctx, repoPath, "lfs", "track"); err == nil {
		for _, line := range strings.Split(out, "\n") {
			line = strings.TrimSpace(line)
			if strings.HasPrefix(line, "Listing tracked patterns") || line == "" {
				continue
			}
			if i := strings.Index(line, " ("); i > 0 {
				line = line[:i]
			}
			st.Patterns = append(st.Patterns, line)
		}
	}
	return st
}

// LFSInstall enables Git LFS hooks for one repository.
func LFSInstall(ctx context.Context, repoPath string) (string, error) {
	return RunGit(ctx, repoPath, "lfs", "install", "--local")
}

// LFSPull downloads missing LFS objects for the current checkout.
func LFSPull(ctx context.Context, repoPath string) (string, error) {
	return RunGitLong(ctx, repoPath, "lfs", "pull")
}

// LFSTrack adds a pattern to .gitattributes and returns the command output.
func LFSTrack(ctx context.Context, repoPath, pattern string) (string, error) {
	pattern = strings.TrimSpace(pattern)
	if pattern == "" {
		return "", fmt.Errorf("LFS 模式不能为空")
	}
	return RunGit(ctx, repoPath, "lfs", "track", pattern)
}

// SparseState is the repository's sparse-checkout configuration.
type SparseState struct {
	Enabled  bool
	Patterns []string
}

// InspectSparse reports whether sparse checkout is enabled and its patterns.
func InspectSparse(ctx context.Context, repoPath string) SparseState {
	var st SparseState
	out, err := RunGit(ctx, repoPath, "sparse-checkout", "list")
	if err != nil {
		return st
	}
	st.Enabled = true
	for _, line := range strings.Split(out, "\n") {
		if line = strings.TrimSpace(line); line != "" {
			st.Patterns = append(st.Patterns, line)
		}
	}
	return st
}

// SetSparse enables sparse checkout with no-cone patterns.
func SetSparse(ctx context.Context, repoPath string, patterns []string) (string, error) {
	clean := make([]string, 0, len(patterns))
	for _, p := range patterns {
		if p = strings.TrimSpace(p); p != "" {
			clean = append(clean, p)
		}
	}
	if len(clean) == 0 {
		return "", fmt.Errorf("至少需要一个 sparse-checkout 模式")
	}
	args := append([]string{"sparse-checkout", "set", "--no-cone"}, clean...)
	return RunGit(ctx, repoPath, args...)
}

// DisableSparse turns sparse checkout off and restores the full worktree.
func DisableSparse(ctx context.Context, repoPath string) (string, error) {
	return RunGit(ctx, repoPath, "sparse-checkout", "disable")
}

// Worktree is one entry of git worktree list.
type Worktree struct {
	Path     string
	Head     string
	Branch   string
	Bare     bool
	Detached bool
	Prunable string
}

// ListWorktrees returns every worktree attached to a repository.
func ListWorktrees(ctx context.Context, repoPath string) ([]Worktree, error) {
	out, err := RunGit(ctx, repoPath, "worktree", "list", "--porcelain")
	if err != nil {
		return nil, err
	}
	var list []Worktree
	var cur Worktree
	flush := func() {
		if cur.Path != "" {
			list = append(list, cur)
		}
		cur = Worktree{}
	}
	for _, line := range strings.Split(out, "\n") {
		if line == "" {
			flush()
			continue
		}
		switch {
		case strings.HasPrefix(line, "worktree "):
			cur.Path = strings.TrimPrefix(line, "worktree ")
		case strings.HasPrefix(line, "HEAD "):
			cur.Head = strings.TrimPrefix(line, "HEAD ")
		case strings.HasPrefix(line, "branch "):
			cur.Branch = strings.TrimPrefix(strings.TrimPrefix(line, "branch "), "refs/heads/")
		case line == "bare":
			cur.Bare = true
		case line == "detached":
			cur.Detached = true
		case strings.HasPrefix(line, "prunable"):
			cur.Prunable = strings.TrimSpace(strings.TrimPrefix(line, "prunable"))
		}
	}
	flush()
	return list, nil
}

// AddWorktree creates a linked worktree at path, optionally checking out branch.
func AddWorktree(ctx context.Context, repoPath, path, branch string) (string, error) {
	path = filepath.Clean(strings.TrimSpace(path))
	if path == "" {
		return "", fmt.Errorf("worktree 路径不能为空")
	}
	args := []string{"worktree", "add"}
	if branch = strings.TrimSpace(branch); branch != "" {
		args = append(args, path, branch)
	} else {
		args = append(args, "--detach", path)
	}
	return RunGit(ctx, repoPath, args...)
}

// RemoveWorktree removes a linked worktree; force removes a dirty one.
func RemoveWorktree(ctx context.Context, repoPath, path string, force bool) (string, error) {
	args := []string{"worktree", "remove"}
	if force {
		args = append(args, "--force")
	}
	args = append(args, path)
	return RunGit(ctx, repoPath, args...)
}

// PruneWorktrees removes stale worktree metadata.
func PruneWorktrees(ctx context.Context, repoPath string) (string, error) {
	return RunGit(ctx, repoPath, "worktree", "prune")
}

// SigningConfig is a repository's commit signing configuration.
type SigningConfig struct {
	Enabled bool
	Format  string // "openpgp" or "ssh"
	Key     string
}

// InspectSigning reads repository-local signing configuration.
func InspectSigning(ctx context.Context, repoPath string) SigningConfig {
	var cfg SigningConfig
	cfg.Enabled = gitConfigBool(ctx, repoPath, "commit.gpgsign")
	cfg.Format = strings.TrimSpace(gitConfig(ctx, repoPath, "gpg.format"))
	if cfg.Format == "" {
		cfg.Format = "openpgp"
	}
	cfg.Key = strings.TrimSpace(gitConfig(ctx, repoPath, "user.signingkey"))
	return cfg
}

func gitConfig(ctx context.Context, repoPath, key string) string {
	out, err := RunGit(ctx, repoPath, "config", "--local", "--get", key)
	if err != nil {
		return ""
	}
	return strings.TrimSpace(out)
}

func gitConfigBool(ctx context.Context, repoPath, key string) bool {
	out, err := RunGit(ctx, repoPath, "config", "--local", "--bool", "--get", key)
	return err == nil && strings.TrimSpace(out) == "true"
}

// SetSigning stores commit signing settings locally; empty key disables signing.
func SetSigning(ctx context.Context, repoPath string, enabled bool, format, key string) (string, error) {
	format = strings.TrimSpace(format)
	key = strings.TrimSpace(key)
	if format == "" {
		format = "openpgp"
	}
	if enabled && key == "" {
		return "", fmt.Errorf("启用签名需要签名密钥")
	}
	if err := gitConfigSet(ctx, repoPath, "commit.gpgsign", fmt.Sprintf("%t", enabled)); err != nil {
		return "", err
	}
	if err := gitConfigSet(ctx, repoPath, "gpg.format", format); err != nil {
		return "", err
	}
	if err := gitConfigSet(ctx, repoPath, "user.signingkey", key); err != nil {
		return "", err
	}
	if enabled {
		return "已启用提交签名（" + format + "）", nil
	}
	return "已关闭提交签名", nil
}

func gitConfigSet(ctx context.Context, repoPath, key, value string) error {
	_, err := RunGit(ctx, repoPath, "config", "--local", key, value)
	return err
}
