package gitops

import (
	"bytes"
	"context"
	"errors"
	"fmt"
	"os"
	"os/exec"
	"path/filepath"
	"sort"
	"strconv"
	"strings"
	"sync"
	"time"

	"github.com/iwangjie/nemorous/internal/mani"
)

// RepoHealth represents the high-level status of a repository.
type RepoHealth string

const (
	HealthClean    RepoHealth = "clean"
	HealthDirty    RepoHealth = "dirty"
	HealthConflict RepoHealth = "conflict"
	HealthBehind   RepoHealth = "behind"
	HealthAhead    RepoHealth = "ahead"
	HealthDiverged RepoHealth = "diverged"
)

// ChangedFile represents a single modified, staged, untracked, or conflicted file.
type ChangedFile struct {
	RepoID         string
	RepoName       string
	RepoPath       string
	Path           string
	OrigPath       string
	Kind           string // "M", "A", "D", "R", "U", "?"
	IndexStatus    byte
	WorktreeStatus byte
	Staged         bool
	Unstaged       bool
	Conflicted     bool
	Untracked      bool
	AddedLines     int
	DeletedLines   int
}

// Key returns a unique identifier for selection/tracking across repositories.
func (f ChangedFile) Key() string {
	return f.RepoID + "::" + f.Path
}

// CommitInfo represents a git log entry.
type CommitInfo struct {
	Hash    string
	Subject string
	RelTime string
	Author  string
}

// BranchInfo holds branch metadata sorted by recent commit activity.
type BranchInfo struct {
	Name       string
	CommitUnix int64
	RelTime    string
	IsRemote   bool
	// Mine reports that the branch tip was authored by this repository's
	// configured user.email.
	Mine bool
}

// IsRecent reports whether the branch has had a commit within the last 7 days.
func (b BranchInfo) IsRecent(now time.Time) bool {
	if b.CommitUnix <= 0 {
		return false
	}
	return now.Sub(time.Unix(b.CommitUnix, 0)) <= 7*24*time.Hour
}

// RepoStatus holds the complete live snapshot of a repository.
type RepoStatus struct {
	Repo           mani.DiscoveredRepo
	Branch         string
	DefaultBranch  string
	Upstream       string
	Detached       bool
	Ahead          int
	Behind         int
	Files          []ChangedFile
	StagedCount    int
	ModifiedCount  int
	UntrackedCount int
	ConflictCount  int
	AddedLines     int
	DeletedLines   int
	RecentCommits  []CommitInfo
	LocalBranches  []string
	RemoteBranches []string
	BranchInfos    []BranchInfo
	Health         RepoHealth
	LastUpdated    time.Time
	Err            error
}

// Dirty reports whether the repository has any uncommitted or untracked changes.
func (s RepoStatus) Dirty() bool {
	return len(s.Files) > 0
}

// OpResult captures the outcome of a batch operation on a single repository.
type OpResult struct {
	RepoID   string
	RepoName string
	Status   string // "OK", "SKIP", "FAIL"
	Message  string
	Duration time.Duration
	Err      error
}

// WorkspaceFile represents a file entry for the multi-repo Code Explorer & fuzzy search.
type WorkspaceFile struct {
	RepoID   string
	RepoName string
	RepoPath string
	Monogram string
	ColorIdx int
	RelPath  string
	AbsPath  string
}

// RunGit runs a git command in repoPath with a timeout and returns trimmed stdout.
// Passes -c core.quotepath=false so UTF-8 Chinese filenames are never octal-escaped.
func RunGit(ctx context.Context, repoPath string, args ...string) (string, error) {
	return runGitOutput(ctx, repoPath, true, args...)
}

// RunGitRaw runs git like RunGit but keeps trailing newlines, for file
// contents whose exact final bytes are meaningful to a diff.
func RunGitRaw(ctx context.Context, repoPath string, args ...string) (string, error) {
	return runGitOutput(ctx, repoPath, false, args...)
}

// runGitInput runs git with stdin and returns combined output.
func runGitInput(ctx context.Context, repoPath, stdin string, args ...string) (string, error) {
	cctx, cancel := context.WithTimeout(ctx, 30*time.Second)
	defer cancel()
	fullArgs := append([]string{"-c", "core.quotepath=false"}, args...)
	cmd := exec.CommandContext(cctx, "git", fullArgs...)
	cmd.Dir = repoPath
	cmd.Env = append(os.Environ(), "GIT_OPTIONAL_LOCKS=0", "LC_ALL=en_US.UTF-8", "LANG=en_US.UTF-8")
	cmd.Stdin = strings.NewReader(stdin)
	var stdout, stderr bytes.Buffer
	cmd.Stdout = &stdout
	cmd.Stderr = &stderr
	err := cmd.Run()
	if err != nil {
		out := strings.TrimSpace(stderr.String())
		if out == "" {
			out = strings.TrimSpace(stdout.String())
		}
		return out, err
	}
	return strings.TrimSpace(stdout.String()), nil
}

// runGitEnv runs git with extra environment variables and no implicit output trimming.
func runGitEnv(ctx context.Context, repoPath string, extraEnv []string, args ...string) (string, error) {
	cctx, cancel := context.WithTimeout(ctx, 5*time.Minute)
	defer cancel()
	fullArgs := append([]string{"-c", "core.quotepath=false"}, args...)
	cmd := exec.CommandContext(cctx, "git", fullArgs...)
	cmd.Dir = repoPath
	cmd.Env = append(os.Environ(), "GIT_OPTIONAL_LOCKS=0", "LC_ALL=en_US.UTF-8", "LANG=en_US.UTF-8")
	cmd.Env = append(cmd.Env, extraEnv...)
	var stdout, stderr bytes.Buffer
	cmd.Stdout = &stdout
	cmd.Stderr = &stderr
	err := cmd.Run()
	out := strings.TrimSpace(stdout.String() + "\n" + stderr.String())
	return out, err
}

func runGitOutput(ctx context.Context, repoPath string, trim bool, args ...string) (string, error) {
	return runGitOutputTimeout(ctx, repoPath, trim, 30*time.Second, args...)
}

// RunGitLong runs git with a longer deadline, for clones and LFS transfers.
func RunGitLong(ctx context.Context, repoPath string, args ...string) (string, error) {
	return runGitOutputTimeout(ctx, repoPath, true, 15*time.Minute, args...)
}

func runGitOutputTimeout(ctx context.Context, repoPath string, trim bool, timeout time.Duration, args ...string) (string, error) {
	cctx, cancel := context.WithTimeout(ctx, timeout)
	defer cancel()

	fullArgs := append([]string{"-c", "core.quotepath=false"}, args...)
	cmd := exec.CommandContext(cctx, "git", fullArgs...)
	cmd.Dir = repoPath
	cmd.Env = append(os.Environ(), "GIT_OPTIONAL_LOCKS=0", "LC_ALL=en_US.UTF-8", "LANG=en_US.UTF-8")
	var stdout, stderr bytes.Buffer
	cmd.Stdout = &stdout
	cmd.Stderr = &stderr
	err := cmd.Run()
	if err != nil {
		errMsg := strings.TrimSpace(stderr.String())
		if errMsg == "" {
			errMsg = strings.TrimSpace(stdout.String())
		}
		if errMsg != "" {
			return stdout.String(), fmt.Errorf("%s: %w", errMsg, err)
		}
		return stdout.String(), err
	}
	out := stdout.String()
	if trim {
		out = strings.TrimRight(out, "\r\n")
	}
	return out, nil
}

func unquoteGitPath(p string) string {
	p = strings.TrimSpace(p)
	if len(p) >= 2 && p[0] == '"' && p[len(p)-1] == '"' {
		if uq, err := strconv.Unquote(p); err == nil {
			return uq
		}
		return strings.Trim(p, `"`)
	}
	return p
}

// normalizeEmail compares git identities without the angle brackets that
// for-each-ref prints around %(authoremail).
func normalizeEmail(email string) string {
	email = strings.TrimSpace(email)
	email = strings.TrimPrefix(email, "<")
	email = strings.TrimSuffix(email, ">")
	return strings.ToLower(strings.TrimSpace(email))
}

// InspectRepos inspects all discovered repositories in parallel.
func InspectRepos(ctx context.Context, repos []mani.DiscoveredRepo, concurrency int) []RepoStatus {
	if concurrency <= 0 {
		concurrency = 8
	}
	results := make([]RepoStatus, len(repos))
	sem := make(chan struct{}, concurrency)
	var wg sync.WaitGroup

	for i, repo := range repos {
		wg.Add(1)
		go func(idx int, r mani.DiscoveredRepo) {
			defer wg.Done()
			sem <- struct{}{}
			defer func() { <-sem }()
			results[idx] = InspectRepo(ctx, r)
		}(i, repo)
	}
	wg.Wait()
	return results
}

// InspectRepo inspects a single Git repository.
func InspectRepo(ctx context.Context, repo mani.DiscoveredRepo) RepoStatus {
	st := RepoStatus{
		Repo:          repo,
		DefaultBranch: detectDefaultBranch(ctx, repo.AbsPath),
		LastUpdated:   time.Now(),
	}

	// 1. Branch & upstream & ahead/behind via git status --porcelain=v1 -b -uall
	rawStatus, err := RunGit(ctx, repo.AbsPath, "status", "--porcelain=v1", "-b", "-uall")
	if err != nil {
		st.Err = err
		st.Branch = "unknown"
		st.Health = HealthClean
		return st
	}

	// Parse numstat for unstaged and staged line counts
	lineStats := make(map[string][2]int)
	mergeNumstat(ctx, repo.AbsPath, lineStats, false)
	mergeNumstat(ctx, repo.AbsPath, lineStats, true)

	lines := strings.Split(rawStatus, "\n")
	for i, line := range lines {
		if line == "" {
			continue
		}
		if i == 0 && strings.HasPrefix(line, "## ") {
			parseBranchHeader(line[3:], &st)
			continue
		}
		if len(line) < 3 {
			continue
		}
		x := line[0]
		y := line[1]
		pathPart := strings.TrimSpace(line[3:])
		origPath := ""
		if idx := strings.Index(pathPart, " -> "); idx >= 0 {
			origPath = unquoteGitPath(pathPart[:idx])
			pathPart = unquoteGitPath(pathPart[idx+4:])
		} else {
			pathPart = unquoteGitPath(pathPart)
		}

		// Ignore nested git repositories (untracked directory entries ending with "/" or containing .git)
		if strings.HasSuffix(pathPart, "/") {
			continue
		}
		if x == '?' && y == '?' {
			if _, err := os.Stat(filepath.Join(repo.AbsPath, pathPart, ".git")); err == nil {
				continue
			}
		}

		cf := ChangedFile{
			RepoID:         repo.ID,
			RepoName:       repo.Name,
			RepoPath:       repo.AbsPath,
			Path:           pathPart,
			OrigPath:       origPath,
			IndexStatus:    x,
			WorktreeStatus: y,
		}
		switch {
		case x == 'U' || y == 'U' || (x == 'A' && y == 'A') || (x == 'D' && y == 'D'):
			cf.Kind = "U"
			cf.Conflicted = true
			st.ConflictCount++
		case x == '?' && y == '?':
			cf.Kind = "?"
			cf.Untracked = true
			cf.Unstaged = true
			st.UntrackedCount++
		default:
			if x != ' ' && x != '?' {
				cf.Staged = true
				st.StagedCount++
			}
			if y != ' ' && y != '?' {
				cf.Unstaged = true
				st.ModifiedCount++
			}
			switch {
			case x == 'A' || y == 'A':
				cf.Kind = "A"
			case x == 'D' || y == 'D':
				cf.Kind = "D"
			case x == 'R' || y == 'R':
				cf.Kind = "R"
			default:
				cf.Kind = "M"
			}
		}

		if stats, ok := lineStats[cf.Path]; ok {
			cf.AddedLines = stats[0]
			cf.DeletedLines = stats[1]
			st.AddedLines += stats[0]
			st.DeletedLines += stats[1]
		}
		st.Files = append(st.Files, cf)
	}

	// 2. Recent commits
	logOut, err := RunGit(ctx, repo.AbsPath, "log", "-n", "10", "--pretty=format:%h|%s|%cr|%an")
	if err == nil && logOut != "" {
		for _, l := range strings.Split(logOut, "\n") {
			parts := strings.SplitN(l, "|", 4)
			if len(parts) == 4 {
				st.RecentCommits = append(st.RecentCommits, CommitInfo{
					Hash:    parts[0],
					Subject: parts[1],
					RelTime: parts[2],
					Author:  parts[3],
				})
			}
		}
	}

	// 3. Branches sorted by recent activity (--sort=-committerdate)
	userEmail := ""
	if out, cfgErr := RunGit(ctx, repo.AbsPath, "config", "user.email"); cfgErr == nil {
		userEmail = normalizeEmail(out)
	}
	refsOut, err := RunGit(ctx, repo.AbsPath, "for-each-ref", "--sort=-committerdate", "--format=%(refname)|%(committerdate:unix)|%(committerdate:relative)|%(authoremail)", "refs/heads", "refs/remotes/origin")
	if err == nil && refsOut != "" {
		for _, line := range strings.Split(refsOut, "\n") {
			line = strings.TrimSpace(line)
			if line == "" {
				continue
			}
			parts := strings.SplitN(line, "|", 4)
			refName := parts[0]
			var unixTs int64
			var relTime, authorEmail string
			if len(parts) >= 2 {
				unixTs, _ = strconv.ParseInt(strings.TrimSpace(parts[1]), 10, 64)
			}
			if len(parts) >= 3 {
				relTime = strings.TrimSpace(parts[2])
			}
			if len(parts) == 4 {
				authorEmail = normalizeEmail(parts[3])
			}
			mine := userEmail != "" && authorEmail == userEmail
			switch {
			case strings.HasPrefix(refName, "refs/heads/"):
				b := strings.TrimPrefix(refName, "refs/heads/")
				st.LocalBranches = append(st.LocalBranches, b)
				st.BranchInfos = append(st.BranchInfos, BranchInfo{
					Name:       b,
					CommitUnix: unixTs,
					RelTime:    relTime,
					IsRemote:   false,
					Mine:       mine,
				})
			case strings.HasPrefix(refName, "refs/remotes/origin/"):
				b := strings.TrimPrefix(refName, "refs/remotes/origin/")
				if b != "HEAD" {
					st.RemoteBranches = append(st.RemoteBranches, b)
					st.BranchInfos = append(st.BranchInfos, BranchInfo{
						Name:       b,
						CommitUnix: unixTs,
						RelTime:    relTime,
						IsRemote:   true,
					})
				}
			}
		}
	}

	// Determine overall health
	switch {
	case st.ConflictCount > 0:
		st.Health = HealthConflict
	case len(st.Files) > 0:
		st.Health = HealthDirty
	case st.Ahead > 0 && st.Behind > 0:
		st.Health = HealthDiverged
	case st.Behind > 0:
		st.Health = HealthBehind
	case st.Ahead > 0:
		st.Health = HealthAhead
	default:
		st.Health = HealthClean
	}

	return st
}

func parseBranchHeader(hdr string, st *RepoStatus) {
	// Examples:
	// main...origin/main [ahead 1, behind 2]
	// feature/foo
	// HEAD (no branch)
	// No commits yet on main
	if strings.HasPrefix(hdr, "HEAD ") || hdr == "HEAD" {
		st.Detached = true
		st.Branch = "HEAD (detached)"
		return
	}
	if strings.HasPrefix(hdr, "No commits yet on ") {
		st.Branch = strings.TrimPrefix(hdr, "No commits yet on ")
		return
	}
	bracketIdx := strings.Index(hdr, " [")
	branchPart := hdr
	if bracketIdx >= 0 {
		branchPart = hdr[:bracketIdx]
		meta := strings.TrimSuffix(hdr[bracketIdx+2:], "]")
		for _, item := range strings.Split(meta, ",") {
			item = strings.TrimSpace(item)
			if strings.HasPrefix(item, "ahead ") {
				st.Ahead, _ = strconv.Atoi(strings.TrimPrefix(item, "ahead "))
			} else if strings.HasPrefix(item, "behind ") {
				st.Behind, _ = strconv.Atoi(strings.TrimPrefix(item, "behind "))
			}
		}
	}
	if dots := strings.Index(branchPart, "..."); dots >= 0 {
		st.Branch = branchPart[:dots]
		st.Upstream = branchPart[dots+3:]
	} else {
		st.Branch = strings.TrimSpace(branchPart)
	}
}

func mergeNumstat(ctx context.Context, repoPath string, dst map[string][2]int, cached bool) {
	args := []string{"diff", "--numstat"}
	if cached {
		args = []string{"diff", "--cached", "--numstat"}
	}
	out, err := RunGit(ctx, repoPath, args...)
	if err != nil || out == "" {
		return
	}
	for _, line := range strings.Split(out, "\n") {
		parts := strings.Split(line, "\t")
		if len(parts) < 3 {
			continue
		}
		a, _ := strconv.Atoi(parts[0])
		d, _ := strconv.Atoi(parts[1])
		p := unquoteGitPath(parts[2])
		cur := dst[p]
		dst[p] = [2]int{cur[0] + a, cur[1] + d}
	}
}

func detectDefaultBranch(ctx context.Context, repoPath string) string {
	ref, err := RunGit(ctx, repoPath, "symbolic-ref", "--quiet", "--short", "refs/remotes/origin/HEAD")
	if err == nil && ref != "" {
		return strings.TrimPrefix(ref, "origin/")
	}
	for _, candidate := range []string{"main", "master"} {
		if _, err := RunGit(ctx, repoPath, "show-ref", "--verify", "--quiet", "refs/heads/"+candidate); err == nil {
			return candidate
		}
		if _, err := RunGit(ctx, repoPath, "show-ref", "--verify", "--quiet", "refs/remotes/origin/"+candidate); err == nil {
			return candidate
		}
	}
	return "main"
}

// GetFileDiff returns the old content (HEAD/index), new content (working tree or staged), and unified diff patch for a file.
func GetFileDiff(ctx context.Context, repoPath, relPath string, stagedOnly bool) (oldText, newText, patch string, err error) {
	cleanRel := filepath.Clean(relPath)
	if strings.HasPrefix(cleanRel, "..") {
		return "", "", "", errors.New("invalid relative file path")
	}
	absFile := filepath.Join(repoPath, cleanRel)

	if stagedOnly {
		oldOut, _ := RunGitRaw(ctx, repoPath, "show", "HEAD:"+filepath.ToSlash(cleanRel))
		newOut, _ := RunGitRaw(ctx, repoPath, "show", ":"+filepath.ToSlash(cleanRel))
		patchOut, _ := RunGit(ctx, repoPath, "diff", "--cached", "--", cleanRel)
		return oldOut, newOut, patchOut, nil
	}

	// Default: compare HEAD (or index if untracked/conflicted) against current working tree file on disk
	oldOut, showErr := RunGitRaw(ctx, repoPath, "show", "HEAD:"+filepath.ToSlash(cleanRel))
	if showErr != nil {
		// Might be newly staged or untracked
		oldOut, _ = RunGitRaw(ctx, repoPath, "show", ":"+filepath.ToSlash(cleanRel))
	}

	diskBytes, readErr := os.ReadFile(absFile)
	if readErr == nil {
		newText = string(diskBytes)
	}

	patchOut, _ := RunGit(ctx, repoPath, "diff", "HEAD", "--", cleanRel)
	if patchOut == "" {
		patchOut, _ = RunGit(ctx, repoPath, "diff", "--", cleanRel)
	}
	return oldOut, newText, patchOut, nil
}

// ReadRepoFile reads a file from a repository and also fetches its HEAD version for gutter diff bars.
func ReadRepoFile(ctx context.Context, repoPath, relPath string) (currentText string, headText string, err error) {
	cleanRel := filepath.Clean(relPath)
	if strings.HasPrefix(cleanRel, "..") {
		return "", "", errors.New("invalid relative path")
	}
	absPath := filepath.Join(repoPath, cleanRel)
	data, err := os.ReadFile(absPath)
	if err != nil {
		return "", "", err
	}
	headOut, _ := RunGitRaw(ctx, repoPath, "show", "HEAD:"+filepath.ToSlash(cleanRel))
	return string(data), headOut, nil
}

// SaveMicroEdit safely writes updated text to a file inside repoPath.
func SaveMicroEdit(repoPath, relPath, content string) error {
	cleanRepo := filepath.Clean(repoPath)
	cleanRel := filepath.Clean(relPath)
	if strings.HasPrefix(cleanRel, "..") || filepath.IsAbs(cleanRel) {
		return errors.New("refusing to write outside repository root")
	}
	absTarget := filepath.Join(cleanRepo, cleanRel)
	if !strings.HasPrefix(absTarget, cleanRepo+string(filepath.Separator)) {
		return errors.New("path escapes repository boundary")
	}
	return os.WriteFile(absTarget, []byte(content), 0o644)
}

// StageFile stages a single file in repoPath.
func StageFile(ctx context.Context, repoPath, relPath string) error {
	_, err := RunGit(ctx, repoPath, "add", "--", relPath)
	return err
}

// UnstageFile unstages a single file in repoPath.
func UnstageFile(ctx context.Context, repoPath, relPath string) error {
	_, err := RunGit(ctx, repoPath, "reset", "HEAD", "--", relPath)
	return err
}

// StageAll stages all modified and untracked files in repoPath.
func StageAll(ctx context.Context, repoPath string) error {
	_, err := RunGit(ctx, repoPath, "add", "-A")
	return err
}

// UnstageAll unstages all staged files in repoPath.
func UnstageAll(ctx context.Context, repoPath string) error {
	_, err := RunGit(ctx, repoPath, "reset", "HEAD")
	return err
}

// DiscardFile safely restores a tracked file to HEAD or removes an untracked file.
func DiscardFile(ctx context.Context, repoPath, relPath string, untracked bool) error {
	cleanRel := filepath.Clean(relPath)
	if strings.HasPrefix(cleanRel, "..") || filepath.IsAbs(cleanRel) {
		return errors.New("invalid file path")
	}
	if untracked {
		abs := filepath.Join(repoPath, cleanRel)
		return os.Remove(abs)
	}
	_, err := RunGit(ctx, repoPath, "checkout", "HEAD", "--", cleanRel)
	return err
}

// CommitRepo commits staged changes (or stages selected files first if provided).
func CommitRepo(ctx context.Context, repoPath string, message string, filesToStage []string) (string, error) {
	return commitRepo(ctx, repoPath, message, filesToStage, false)
}

// commitRepo stages the given files, then commits; amend rewrites the previous commit.
func commitRepo(ctx context.Context, repoPath string, message string, filesToStage []string, amend bool) (string, error) {
	message = strings.TrimSpace(message)
	if message == "" {
		return "", errors.New("commit message cannot be empty")
	}
	if len(filesToStage) > 0 {
		args := append([]string{"add", "--"}, filesToStage...)
		if _, err := RunGit(ctx, repoPath, args...); err != nil {
			return "", err
		}
	}
	args := []string{"commit"}
	if amend {
		args = append(args, "--amend")
	}
	args = append(args, "-m", message)
	return RunGit(ctx, repoPath, args...)
}

// BatchCommitTarget describes which files to commit in a repository during a unified multi-repo commit.
type BatchCommitTarget struct {
	Repo    mani.DiscoveredRepo
	Files   []string // if non-empty, stage these specific files before committing
	Message string   // per-repository message; the batch message is used when empty
	Amend   bool     // rewrite the previous commit instead of adding one
}

// BatchCommit performs a unified commit across multiple repositories in parallel.
func BatchCommit(ctx context.Context, targets []BatchCommitTarget, message string, pushAfter bool, concurrency int) []OpResult {
	if concurrency <= 0 {
		concurrency = 8
	}
	results := make([]OpResult, len(targets))
	sem := make(chan struct{}, concurrency)
	var wg sync.WaitGroup

	for i, t := range targets {
		wg.Add(1)
		go func(idx int, target BatchCommitTarget) {
			defer wg.Done()
			sem <- struct{}{}
			defer func() { <-sem }()

			start := time.Now()
			repoMessage := message
			if strings.TrimSpace(target.Message) != "" {
				repoMessage = target.Message
			}
			out, err := commitRepo(ctx, target.Repo.AbsPath, repoMessage, target.Files, target.Amend)
			if err != nil {
				results[idx] = OpResult{
					RepoID:   target.Repo.ID,
					RepoName: target.Repo.Name,
					Status:   "FAIL",
					Message:  err.Error(),
					Duration: time.Since(start),
					Err:      err,
				}
				return
			}
			msg := firstLine(out)
			if pushAfter {
				pushOut, pushErr := PushRepo(ctx, target.Repo.AbsPath)
				if pushErr != nil {
					results[idx] = OpResult{
						RepoID:   target.Repo.ID,
						RepoName: target.Repo.Name,
						Status:   "FAIL",
						Message:  "已提交但推送失败: " + pushErr.Error(),
						Duration: time.Since(start),
						Err:      pushErr,
					}
					return
				}
				if pushOut != "" {
					msg += " · pushed"
				}
			}
			results[idx] = OpResult{
				RepoID:   target.Repo.ID,
				RepoName: target.Repo.Name,
				Status:   "OK",
				Message:  msg,
				Duration: time.Since(start),
			}
		}(i, t)
	}
	wg.Wait()
	return results
}

// UndoLastCommit soft-resets the branch one commit back, keeping the changes
// staged. It refuses to rewrite commits that already exist on the upstream.
func UndoLastCommit(ctx context.Context, repoPath string) (string, error) {
	branch, err := RunGit(ctx, repoPath, "branch", "--show-current")
	if err != nil || strings.TrimSpace(branch) == "" {
		return "", errors.New("detached HEAD 无法撤销提交")
	}
	count, err := RunGit(ctx, repoPath, "rev-list", "--count", "HEAD")
	if err != nil {
		return "", err
	}
	if strings.TrimSpace(count) == "1" {
		return "", errors.New("这是首个提交，无法撤销")
	}
	if _, err := RunGit(ctx, repoPath, "rev-parse", "--abbrev-ref", "--symbolic-full-name", "@{u}"); err == nil {
		ahead, _ := RunGit(ctx, repoPath, "rev-list", "--count", "@{u}..HEAD")
		if strings.TrimSpace(ahead) == "0" {
			return "", errors.New("该提交已存在于远端，为安全起见未撤销")
		}
	}
	if _, err := RunGit(ctx, repoPath, "reset", "--soft", "HEAD~1"); err != nil {
		return "", err
	}
	return "已撤销上一次提交，改动保留在暂存区", nil
}

// FileHistory lists recent commits touching relPath, following renames.
func FileHistory(ctx context.Context, repoPath, relPath string, limit int) []CommitInfo {
	if limit <= 0 {
		limit = 30
	}
	out, err := RunGit(ctx, repoPath, "log", "--follow", fmt.Sprintf("-n%d", limit), "--pretty=format:%h|%s|%cr|%an", "--", relPath)
	if err != nil || strings.TrimSpace(out) == "" {
		return nil
	}
	var commits []CommitInfo
	for _, line := range strings.Split(out, "\n") {
		parts := strings.SplitN(line, "|", 4)
		if len(parts) == 4 {
			commits = append(commits, CommitInfo{
				Hash:    parts[0],
				Subject: parts[1],
				RelTime: parts[2],
				Author:  parts[3],
			})
		}
	}
	return commits
}

// PushRepo safely pushes the current branch to origin (never force-pushes).
func PushRepo(ctx context.Context, repoPath string) (string, error) {
	branch, err := RunGit(ctx, repoPath, "branch", "--show-current")
	if err != nil || branch == "" {
		return "", errors.New("cannot push detached HEAD")
	}
	// Check if upstream is configured
	if _, upErr := RunGit(ctx, repoPath, "rev-parse", "--abbrev-ref", "--symbolic-full-name", "@{u}"); upErr != nil {
		return RunGit(ctx, repoPath, "push", "-u", "origin", branch)
	}
	return RunGit(ctx, repoPath, "push")
}

// BatchPush pushes all specified repositories in parallel.
func BatchPush(ctx context.Context, repos []mani.DiscoveredRepo, concurrency int) []OpResult {
	return runParallelOp(repos, concurrency, func(r mani.DiscoveredRepo) (string, string, error) {
		out, err := PushRepo(ctx, r.AbsPath)
		if err != nil {
			return "FAIL", err.Error(), err
		}
		if out == "" || strings.Contains(out, "Everything up-to-date") {
			return "OK", "already up-to-date", nil
		}
		return "OK", firstLine(out), nil
	})
}

// BatchStash stashes every repository's changes, untracked files included.
func BatchStash(ctx context.Context, repos []mani.DiscoveredRepo, message string, concurrency int) []OpResult {
	return runParallelOp(repos, concurrency, func(r mani.DiscoveredRepo) (string, string, error) {
		porcelain, err := RunGit(ctx, r.AbsPath, "status", "--porcelain")
		if err != nil {
			return "FAIL", firstLine(err.Error()), err
		}
		if strings.TrimSpace(porcelain) == "" {
			return "SKIP", "没有需要贮藏的改动", nil
		}
		if _, err := RunGit(ctx, r.AbsPath, "stash", "push", "--include-untracked", "-m", message); err != nil {
			return "FAIL", firstLine(err.Error()), err
		}
		return "OK", "已贮藏改动", nil
	})
}

// BatchStashPop restores the most recent stash in each repository.
func BatchStashPop(ctx context.Context, repos []mani.DiscoveredRepo, concurrency int) []OpResult {
	return runParallelOp(repos, concurrency, func(r mani.DiscoveredRepo) (string, string, error) {
		list, err := RunGit(ctx, r.AbsPath, "stash", "list", "-n", "1")
		if err != nil {
			return "FAIL", firstLine(err.Error()), err
		}
		if strings.TrimSpace(list) == "" {
			return "SKIP", "没有贮藏", nil
		}
		if _, err := RunGit(ctx, r.AbsPath, "stash", "pop"); err != nil {
			return "FAIL", "恢复贮藏失败（可能冲突）：解决后执行 git stash drop 清理", err
		}
		return "OK", "已恢复贮藏", nil
	})
}

// FetchAll runs git fetch --prune across all repositories in parallel.
func FetchAll(ctx context.Context, repos []mani.DiscoveredRepo, concurrency int) []OpResult {
	return runParallelOp(repos, concurrency, func(r mani.DiscoveredRepo) (string, string, error) {
		_, err := RunGit(ctx, r.AbsPath, "fetch", "--prune", "--quiet")
		if err != nil {
			return "FAIL", err.Error(), err
		}
		return "OK", "fetched origin", nil
	})
}

// PullAll runs safe fast-forward pulls across all repositories in parallel.
// Local changes are stashed before the pull and restored afterwards, matching
// the "stash → pull → unstash" behavior IDEA applies to a dirty worktree.
func PullAll(ctx context.Context, repos []mani.DiscoveredRepo, concurrency int) []OpResult {
	return runParallelOp(repos, concurrency, func(r mani.DiscoveredRepo) (string, string, error) {
		b, err := RunGit(ctx, r.AbsPath, "branch", "--show-current")
		if err != nil || b == "" {
			return "SKIP", "detached HEAD", nil
		}
		if _, err := RunGit(ctx, r.AbsPath, "rev-parse", "--abbrev-ref", "@{u}"); err != nil {
			return "SKIP", fmt.Sprintf("%s (未关联远程跟踪分支)", b), nil
		}
		if unmerged, _ := RunGit(ctx, r.AbsPath, "ls-files", "-u"); strings.TrimSpace(unmerged) != "" {
			return "SKIP", fmt.Sprintf("%s (存在未解决的冲突，先解决后再拉取)", b), nil
		}
		porcelain, _ := RunGit(ctx, r.AbsPath, "status", "--porcelain")
		dirty := strings.TrimSpace(porcelain) != ""
		if dirty {
			if _, err := RunGit(ctx, r.AbsPath, "stash", "push", "--include-untracked", "-m", "nemorous: pull autostash"); err != nil {
				return "FAIL", fmt.Sprintf("%s 自动暂存本地改动失败: %v", b, err), err
			}
		}

		before, _ := RunGit(ctx, r.AbsPath, "rev-parse", "HEAD")
		_, pullErr := RunGit(ctx, r.AbsPath, "pull", "--ff-only", "--quiet")
		after, _ := RunGit(ctx, r.AbsPath, "rev-parse", "HEAD")
		if pullErr != nil {
			if dirty {
				if _, popErr := RunGit(ctx, r.AbsPath, "stash", "pop"); popErr != nil {
					return "FAIL", fmt.Sprintf("%s 拉取失败，且恢复本地改动失败（改动保留在 stash）: %v", b, popErr), pullErr
				}
				return "FAIL", fmt.Sprintf("%s 拉取失败，已恢复本地改动: %v", b, pullErr), pullErr
			}
			return "FAIL", fmt.Sprintf("%s (非快进或远端冲突: %v)", b, pullErr), pullErr
		}
		if dirty {
			if _, popErr := RunGit(ctx, r.AbsPath, "stash", "pop"); popErr != nil {
				return "FAIL", fmt.Sprintf("%s 已更新，但恢复本地改动发生冲突（改动保留在 stash，解决后 git stash drop）: %v", b, popErr), popErr
			}
		}

		updated := before != "" && after != "" && before != after
		if updated {
			return "OK", fmt.Sprintf("%s 已快进更新", b), nil
		}
		return "OK", fmt.Sprintf("%s 已是最新", b), nil
	})
}

// RepoBranchTarget specifies the target branch for a single repository in a multi-project switch.
type RepoBranchTarget struct {
	Repo         mani.DiscoveredRepo
	TargetBranch string
}

func switchSingleRepoBranch(ctx context.Context, r mani.DiscoveredRepo, targetBranch string, createIfMissing bool) (string, string, error) {
	targetBranch = strings.TrimSpace(targetBranch)
	if targetBranch == "" {
		return "FAIL", "目标分支不能为空", errors.New("empty branch")
	}
	cur, _ := RunGit(ctx, r.AbsPath, "branch", "--show-current")
	if cur == targetBranch {
		return "OK", fmt.Sprintf("已在 %s", targetBranch), nil
	}
	porcelain, _ := RunGit(ctx, r.AbsPath, "status", "--porcelain", "-uno")
	if strings.TrimSpace(porcelain) != "" {
		return "SKIP", fmt.Sprintf("当前 %s 有未提交改动，安全跳过", cur), nil
	}

	// 1. Check local branch
	if _, err := RunGit(ctx, r.AbsPath, "show-ref", "--verify", "--quiet", "refs/heads/"+targetBranch); err == nil {
		if _, err := RunGit(ctx, r.AbsPath, "switch", "--quiet", targetBranch); err != nil {
			return "FAIL", err.Error(), err
		}
		return "OK", fmt.Sprintf("%s → %s (本地)", cur, targetBranch), nil
	}

	// 2. Check remote origin/<targetBranch>
	_, _ = RunGit(ctx, r.AbsPath, "fetch", "origin", targetBranch, "--quiet")
	if _, err := RunGit(ctx, r.AbsPath, "show-ref", "--verify", "--quiet", "refs/remotes/origin/"+targetBranch); err == nil {
		if _, err := RunGit(ctx, r.AbsPath, "switch", "-c", targetBranch, "--track", "origin/"+targetBranch, "--quiet"); err != nil {
			return "FAIL", err.Error(), err
		}
		return "OK", fmt.Sprintf("%s → %s (跟踪 origin/%s)", cur, targetBranch, targetBranch), nil
	}

	// 3. Create new branch if requested
	if createIfMissing {
		if _, err := RunGit(ctx, r.AbsPath, "switch", "-c", targetBranch, "--quiet"); err != nil {
			return "FAIL", err.Error(), err
		}
		return "OK", fmt.Sprintf("%s → 新建分支 %s", cur, targetBranch), nil
	}

	return "SKIP", fmt.Sprintf("不存在分支 %s (保持在 %s)", targetBranch, cur), nil
}

// SwitchBranchAll switches (or creates) targetBranch across all specified repositories safely.
func SwitchBranchAll(ctx context.Context, repos []mani.DiscoveredRepo, targetBranch string, createIfMissing bool, concurrency int) []OpResult {
	return runParallelOp(repos, concurrency, func(r mani.DiscoveredRepo) (string, string, error) {
		return switchSingleRepoBranch(ctx, r, targetBranch, createIfMissing)
	})
}

// SwitchBranchPlan switches each repository in targets to its own specified TargetBranch in parallel.
func SwitchBranchPlan(ctx context.Context, targets []RepoBranchTarget, createIfMissing bool, concurrency int) []OpResult {
	if concurrency <= 0 {
		concurrency = 8
	}
	results := make([]OpResult, len(targets))
	sem := make(chan struct{}, concurrency)
	var wg sync.WaitGroup

	for i, t := range targets {
		wg.Add(1)
		go func(idx int, item RepoBranchTarget) {
			defer wg.Done()
			sem <- struct{}{}
			defer func() { <-sem }()
			start := time.Now()
			st, msg, err := switchSingleRepoBranch(ctx, item.Repo, item.TargetBranch, createIfMissing)
			results[idx] = OpResult{
				RepoID:   item.Repo.ID,
				RepoName: item.Repo.Name,
				Status:   st,
				Message:  msg,
				Duration: time.Since(start),
				Err:      err,
			}
		}(i, t)
	}
	wg.Wait()
	return results
}

// SwitchToMainAll switches all clean repositories to their default branch (main/master) and pulls --ff-only.
func SwitchToMainAll(ctx context.Context, repos []mani.DiscoveredRepo, concurrency int) []OpResult {
	return runParallelOp(repos, concurrency, func(r mani.DiscoveredRepo) (string, string, error) {
		porcelain, _ := RunGit(ctx, r.AbsPath, "status", "--porcelain")
		if strings.TrimSpace(porcelain) != "" {
			cur, _ := RunGit(ctx, r.AbsPath, "branch", "--show-current")
			return "SKIP", fmt.Sprintf("工作区有改动 (保持在 %s)", cur), nil
		}
		defBranch := detectDefaultBranch(ctx, r.AbsPath)
		if _, err := RunGit(ctx, r.AbsPath, "show-ref", "--verify", "--quiet", "refs/heads/"+defBranch); err == nil {
			if _, err := RunGit(ctx, r.AbsPath, "switch", "--quiet", defBranch); err != nil {
				return "FAIL", err.Error(), err
			}
		} else {
			if _, err := RunGit(ctx, r.AbsPath, "switch", "-c", defBranch, "--track", "origin/"+defBranch, "--quiet"); err != nil {
				return "FAIL", err.Error(), err
			}
		}
		_, pullErr := RunGit(ctx, r.AbsPath, "pull", "--ff-only", "--quiet")
		if pullErr != nil {
			return "OK", fmt.Sprintf("已切到 %s (远端未快进)", defBranch), nil
		}
		return "OK", fmt.Sprintf("已切到 %s 并同步最新", defBranch), nil
	})
}

// ListWorkspaceFiles lists tracked/unignored files across all repositories in parallel for fast code browsing and Cmd+K search.
func ListWorkspaceFiles(ctx context.Context, repos []mani.DiscoveredRepo, maxPerRepo int) []WorkspaceFile {
	if maxPerRepo <= 0 {
		maxPerRepo = 2500
	}
	perRepo := make([][]WorkspaceFile, len(repos))
	var wg sync.WaitGroup
	sem := make(chan struct{}, 8)

	for i, repo := range repos {
		wg.Add(1)
		go func(idx int, r mani.DiscoveredRepo) {
			defer wg.Done()
			sem <- struct{}{}
			defer func() { <-sem }()

			out, err := RunGit(ctx, r.AbsPath, "ls-files", "--cached", "--others", "--exclude-standard")
			if err != nil || out == "" {
				return
			}
			lines := strings.Split(out, "\n")
			if len(lines) > maxPerRepo {
				lines = lines[:maxPerRepo]
			}
			files := make([]WorkspaceFile, 0, len(lines))
			for _, l := range lines {
				l = unquoteGitPath(l)
				if l == "" {
					continue
				}
				// Skip nested child git repos when listing workspace root
				if r.IsRoot && strings.Contains(l, "/") {
					firstSeg := l[:strings.Index(l, "/")]
					isChildRepo := false
					for _, other := range repos {
						if !other.IsRoot && other.RelPath == firstSeg {
							isChildRepo = true
							break
						}
					}
					if isChildRepo {
						continue
					}
				}
				files = append(files, WorkspaceFile{
					RepoID:   r.ID,
					RepoName: r.Name,
					RepoPath: r.AbsPath,
					Monogram: r.Monogram,
					ColorIdx: r.ColorIndex,
					RelPath:  l,
					AbsPath:  filepath.Join(r.AbsPath, l),
				})
			}
			perRepo[idx] = files
		}(i, repo)
	}
	wg.Wait()

	var all []WorkspaceFile
	for _, list := range perRepo {
		all = append(all, list...)
	}
	sort.Slice(all, func(i, j int) bool {
		if all[i].RepoName != all[j].RepoName {
			return all[i].RepoName < all[j].RepoName
		}
		return all[i].RelPath < all[j].RelPath
	})
	return all
}

func runParallelOp(repos []mani.DiscoveredRepo, concurrency int, fn func(r mani.DiscoveredRepo) (status, msg string, err error)) []OpResult {
	if concurrency <= 0 {
		concurrency = 8
	}
	results := make([]OpResult, len(repos))
	sem := make(chan struct{}, concurrency)
	var wg sync.WaitGroup

	for i, repo := range repos {
		wg.Add(1)
		go func(idx int, r mani.DiscoveredRepo) {
			defer wg.Done()
			sem <- struct{}{}
			defer func() { <-sem }()
			start := time.Now()
			st, msg, err := fn(r)
			results[idx] = OpResult{
				RepoID:   r.ID,
				RepoName: r.Name,
				Status:   st,
				Message:  msg,
				Duration: time.Since(start),
				Err:      err,
			}
		}(i, repo)
	}
	wg.Wait()
	return results
}

func firstLine(s string) string {
	s = strings.TrimSpace(s)
	if idx := strings.IndexByte(s, '\n'); idx >= 0 {
		return s[:idx]
	}
	return s
}
