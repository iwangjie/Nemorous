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

// codeRepoGroup is one repository's slice of the code explorer tree.
type codeRepoGroup struct {
	repoID   string
	repoName string
	monogram string
	colorIdx int
	files    []gitops.WorkspaceFile
}

// codeRepoGroups filters the workspace file list by query. The result is cached
// per query so typing does not scan every file each frame.
func (a *App) codeRepoGroups(query string) []codeRepoGroup {
	key := "q|" + query
	if a.codeGroupsKey == key {
		return a.codeGroups
	}
	var groups []codeRepoGroup
	groupIdx := make(map[string]int)

	for _, wf := range a.workspaceFiles {
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
	sort.Slice(groups, func(i, j int) bool {
		return strings.ToLower(groups[i].repoName) < strings.ToLower(groups[j].repoName)
	})
	a.codeGroupsKey = key
	a.codeGroups = groups
	return groups
}

const (
	codeTreeIndent      = 12
	codeTreeRowHeight   = 22
	codeTreeChevronSlot = 15
	codeTreeRootHeight  = 24
	codeTreeRepoHeight  = 24
)

// codeFileIconColor tints file icons by type, so the tree reads at a glance
// without adding another badge.
func codeFileIconColor(name string, p Palette) ui.Color {
	switch strings.ToLower(filepath.Ext(name)) {
	case ".go", ".java", ".kt", ".ts", ".tsx", ".js", ".jsx", ".py", ".rs", ".c", ".h", ".cpp", ".cs", ".rb", ".php", ".swift":
		return p.Blue
	case ".json", ".yaml", ".yml", ".xml", ".toml", ".properties", ".conf", ".ini":
		return p.Amber
	default:
		return p.TextMuted
	}
}

// drawCodeTreeGuides paints one vertical rail per ancestor level, aligned under
// the parent chevrons so deep trees keep a readable spine.
func drawCodeTreeGuides(c *ui.Context, p Palette, depth int) {
	for level := range depth {
		ui.Box(c).Absolute().Left(float32(15 + level*codeTreeIndent)).Top(0).Width(1).Height(codeTreeRowHeight).Background(p.Border)
	}
}

func (a *App) buildCodeLeftPane(c *ui.Context, p Palette) {
	query := strings.ToLower(strings.TrimSpace(a.codeFilterQuery))
	groups := a.codeRepoGroups(query)

	ui.Column(c).Fill().Padding(8).Gap(6).Background(p.RailBg).Children(func() {
		ui.Row(c).Gap(6).AlignItems(ui.Center).Children(func() {
			ui.SearchField(c.Key("code-search"), &a.codeFilterQuery).Bind(&a.codeFilterHandle).Placeholder("搜索文件名或路径...").Grow(1)
			if len(a.expandedCodeRepos) > 0 {
				miniBtn(c, p, "全部收起", func() {
					a.expandedCodeRepos = make(map[string]bool)
					a.expandedCodeDirs = make(map[string]bool)
					a.codeRootCollapsed = true
				})
			}
		})

		// One continuous explorer: workspace root → repositories → directories.
		ui.Column(c).Grow(1).Background(p.Surface).Children(func() {
			ui.Scroll(c).Grow(1).Children(func() {
				rootName := filepath.Base(a.cfg.WorkspacePath)
				if rootName == "" || rootName == "." {
					rootName = "workspace"
				}
				rootExpanded := query != "" || !a.codeRootCollapsed
				rkc := c.Key("croot")
				rootRow := ui.Row(rkc).Height(codeTreeRootHeight).Padding(0, 10).Gap(5).Cursor(ui.CursorPointer).AlignItems(ui.Center)
				if rootRow.Hovered() {
					rootRow.Background(p.RailBg)
				}
				if rootRow.Clicked() && query == "" {
					a.codeRootCollapsed = !a.codeRootCollapsed
				}
				rootRow.Children(func() {
					chev := icoChevronRight
					if rootExpanded {
						chev = icoChevronDown
					}
					drawIcon(rkc, chev, 10, p.TextMuted)
					drawIcon(rkc, icoFolder, 13, p.TextControl)
					ui.Text(rkc, rootName).FontSize(12).FontWeight(weightSemibold).TextColor(p.TextPrimary).MaxLines(1)
					ui.Text(rkc, displayPath(a.cfg.WorkspacePath)).FontSize(10.5).TextColor(p.TextMuted).MaxLines(1).Grow(1)
				})

				if !rootExpanded {
					return
				}
				for _, grp := range groups {
					grpCopy := grp
					expanded := query != "" || a.expandedCodeRepos[grpCopy.repoID]
					badge := repoBadgeColors(grpCopy.colorIdx, p.IsDark)
					treeKey := grpCopy.repoID + "|" + query
					treeRoot, cached := a.codeTreeCache[treeKey]
					if !cached {
						treeRoot = buildRepoCodeTree(grpCopy.files)
						a.codeTreeCache[treeKey] = treeRoot
					}

					gkc := c.Key("cgrp-" + grpCopy.repoID)
					hdr := ui.Row(gkc).Height(codeTreeRepoHeight).Padding(0, 10, 0, 10+codeTreeIndent).Gap(5).Cursor(ui.CursorPointer).AlignItems(ui.Center)
					if hdr.Hovered() {
						hdr.Background(p.RailBg)
					}
					if hdr.Clicked() {
						if expanded {
							delete(a.expandedCodeRepos, grpCopy.repoID)
						} else {
							a.expandedCodeRepos[grpCopy.repoID] = true
						}
					}
					hdr.Children(func() {
						drawCodeTreeGuides(gkc, p, 1)
						chev := icoChevronRight
						if expanded {
							chev = icoChevronDown
						}
						drawIcon(gkc, chev, 10, p.TextMuted)
						ui.Box(gkc).Size(15, 15).Radius(3).Background(badge.Bg).Center().Children(func() {
							ui.Text(gkc, grpCopy.monogram).FontSize(8.5).FontWeight(weightBold).TextColor(badge.Text)
						})
						ui.Text(gkc, grpCopy.repoName).FontSize(11.5).FontWeight(weightSemibold).TextColor(p.TextPrimary).MaxLines(1).Grow(1)
					})

					if expanded {
						a.renderCodeTreeNode(gkc, p, grpCopy.repoID, treeRoot, 2, query != "")
					}
				}
			})
		})
	})
}

func (a *App) renderCodeTreeNode(c *ui.Context, p Palette, repoID string, node *codeTreeDir, depth int, forceExpand bool) {
	indent := float32(10 + depth*codeTreeIndent)

	// 1. Render subdirectories first
	for _, sub := range node.subdirs {
		subCopy := sub
		dirKey := repoID + "::" + subCopy.relPath
		expanded := forceExpand || a.expandedCodeDirs[dirKey]

		dkc := c.Key("dir-" + dirKey)
		dRow := ui.Row(dkc).Height(codeTreeRowHeight).Padding(0, 10, 0, indent).Gap(5).Background(p.Surface).Cursor(ui.CursorPointer).AlignItems(ui.Center)
		if dRow.Hovered() {
			dRow.Background(p.RailBg)
		}
		if dRow.Clicked() {
			a.expandedCodeDirs[dirKey] = !a.expandedCodeDirs[dirKey]
		}
		dRow.Children(func() {
			drawCodeTreeGuides(dkc, p, depth)
			chev := icoChevronRight
			if expanded {
				chev = icoChevronDown
			}
			drawIcon(dkc, chev, 10, p.TextMuted)
			drawIcon(dkc, icoFolder, 12, p.TextControl)
			ui.Text(dkc, subCopy.name).FontSize(11.5).FontWeight(weightMedium).TextColor(p.TextPrimary).MaxLines(1).Grow(1)
		})

		if expanded {
			a.renderCodeTreeNode(dkc, p, repoID, subCopy, depth+1, forceExpand)
		}
	}

	// 2. Render files in this directory
	for _, wf := range node.files {
		wfCopy := wf
		activeFile := a.activeCodeFile()
		selected := activeFile != nil && activeFile.RepoID == wf.RepoID && activeFile.RelPath == wf.RelPath
		rowBg := p.Surface
		textCol := p.TextSecondary
		if selected {
			rowBg = p.ActiveRowBg
			textCol = p.TextPrimary
		}
		base := filepath.Base(wf.RelPath)

		fkc := c.Key("wf-" + wf.RepoID + "::" + wf.RelPath)
		fRow := ui.Row(fkc).Height(codeTreeRowHeight).Padding(0, 10, 0, indent+codeTreeChevronSlot).Gap(5).Background(rowBg).Cursor(ui.CursorPointer).AlignItems(ui.Center)
		if fRow.Hovered() && !selected {
			fRow.Background(p.RailBg)
		}
		if fRow.Clicked() {
			a.selectCodeFile(wfCopy)
		}
		fRow.Children(func() {
			drawCodeTreeGuides(fkc, p, depth)
			drawIcon(fkc, icoFile, 12, codeFileIconColor(base, p))
			ui.Text(fkc, base).FontSize(11.5).TextColor(textCol).MaxLines(1).Grow(1)
		})
	}
}

func (a *App) buildCodeWelcome(c *ui.Context, p Palette) {
	ui.Column(c).Grow(1).FillWidth().Center().Gap(8).Background(p.AppBg).Children(func() {
		drawIcon(c, icoCode, 28, p.TextMuted)
		ui.Text(c, "未打开文件").FontSize(15).FontWeight(weightSemibold).TextColor(p.TextPrimary)
		ui.Text(c, "从左侧文件树选择文件，或按 ⌘K 搜索文件").FontSize(12).TextColor(p.TextMuted)
	})
}

// buildCodeTabBar renders the open files like an editor's tabs: one row that
// scrolls horizontally, with the active file in the surface color.
func (a *App) buildCodeTabBar(c *ui.Context, p Palette) {
	ui.Row(c).Height(36).FillWidth().Padding(0, 6).Background(p.RailBg).BorderWidth(0, 0, 1, 0).BorderColor(p.Border).AlignItems(ui.Center).Children(func() {
		ui.ScrollHorizontal(c).Height(34).Grow(1).Children(func() {
			for i, tab := range a.codeTabs {
				tabIndex := i
				tabCopy := tab
				active := i == a.codeTabIdx
				borderCol := ui.Transparent
				if active {
					borderCol = p.Border
				}
				// Keep a 1px border and the close slot on every tab so switching
				// the active tab never changes any tab's width.
				row := ui.Row(c).Height(28).Padding(0, 8).Gap(6).Radius(6).AlignItems(ui.Center).Cursor(ui.CursorPointer).Border(1, borderCol)
				if active {
					row.Background(p.Surface)
				} else if row.Hovered() {
					row.Background(p.Surface2)
				}
				closed := false
				row.Children(func() {
					badge := repoBadgeColors(tabCopy.file.ColorIdx, p.IsDark)
					ui.Box(c).Size(6, 6).Radius(3).Background(badge.Text)
					name := filepath.Base(tabCopy.file.RelPath)
					textCol := p.TextSecondary
					if active {
						textCol = p.TextPrimary
					}
					ui.Text(c, name).FontSize(11.5).FontWeight(weightMedium).TextColor(textCol).MaxWidth(180).SingleLine()
					if tabCopy.dirty() {
						ui.Box(c).Size(6, 6).Radius(3).Background(p.Amber)
					}
					xb := ui.Box(c).Size(16, 16).Radius(3).Center().Shrink(0)
					if active || row.Hovered() {
						xb.Cursor(ui.CursorPointer)
						if xb.Hovered() {
							xb.Background(p.SurfaceHover)
						}
						if xb.Clicked() {
							closed = true
							a.requestCloseCodeTab(tabCopy)
						}
						xb.Children(func() {
							col := p.TextMuted
							if xb.Hovered() {
								col = p.TextPrimary
							}
							drawIcon(c, icoClose, 10, col)
						})
					}
				})
				if row.Clicked() && !closed {
					a.activateCodeTab(tabIndex)
				}
			}
		})
	})
}

func (a *App) buildCodeRightPane(c *ui.Context, p Palette) {
	ui.Column(c).Fill().Background(p.Surface).Children(func() {
		tab := a.activeCodeTab()
		if tab == nil {
			a.buildCodeWelcome(c, p)
			return
		}

		a.buildCodeTabBar(c, p)
		wf := tab.file
		isModified := tab.dirty()

		ui.Row(c).Height(42).FillWidth().Padding(0, 14).Background(p.Surface).BorderWidth(0, 0, 1, 0).BorderColor(p.Border).Justify(ui.SpaceBetween).AlignItems(ui.Center).Children(func() {
			ui.Row(c).Gap(8).AlignItems(ui.Center).Grow(1).Children(func() {
				badgeBox(c, wf.RepoName, p.AccentSoftText, p.AccentSoft, ui.Color{}, 11, true)
				ui.Text(c, wf.RelPath).FontSize(12.5).FontWeight(weightSemibold).TextColor(p.TextPrimary).SingleLine()
				if isModified {
					badgeBox(c, "未保存", p.Amber, p.AmberSoft, ui.Color{}, 10.5, true)
				}
			})

			ui.Row(c).Gap(6).AlignItems(ui.Center).Children(func() {
				if isModified {
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
					for _, t := range a.codeTabs {
						if t.state != nil {
							t.state.SoftWrap = a.cfg.EditorSoftWrap
						}
					}
				})
				miniBtn(c, p, fmt.Sprintf("字号：%.0fpt", a.cfg.EditorFontSize), func() {
					a.cfg.EditorFontSize += 1
					if a.cfg.EditorFontSize > 17 {
						a.cfg.EditorFontSize = 12
					}
					for _, t := range a.codeTabs {
						if t.state != nil {
							t.state.Font = ui.Font{Size: a.cfg.EditorFontSize}
						}
					}
				})
			})
		}

		editor.View(c, tab.state).Grow(1).FillWidth()
	})
}
