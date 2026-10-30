package gitops

import (
	"context"
	"fmt"
	"os"
	"os/exec"
	"path/filepath"
	"regexp"
	"strconv"
	"strings"
	"time"
)

// StashEntry is one entry of `git stash list`.
type StashEntry struct {
	Ref     string
	Index   int
	Message string
	Date    string
}

// TagInfo is one ref under refs/tags.
type TagInfo struct {
	Name    string
	Hash    string
	Date    string
	Subject string
}

// RemoteInfo is one git remote.
type RemoteInfo struct {
	Name     string
	FetchURL string
	PushURL  string
}

// SubmoduleInfo is one entry of `git submodule status`.
type SubmoduleInfo struct {
	Path        string
	Hash        string
	URL         string
	Branch      string
	Initialized bool
	Modified    bool
}

// RepoOpState says which multi-step Git operation is in progress.
type RepoOpState struct {
	Rebase     bool
	CherryPick bool
	Revert     bool
	Merge      bool
	Conflicts  []string
}

// Busy reports whether a multi-step operation (or an unresolved merge)
// needs the user's attention.
func (s RepoOpState) Busy() bool {
	return s.Rebase || s.CherryPick || s.Revert || s.Merge
}

var (
	stashRefPattern = regexp.MustCompile(`^stash@\{\d+\}$`)
	hashPattern     = regexp.MustCompile(`^[0-9a-fA-F]{4,40}$`)
	remotePattern   = regexp.MustCompile(`^[A-Za-z0-9][A-Za-z0-9._-]*$`)
)

// ListStashes returns a repository's stash stack, newest first.
func ListStashes(ctx context.Context, repoPath string) ([]StashEntry, error) {
	out, err := RunGit(ctx, repoPath, "stash", "list", "--format=%gd%x1f%gs%x1f%ci")
	if err != nil {
		return nil, err
	}
	var entries []StashEntry
	for _, line := range strings.Split(out, "\n") {
		if strings.TrimSpace(line) == "" {
			continue
		}
		parts := strings.SplitN(line, "\x1f", 3)
		if len(parts) < 3 {
			continue
		}
		entry := StashEntry{Ref: parts[0], Message: parts[1], Date: parts[2]}
		if n, ok := stashIndex(entry.Ref); ok {
			entry.Index = n
		}
		entries = append(entries, entry)
	}
	return entries, nil
}

func stashIndex(ref string) (int, bool) {
	if !stashRefPattern.MatchString(ref) {
		return 0, false
	}
	n, err := strconv.Atoi(ref[len("stash@{") : len(ref)-1])
	return n, err == nil
}

// StashCreate stashes the working tree changes of one repository.
func StashCreate(ctx context.Context, repoPath, message string, includeUntracked bool) (string, error) {
	args := []string{"stash", "push"}
	if includeUntracked {
		args = append(args, "--include-untracked")
	}
	if msg := strings.TrimSpace(message); msg != "" {
		args = append(args, "-m", msg)
	}
	out, err := RunGit(ctx, repoPath, args...)
	if err != nil {
		return "", err
	}
	if strings.Contains(out, "No local changes") {
		return "没有可贮藏的改动", nil
	}
	return firstLine(out), nil
}

// StashDiff shows one stash's patch.
func StashDiff(ctx context.Context, repoPath, ref string) (string, error) {
	if err := checkStashRef(ref); err != nil {
		return "", err
	}
	return RunGit(ctx, repoPath, "stash", "show", "-p", "--no-color", ref)
}

func checkStashRef(ref string) error {
	if !stashRefPattern.MatchString(ref) {
		return fmt.Errorf("无效的 stash 引用 %q", ref)
	}
	return nil
}

// StashApply applies one stash without dropping it.
func StashApply(ctx context.Context, repoPath, ref string) error {
	if err := checkStashRef(ref); err != nil {
		return err
	}
	_, err := RunGit(ctx, repoPath, "stash", "apply", ref)
	return err
}

// StashPop applies and drops one stash.
func StashPop(ctx context.Context, repoPath, ref string) error {
	if err := checkStashRef(ref); err != nil {
		return err
	}
	_, err := RunGit(ctx, repoPath, "stash", "pop", ref)
	return err
}

// StashDrop removes one stash without applying it.
func StashDrop(ctx context.Context, repoPath, ref string) error {
	if err := checkStashRef(ref); err != nil {
		return err
	}
	_, err := RunGit(ctx, repoPath, "stash", "drop", ref)
	return err
}

func checkBranchOrRef(name string) error {
	name = strings.TrimSpace(name)
	if name == "" {
		return fmt.Errorf("分支或提交不能为空")
	}
	if strings.HasPrefix(name, "-") || strings.ContainsAny(name, "\x00\n\r") {
		return fmt.Errorf("无效的分支或提交 %q", name)
	}
	return nil
}

// Rebase rebases the current branch onto onto.
func Rebase(ctx context.Context, repoPath, onto string) (string, error) {
	if err := checkBranchOrRef(onto); err != nil {
		return "", err
	}
	out, err := RunGit(ctx, repoPath, "rebase", onto)
	if err != nil {
		return strings.TrimSpace(out), err
	}
	return firstLine(out), nil
}

// RebaseAbort aborts an in-progress rebase.
func RebaseAbort(ctx context.Context, repoPath string) error {
	_, err := RunGit(ctx, repoPath, "rebase", "--abort")
	return err
}

// RebaseContinue continues a rebase after conflicts were staged.
func RebaseContinue(ctx context.Context, repoPath string) (string, error) {
	out, err := RunGit(ctx, repoPath, "-c", "core.editor=true", "rebase", "--continue")
	return strings.TrimSpace(out), err
}

// CherryPick applies one commit onto the current branch.
func CherryPick(ctx context.Context, repoPath, hash string) (string, error) {
	if !hashPattern.MatchString(strings.TrimSpace(hash)) {
		return "", fmt.Errorf("无效的提交 %q", hash)
	}
	out, err := RunGit(ctx, repoPath, "cherry-pick", hash)
	return strings.TrimSpace(out), err
}

// CherryPickAbort aborts an in-progress cherry-pick.
func CherryPickAbort(ctx context.Context, repoPath string) error {
	_, err := RunGit(ctx, repoPath, "cherry-pick", "--abort")
	return err
}

// CherryPickContinue continues a cherry-pick after conflicts were staged.
func CherryPickContinue(ctx context.Context, repoPath string) (string, error) {
	out, err := RunGit(ctx, repoPath, "-c", "core.editor=true", "cherry-pick", "--continue")
	return strings.TrimSpace(out), err
}

// Revert creates a commit reverting hash. mainline selects the parent of a
// merge commit, and is 0 for ordinary commits.
func Revert(ctx context.Context, repoPath, hash string, mainline int) (string, error) {
	if !hashPattern.MatchString(strings.TrimSpace(hash)) {
		return "", fmt.Errorf("无效的提交 %q", hash)
	}
	args := []string{"revert", "--no-edit"}
	if mainline > 0 {
		args = append(args, "-m", strconv.Itoa(mainline))
	}
	args = append(args, hash)
	out, err := RunGit(ctx, repoPath, args...)
	return strings.TrimSpace(out), err
}

// RevertAbort aborts an in-progress revert.
func RevertAbort(ctx context.Context, repoPath string) error {
	_, err := RunGit(ctx, repoPath, "revert", "--abort")
	return err
}

// RevertContinue continues a revert after conflicts were staged.
func RevertContinue(ctx context.Context, repoPath string) (string, error) {
	out, err := RunGit(ctx, repoPath, "-c", "core.editor=true", "revert", "--continue")
	return strings.TrimSpace(out), err
}

// ListTags returns the repository's tags, newest first.
func ListTags(ctx context.Context, repoPath string) ([]TagInfo, error) {
	out, err := RunGit(ctx, repoPath, "for-each-ref", "--sort=-creatordate",
		"--format=%(refname:short)%1f%(objectname:short)%1f%(creatordate:iso-strict)%1f%(subject)", "refs/tags")
	if err != nil {
		return nil, err
	}
	var tags []TagInfo
	for _, line := range strings.Split(out, "\n") {
		if strings.TrimSpace(line) == "" {
			continue
		}
		parts := strings.SplitN(line, "\x1f", 4)
		if len(parts) < 4 {
			continue
		}
		tags = append(tags, TagInfo{Name: parts[0], Hash: parts[1], Date: parts[2], Subject: parts[3]})
	}
	return tags, nil
}

// CreateTag creates a lightweight tag, or an annotated one when message is set.
func CreateTag(ctx context.Context, repoPath, name, message, ref string, force bool) error {
	name = strings.TrimSpace(name)
	if name == "" || strings.HasPrefix(name, "-") || strings.ContainsAny(name, " \t\n\r\x00") {
		return fmt.Errorf("无效的标签名 %q", name)
	}
	if _, err := RunGit(ctx, repoPath, "check-ref-format", "refs/tags/"+name); err != nil {
		return fmt.Errorf("无效的标签名 %q", name)
	}
	if ref != "" {
		if err := checkBranchOrRef(ref); err != nil {
			return err
		}
	}
	args := []string{"tag"}
	if force {
		args = append(args, "-f")
	}
	if msg := strings.TrimSpace(message); msg != "" {
		args = append(args, "-a", name, "-m", msg)
	} else {
		args = append(args, name)
	}
	if ref != "" {
		args = append(args, ref)
	}
	_, err := RunGit(ctx, repoPath, args...)
	return err
}

// DeleteTag deletes a local tag.
func DeleteTag(ctx context.Context, repoPath, name string) error {
	if strings.TrimSpace(name) == "" {
		return fmt.Errorf("标签名不能为空")
	}
	if strings.HasPrefix(name, "-") {
		return fmt.Errorf("无效的标签名 %q", name)
	}
	_, err := RunGit(ctx, repoPath, "tag", "-d", "--", name)
	return err
}

// PushTag pushes one tag to origin.
func PushTag(ctx context.Context, repoPath, name string) error {
	if strings.TrimSpace(name) == "" {
		return fmt.Errorf("标签名不能为空")
	}
	_, err := RunGit(ctx, repoPath, "push", "origin", "refs/tags/"+name)
	return err
}

// ListRemotes returns the repository's remotes with their URLs.
func ListRemotes(ctx context.Context, repoPath string) ([]RemoteInfo, error) {
	out, err := RunGit(ctx, repoPath, "remote", "-v")
	if err != nil {
		return nil, err
	}
	byName := make(map[string]*RemoteInfo)
	order := make([]string, 0)
	for _, line := range strings.Split(out, "\n") {
		fields := strings.Fields(line)
		if len(fields) < 3 {
			continue
		}
		name, url, kind := fields[0], fields[1], fields[2]
		r, ok := byName[name]
		if !ok {
			r = &RemoteInfo{Name: name}
			byName[name] = r
			order = append(order, name)
		}
		switch kind {
		case "(fetch)":
			r.FetchURL = url
		case "(push)":
			r.PushURL = url
		}
	}
	outList := make([]RemoteInfo, 0, len(order))
	for _, name := range order {
		outList = append(outList, *byName[name])
	}
	return outList, nil
}

// AddRemote adds a remote.
func AddRemote(ctx context.Context, repoPath, name, url string) error {
	if !remotePattern.MatchString(name) {
		return fmt.Errorf("无效的 remote 名称 %q", name)
	}
	if strings.TrimSpace(url) == "" {
		return fmt.Errorf("remote 地址不能为空")
	}
	_, err := RunGit(ctx, repoPath, "remote", "add", "--", name, url)
	return err
}

// SetRemoteURL changes a remote's URL.
func SetRemoteURL(ctx context.Context, repoPath, name, url string) error {
	if !remotePattern.MatchString(name) {
		return fmt.Errorf("无效的 remote 名称 %q", name)
	}
	if strings.TrimSpace(url) == "" {
		return fmt.Errorf("remote 地址不能为空")
	}
	_, err := RunGit(ctx, repoPath, "remote", "set-url", "--", name, url)
	return err
}

// RemoveRemote removes a remote.
func RemoveRemote(ctx context.Context, repoPath, name string) error {
	if !remotePattern.MatchString(name) {
		return fmt.Errorf("无效的 remote 名称 %q", name)
	}
	_, err := RunGit(ctx, repoPath, "remote", "remove", name)
	return err
}

// ListSubmodules returns the repository's submodules, initialized or not.
func ListSubmodules(ctx context.Context, repoPath string) ([]SubmoduleInfo, error) {
	// Parse again by key so path/url/branch of one submodule meet.
	meta := make(map[string]SubmoduleInfo)
	if config, err := RunGit(ctx, repoPath, "config", "--file", ".gitmodules", "--get-regexp",
		`^submodule\..*\.(path|url|branch)$`); err == nil {
		for _, line := range strings.Split(config, "\n") {
			fields := strings.Fields(line)
			if len(fields) < 2 {
				continue
			}
			key, value := fields[0], strings.Join(fields[1:], " ")
			prefix := key[:strings.LastIndex(key, ".")]
			entry := meta[prefix]
			switch {
			case strings.HasSuffix(key, ".path"):
				entry.Path = value
			case strings.HasSuffix(key, ".url"):
				entry.URL = value
			case strings.HasSuffix(key, ".branch"):
				entry.Branch = value
			}
			meta[prefix] = entry
		}
	}
	out, err := RunGit(ctx, repoPath, "submodule", "status", "--recursive")
	if err != nil {
		return nil, err
	}
	byPath := make(map[string]SubmoduleInfo)
	for _, entry := range meta {
		if entry.Path != "" {
			byPath[entry.Path] = entry
		}
	}
	var modules []SubmoduleInfo
	for _, line := range strings.Split(out, "\n") {
		if strings.TrimSpace(line) == "" {
			continue
		}
		mod := SubmoduleInfo{Initialized: true}
		switch line[0] {
		case '-':
			mod.Initialized = false
		case '+':
			mod.Modified = true
		}
		rest := strings.TrimSpace(line[1:])
		fields := strings.Fields(rest)
		if len(fields) >= 2 {
			mod.Hash = fields[0]
			mod.Path = fields[1]
		}
		if len(fields) >= 3 {
			mod.URL = strings.Trim(fields[2], "()")
		}
		if metaMod, ok := byPath[mod.Path]; ok {
			if mod.URL == "" {
				mod.URL = metaMod.URL
			}
			mod.Branch = metaMod.Branch
		}
		modules = append(modules, mod)
	}
	// Include configured submodules that have not been checked out at all.
	seen := make(map[string]bool, len(modules))
	for _, mod := range modules {
		seen[mod.Path] = true
	}
	for _, mod := range meta {
		if mod.Path != "" && !seen[mod.Path] {
			mod.Initialized = false
			modules = append(modules, mod)
		}
	}
	return modules, nil
}

// UpdateSubmodules initializes and updates every submodule.
func UpdateSubmodules(ctx context.Context, repoPath string) (string, error) {
	return RunGitWithEnv(ctx, repoPath, []string{"GIT_TERMINAL_PROMPT=0"}, "submodule", "update", "--init", "--recursive")
}

// SyncSubmodules syncs submodule URLs from .gitmodules.
func SyncSubmodules(ctx context.Context, repoPath string) (string, error) {
	out, err := RunGit(ctx, repoPath, "submodule", "sync", "--recursive")
	return strings.TrimSpace(out), err
}

// AddSubmodule adds a submodule at dest.
func AddSubmodule(ctx context.Context, repoPath, url, dest string) (string, error) {
	if strings.TrimSpace(url) == "" || strings.TrimSpace(dest) == "" {
		return "", fmt.Errorf("子模块地址与目录不能为空")
	}
	if filepath.IsAbs(dest) || strings.HasPrefix(filepath.Clean(dest), "..") {
		return "", fmt.Errorf("子模块目录必须在仓库内")
	}
	out, err := RunGit(ctx, repoPath, "-c", "protocol.file.allow=always", "submodule", "add", "--", url, dest)
	return strings.TrimSpace(out), err
}

// RemoveSubmodule deinitializes and removes a submodule at path.
func RemoveSubmodule(ctx context.Context, repoPath, subPath string) error {
	if strings.TrimSpace(subPath) == "" {
		return fmt.Errorf("子模块目录不能为空")
	}
	if filepath.IsAbs(subPath) || strings.HasPrefix(filepath.Clean(subPath), "..") {
		return fmt.Errorf("子模块目录必须在仓库内")
	}
	_, _ = RunGit(ctx, repoPath, "submodule", "deinit", "-f", "--", subPath)
	if _, err := RunGit(ctx, repoPath, "rm", "-f", "--", subPath); err != nil {
		return err
	}
	name := filepath.Base(filepath.Clean(subPath))
	_, _ = RunGit(ctx, repoPath, "config", "--remove-section", "submodule."+name)
	return nil
}

// RepoOperationState reads whether a multi-step operation is in progress
// and which files still conflict.
func RepoOperationState(ctx context.Context, repoPath string) RepoOpState {
	var st RepoOpState
	gitPath := func(name string) string {
		out, err := RunGit(ctx, repoPath, "rev-parse", "--git-path", name)
		if err != nil {
			return ""
		}
		p := strings.TrimSpace(out)
		if p == "" {
			return ""
		}
		if !filepath.IsAbs(p) {
			p = filepath.Join(repoPath, p)
		}
		return p
	}
	if p := gitPath("rebase-merge"); p != "" {
		if _, err := os.Stat(p); err == nil {
			st.Rebase = true
		}
	}
	if p := gitPath("rebase-apply"); p != "" {
		if _, err := os.Stat(p); err == nil {
			st.Rebase = true
		}
	}
	if p := gitPath("CHERRY_PICK_HEAD"); p != "" {
		if _, err := os.Stat(p); err == nil {
			st.CherryPick = true
		}
	}
	if p := gitPath("REVERT_HEAD"); p != "" {
		if _, err := os.Stat(p); err == nil {
			st.Revert = true
		}
	}
	if p := gitPath("MERGE_HEAD"); p != "" {
		if _, err := os.Stat(p); err == nil {
			st.Merge = true
		}
	}
	if out, err := RunGit(ctx, repoPath, "diff", "--name-only", "--diff-filter=U"); err == nil {
		for _, line := range strings.Split(out, "\n") {
			if strings.TrimSpace(line) != "" {
				st.Conflicts = append(st.Conflicts, strings.TrimSpace(line))
			}
		}
	}
	return st
}

// ContinueOperation continues whichever operation is in progress.
func ContinueOperation(ctx context.Context, repoPath string) (string, error) {
	st := RepoOperationState(ctx, repoPath)
	switch {
	case st.Rebase:
		return RebaseContinue(ctx, repoPath)
	case st.CherryPick:
		return CherryPickContinue(ctx, repoPath)
	case st.Revert:
		return RevertContinue(ctx, repoPath)
	case st.Merge:
		return RunGit(ctx, repoPath, "-c", "core.editor=true", "commit", "--no-edit")
	default:
		return "", fmt.Errorf("没有进行中的 rebase/cherry-pick/revert/merge")
	}
}

// AbortOperation aborts whichever operation is in progress.
func AbortOperation(ctx context.Context, repoPath string) error {
	st := RepoOperationState(ctx, repoPath)
	switch {
	case st.Rebase:
		return RebaseAbort(ctx, repoPath)
	case st.CherryPick:
		return CherryPickAbort(ctx, repoPath)
	case st.Revert:
		return RevertAbort(ctx, repoPath)
	default:
		return fmt.Errorf("没有进行中的 rebase/cherry-pick/revert")
	}
}

// RunGitWithEnv runs git with extra environment values, for commands like
// `git submodule update` that may prompt.
func RunGitWithEnv(ctx context.Context, repoPath string, env []string, args ...string) (string, error) {
	cctx, cancel := context.WithTimeout(ctx, 60*time.Second)
	defer cancel()
	fullArgs := append([]string{"-c", "core.quotepath=false"}, args...)
	cmd := exec.CommandContext(cctx, "git", fullArgs...)
	cmd.Dir = repoPath
	cmd.Env = append(os.Environ(), "GIT_OPTIONAL_LOCKS=0", "LC_ALL=en_US.UTF-8", "LANG=en_US.UTF-8")
	cmd.Env = append(cmd.Env, env...)
	out, err := cmd.CombinedOutput()
	text := strings.TrimSpace(string(out))
	if err != nil {
		if text != "" {
			return text, fmt.Errorf("%s: %w", text, err)
		}
		return text, err
	}
	return text, nil
}
