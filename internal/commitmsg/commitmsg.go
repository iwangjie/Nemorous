// Package commitmsg asks the configured AI CLI for a commit message grounded
// in the exact patch the user is about to commit.
package commitmsg

import (
	"bytes"
	"context"
	"fmt"
	"os/exec"
	"strings"
	"time"
	"unicode/utf8"
)

// Change is one file included in the pending commit.
type Change struct {
	Repo   string
	Path   string
	Patch  string
	Status string
}

// Request contains only the context the model needs; no credentials or
// unrelated workspace files are sent.
type Request struct {
	Branch   string
	Changes  []Change
	Subjects []string
}

// Generate runs the selected AI CLI and returns a cleaned commit message.
func Generate(ctx context.Context, agentName, agentBin string, req Request) (string, error) {
	agentBin = strings.TrimSpace(agentBin)
	if agentBin == "" {
		return "", fmt.Errorf("未检测到可用的 AI 命令行工具 (%s)，请在设置中配置 omp / pi / claude / codex", agentName)
	}
	prompt := buildPrompt(req)
	ctx, cancel := context.WithTimeout(ctx, 2*time.Minute)
	defer cancel()

	var args []string
	switch agentName {
	case "claude":
		args = []string{"-p", prompt}
	case "codex":
		args = []string{"exec", prompt}
	case "pi":
		args = []string{"-p", "--no-session", prompt}
	default: // omp or auto-resolved binary
		args = []string{"-p", "--no-session", prompt}
	}
	cmd := exec.CommandContext(ctx, agentBin, args...)
	var out, stderr bytes.Buffer
	cmd.Stdout = &cappedWriter{w: &out, remaining: 1 << 20}
	cmd.Stderr = &cappedWriter{w: &stderr, remaining: 64 << 10}
	if err := cmd.Run(); err != nil {
		msg := strings.TrimSpace(stderr.String())
		if msg == "" {
			msg = strings.TrimSpace(out.String())
		}
		if msg == "" {
			msg = err.Error()
		}
		return "", fmt.Errorf("AI 生成失败：%s", msg)
	}
	msg := Clean(out.String())
	if msg == "" {
		return "", fmt.Errorf("AI 返回了空提交说明")
	}
	return msg, nil
}

func buildPrompt(req Request) string {
	var b strings.Builder
	b.WriteString("Write a Git commit message for the staged changes below.\n")
	b.WriteString("Return ONLY the commit message, without code fences, quotes, or explanations.\n")
	b.WriteString("Use an imperative, concise subject line (<= 72 characters), then an optional blank line and short body bullets when useful.\n")
	b.WriteString("Match the repository's existing language and style.\n")
	if strings.TrimSpace(req.Branch) != "" {
		b.WriteString("Branch: " + strings.TrimSpace(req.Branch) + "\n")
	}
	if len(req.Subjects) > 0 {
		b.WriteString("\nRecent commit subjects for style:\n")
		for _, s := range req.Subjects {
			if s = strings.TrimSpace(s); s != "" {
				b.WriteString("- " + s + "\n")
			}
		}
	}
	b.WriteString("\nChanged files and patches:\n")
	remaining := 24 << 10
	for _, ch := range req.Changes {
		header := fmt.Sprintf("\n### %s: %s (%s)\n", ch.Repo, ch.Path, ch.Status)
		if len(header) >= remaining {
			break
		}
		b.WriteString(header)
		remaining -= len(header)
		patch := strings.TrimSpace(ch.Patch)
		if patch == "" {
			patch = "(content unavailable)"
		}
		if len(patch) > remaining {
			patch = truncateUTF8(patch, remaining)
		}
		b.WriteString(patch)
		b.WriteByte('\n')
		remaining -= len(patch) + 1
		if remaining <= 0 {
			b.WriteString("\n[diff truncated]\n")
			break
		}
	}
	return b.String()
}

func truncateUTF8(s string, max int) string {
	if len(s) <= max {
		return s
	}
	cut := max
	for cut > 0 && !utf8.RuneStart(s[cut]) {
		cut--
	}
	return s[:cut] + "\n[diff truncated]"
}

// Clean strips common agent wrappers and ANSI escapes, leaving a commit
// message that can be written to Git verbatim.
func Clean(raw string) string {
	s := strings.TrimSpace(raw)
	// Remove ANSI CSI sequences.
	var b strings.Builder
	for i := 0; i < len(s); {
		if s[i] == 0x1b && i+1 < len(s) && s[i+1] == '[' {
			i += 2
			for i < len(s) && (s[i] < 0x40 || s[i] > 0x7e) {
				i++
			}
			if i < len(s) {
				i++
			}
			continue
		}
		b.WriteByte(s[i])
		i++
	}
	s = strings.TrimSpace(b.String())
	// Drop prose wrappers before and after a fenced reply.
	lines := strings.Split(s, "\n")
	dropWrapper := func(line string) bool {
		line = strings.TrimSpace(line)
		lower := strings.ToLower(line)
		return line == "" || strings.HasPrefix(lower, "here is") || strings.HasPrefix(lower, "here's") ||
			strings.HasPrefix(lower, "commit message:") || strings.HasPrefix(lower, "suggested commit message:")
	}
	for len(lines) > 0 && dropWrapper(lines[0]) {
		lines = lines[1:]
	}
	s = strings.Join(lines, "\n")
	// Strip a surrounding Markdown code fence, with or without a language tag.
	if strings.HasPrefix(strings.TrimSpace(s), "```") {
		if i := strings.IndexByte(s, '\n'); i >= 0 {
			s = s[i+1:]
		}
		if i := strings.LastIndex(s, "```"); i >= 0 {
			s = s[:i]
		}
	}
	lines = strings.Split(strings.TrimSpace(s), "\n")
	for len(lines) > 0 && dropWrapper(lines[0]) {
		lines = lines[1:]
	}
	for len(lines) > 0 {
		last := strings.TrimSpace(lines[len(lines)-1])
		if last == "" || strings.HasPrefix(strings.ToLower(last), "let me know") || strings.HasPrefix(strings.ToLower(last), "this commit") {
			lines = lines[:len(lines)-1]
			continue
		}
		break
	}
	if len(lines) > 0 {
		first := strings.TrimSpace(lines[0])
		if strings.HasPrefix(first, "`") && strings.HasSuffix(first, "`") && len(first) > 2 {
			lines[0] = strings.Trim(first, "`")
		}
	}
	s = strings.TrimSpace(strings.Join(lines, "\n"))
	if len(s) > 8<<10 {
		s = s[:8<<10]
	}
	return s
}

type cappedWriter struct {
	w         *bytes.Buffer
	remaining int
}

func (c *cappedWriter) Write(p []byte) (int, error) {
	n := len(p)
	if c.remaining <= 0 {
		return n, nil
	}
	if len(p) > c.remaining {
		p = p[:c.remaining]
	}
	_, _ = c.w.Write(p)
	c.remaining -= len(p)
	return n, nil
}
