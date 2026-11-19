package gitops

import (
	"context"
	"errors"
	"fmt"
	"os"
	"path/filepath"
	"slices"
	"strings"
)

// ChangeRange identifies one hunk by its half-open indexes on both sides.
type ChangeRange struct {
	OldStart, OldEnd, NewStart, NewEnd int
}

func (h rawHunk) changeRange() ChangeRange {
	old, next := h.oldStart, h.newStart
	if h.oldCount > 0 {
		old--
	}
	if h.newCount > 0 {
		next--
	}
	return ChangeRange{old, old + h.oldCount, next, next + h.newCount}
}

type PartialCommitFile struct {
	File     ChangedFile
	Old, New ReviewVersion
	Included []ChangeRange
}

// commitSelectedRanges uses a temporary Git index for the selected tree.
// The real index is locked while committing, so unrelated staged entries
// survive unchanged and failure before commit leaves the index intact.
func commitSelectedRanges(ctx context.Context, target BatchCommitTarget, message string) (string, error) {
	repo := target.Repo.AbsPath
	head, err := RunGit(ctx, repo, "rev-parse", "--verify", "HEAD")
	if err != nil {
		return "", err
	}
	index, err := lockWorktreeIndex(ctx, repo)
	if err != nil {
		return "", err
	}
	defer index.close()

	partIndex := filepath.Join(index.dir, "commit")
	run := func(args ...string) (string, error) {
		return runWithIndex(ctx, repo, partIndex, args...)
	}
	if _, err := run("read-tree", head); err != nil {
		return "", err
	}
	if len(target.Files) > 0 {
		if _, err := run(append([]string{"--literal-pathspecs", "add", "-A", "--"}, target.Files...)...); err != nil {
			return "", err
		}
	}
	paths := slices.Clone(target.Files)
	for _, selection := range target.Partial {
		file := selection.File
		snapshot, err := ReadReviewChange(ctx, file, false)
		base, baseErr := ReadReviewBlob(ctx, repo, head+":"+filepath.ToSlash(file.Path))
		if err != nil || baseErr != nil || snapshot.Old != base || ReviewTextVersion(snapshot.Old) != selection.Old || ReviewTextVersion(snapshot.New) != selection.New {
			return "", fmt.Errorf("%s 的差异已经变化，请重新勾选变更块", file.Path)
		}
		content, err := buildSelectedFile(snapshot.Old, snapshot.New, selection.Included)
		if err != nil {
			return "", err
		}
		rel := filepath.ToSlash(file.Path)
		indexData, err := index.run(ctx, "ls-files", "--stage", "-z", "--", ":(literal)"+rel)
		if err != nil {
			return "", err
		}
		entry, err := parseStageZero(indexData)
		if err != nil || entry == "" {
			return "", fmt.Errorf("%s 没有可部分提交的索引版本", file.Path)
		}
		hash, err := runGitInput(ctx, repo, content, "hash-object", "-w", "--stdin", "--path="+rel)
		if err != nil {
			return "", fmt.Errorf("暂存所选的变更块失败：%w", err)
		}
		if _, err := run("update-index", "--cacheinfo", strings.Fields(entry)[0], hash, rel); err != nil {
			return "", err
		}
		paths = append(paths, file.Path)
	}
	tree, err := run("write-tree")
	if err != nil {
		return "", err
	}
	if _, err := index.run(ctx, append([]string{"--literal-pathspecs", "restore", "--staged", "--source=" + tree, "--"}, paths...)...); err != nil {
		return "", err
	}
	if latest, err := RunGit(ctx, repo, "rev-parse", "--verify", "HEAD"); err != nil || latest != head {
		return "", errors.New("当前提交已变化，请刷新后再提交")
	}
	args := []string{"commit", "-m", message}
	if target.Amend {
		args = append(args, "--amend")
	}
	out, err := run(args...)
	if err != nil {
		return "", err
	}
	actual, err := RunGit(ctx, repo, "rev-parse", "HEAD^{tree}")
	if err != nil || actual != tree {
		return "", errors.New("提交已创建，Git 结果与选区索引不同，原暂存区仍在")
	}
	if err := index.install(); err != nil {
		return "", fmt.Errorf("提交已创建，更新暂存区失败：%w", err)
	}
	return out, nil
}

func buildSelectedFile(base, current string, included []ChangeRange) (string, error) {
	if len(included) == 0 {
		return "", errors.New("没有勾选可提交的变更块")
	}
	oldLines, newLines := fileLineStarts(base), fileLineStarts(current)
	ranges := slices.Clone(included)
	slices.SortFunc(ranges, func(a, b ChangeRange) int {
		if a.OldStart == b.OldStart {
			return a.NewStart - b.NewStart
		}
		return a.OldStart - b.OldStart
	})
	var out strings.Builder
	old, next := 0, 0
	for _, h := range ranges {
		if h.OldStart < old || h.NewStart < next || h.OldEnd < h.OldStart || h.NewEnd < h.NewStart ||
			h.OldEnd >= len(oldLines) || h.NewEnd >= len(newLines) {
			return "", errors.New("勾选的变更块位置不一致，请重新选择")
		}
		out.WriteString(base[oldLines[old]:oldLines[h.OldStart]])
		out.WriteString(current[newLines[h.NewStart]:newLines[h.NewEnd]])
		old, next = h.OldEnd, h.NewEnd
	}
	out.WriteString(base[oldLines[old]:])
	return out.String(), nil
}

func fileLineStarts(text string) []int {
	starts := []int{0}
	for i := 0; i < len(text); i++ {
		if text[i] == '\n' && i+1 < len(text) {
			starts = append(starts, i+1)
		}
	}
	if text != "" {
		starts = append(starts, len(text))
	}
	return starts
}

type worktreeIndexLock struct {
	repo, path, dir, snapshot string
	lock                      *os.File
}

func lockWorktreeIndex(ctx context.Context, repo string) (*worktreeIndexLock, error) {
	path, err := RunGit(ctx, repo, "rev-parse", "--path-format=absolute", "--git-path", "index")
	if err != nil {
		return nil, err
	}
	lock, err := os.OpenFile(path+".lock", os.O_WRONLY|os.O_CREATE|os.O_EXCL, 0o666)
	if err != nil {
		return nil, fmt.Errorf("暂存区不可写：%w", err)
	}
	index := &worktreeIndexLock{repo: repo, path: path, lock: lock}
	index.dir, err = os.MkdirTemp(filepath.Dir(path), "nemorous-index-")
	if err != nil {
		index.close()
		return nil, err
	}
	index.snapshot = filepath.Join(index.dir, "index")
	existing, err := os.ReadFile(path)
	if err == nil {
		err = os.WriteFile(index.snapshot, existing, 0o600)
	} else if errors.Is(err, os.ErrNotExist) {
		_, err = index.run(ctx, "read-tree", "--empty")
	}
	if err != nil {
		index.close()
		return nil, err
	}
	return index, nil
}

func (i *worktreeIndexLock) close() {
	_ = i.lock.Close()
	if i.path != "" {
		_ = os.Remove(i.path + ".lock")
	}
	if i.dir != "" {
		_ = os.RemoveAll(i.dir)
	}
}

func (i *worktreeIndexLock) install() error {
	data, err := os.ReadFile(i.snapshot)
	if err != nil {
		return err
	}
	if _, err := i.lock.Write(data); err != nil {
		return err
	}
	if err := i.lock.Sync(); err != nil {
		return err
	}
	if err := i.lock.Close(); err != nil {
		return err
	}
	if err := os.Rename(i.path+".lock", i.path); err != nil {
		return err
	}
	i.path = ""
	return nil
}

func (i *worktreeIndexLock) run(ctx context.Context, args ...string) (string, error) {
	return runWithIndex(ctx, i.repo, i.snapshot, args...)
}

func runWithIndex(ctx context.Context, repo, index string, args ...string) (string, error) {
	out, err := runGitEnv(ctx, repo, []string{"GIT_INDEX_FILE=" + index}, args...)
	if err != nil {
		return "", fmt.Errorf("%s: %w", strings.TrimSpace(out), err)
	}
	return strings.TrimSpace(out), nil
}
