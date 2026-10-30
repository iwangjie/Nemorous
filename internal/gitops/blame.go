package gitops

import (
	"context"
	"strconv"
	"strings"
	"time"
)

// BlameLine is one line of a file's git blame history.
type BlameLine struct {
	Line       int
	Hash       string
	Author     string
	AuthorUnix int64
	Summary    string
}

// FileBlame returns the blame of every line of relPath in repoPath. It uses
// the working tree, so uncommitted lines come back as "Not Committed Yet",
// as git reports them.
func FileBlame(ctx context.Context, repoPath, relPath string) ([]BlameLine, error) {
	out, err := RunGit(ctx, repoPath, "blame", "--line-porcelain", "--", relPath)
	if err != nil {
		return nil, err
	}
	lines := strings.Split(out, "\n")
	byLine := make(map[int]BlameLine)
	var cur BlameLine
	for _, line := range lines {
		switch {
		case strings.HasPrefix(line, "\t"):
			if cur.Line > 0 {
				byLine[cur.Line] = cur
			}
		case strings.HasPrefix(line, "author "):
			cur.Author = strings.TrimPrefix(line, "author ")
		case strings.HasPrefix(line, "author-time "):
			if n, err := strconv.ParseInt(strings.TrimSpace(strings.TrimPrefix(line, "author-time ")), 10, 64); err == nil {
				cur.AuthorUnix = n
			}
		case strings.HasPrefix(line, "summary "):
			cur.Summary = strings.TrimPrefix(line, "summary ")
		default:
			fields := strings.Fields(line)
			if len(fields) >= 3 && isBlameHash(fields[0]) {
				if n, err := strconv.Atoi(fields[2]); err == nil {
					cur = BlameLine{Line: n, Hash: fields[0]}
				}
			}
		}
	}
	maxLine := 0
	for line := range byLine {
		if line > maxLine {
			maxLine = line
		}
	}
	result := make([]BlameLine, 0, len(byLine))
	for line := 1; line <= maxLine; line++ {
		if entry, ok := byLine[line]; ok {
			result = append(result, entry)
		}
	}
	return result, nil
}

func isBlameHash(s string) bool {
	if len(s) < 7 || len(s) > 40 {
		return false
	}
	for _, r := range s {
		if !strings.ContainsRune("0123456789abcdef", r) {
			return false
		}
	}
	return true
}

// BlameDisplay renders the default blame metadata: the author and date.
func BlameDisplay(entry BlameLine) string {
	date := time.Unix(entry.AuthorUnix, 0).Format("2006-01-02")
	if entry.AuthorUnix == 0 {
		date = ""
	}
	label := entry.Author
	if date != "" {
		label += " · " + date
	}
	return label
}
