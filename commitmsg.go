package main

import (
	"fmt"
	"strings"

	"github.com/iwangjie/nemorous/internal/gitops"
)

// commitMessageSet holds the text shown in the commit input and the message used
// for each repository's commit. A multi-repo commit gets one message per repo so
// file lists never mention files another repository does not have.
type commitMessageSet struct {
	root   string
	byRepo map[string]string
}

func kindLabel(kind string) string {
	switch kind {
	case "A", "?":
		return "新增"
	case "D":
		return "删除"
	case "R":
		return "重命名"
	case "U":
		return "冲突"
	default:
		return "修改"
	}
}

type repoCommitFiles struct {
	repoName string
	files    []gitops.ChangedFile
}

// buildCommitMessages summarizes the selected files, grouped by repository.
func buildCommitMessages(files []gitops.ChangedFile) commitMessageSet {
	if len(files) == 0 {
		return commitMessageSet{}
	}
	groupOrder := make([]string, 0, 4)
	groups := make(map[string]*repoCommitFiles)
	for _, f := range files {
		g, ok := groups[f.RepoID]
		if !ok {
			g = &repoCommitFiles{repoName: f.RepoName}
			groups[f.RepoID] = g
			groupOrder = append(groupOrder, f.RepoID)
		}
		g.files = append(g.files, f)
	}

	set := commitMessageSet{byRepo: make(map[string]string, len(groups))}
	repoNames := make([]string, 0, len(groupOrder))
	kindCount := map[string]int{}
	for _, repoID := range groupOrder {
		g := groups[repoID]
		repoNames = append(repoNames, g.repoName)
		for _, f := range g.files {
			kindCount[kindLabel(f.Kind)]++
		}
		set.byRepo[repoID] = repoMessage(g)
	}

	if len(groupOrder) == 1 {
		g := groups[groupOrder[0]]
		set.root = repoMessage(g)
		return set
	}

	head := repoNames
	if len(head) > 3 {
		head = append(head[:3], "等")
	}
	title := fmt.Sprintf("更新 %s（%d 个仓库，%d 个文件）", strings.Join(head, "、"), len(groupOrder), len(files))
	var b strings.Builder
	b.WriteString(title)
	b.WriteString("\n\n")
	b.WriteString(summaryLine(kindCount))
	for _, repoID := range groupOrder {
		g := groups[repoID]
		b.WriteString(fmt.Sprintf("\n- %s：%d 个文件", g.repoName, len(g.files)))
	}
	set.root = b.String()
	return set
}

func repoMessage(g *repoCommitFiles) string {
	title := fmt.Sprintf("更新 %s 的 %d 个文件", g.repoName, len(g.files))
	if len(g.files) == 1 {
		title = fmt.Sprintf("更新 %s：%s", g.repoName, g.files[0].Path)
	}
	kindCount := map[string]int{}
	for _, f := range g.files {
		kindCount[kindLabel(f.Kind)]++
	}
	var b strings.Builder
	b.WriteString(title)
	if len(g.files) > 1 {
		b.WriteString("\n\n")
		b.WriteString(summaryLine(kindCount))
		limit := min(len(g.files), 8)
		for _, f := range g.files[:limit] {
			b.WriteString(fmt.Sprintf("\n- %s（%s，+%d -%d）", f.Path, kindLabel(f.Kind), f.AddedLines, f.DeletedLines))
		}
		if len(g.files) > limit {
			b.WriteString(fmt.Sprintf("\n- …等 %d 个文件", len(g.files)))
		}
	}
	return b.String()
}

func summaryLine(kindCount map[string]int) string {
	order := []string{"新增", "修改", "删除", "重命名", "冲突"}
	parts := make([]string, 0, len(order))
	for _, kind := range order {
		if n := kindCount[kind]; n > 0 {
			parts = append(parts, fmt.Sprintf("%s %d", kind, n))
		}
	}
	return strings.Join(parts, " · ")
}
