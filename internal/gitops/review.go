package gitops

import (
	"bytes"
	"context"
	"crypto/rand"
	"crypto/sha256"
	"errors"
	"fmt"
	"io"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"time"
	"unicode/utf8"
)

const reviewFileLimit = 2 << 20

var (
	ErrReviewFileChanged = errors.New("磁盘文件已被其他程序修改，请先核对外部改动")
	errReviewFileTooBig  = errors.New("文件超过 2 MiB，无法在文本编辑器中显示")
	errReviewFileBinary  = errors.New("文件不是 UTF-8 文本")
)

type ReviewVersion [sha256.Size]byte

func ReviewTextVersion(text string) ReviewVersion { return sha256.Sum256([]byte(text)) }

type ReviewDiff struct {
	Old, New       string
	WorktreeExists bool
}

func readReviewText(src io.Reader) (string, error) {
	data, err := io.ReadAll(io.LimitReader(src, reviewFileLimit+1))
	if err != nil {
		return "", err
	}
	if len(data) > reviewFileLimit {
		return "", errReviewFileTooBig
	}
	if bytes.IndexByte(data, 0) >= 0 || !utf8.Valid(data) {
		return "", errReviewFileBinary
	}
	return string(data), nil
}

func openReviewRoot(repoPath, path string) (*os.Root, string, error) {
	if repoPath == "" || !filepath.IsLocal(path) || filepath.Clean(path) == "." {
		return nil, "", errors.New("无效的仓库文件路径")
	}
	root, err := os.OpenRoot(repoPath)
	return root, filepath.Clean(path), err
}

func readWorktreeText(root *os.Root, rel string) (text string, info os.FileInfo, err error) {
	info, err = root.Lstat(rel)
	if err != nil {
		return "", nil, err
	}
	if !info.Mode().IsRegular() {
		return "", nil, errors.New("仅支持读取普通文本文件")
	}
	if info.Size() > reviewFileLimit {
		return "", nil, errReviewFileTooBig
	}
	f, err := root.Open(rel)
	if err != nil {
		return "", nil, err
	}
	defer f.Close()
	opened, err := f.Stat()
	if err != nil {
		return "", nil, err
	}
	if !os.SameFile(info, opened) || !opened.Mode().IsRegular() {
		return "", nil, ErrReviewFileChanged
	}
	text, err = readReviewText(f)
	return text, info, err
}

func ReadReviewWorktree(repoPath, relPath string) (string, error) {
	root, rel, err := openReviewRoot(repoPath, relPath)
	if err != nil {
		return "", err
	}
	defer root.Close()
	text, _, err := readWorktreeText(root, rel)
	return text, err
}

// Review blobs use a byte limit before decoding or building an editor buffer.
// A missing blob represents an absent revision, as in file additions/deletions.
func ReadReviewBlob(ctx context.Context, repoPath, object string) (string, error) {
	return readReviewBlob(ctx, repoPath, object, true)
}

func readReviewBlob(ctx context.Context, repoPath, object string, allowMissing bool) (string, error) {
	ctx, cancel := context.WithTimeout(ctx, 4*time.Second)
	defer cancel()
	cmd := exec.CommandContext(ctx, "git", "cat-file", "blob", object)
	cmd.Dir = repoPath
	cmd.Env = append(os.Environ(), "LC_ALL=C", "GIT_OPTIONAL_LOCKS=0")
	var stderr bytes.Buffer
	cmd.Stderr = &stderr
	stdout, err := cmd.StdoutPipe()
	if err != nil {
		return "", err
	}
	if err := cmd.Start(); err != nil {
		return "", err
	}
	text, readErr := readReviewText(stdout)
	if readErr != nil {
		_ = cmd.Process.Kill()
	}
	waitErr := cmd.Wait()
	if readErr != nil {
		return "", readErr
	}
	if ctx.Err() != nil {
		return "", ctx.Err()
	}
	var exitErr *exec.ExitError
	if errors.As(waitErr, &exitErr) {
		message := strings.TrimSpace(stderr.String())
		if allowMissing && (strings.Contains(message, "not in '") || strings.Contains(message, "does not exist") || strings.Contains(message, "invalid object name") || strings.Contains(message, "not at stage 0")) {
			return "", nil
		}
		return "", fmt.Errorf("读取 Git 文件版本失败：%s (%w)", message, waitErr)
	}
	return text, waitErr
}

func ReadReviewChange(ctx context.Context, file ChangedFile, staged bool) (ReviewDiff, error) {
	rel := filepath.ToSlash(file.Path)
	readBlob := func(object string) (string, error) { return ReadReviewBlob(ctx, file.RepoPath, object) }
	var diff ReviewDiff
	var err error
	if staged {
		oldRel := rel
		if file.OrigPath != "" && (file.IndexStatus == 'R' || file.IndexStatus == 'C') {
			oldRel = filepath.ToSlash(file.OrigPath)
		}
		diff.Old, err = readBlob("HEAD:" + oldRel)
		if err == nil {
			diff.New, err = readBlob(":" + rel)
		}
		return diff, err
	}
	if !file.Untracked {
		base := ":" + rel
		if file.Conflicted {
			base = "HEAD:" + rel
		}
		if diff.Old, err = readBlob(base); err != nil {
			return diff, err
		}
	}
	diff.New, err = ReadReviewWorktree(file.RepoPath, file.Path)
	if errors.Is(err, os.ErrNotExist) {
		return diff, nil
	}
	diff.WorktreeExists = err == nil
	return diff, err
}

// SaveReviewText compares with the exact bytes read by the editor, including
// line endings. A sibling temporary file keeps partial writes out of the target.
func SaveReviewText(repoPath, relPath, content string, expected ReviewVersion) error {
	if len(content) > reviewFileLimit {
		return errReviewFileTooBig
	}
	root, rel, err := openReviewRoot(repoPath, relPath)
	if err != nil {
		return err
	}
	defer root.Close()
	check := func() (os.FileInfo, error) {
		text, info, err := readWorktreeText(root, rel)
		if err != nil {
			return nil, fmt.Errorf("%w: %v", ErrReviewFileChanged, err)
		}
		if ReviewTextVersion(text) != expected {
			return nil, ErrReviewFileChanged
		}
		return info, nil
	}
	info, err := check()
	if err != nil {
		return err
	}
	if info.Mode().Perm()&0o222 == 0 {
		return os.ErrPermission
	}
	temp := filepath.Join(filepath.Dir(rel), ".nemorous-save-"+rand.Text())
	f, err := root.OpenFile(temp, os.O_WRONLY|os.O_CREATE|os.O_EXCL, 0o600)
	if err != nil {
		return err
	}
	defer root.Remove(temp)
	_, writeErr := io.WriteString(f, content)
	modeErr := f.Chmod(info.Mode().Perm())
	if err := errors.Join(writeErr, modeErr, f.Close()); err != nil {
		return err
	}
	latest, err := check()
	if err != nil {
		return err
	}
	if !os.SameFile(info, latest) || info.Mode() != latest.Mode() {
		return ErrReviewFileChanged
	}
	return root.Rename(temp, rel)
}
