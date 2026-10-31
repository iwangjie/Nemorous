package gitops

import (
	"context"
	"fmt"
	"os"
	"path/filepath"
	"runtime"
	"strings"
)

// RebaseCommit is one commit that interactive rebase can edit.
type RebaseCommit struct {
	Hash    string
	Subject string
}

// ListRebaseCommits returns commits from upstream (exclusive) to HEAD, oldest
// first, the order an interactive rebase todo uses.
func ListRebaseCommits(ctx context.Context, repoPath, upstream string) ([]RebaseCommit, error) {
	upstream = strings.TrimSpace(upstream)
	if upstream == "" {
		return nil, fmt.Errorf("目标分支不能为空")
	}
	if err := checkBranchOrRef(upstream); err != nil {
		return nil, err
	}
	out, err := RunGit(ctx, repoPath, "log", "--reverse", "--format=%H%x09%s", upstream+"..HEAD")
	if err != nil {
		return nil, err
	}
	var commits []RebaseCommit
	for _, line := range strings.Split(out, "\n") {
		line = strings.TrimSpace(line)
		if line == "" {
			continue
		}
		parts := strings.SplitN(line, "\t", 2)
		if len(parts) == 2 {
			commits = append(commits, RebaseCommit{Hash: parts[0], Subject: parts[1]})
		}
	}
	return commits, nil
}

// RebaseStep is one action in an interactive rebase plan.
type RebaseStep struct {
	Hash    string
	Action  string // pick, squash, reword, drop, edit
	Message string // replacement message for reword
}

func normalizeRebaseAction(a string) string {
	switch strings.ToLower(strings.TrimSpace(a)) {
	case "", "pick":
		return "pick"
	case "squash", "fixup":
		return "squash"
	case "reword":
		return "reword"
	case "drop":
		return "drop"
	case "edit":
		return "edit"
	default:
		return ""
	}
}

// InteractiveRebase applies a todo plan without opening an external editor.
// The plan is oldest first. At most one reword is supported per operation;
// rewording and squashing cannot be combined because both use Git's message
// editor with different expected contents.
func InteractiveRebase(ctx context.Context, repoPath, upstream string, steps []RebaseStep) (string, error) {
	upstream = strings.TrimSpace(upstream)
	if upstream == "" || len(steps) == 0 {
		return "", fmt.Errorf("变基计划和目标分支不能为空")
	}
	if err := checkBranchOrRef(upstream); err != nil {
		return "", err
	}
	var todo strings.Builder
	rewords, squashes := 0, 0
	rewordMessage := ""
	for i, step := range steps {
		if !hashPattern.MatchString(strings.TrimSpace(step.Hash)) {
			return "", fmt.Errorf("无效的提交 %q", step.Hash)
		}
		action := normalizeRebaseAction(step.Action)
		if action == "" {
			return "", fmt.Errorf("无效的变基动作 %q", step.Action)
		}
		if i == 0 && action == "squash" {
			return "", fmt.Errorf("第一个提交不能被 squash")
		}
		switch action {
		case "reword":
			rewords++
			rewordMessage = strings.TrimSpace(step.Message)
			if rewordMessage == "" {
				return "", fmt.Errorf("reword 需要新的提交信息")
			}
		case "squash":
			squashes++
		}
		todo.WriteString(action + " " + strings.TrimSpace(step.Hash) + "\n")
	}
	if rewords > 1 {
		return "", fmt.Errorf("一次变基只支持一个 reword")
	}
	if rewords > 0 && squashes > 0 {
		return "", fmt.Errorf("reword 与 squash 请分两次执行")
	}

	dir, err := os.MkdirTemp("", "nemorous-rebase-")
	if err != nil {
		return "", err
	}
	defer os.RemoveAll(dir)
	todoPath := filepath.Join(dir, "todo")
	if err := os.WriteFile(todoPath, []byte(todo.String()), 0o600); err != nil {
		return "", err
	}
	msgPath := filepath.Join(dir, "message")
	if rewords > 0 {
		if err := os.WriteFile(msgPath, []byte(rewordMessage+"\n"), 0o600); err != nil {
			return "", err
		}
	}
	seqPath := filepath.Join(dir, "sequence-editor")
	editPath := filepath.Join(dir, "message-editor")
	if runtime.GOOS == "windows" {
		seqPath += ".cmd"
		editPath += ".cmd"
		if err := os.WriteFile(seqPath, []byte("@echo off\r\ncopy /Y \""+todoPath+"\" \"%~1\" >nul\r\n"), 0o700); err != nil {
			return "", err
		}
		if rewords > 0 {
			err = os.WriteFile(editPath, []byte("@echo off\r\ncopy /Y \""+msgPath+"\" \"%~1\" >nul\r\n"), 0o700)
		} else {
			err = os.WriteFile(editPath, []byte("@echo off\r\nexit /b 0\r\n"), 0o700)
		}
		if err != nil {
			return "", err
		}
	} else {
		if err := os.WriteFile(seqPath, []byte("#!/bin/sh\ncat '"+strings.ReplaceAll(todoPath, "'", `'\''`)+"' > \"$1\"\n"), 0o700); err != nil {
			return "", err
		}
		body := "#!/bin/sh\nexit 0\n"
		if rewords > 0 {
			body = "#!/bin/sh\ncat '" + strings.ReplaceAll(msgPath, "'", `'\''`) + "' > \"$1\"\n"
		}
		if err := os.WriteFile(editPath, []byte(body), 0o700); err != nil {
			return "", err
		}
	}
	out, err := runGitEnv(ctx, repoPath, []string{
		"GIT_SEQUENCE_EDITOR=" + shellQuote(seqPath),
		"GIT_EDITOR=" + shellQuote(editPath),
	}, "rebase", "-i", upstream)
	if err != nil {
		return out, fmt.Errorf("%s", strings.TrimSpace(out))
	}
	return out, nil
}

func shellQuote(path string) string {
	if runtime.GOOS == "windows" {
		return `"` + path + `"`
	}
	return "'" + strings.ReplaceAll(path, "'", `'\''`) + "'"
}
