// Package github talks to GitHub through the gh CLI, so Nemorous never
// stores a token of its own.
package github

import (
	"bytes"
	"context"
	"encoding/json"
	"fmt"
	"os/exec"
	"regexp"
	"strings"
	"time"

	"github.com/iwangjie/nemorous/internal/gitops"
)

// Repo is a GitHub repository.
type Repo struct {
	Owner string
	Name  string
}

func (r Repo) String() string { return r.Owner + "/" + r.Name }

// PR is one pull request.
type PR struct {
	Number         int    `json:"number"`
	Title          string `json:"title"`
	State          string `json:"state"`
	URL            string `json:"url"`
	HeadRefName    string `json:"headRefName"`
	BaseRefName    string `json:"baseRefName"`
	IsDraft        bool   `json:"isDraft"`
	UpdatedAt      string `json:"updatedAt"`
	ReviewDecision string `json:"reviewDecision"`
	Author         string `json:"-"`
}

// Check is one CI check.
type Check struct {
	Name   string `json:"name"`
	State  string `json:"state"`
	Bucket string `json:"bucket"`
	Link   string `json:"link"`
}

// Comment is one issue comment or review body.
type Comment struct {
	Author    string `json:"author"`
	Body      string `json:"body"`
	CreatedAt string `json:"createdAt"`
	Path      string `json:"path,omitempty"`
	Line      int    `json:"line,omitempty"`
}

// Details is one PR with its body, checks and comments.
type Details struct {
	PR
	Body     string    `json:"body"`
	Checks   []Check   `json:"checks"`
	Comments []Comment `json:"comments"`
}

// Available reports whether gh is installed.
func Available() bool {
	_, err := exec.LookPath("gh")
	return err == nil
}

// Detect reads the origin remote and parses a GitHub owner/repo.
func Detect(ctx context.Context, repoPath string) (Repo, error) {
	out, err := gitops.RunGit(ctx, repoPath, "remote", "get-url", "origin")
	if err != nil {
		return Repo{}, fmt.Errorf("读取 origin 失败：%w", err)
	}
	return ParseRemote(out)
}

var (
	httpsRemote = regexp.MustCompile(`^(?:https?|ssh)://(?:[^@/]+@)?github\.com/([^/]+)/(.+?)(?:\.git)?/?$`)
	gitRemote   = regexp.MustCompile(`^(?:[^@/]+@)?github\.com:([^/]+)/(.+?)(?:\.git)?$`)
)

// ParseRemote parses GitHub remote URLs in HTTPS, SSH and scp-like forms.
func ParseRemote(remote string) (Repo, error) {
	remote = strings.TrimSpace(remote)
	for _, re := range []*regexp.Regexp{httpsRemote, gitRemote} {
		if m := re.FindStringSubmatch(remote); m != nil {
			repo := Repo{Owner: m[1], Name: strings.TrimSuffix(m[2], ".git")}
			if repo.Owner == "" || repo.Name == "" {
				break
			}
			return repo, nil
		}
	}
	return Repo{}, fmt.Errorf("无法解析 GitHub 仓库地址 %q", remote)
}

func runGH(ctx context.Context, repoPath string, args ...string) (string, error) {
	cctx, cancel := context.WithTimeout(ctx, 45*time.Second)
	defer cancel()
	cmd := exec.CommandContext(cctx, "gh", args...)
	cmd.Dir = repoPath
	cmd.Env = append(cmd.Environ(), "GH_PROMPT_DISABLED=1", "NO_COLOR=1", "CLICOLOR=0")
	var stdout, stderr bytes.Buffer
	cmd.Stdout = &stdout
	cmd.Stderr = &stderr
	err := cmd.Run()
	if err != nil {
		msg := strings.TrimSpace(stderr.String())
		if msg == "" {
			msg = strings.TrimSpace(stdout.String())
		}
		if msg == "" {
			msg = err.Error()
		}
		return stdout.String(), fmt.Errorf("%s", msg)
	}
	return stdout.String(), nil
}

// ListPRs lists pull requests, newest first.
func ListPRs(ctx context.Context, repoPath, state string) ([]PR, error) {
	if state == "" {
		state = "open"
	}
	out, err := runGH(ctx, repoPath, "pr", "list", "--state", state, "--limit", "30",
		"--json", "number,title,state,url,headRefName,baseRefName,isDraft,updatedAt,reviewDecision,author")
	if err != nil {
		return nil, err
	}
	var raw []struct {
		PR
		Author struct {
			Login string `json:"login"`
		} `json:"author"`
	}
	if err := json.Unmarshal([]byte(out), &raw); err != nil {
		return nil, fmt.Errorf("解析 PR 列表：%w", err)
	}
	prs := make([]PR, 0, len(raw))
	for _, r := range raw {
		pr := r.PR
		pr.Author = r.Author.Login
		prs = append(prs, pr)
	}
	return prs, nil
}

// ViewPR reads a PR's body, checks and comments.
func ViewPR(ctx context.Context, repoPath string, number int) (Details, error) {
	out, err := runGH(ctx, repoPath, "pr", "view", fmt.Sprint(number),
		"--json", "number,title,state,url,headRefName,baseRefName,isDraft,body,author,updatedAt,reviewDecision,statusCheckRollup,comments,reviews")
	if err != nil {
		return Details{}, err
	}
	var raw struct {
		PR
		Author struct {
			Login string `json:"login"`
		} `json:"author"`
		Body  string `json:"body"`
		Check []struct {
			Name       string `json:"name"`
			Context    string `json:"context"`
			State      string `json:"state"`
			Status     string `json:"status"`
			Conclusion string `json:"conclusion"`
			DetailsURL string `json:"detailsUrl"`
			TargetURL  string `json:"targetUrl"`
		} `json:"statusCheckRollup"`
		Comments []struct {
			Author struct {
				Login string `json:"login"`
			} `json:"author"`
			Body      string `json:"body"`
			CreatedAt string `json:"createdAt"`
		} `json:"comments"`
		Reviews []struct {
			Author struct {
				Login string `json:"login"`
			} `json:"author"`
			Body      string `json:"body"`
			CreatedAt string `json:"createdAt"`
		} `json:"reviews"`
	}
	if err := json.Unmarshal([]byte(out), &raw); err != nil {
		return Details{}, fmt.Errorf("解析 PR 详情：%w", err)
	}
	details := Details{PR: raw.PR, Body: raw.Body}
	details.Author = raw.Author.Login
	for _, c := range raw.Check {
		name := c.Name
		if name == "" {
			name = c.Context
		}
		state := c.Conclusion
		if state == "" {
			state = c.State
		}
		if state == "" {
			state = c.Status
		}
		link := c.DetailsURL
		if link == "" {
			link = c.TargetURL
		}
		details.Checks = append(details.Checks, Check{Name: name, State: state, Bucket: CheckBucket(state), Link: link})
	}
	for _, c := range raw.Comments {
		details.Comments = append(details.Comments, Comment{Author: c.Author.Login, Body: c.Body, CreatedAt: c.CreatedAt})
	}
	for _, c := range raw.Reviews {
		if strings.TrimSpace(c.Body) == "" {
			continue
		}
		details.Comments = append(details.Comments, Comment{Author: c.Author.Login, Body: c.Body, CreatedAt: c.CreatedAt})
	}
	return details, nil
}

// CheckBucket normalizes a check state to success, failure or pending.
func CheckBucket(state string) string {
	switch strings.ToUpper(state) {
	case "SUCCESS", "PASS", "PASSED", "NEUTRAL", "SKIPPED":
		return "success"
	case "FAILURE", "FAIL", "FAILED", "ERROR", "TIMED_OUT", "ACTION_REQUIRED", "CANCELLED":
		return "failure"
	default:
		return "pending"
	}
}

// InlineComments reads review comments anchored to files.
func InlineComments(ctx context.Context, repoPath string, repo Repo, number int) ([]Comment, error) {
	out, err := runGH(ctx, repoPath, "api", "--paginate",
		fmt.Sprintf("repos/%s/%s/pulls/%d/comments", repo.Owner, repo.Name, number))
	if err != nil {
		return nil, err
	}
	var raw []struct {
		User struct {
			Login string `json:"login"`
		} `json:"user"`
		Body      string `json:"body"`
		CreatedAt string `json:"created_at"`
		Path      string `json:"path"`
		Line      int    `json:"line"`
	}
	if err := json.Unmarshal([]byte(out), &raw); err != nil {
		return nil, fmt.Errorf("解析行内评论：%w", err)
	}
	comments := make([]Comment, 0, len(raw))
	for _, c := range raw {
		comments = append(comments, Comment{Author: c.User.Login, Body: c.Body, CreatedAt: c.CreatedAt, Path: c.Path, Line: c.Line})
	}
	return comments, nil
}

// AddComment adds a review comment to a PR.
func AddComment(ctx context.Context, repoPath string, number int, body string) error {
	body = strings.TrimSpace(body)
	if body == "" {
		return fmt.Errorf("评论不能为空")
	}
	_, err := runGH(ctx, repoPath, "pr", "comment", fmt.Sprint(number), "--body", body)
	return err
}

// CreatePR opens a PR for the current branch and returns its URL.
func CreatePR(ctx context.Context, repoPath, title, body, base string, draft bool) (string, error) {
	if strings.TrimSpace(title) == "" {
		return "", fmt.Errorf("PR 标题不能为空")
	}
	args := []string{"pr", "create", "--title", title, "--body", body}
	if strings.TrimSpace(base) != "" {
		args = append(args, "--base", base)
	}
	if draft {
		args = append(args, "--draft")
	}
	out, err := runGH(ctx, repoPath, args...)
	if err != nil {
		return "", err
	}
	return strings.TrimSpace(out), nil
}
