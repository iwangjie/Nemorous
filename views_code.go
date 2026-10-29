package main

import (
	"fmt"
	"path/filepath"
	"sort"
	"strings"

	"github.com/egoist/mygo/ui"
	"github.com/egoist/mygo/ui/editor"
	"github.com/iwangjie/nemorous/internal/gitops"
)

type codeTreeDir struct {
	name    string
	relPath string
	subMap  map[string]*codeTreeDir
	subdirs []*codeTreeDir
	files   []gitops.WorkspaceFile
}

func buildRepoCodeTree(files []gitops.WorkspaceFile) *codeTreeDir {
	root := &codeTreeDir{subMap: make(map[string]*codeTreeDir)}
	for _, wf := range files {
		rel := filepath.ToSlash(wf.RelPath)
		parts := strings.Split(rel, "/")
		cur := root
		curPath := ""
		for i := 0; i < len(parts)-1; i++ {
			seg := parts[i]
			if curPath == "" {
				curPath = seg
			} else {
				curPath = curPath + "/" + seg
			}
			child, ok := cur.subMap[seg]
			if !ok {
				child = &codeTreeDir{
					name:    seg,
					relPath: curPath,
					subMap:  make(map[string]*codeTreeDir),
				}
				cur.subMap[seg] = child
				cur.subdirs = append(cur.subdirs, child)
			}
			cur = child
		}
		cur.files = append(cur.files, wf)
	}
	finalizeCodeTree(root)
	return root
}

func finalizeCodeTree(node *codeTreeDir) {
	sort.Slice(node.subdirs, func(i, j int) bool {
		return node.subdirs[i].name < node.subdirs[j].name
	})
	sort.Slice(node.files, func(i, j int) bool {
		return filepath.Base(node.files[i].RelPath) < filepath.Base(node.files[j].RelPath)
	})
	for i, child := range node.subdirs {
		finalizeCodeTree(child)
		// Compact single-child empty middle directories (like IDEA's Compact Middle Packages)
		for len(child.subdirs) == 1 && len(child.files) == 0 {
			only := child.subdirs[0]
			child.name = child.name + "/" + only.name
			child.relPath = only.relPath
			child.subdirs = only.subdirs
			child.files = only.files
		}
		node.subdirs[i] = child
	}
}

func (a *App) buildCodeView(c *ui.Context, p Palette) {
	ui.Split(c, &a.codeSplit,
		func() { a.buildCodeLeftPane(c, p) },
		func() { a.buildCodeRightPane(c, p) },
	).Grow(1).Fill()
}

// codeRepoGroup is one repository's slice of the code explorer tree.
type codeRepoGroup struct {
	repoID   string
	repoName string
	monogram string
	colorIdx int
	files    []gitops.WorkspaceFile
}

// codeRepoGroups filters the workspace file list by repository and query. The
// result is cached per query so typing does not scan every file each frame.
func (a *App) codeRepoGroups(query string) []codeRepoGroup {
	key := a.repoFilterID + "|" + query
	if a.codeGroupsKey == key {
		return a.codeGroups
	}
	var groups []codeRepoGroup
	groupIdx := make(map[string]int)

	for _, wf := range a.workspaceFiles {
		if a.repoFilterID != "" && wf.RepoID != a.repoFilterID {
			continue
		}
		if query != "" {
			hay := strings.ToLower(wf.RepoName + "/" + wf.RelPath)
			if !strings.Contains(hay, query) {
				continue
			}
		}
		idx, ok := groupIdx[wf.RepoID]
		if !ok {
			idx = len(groups)
			groupIdx[wf.RepoID] = idx
			groups = append(groups, codeRepoGroup{
				repoID:   wf.RepoID,
				repoName: wf.RepoName,
				monogram: wf.Monogram,
				colorIdx: wf.ColorIdx,
			})
		}
		if len(groups[idx].files) < 200 {
			groups[idx].files = append(groups[idx].files, wf)
		}
	}
	a.codeGroupsKey = key
	a.codeGroups = groups
	return groups
}

func (a *App) buildCodeLeftPane(c *ui.Context, p Palette) {
	query := strings.ToLower(strings.TrimSpace(a.codeFilterQuery))
	groups := a.codeRepoGroups(query)

	ui.Column(c).Fill().Padding(12).Gap(8).Background(p.RailBg).Children(func() {
		ui.Row(c).Gap(6).AlignItems(ui.Center).Children(func() {
			ui.SearchField(c.Key("code-search"), &a.codeFilterQuery).Bind(&a.codeFilterHandle).Placeholder("搜索文件名或路径...").Grow(1)
			if len(a.expandedCodeDirs) > 0 {
				miniBtn(c, p, "收起目录", func() {
					a.expandedCodeDirs = make(map[string]bool)
				})
			}
		})

		// Quick Repository Filter Pills
		ui.Row(c).Wrap().Gap(4).Children(func() {
			pill(c.Key("cflt-all"), p, "全部", a.repoFilterID == "", func() {
				a.repoFilterID = ""
			})
			for _, st := range a.statuses {
				stCopy := st
				active := a.repoFilterID == st.Repo.ID
				pill(c.Key("cflt-"+st.Repo.ID), p, st.Repo.Name, active, func() {
					if a.repoFilterID == stCopy.Repo.ID {
						a.repoFilterID = ""
					} else {
						a.repoFilterID = stCopy.Repo.ID
					}
				})
			}
		})

		// Hierarchical Repository & Directory File Tree
		ui.Scroll(c).Grow(1).Gap(6).Children(func() {
			for _, grp := range groups {
				grpCopy := grp
				collapsed := a.collapsedCodeRepos[grpCopy.repoID] && query == ""
				badge := repoBadgeColors(grpCopy.colorIdx, p.IsDark)
				treeKey := grpCopy.repoID + "|" + query
				treeRoot, cached := a.codeTreeCache[treeKey]
				if !cached {
					treeRoot = buildRepoCodeTree(grpCopy.files)
					a.codeTreeCache[treeKey] = treeRoot
				}

				gkc := c.Key("cgrp-" + grpCopy.repoID)
				ui.Column(gkc).Radius(8).Background(p.Surface).Border(1, p.Border).Children(func() {
					// Repo Root Header
					hdr := ui.Row(gkc).Padding(7, 10).Gap(6).Cursor(ui.CursorPointer).AlignItems(ui.Center)
					if !collapsed {
						hdr.BorderWidth(0, 0, 1, 0).BorderColor(p.BorderSubtle)
					}
					if hdr.Hovered() {
						hdr.Background(p.RailBg)
					}
					if hdr.Clicked() {
						a.collapsedCodeRepos[grpCopy.repoID] = !a.collapsedCodeRepos[grpCopy.repoID]
					}
					hdr.Children(func() {
						chev := icoChevronDown
						if collapsed {
							chev = icoChevronRight
						}
						drawIcon(gkc, chev, 11, p.TextMuted)
						ui.Box(gkc).Size(17, 17).Radius(4).Background(badge.Bg).Center().Children(func() {
							ui.Text(gkc, grpCopy.monogram).FontSize(9).FontWeight(weightBold).TextColor(badge.Text)
						})
						ui.Text(gkc, grpCopy.repoName).FontSize(12).FontWeight(weightSemibold).TextColor(p.TextPrimary).Grow(1)
						ui.Text(gkc, fmt.Sprintf("%d", len(grpCopy.files))).FontSize(11).TextColor(p.TextMuted)
					})

					if !collapsed {
						a.renderCodeTreeNode(gkc, p, grpCopy.repoID, treeRoot, 0, query != "")
					}
				})
			}
		})
	})
}

func (a *App) renderCodeTreeNode(c *ui.Context, p Palette, repoID string, node *codeTreeDir, depth int, forceExpand bool) {
	indent := float32(10 + depth*14)

	// 1. Render subdirectories first
	for _, sub := range node.subdirs {
		subCopy := sub
		dirKey := repoID + "::" + subCopy.relPath
		expanded := forceExpand || a.expandedCodeDirs[dirKey]

		dkc := c.Key("dir-" + dirKey)
		dRow := ui.Row(dkc).Padding(5, 10, 5, indent).Gap(6).Background(p.Surface).BorderWidth(0, 0, 1, 0).BorderColor(p.BorderSubtle).Cursor(ui.CursorPointer).AlignItems(ui.Center)
		if dRow.Hovered() {
			dRow.Background(p.RailBg)
		}
		if dRow.Clicked() {
			a.expandedCodeDirs[dirKey] = !a.expandedCodeDirs[dirKey]
		}
		dRow.Children(func() {
			chev := icoChevronRight
			if expanded {
				chev = icoChevronDown
			}
			drawIcon(dkc, chev, 11, p.TextMuted)
			drawIcon(dkc, icoFolder, 13, p.Accent)
			ui.Text(dkc, subCopy.name).FontSize(12).FontWeight(weightMedium).TextColor(p.TextPrimary).MaxLines(1).Grow(1)
		})

		if expanded {
			a.renderCodeTreeNode(dkc, p, repoID, subCopy, depth+1, forceExpand)
		}
	}

	// 2. Render files in this directory
	for _, wf := range node.files {
		wfCopy := wf
		selected := a.selectedCodeFile != nil && a.selectedCodeFile.RepoID == wf.RepoID && a.selectedCodeFile.RelPath == wf.RelPath
		rowBg := p.Surface
		if selected {
			rowBg = p.ActiveRowBg
		}
		base := filepath.Base(wf.RelPath)

		fkc := c.Key("wf-" + wf.RepoID + "::" + wf.RelPath)
		fRow := ui.Row(fkc).Padding(5, 10, 5, indent+14).Gap(6).Background(rowBg).BorderWidth(0, 0, 1, 0).BorderColor(p.BorderSubtle).Cursor(ui.CursorPointer).AlignItems(ui.Center)
		if fRow.Hovered() && !selected {
			fRow.Background(p.RailBg)
		}
		if fRow.Clicked() {
			a.selectCodeFile(wfCopy)
		}
		fRow.Children(func() {
			drawIcon(fkc, icoFile, 13, p.TextMuted)
			ui.Text(fkc, base).FontSize(12).FontWeight(weightMedium).TextColor(p.TextPrimary).MaxLines(1).Grow(1)
		})
	}
}

func (a *App) buildCodeRightPane(c *ui.Context, p Palette) {
	ui.Column(c).Fill().Background(p.Surface).Children(func() {
		if a.selectedCodeFile != nil {
			wf := *a.selectedCodeFile
			isModified := a.codeDirty || (a.codeEditorState != nil && a.codeEditorState.Buffer != nil && a.codeEditorState.Buffer.Modified())

			ui.Row(c).Height(42).FillWidth().Padding(0, 14).Background(p.Surface).BorderWidth(0, 0, 1, 0).BorderColor(p.Border).Justify(ui.SpaceBetween).AlignItems(ui.Center).Children(func() {
				ui.Row(c).Gap(8).AlignItems(ui.Center).Grow(1).Children(func() {
					badgeBox(c, wf.RepoName, p.Accent, p.AccentSoft, ui.Color{}, 11, true)
					ui.Text(c, wf.RelPath).FontSize(12.5).FontWeight(weightSemibold).TextColor(p.TextPrimary).SingleLine()
					if isModified {
						badgeBox(c, "未保存", p.Amber, p.AmberSoft, ui.Color{}, 10.5, true)
					} else if !a.codeMicroEdit {
						badgeBox(c, "只读", p.TextMuted, p.Surface2, ui.Color{}, 10.5, false)
					}
				})

				ui.Row(c).Gap(6).AlignItems(ui.Center).Children(func() {
					miniBtn(c, p, "查找 (⌘F)", func() {
						if a.codeEditorState != nil {
							a.codeEditorState.Run(editor.Find)
						}
					})
					miniBtn(c, p, "跳行", func() {
						if a.codeEditorState != nil {
							a.codeEditorState.Run(editor.GoToLine)
						}
					})

					editLabel := "编辑"
					if a.codeMicroEdit {
						editLabel = "退出编辑"
					}
					iconPill(c, p, icoEdit, editLabel, a.codeMicroEdit, func() {
						a.codeMicroEdit = !a.codeMicroEdit
						if a.codeEditorState != nil {
							a.codeEditorState.ReadOnly = !a.codeMicroEdit
						}
					})

					if a.codeMicroEdit || isModified {
						pill(c, p, "保存 (⌘S)", true, func() {
							a.saveCodeMicroEdit()
						})
					}

					ghostBtn(c, p, icoMore, "", a.showCodeOptions, func() {
						a.showCodeOptions = !a.showCodeOptions
					})
				})
			})

			if a.showCodeOptions {
				ui.Row(c).Padding(6, 14).Gap(10).Background(p.RailBg).BorderWidth(0, 0, 1, 0).BorderColor(p.Border).AlignItems(ui.Center).Children(func() {
					ui.Text(c, "显示选项：").FontSize(11.5).TextColor(p.TextMuted)
					wrapLabel := "自动换行：关"
					if a.cfg.EditorSoftWrap {
						wrapLabel = "自动换行：开"
					}
					miniBtn(c, p, wrapLabel, func() {
						a.cfg.EditorSoftWrap = !a.cfg.EditorSoftWrap
						if a.codeEditorState != nil {
							a.codeEditorState.SoftWrap = a.cfg.EditorSoftWrap
						}
					})
					miniBtn(c, p, fmt.Sprintf("字号：%.0fpt", a.cfg.EditorFontSize), func() {
						a.cfg.EditorFontSize += 1
						if a.cfg.EditorFontSize > 17 {
							a.cfg.EditorFontSize = 12
						}
						if a.codeEditorState != nil {
							a.codeEditorState.Font = ui.Font{Size: a.cfg.EditorFontSize}
						}
					})
				})
			}
		}

		editor.View(c, a.codeEditorState).Grow(1).FillWidth()
	})
}
