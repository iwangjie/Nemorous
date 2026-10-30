package main

import (
	"fmt"
	"path/filepath"
	"sort"
	"strings"

	"github.com/egoist/mygo/ui"
	"github.com/iwangjie/nemorous/internal/gitops"
)

// gitCommitTreeRowHeight keeps commit file trees compact: a commit with many
// changed files should not push the rest of the detail pane out of view.
const gitCommitTreeRowHeight float32 = 18

// gitCommitTreeNode is a directory hierarchy of one commit's changed files.
type gitCommitTreeNode struct {
	name    string
	relPath string
	dirs    []*gitCommitTreeNode
	files   []gitops.CommitFile
}

func buildGitCommitTree(files []gitops.CommitFile) *gitCommitTreeNode {
	root := &gitCommitTreeNode{}
	for _, file := range files {
		parts := strings.Split(filepath.ToSlash(file.Path), "/")
		cur := root
		curPath := ""
		for i := 0; i < len(parts)-1; i++ {
			seg := parts[i]
			if seg == "" {
				continue
			}
			if curPath == "" {
				curPath = seg
			} else {
				curPath += "/" + seg
			}
			var child *gitCommitTreeNode
			for _, candidate := range cur.dirs {
				if candidate.name == seg {
					child = candidate
					break
				}
			}
			if child == nil {
				child = &gitCommitTreeNode{name: seg, relPath: curPath}
				cur.dirs = append(cur.dirs, child)
			}
			cur = child
		}
		cur.files = append(cur.files, file)
	}
	finalizeGitCommitTree(root)
	return root
}

func finalizeGitCommitTree(node *gitCommitTreeNode) {
	sort.Slice(node.dirs, func(i, j int) bool { return node.dirs[i].name < node.dirs[j].name })
	sort.Slice(node.files, func(i, j int) bool {
		return filepath.Base(node.files[i].Path) < filepath.Base(node.files[j].Path)
	})
	for i, child := range node.dirs {
		finalizeGitCommitTree(child)
		for len(child.dirs) == 1 && len(child.files) == 0 {
			only := child.dirs[0]
			child.name += "/" + only.name
			child.relPath = only.relPath
			child.dirs = only.dirs
			child.files = only.files
		}
		node.dirs[i] = child
	}
}

func gitCommitTreeFileCount(node *gitCommitTreeNode) int {
	n := len(node.files)
	for _, dir := range node.dirs {
		n += gitCommitTreeFileCount(dir)
	}
	return n
}

func (a *App) renderGitCommitTree(c *ui.Context, p Palette, detail gitops.CommitDetail, node *gitCommitTreeNode, depth int) {
	indent := float32(6 + depth*12)
	for _, dir := range node.dirs {
		dirCopy := dir
		key := detail.Hash + "::" + dirCopy.relPath
		collapsed := a.gitDetailCollapsedDirs[key]
		kc := c.Key("git-dir-" + key)
		row := ui.Row(kc).Height(gitCommitTreeRowHeight).Padding(0, 6, 0, indent).Gap(5).Cursor(ui.CursorPointer).AlignItems(ui.Center)
		if row.Hovered() {
			row.Background(p.SurfaceHover)
		}
		chevKc := c.Key("git-dir-chev-" + key)
		chev := icoChevronRight
		if !collapsed {
			chev = icoChevronDown
		}
		row.Children(func() {
			chevEl := ui.Row(chevKc).Padding(1, 0).Cursor(ui.CursorPointer).Shrink(0)
			if chevEl.Clicked() {
				a.gitDetailCollapsedDirs[key] = !collapsed
			}
			chevEl.Children(func() { drawIcon(chevKc, chev, 9, p.TextMuted) })
			infoKc := c.Key("git-dir-info-" + key)
			info := ui.Row(infoKc).Grow(1).Gap(5).AlignItems(ui.Center).Cursor(ui.CursorPointer)
			if info.Clicked() {
				a.gitDetailCollapsedDirs[key] = !collapsed
			}
			info.Children(func() {
				drawIcon(infoKc, icoFolder, 11, p.TextMuted)
				ui.Text(infoKc, dirCopy.name).FontSize(10.5).TextColor(p.TextSecondary).MaxLines(1).Grow(1)
				ui.Text(infoKc, fmt.Sprintf("%d", gitCommitTreeFileCount(dirCopy))).FontSize(9.5).TextColor(p.TextMuted)
			})
		})
		if !collapsed {
			a.renderGitCommitTree(c, p, detail, dirCopy, depth+1)
		}
	}
	for _, file := range node.files {
		fileCopy := file
		kc := c.Key("git-detail-file-" + detail.Hash + "::" + fileCopy.Path)
		row := ui.Row(kc).Height(gitCommitTreeRowHeight).Padding(0, 6, 0, indent+8).Gap(6).Cursor(ui.CursorPointer).AlignItems(ui.Center)
		if row.Hovered() {
			row.Background(p.SurfaceHover)
		}
		if row.Clicked() {
			a.TriggerGitFileDiff(detail, fileCopy)
		}
		row.Children(func() {
			col, bg := p.Green, p.GreenSoft
			if fileCopy.Status == "D" {
				col, bg = p.Danger, p.DangerSoft
			} else if fileCopy.Status != "A" {
				col, bg = p.Amber, p.AmberSoft
			}
			badgeBox(kc, fileCopy.Status, col, bg, ui.Color{}, 9, true)
			ui.Text(kc, filepath.Base(fileCopy.Path)).FontSize(10.5).TextColor(p.TextPrimary).MaxLines(1).Grow(1)
			if fileCopy.Added > 0 || fileCopy.Deleted > 0 {
				ui.Text(kc, fmt.Sprintf("+%d -%d", fileCopy.Added, fileCopy.Deleted)).FontSize(9).TextColor(p.TextMuted).Shrink(0)
			}
		})
	}
}
