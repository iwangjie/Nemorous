package gitops

import (
	"context"
	"errors"
	"fmt"
	"os"
	"path/filepath"
	"strings"
)

type StagedReview struct {
	Head, Index, Worktree string
	Entry                 string
}

func ReadStagedReview(ctx context.Context, file ChangedFile) (StagedReview, error) {
	var view StagedReview
	path := filepath.ToSlash(file.Path)
	oldPath := path
	if file.OrigPath != "" && (file.IndexStatus == 'R' || file.IndexStatus == 'C') {
		oldPath = filepath.ToSlash(file.OrigPath)
	}
	var err error
	view.Head, err = ReadReviewBlob(ctx, file.RepoPath, "HEAD:"+oldPath)
	if err != nil {
		return view, err
	}
	data, err := RunGitRaw(ctx, file.RepoPath, "ls-files", "--stage", "-z", "--", ":(literal)"+path)
	if err != nil {
		return view, err
	}
	entry, err := parseStageZero(data)
	if err != nil {
		return view, err
	}
	view.Entry = entry
	if entry != "" {
		view.Index, err = ReadReviewBlob(ctx, file.RepoPath, strings.Fields(entry)[1])
		if err != nil {
			return view, err
		}
	}
	view.Worktree, err = ReadReviewWorktree(file.RepoPath, file.Path)
	if errors.Is(err, os.ErrNotExist) {
		err = nil
	}
	return view, err
}

func parseStageZero(data string) (string, error) {
	data = strings.TrimSuffix(data, "\x00")
	if data == "" {
		return "", nil
	}
	meta, _, ok := strings.Cut(data, "\t")
	fields := strings.Fields(meta)
	if !ok || strings.ContainsRune(data, 0) || len(fields) != 3 || fields[2] != "0" ||
		(fields[0] != "100644" && fields[0] != "100755") {
		return "", errors.New("暂存区没有可编辑的普通文本版本")
	}
	return meta, nil
}

// SaveStagedReview replaces only the stage-zero blob, keeping its mode and
// checking the expected index entry while the Git index lock is held.
func SaveStagedReview(ctx context.Context, repo, path, content, expectedEntry string) (string, error) {
	if expectedEntry == "" || !filepath.IsLocal(path) || filepath.Clean(path) == "." {
		return "", errors.New("文件没有可编辑的暂存版本")
	}
	if len(content) > reviewFileLimit {
		return "", errReviewFileTooBig
	}
	index, err := lockWorktreeIndex(ctx, repo)
	if err != nil {
		return "", err
	}
	defer index.close()
	rel := filepath.ToSlash(path)
	data, err := index.run(ctx, "ls-files", "--stage", "-z", "--", ":(literal)"+rel)
	if err != nil {
		return "", err
	}
	entry, err := parseStageZero(data)
	if err != nil || entry != expectedEntry {
		return "", errors.New("暂存区文件已被修改，请重新打开对比")
	}
	hash, err := runGitInput(ctx, repo, content, "hash-object", "-w", "--stdin")
	if err != nil {
		return "", fmt.Errorf("保存暂存版本失败：%w", err)
	}
	mode := strings.Fields(entry)[0]
	if _, err := index.run(ctx, "update-index", "--cacheinfo", mode, hash, rel); err != nil {
		return "", err
	}
	if err := index.install(); err != nil {
		return "", err
	}
	return mode + " " + hash + " 0", nil
}
