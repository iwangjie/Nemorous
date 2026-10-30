package main

import (
	"fmt"
	"path/filepath"
	"strings"

	"github.com/egoist/mygo/ui"
	"github.com/egoist/mygo/ui/editor"
	"github.com/iwangjie/nemorous/internal/gitops"
)

// changeTreeNode is the directory hierarchy of one repository's changed files.
type changeTreeNode struct {
	name    string
	relPath string
	dirs    []*changeTreeNode
	files   []gitops.ChangedFile
}

func buildChangeTree(files []gitops.ChangedFile) *changeTreeNode {
	root := &changeTreeNode{}
	for _, f := range files {
		parts := strings.Split(filepath.ToSlash(f.Path), "/")
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
			var child *changeTreeNode
			for _, candidate := range cur.dirs {
				if candidate.name == seg {
					child = candidate
					break
				}
			}
			if child == nil {
				child = &changeTreeNode{name: seg, relPath: curPath}
				cur.dirs = append(cur.dirs, child)
			}
			cur = child
		}
		cur.files = append(cur.files, f)
	}
	finalizeChangeTree(root)
	return root
}

func finalizeChangeTree(node *changeTreeNode) {
	sortChangeDirs(node)
	for i, child := range node.dirs {
		finalizeChangeTree(child)
		for len(child.dirs) == 1 && len(child.files) == 0 {
			only := child.dirs[0]
			child.name = child.name + "/" + only.name
			child.relPath = only.relPath
			child.dirs = only.dirs
			child.files = only.files
		}
		node.dirs[i] = child
	}
	sortChangeFiles(node)
}

func sortChangeDirs(node *changeTreeNode) {
	for i := 1; i < len(node.dirs); i++ {
		for j := i; j > 0 && node.dirs[j].name < node.dirs[j-1].name; j-- {
			node.dirs[j], node.dirs[j-1] = node.dirs[j-1], node.dirs[j]
		}
	}
}

func sortChangeFiles(node *changeTreeNode) {
	for i := 1; i < len(node.files); i++ {
		for j := i; j > 0 && filepath.Base(node.files[j].Path) < filepath.Base(node.files[j-1].Path); j-- {
			node.files[j], node.files[j-1] = node.files[j-1], node.files[j]
		}
	}
}

func collectChangeFiles(node *changeTreeNode) []gitops.ChangedFile {
	var out []gitops.ChangedFile
	out = append(out, node.files...)
	for _, dir := range node.dirs {
		out = append(out, collectChangeFiles(dir)...)
	}
	return out
}

func kindTextColor(kind string, p Palette) ui.Color {
	switch kind {
	case "A", "?":
		return p.Green
	case "D", "U":
		return p.Danger
	case "R":
		return p.Purple
	case "M":
		return p.Blue
	default:
		return p.Amber
	}
}

func kindChipBg(kind string, p Palette) ui.Color {
	switch kind {
	case "A", "?":
		return p.GreenSoft
	case "D", "U":
		return p.DangerSoft
	case "R":
		return p.PurpleSoft
	case "M":
		return p.BlueSoft
	default:
		return p.AmberSoft
	}
}

// buildChangesLeftPane renders repositories as IDEA-style file trees: directories
// collapse, files carry status and stage badges, right-click offers staging,
// discarding and per-file selection.
func (a *App) buildChangesLeftPane(c *ui.Context, p Palette) {
	allTotal := 0
	checkedFiles := 0
	checkedRepos := 0
	visibleTotal := 0
	var dirtyStatuses []gitops.RepoStatus
	filteredByRepo := make(map[string][]gitops.ChangedFile, len(a.statuses))

	for _, st := range a.statuses {
		if len(st.Files) == 0 {
			continue
		}
		dirtyStatuses = append(dirtyStatuses, st)
		allTotal += len(st.Files)
		repoHasChecked := false
		for _, f := range st.Files {
			if a.checkedChanges[f.Key()] {
				checkedFiles++
				repoHasChecked = true
			}
		}
		if repoHasChecked {
			checkedRepos++
		}
		files := a.filterChangeFiles(st.Repo.Name, st.Files)
		filteredByRepo[st.Repo.ID] = files
		visibleTotal += len(files)
	}
	filtered := strings.TrimSpace(a.changesFilterQuery) != ""
	aheadRepos := 0
	for _, st := range a.statuses {
		if st.Ahead > 0 {
			aheadRepos++
		}
	}

	ui.Column(c).Fill().Padding(6).Gap(6).Background(p.RailBg).Children(func() {
		visibleFiles := make([]gitops.ChangedFile, 0, visibleTotal)
		for _, st := range dirtyStatuses {
			visibleFiles = append(visibleFiles, filteredByRepo[st.Repo.ID]...)
		}
		checkedVisible := 0
		for _, f := range visibleFiles {
			if a.checkedChanges[f.Key()] {
				checkedVisible++
			}
		}
		allVisibleChecked := visibleTotal > 0 && checkedVisible == visibleTotal
		someVisibleChecked := checkedVisible > 0 && !allVisibleChecked

		ui.Row(c).Justify(ui.SpaceBetween).AlignItems(ui.Center).Children(func() {
			ui.Row(c).Gap(6).AlignItems(ui.Center).Children(func() {
				if visibleTotal > 0 {
					a.changeParentCheck(c, p, "changes-root-check", "选择全部改动", allVisibleChecked, someVisibleChecked, func() {
						a.setChangeFilesChecked(visibleFiles, !allVisibleChecked)
					})
				}
				ui.Text(c, "本地变更").FontSize(12.5).FontWeight(weightSemibold).TextColor(p.TextPrimary)
				if allTotal > 0 {
					ui.Text(c, fmt.Sprintf("%d 个文件", allTotal)).FontSize(10.5).TextColor(p.TextMuted)
				}
				if filtered {
					badgeBox(c, fmt.Sprintf("筛出 %d", visibleTotal), p.AccentSoftText, p.AccentSoft, ui.Color{}, 10, false)
				}
			})
			if allTotal > 0 {
				menuPill(c, p, "Stash ▾", false, func(m *ui.Menu) {
					if m.Item("贮藏全部改动 (Stash)").Chosen() {
						a.TriggerStashAll()
					}
					if m.Item("恢复最近贮藏 (Stash Pop)").Chosen() {
						a.TriggerStashPopAll()
					}
				})
			}
		})

		stagedRepoFiles := make(map[string][]gitops.ChangedFile, len(dirtyStatuses))
		unstagedRepoFiles := make(map[string][]gitops.ChangedFile, len(dirtyStatuses))
		var allStagedFiles []gitops.ChangedFile
		var allUnstagedFiles []gitops.ChangedFile

		for _, st := range dirtyStatuses {
			files := filteredByRepo[st.Repo.ID]
			var stFiles []gitops.ChangedFile
			var unFiles []gitops.ChangedFile
			for _, f := range files {
				if f.Staged {
					stFiles = append(stFiles, f)
					allStagedFiles = append(allStagedFiles, f)
				}
				if f.Unstaged || f.Untracked || (!f.Staged) {
					unFiles = append(unFiles, f)
					allUnstagedFiles = append(allUnstagedFiles, f)
				}
			}
			stagedRepoFiles[st.Repo.ID] = stFiles
			unstagedRepoFiles[st.Repo.ID] = unFiles
		}

		if !a.stagedSectionInit {
			a.stagedSectionInit = true
			if len(allStagedFiles) == 0 {
				a.collapsedStagedSection = true
			}
		}

		stagedChecked := 0
		for _, f := range allStagedFiles {
			if a.checkedChanges[f.Key()] {
				stagedChecked++
			}
		}
		stagedAllChecked := len(allStagedFiles) > 0 && stagedChecked == len(allStagedFiles)
		stagedSomeChecked := stagedChecked > 0 && !stagedAllChecked

		unstagedChecked := 0
		for _, f := range allUnstagedFiles {
			if a.checkedChanges[f.Key()] {
				unstagedChecked++
			}
		}
		unstagedAllChecked := len(allUnstagedFiles) > 0 && unstagedChecked == len(allUnstagedFiles)
		unstagedSomeChecked := unstagedChecked > 0 && !unstagedAllChecked

		ui.Scroll(c).Grow(1).Children(func() {
			if len(dirtyStatuses) == 0 {
				ui.Column(c).Padding(16).Gap(8).Radius(8).Background(p.Surface).Border(1, p.Border).Children(func() {
					ui.Text(c, "所有仓库均无未提交改动").FontSize(13).FontWeight(weightSemibold).TextColor(p.TextPrimary)
					ui.Text(c, "工作区保持干净。可以继续合并到 test，或回到开发分支。").FontSize(11.5).TextColor(p.TextMuted)
					ui.Row(c).Wrap().Gap(6).Children(func() {
						if aheadRepos > 0 {
							pill(c, p, fmt.Sprintf("推送待推送仓库 (%d)", aheadRepos), false, func() {
								a.TriggerPushAllAhead()
							})
						}
						pill(c, p, "合并到 "+a.mergeTargetBranch, true, func() {
							a.enterGitMerge()
							a.TriggerMergeScan()
						})
						if len(a.lastBatchResults) > 0 {
							miniBtn(c, p, "操作记录", func() {
								a.showOpLog = true
							})
						}
					})
				})
			} else if visibleTotal == 0 {
				ui.Column(c).Padding(16).Gap(8).Radius(8).Background(p.Surface).Border(1, p.Border).Children(func() {
					ui.Text(c, "没有匹配的改动文件").FontSize(12.5).FontWeight(weightSemibold).TextColor(p.TextPrimary)
					miniBtn(c, p, "清除筛选", func() {
						a.changesFilterQuery = ""
					})
				})
			} else {
				a.buildChangesSection(c, p, "staged", "已暂存", allStagedFiles, stagedAllChecked, stagedSomeChecked, a.collapsedStagedSection, func() {
					a.collapsedStagedSection = !a.collapsedStagedSection
				}, func() {
					a.setChangeFilesChecked(allStagedFiles, !stagedAllChecked)
				}, func() {
					a.stageChangeFiles(allStagedFiles, false)
				}, dirtyStatuses, stagedRepoFiles, true)

				a.buildChangesSection(c, p, "unstaged", "未暂存", allUnstagedFiles, unstagedAllChecked, unstagedSomeChecked, a.collapsedUnstagedSection, func() {
					a.collapsedUnstagedSection = !a.collapsedUnstagedSection
				}, func() {
					a.setChangeFilesChecked(allUnstagedFiles, !unstagedAllChecked)
				}, func() {
					a.stageChangeFiles(allUnstagedFiles, true)
				}, dirtyStatuses, unstagedRepoFiles, false)
			}
		})

		a.buildCommitResizer(c, p)
		box := a.buildCommitBox(c, p, checkedRepos, checkedFiles, a.changesCommitH)
		if a.changesCommitH == 0 {
			if h := box.Bounds().H; h > 0 {
				a.changesCommitAutoH = h
			}
		}
	})
}

// buildCommitResizer draws the divider above the commit box: drag it, or focus
// it and use Up/Down, to grow or shrink the commit box like the editor splits.
func (a *App) buildCommitResizer(c *ui.Context, p Palette) {
	kc := c.Key("changes-commit-resizer")
	grip := ui.Row(kc).Height(6).FillWidth().Cursor(ui.CursorResizeNS).Focusable().
		AlignItems(ui.Center).Tooltip("拖动调整提交框高度").Label("调整提交框高度")
	_, dy, held := grip.Dragged()
	if held && dy != 0 {
		a.resizeCommitBox(-dy)
	}
	if grip.DoubleClicked() {
		a.changesCommitH = 0
	}
	if grip.Shortcut(0, ui.KeyUp) {
		a.resizeCommitBox(20)
	}
	if grip.Shortcut(0, ui.KeyDown) {
		a.resizeCommitBox(-20)
	}
	grip.Children(func() {
		line := p.BorderSubtle
		if grip.Hovered() || held || grip.FocusVisible() {
			line = p.Accent
		}
		ui.Box(c).Height(1).FillWidth().Background(line)
	})
}

// resizeCommitBox keeps the commit box between the action row's height and a
// size that still leaves room for the changes list.
func (a *App) resizeCommitBox(delta float32) {
	base := a.changesCommitH
	if base <= 0 {
		base = a.changesCommitAutoH
	}
	if base <= 0 {
		base = 160
	}
	a.changesCommitH = min(max(base+delta, 160), 420)
}

func (a *App) changeFileMatchesQuery(repoName string, f gitops.ChangedFile) bool {
	query := strings.ToLower(strings.TrimSpace(a.changesFilterQuery))
	if query == "" {
		return true
	}
	hay := strings.ToLower(repoName + "/" + f.Path)
	return strings.Contains(hay, query)
}

func (a *App) filterChangeFiles(repoName string, files []gitops.ChangedFile) []gitops.ChangedFile {
	out := make([]gitops.ChangedFile, 0, len(files))
	for _, f := range files {
		if !a.changeFileMatchesQuery(repoName, f) {
			continue
		}
		out = append(out, f)
	}
	return out
}

// changeFilterKey identifies the cached tree for the active text filter.
func (a *App) changeFilterKey(repoID string) string {
	return repoID + "|" + strings.ToLower(strings.TrimSpace(a.changesFilterQuery))
}

func (a *App) buildChangesSection(
	c *ui.Context,
	p Palette,
	sectionKey string,
	title string,
	files []gitops.ChangedFile,
	allChecked, someChecked bool,
	collapsed bool,
	toggleCollapse func(),
	toggleCheck func(),
	batchAction func(),
	statuses []gitops.RepoStatus,
	repoFiles map[string][]gitops.ChangedFile,
	isStaged bool,
) {
	skc := c.Key("sec-" + sectionKey)
	ui.Column(skc).Children(func() {
		hdr := ui.Row(skc).Height(24).Padding(0, 4).Gap(5).AlignItems(ui.Center).Radius(4).Cursor(ui.CursorPointer)
		if hdr.Hovered() {
			hdr.Background(p.SurfaceHover)
		}
		hdr.PopoverMenu(func(m *ui.Menu) {
			if isStaged {
				if len(files) > 0 && m.Item("取消暂存全部").Chosen() {
					batchAction()
				}
			} else {
				if len(files) > 0 && m.Item("暂存全部").Chosen() {
					batchAction()
				}
			}
			if len(files) > 0 {
				if m.Item("全部选中").Chosen() {
					a.setChangeFilesChecked(files, true)
				}
				if m.Item("全部取消选中").Chosen() {
					a.setChangeFilesChecked(files, false)
				}
			}
		})

		hdr.Children(func() {
			a.changeParentCheck(skc, p, "sec-check-"+sectionKey, "选择全部"+title+"改动", allChecked, someChecked, toggleCheck)

			chev := icoChevronDown
			if collapsed {
				chev = icoChevronRight
			}
			chevEl := ui.Row(skc.Key("chev")).Padding(1, 0).Cursor(ui.CursorPointer).Shrink(0)
			chevEl.Children(func() { drawIcon(skc.Key("chev-ico"), chev, 10, p.TextMuted) })
			if chevEl.Clicked() {
				toggleCollapse()
			}

			infoRow := ui.Row(skc.Key("info")).Grow(1).Gap(6).AlignItems(ui.Center).Cursor(ui.CursorPointer)
			if infoRow.Clicked() {
				toggleCollapse()
			}
			infoRow.Children(func() {
				ui.Text(skc.Key("txt"), title).FontSize(12).FontWeight(weightSemibold).TextColor(p.TextPrimary)
				ui.Text(skc.Key("cnt"), fmt.Sprintf("%d", len(files))).FontSize(10.5).TextColor(p.TextMuted)
				ui.Spacer(skc.Key("spc"))
				if hdr.Hovered() && len(files) > 0 {
					actionText := "取消暂存"
					if !isStaged {
						actionText = "全部暂存"
					}
					miniBtn(skc.Key("btn"), p, actionText, batchAction)
				}
			})
		})

		if !collapsed {
			if len(files) == 0 {
				emptyMsg := "无已暂存改动"
				if !isStaged {
					emptyMsg = "无未暂存改动"
				}
				ui.Row(skc.Key("empty")).Height(20).Padding(0, 24).Children(func() {
					ui.Text(skc.Key("empty-txt"), emptyMsg).FontSize(10.5).TextColor(p.TextMuted)
				})
			} else {
				for _, st := range statuses {
					rf := repoFiles[st.Repo.ID]
					if len(rf) == 0 {
						continue
					}
					a.buildChangeRepoGroup(c, p, sectionKey, st, rf, isStaged)
				}
			}
		}
	})
}

func (a *App) buildChangeRepoGroup(c *ui.Context, p Palette, section string, st gitops.RepoStatus, files []gitops.ChangedFile, isStaged bool) {
	stCopy := st
	badge := repoBadgeColors(stCopy.Repo.ColorIndex, p.IsDark)
	visible := files
	repoAllChecked := len(visible) > 0
	for _, f := range visible {
		if !a.checkedChanges[f.Key()] {
			repoAllChecked = false
			break
		}
	}
	repoKey := section + "::" + stCopy.Repo.ID
	collapsed := a.collapsedChangeRepos[repoKey]

	gkc := c.Key("grp-" + repoKey)
	ui.Column(gkc).Children(func() {
		hdr := ui.Row(gkc).Height(24).Padding(0, 6, 0, 10).Gap(5).AlignItems(ui.Center)
		if hdr.Hovered() {
			hdr.Background(p.SurfaceHover)
		}
		repoMenu := func(m *ui.Menu) {
			if isStaged {
				if m.Item("取消暂存本仓").Chosen() {
					a.stageChangeFiles(visible, false)
				}
			} else {
				if m.Item("暂存本仓").Chosen() {
					a.stageChangeFiles(visible, true)
				}
			}
			m.Separator()
			if m.Item("复制仓库路径").Chosen() {
				a.pendingClipboard = stCopy.Repo.AbsPath
			}
			m.Separator()
			if m.Item("贮藏本仓改动").Chosen() {
				a.stashRepo(stCopy.Repo, false)
			}
			if m.Item("恢复本仓贮藏").Chosen() {
				a.stashRepo(stCopy.Repo, true)
			}
			if a.maniAvailable() {
				m.Separator()
				m.Submenu("执行 Mani 任务", func(sm *ui.Menu) {
					for _, name := range a.maniTaskNames() {
						taskName := name
						if sm.Item(taskName).Chosen() {
							a.TriggerManiTaskForRepo(taskName, stCopy.Repo.ID)
						}
					}
				})
			}
			m.Separator()
			if m.Item("在文件管理器中显示").Chosen() {
				_ = revealPath(stCopy.Repo.AbsPath)
			}
			if m.Item("撤销上一次提交…").Chosen() {
				a.pendingUndo = &PendingUndo{RepoPath: stCopy.Repo.AbsPath, RepoName: stCopy.Repo.Name}
			}
		}
		hdr.PopoverMenu(repoMenu)
		repoChecked := 0
		for _, f := range visible {
			if a.checkedChanges[f.Key()] {
				repoChecked++
			}
		}
		repoSomeChecked := repoChecked > 0 && !repoAllChecked

		hdr.Children(func() {
			a.changeParentCheck(c, p, "repo-check-"+repoKey, "选择 "+stCopy.Repo.Name+" 的全部改动", repoAllChecked, repoSomeChecked, func() {
				a.setChangeFilesChecked(visible, !repoAllChecked)
			})

			chev := icoChevronDown
			if collapsed {
				chev = icoChevronRight
			}
			chevEl := ui.Row(gkc.Key("chev")).Padding(1, 0).Cursor(ui.CursorPointer).Shrink(0)
			chevEl.Children(func() { drawIcon(gkc.Key("chev-ico"), chev, 10, p.TextMuted) })
			if chevEl.Clicked() {
				a.collapsedChangeRepos[repoKey] = !collapsed
			}

			ui.Box(gkc.Key("mono")).Size(16, 16).Radius(3).Background(badge.Bg).Center().Shrink(0).Children(func() {
				ui.Text(gkc.Key("mono-txt"), stCopy.Repo.Monogram).FontSize(9).FontWeight(weightBold).TextColor(badge.Text)
			})
			ui.Text(gkc.Key("rname"), stCopy.Repo.Name).FontSize(12).FontWeight(weightSemibold).TextColor(p.TextPrimary).SingleLine()
			badgeBox(gkc.Key("branch"), stCopy.Branch, p.TextControl, p.Surface2, ui.Color{}, 9.5, false)
			ui.Spacer(gkc.Key("spc"))
			ui.Text(gkc.Key("rcnt"), fmt.Sprintf("%d", len(visible))).FontSize(10).TextColor(p.TextMuted)
			if hdr.Hovered() {
				more := ui.Row(gkc.Key("more")).Size(16, 16).Radius(3).Center().Cursor(ui.CursorPointer).Shrink(0)
				more.PopoverMenu(repoMenu)
				more.Children(func() {
					drawIcon(gkc.Key("more-ico"), icoMore, 10, p.TextMuted)
				})
			}
		})

		if !collapsed {
			treeKey := section + "|" + a.changeFilterKey(stCopy.Repo.ID)
			tree, ok := a.changeTreeCache[treeKey]
			if !ok {
				tree = buildChangeTree(visible)
				a.changeTreeCache[treeKey] = tree
			}
			ordered := a.renderedChangeFiles(repoKey, tree)
			a.renderChangeTreeNode(c, p, section, stCopy.Repo.ID, tree, 0, ordered, isStaged)
		}
	})
}

func (a *App) renderChangeTreeNode(c *ui.Context, p Palette, section, repoID string, node *changeTreeNode, depth int, ordered []gitops.ChangedFile, isStaged bool) {
	indent := float32(24 + depth*16)
	for _, dir := range node.dirs {
		dirCopy := dir
		dirKey := section + "::" + repoID + "::" + dirCopy.relPath
		collapsed := a.collapsedChangeDirs[dirKey]
		children := collectChangeFiles(dirCopy)
		dirChecked := 0
		for _, f := range children {
			if a.checkedChanges[f.Key()] {
				dirChecked++
			}
		}
		dirAllChecked := len(children) > 0 && dirChecked == len(children)
		dirSomeChecked := dirChecked > 0 && !dirAllChecked

		dkc := c.Key("cdir-" + dirKey)
		dRow := ui.Row(dkc).Height(22).Padding(0, 6, 0, indent).Gap(5).Cursor(ui.CursorPointer).AlignItems(ui.Center)
		if dRow.Hovered() {
			dRow.Background(p.SurfaceHover)
		}
		dRow.PopoverMenu(func(m *ui.Menu) {
			if isStaged {
				if m.Item("取消暂存此目录").Chosen() {
					a.stageChangeFiles(children, false)
				}
			} else {
				if m.Item("暂存此目录").Chosen() {
					a.stageChangeFiles(children, true)
				}
			}
		})
		dRow.Children(func() {
			a.changeParentCheck(c, p, "dir-check-"+dirKey, "选择 "+dirCopy.name+" 的全部改动", dirAllChecked, dirSomeChecked, func() {
				a.setChangeFilesChecked(children, !dirAllChecked)
			})
			chev := icoChevronRight
			if !collapsed {
				chev = icoChevronDown
			}
			chevKc := c.Key("cdir-chev-" + dirKey)
			chevEl := ui.Row(chevKc).Padding(1, 0).Cursor(ui.CursorPointer).Shrink(0)
			if chevEl.Clicked() {
				a.collapsedChangeDirs[dirKey] = !collapsed
			}
			chevEl.Children(func() { drawIcon(chevKc, chev, 9, p.TextMuted) })

			infoKc := c.Key("cdir-info-" + dirKey)
			info := ui.Row(infoKc).Grow(1).Gap(5).AlignItems(ui.Center).Cursor(ui.CursorPointer)
			if info.Clicked() {
				a.collapsedChangeDirs[dirKey] = !collapsed
			}
			info.Children(func() {
				drawIcon(infoKc, icoFolder, 11, p.TextMuted)
				ui.Text(infoKc, dirCopy.name).FontSize(11).FontWeight(weightMedium).TextColor(p.TextSecondary).MaxLines(1).Grow(1)
				ui.Text(infoKc, fmt.Sprintf("%d", len(children))).FontSize(9.5).TextColor(p.TextMuted)
			})
		})

		if !collapsed {
			a.renderChangeTreeNode(c, p, section, repoID, dirCopy, depth+1, ordered, isStaged)
		}
	}
	for _, cf := range node.files {
		a.buildChangeFileRow(c, p, section, cf, indent, ordered, isStaged)
	}
}

func (a *App) buildChangeFileRow(c *ui.Context, p Palette, section string, cf gitops.ChangedFile, indent float32, ordered []gitops.ChangedFile, isStaged bool) {
	cfCopy := cf
	key := cfCopy.Key()
	isSelected := a.selectedChangeKey == key && a.diffStagedOnly == isStaged
	isChecked := a.checkedChanges[key]

	rowBg := ui.Transparent
	if isSelected {
		rowBg = p.ActiveRowBg
	}
	kindColor := kindTextColor(cfCopy.Kind, p)
	kindBg := kindChipBg(cfCopy.Kind, p)

	fkc := c.Key(section + "-cf-" + key)
	fRow := ui.Row(fkc).Height(22).Padding(0, 6, 0, indent).Gap(5).Background(rowBg).AlignItems(ui.Center).Cursor(ui.CursorPointer)
	if fRow.Hovered() && !isSelected {
		fRow.Background(p.SurfaceHover)
	}
	if fRow.RightClicked() {
		a.diffStagedOnly = isStaged
		a.selectChangedFile(cfCopy)
	}
	fRow.PopoverMenu(func(m *ui.Menu) {
		if isStaged {
			if m.Item("取消暂存").Chosen() {
				a.stageChangeFiles([]gitops.ChangedFile{cfCopy}, false)
			}
		} else {
			if m.Item("暂存").Chosen() {
				a.stageChangeFiles([]gitops.ChangedFile{cfCopy}, true)
			}
		}
		if m.Item("只选中此文件").Chosen() {
			a.checkedChanges = map[string]bool{key: true}
			a.diffStagedOnly = isStaged
			a.selectChangedFile(cfCopy)
		}
		if m.Item("查看差异").Chosen() {
			a.diffStagedOnly = isStaged
			a.selectChangedFile(cfCopy)
		}
		if m.Item("在文件视图打开").Chosen() {
			a.selectCodeFile(gitops.WorkspaceFile{
				RepoID:   cfCopy.RepoID,
				RepoName: cfCopy.RepoName,
				RepoPath: cfCopy.RepoPath,
				RelPath:  cfCopy.Path,
			})
			a.activeTab = TabCode
		}
		if m.Item("复制相对路径").Chosen() {
			a.pendingClipboard = cfCopy.Path
		}
		if m.Item("查看文件历史").Chosen() {
			a.openFileHistory(cfCopy)
		}
		absPath := filepath.Join(cfCopy.RepoPath, cfCopy.Path)
		if m.Item("在文件管理器中显示").Chosen() {
			target := absPath
			if cfCopy.Kind == "D" {
				target = filepath.Join(cfCopy.RepoPath, filepath.Dir(cfCopy.Path))
			}
			_ = revealPath(target)
		}
		if cfCopy.Kind != "D" {
			if m.Item("用默认应用打开").Chosen() {
				_ = openPath(absPath)
			}
		}
		m.Separator()
		if m.Item("还原更改").Chosen() {
			a.pendingDiscard = &PendingDiscard{
				RepoPath:  cfCopy.RepoPath,
				RepoName:  cfCopy.RepoName,
				RelPath:   cfCopy.Path,
				Untracked: cfCopy.Untracked,
			}
		}
	})
	fRow.Children(func() {
		fChk := ui.Box(fkc.Key("chk")).Size(14, 14).Radius(3).Cursor(ui.CursorPointer).Center().Shrink(0)
		if isChecked {
			fChk.Background(p.Accent)
		} else {
			fChk.Background(p.Surface2).Border(1, p.BorderStrong)
		}
		if fChk.Clicked() {
			if fChk.ClickModifiers()&ui.Shift != 0 {
				a.toggleChangeRange(ordered, key)
			} else {
				a.checkedChanges[key] = !a.checkedChanges[key]
				a.lastCheckedKey = key
			}
		}
		fChk.Children(func() {
			if isChecked {
				drawIcon(fkc.Key("chk-ico"), icoCheck, 10, p.AccentText)
			}
		})

		// Spacer matching chevron slot in dir row so kind badge aligns with folder icon
		ui.Box(fkc.Key("spc")).Width(10).Shrink(0)

		ui.Box(fkc.Key("kind")).Size(15, 15).Radius(3).Background(kindBg).Center().Shrink(0).Children(func() {
			ui.Text(fkc.Key("kind-txt"), cfCopy.Kind).FontSize(9.5).FontWeight(weightBold).TextColor(kindColor)
		})

		baseName := filepath.Base(cfCopy.Path)

		info := ui.Row(fkc.Key("info")).Grow(1).Gap(5).AlignItems(ui.Center).Cursor(ui.CursorPointer)
		if info.Clicked() {
			a.diffStagedOnly = isStaged
			if info.ClickModifiers()&ui.Shift != 0 {
				a.toggleChangeRange(ordered, key)
			} else {
				a.selectChangedFile(cfCopy)
			}
		}
		info.Children(func() {
			nameColor := kindColor
			if isSelected {
				nameColor = p.TextPrimary
			}
			nameText := ui.Text(fkc.Key("name"), baseName).FontSize(11.5).FontWeight(weightMedium).TextColor(nameColor).MaxLines(1)
			if cfCopy.Kind == "D" {
				nameText.Strikethrough()
			}
			ui.Spacer(fkc.Key("spc2"))
			if !isSelected && (cfCopy.AddedLines > 0 || cfCopy.DeletedLines > 0) {
				ui.Text(fkc.Key("lines"), fmt.Sprintf("+%d -%d", cfCopy.AddedLines, cfCopy.DeletedLines)).FontSize(9.5).TextColor(p.TextMuted).Shrink(0)
			}
		})
	})
}

func (a *App) buildCommitBox(c *ui.Context, p Palette, checkedRepos, checkedFiles int, height float32) ui.Element {
	commitBtnLabel := "提交"
	if checkedFiles > 0 {
		if checkedRepos > 1 {
			commitBtnLabel = fmt.Sprintf("提交 (%d 个仓库)", checkedRepos)
		} else {
			commitBtnLabel = fmt.Sprintf("提交 (%d 个文件)", checkedFiles)
		}
	}

	canCommit, commitHint := a.commitAvailability(checkedFiles)

	box := ui.Column(c).Gap(4)
	if height > 0 {
		box = box.Height(height)
	}
	box.Children(func() {
		ui.Row(c).Padding(2, 4).AlignItems(ui.Center).Children(func() {
			ui.Checkbox(c, &a.commitAmend, "修正")
		})

		card := ui.Column(c).Padding(8).Gap(6).Radius(4).Background(p.AppBg).Border(1, p.Border)
		if height > 0 {
			card = card.Grow(1)
		}
		card.Children(func() {
			ta := ui.TextArea(c.Key("commit-msg"), &a.commitMessage).Bind(&a.commitInputHandle).Placeholder("输入提交说明...").FillWidth()
			if height > 0 {
				ta = ta.Grow(1).MinHeight(40)
			} else {
				ta = ta.Lines(2, 6)
			}
			ta.Background(p.AppBg).Border(0, p.Border).Radius(3).Padding(4, 6)

			ui.Row(c).Wrap().Gap(6).AlignItems(ui.Center).Children(func() {
				if canCommit {
					pill(c, p, commitBtnLabel, true, func() {
						a.TriggerCommit(false)
					})
					pill(c, p, "提交并推送", false, func() {
						a.TriggerCommit(true)
					})
				} else {
					pillDisabled(c, p, commitBtnLabel)
					pillDisabled(c, p, "提交并推送")
				}
			})
			if !canCommit && commitHint != "" {
				ui.Text(c, commitHint).FontSize(10.5).TextColor(p.TextMuted)
			}
		})
	})
	return box
}

func (a *App) buildDiffRightPane(c *ui.Context, p Palette) {
	ui.Column(c).Fill().Background(p.Surface).Children(func() {
		if a.selectedChangeFile == nil {
			ui.Column(c).Grow(1).FillWidth().Center().Gap(10).Background(p.AppBg).Children(func() {
				ui.Text(c, "暂未选择变更文件").FontSize(14).FontWeight(weightSemibold).TextColor(p.TextPrimary)
				ui.Text(c, "在左侧点击任意改动文件查看差异对比，或切换到「文件」浏览代码。").FontSize(12).TextColor(p.TextMuted)
				ui.Row(c).Gap(8).Children(func() {
					iconPill(c, p, icoCode, "浏览代码文件", false, func() {
						a.activeTab = TabCode
					})
					iconPill(c, p, icoBranch, "切换分支 (⌘B)", false, func() {
						a.openBranchSwitchModal()
					})
				})
			})
			return
		}

		cf := *a.selectedChangeFile
		isDirty := a.diffModified || (a.diffState != nil && a.diffState.New != nil && a.diffState.New.Modified())

		// Clean Diff Header Bar
		ui.Row(c).Height(42).FillWidth().Padding(0, 14).Background(p.RailBg).BorderWidth(0, 0, 1, 0).BorderColor(p.Border).Justify(ui.SpaceBetween).AlignItems(ui.Center).Children(func() {
			ui.Row(c).Gap(8).AlignItems(ui.Center).Grow(1).Children(func() {
				badgeBox(c, cf.RepoName, p.AccentSoftText, p.AccentSoft, ui.Color{}, 11, true)
				if cf.OrigPath != "" {
					ui.Text(c, cf.OrigPath+" → "+cf.Path).FontSize(12.5).FontWeight(weightSemibold).TextColor(p.TextPrimary).SingleLine()
				} else {
					ui.Text(c, cf.Path).FontSize(12.5).FontWeight(weightSemibold).TextColor(p.TextPrimary).SingleLine()
				}
				ui.Text(c, fmt.Sprintf("+%d  -%d", cf.AddedLines, cf.DeletedLines)).FontSize(11.5).TextColor(p.TextMuted).SingleLine()
				if isDirty {
					badgeBox(c, "未保存", p.Amber, p.AmberSoft, ui.Color{}, 10.5, true)
				}
			})

			ui.Row(c).Gap(6).AlignItems(ui.Center).Children(func() {
				if cf.Staged || a.diffStagedOnly {
					ui.Row(c).Padding(2).Gap(2).Radius(6).Background(p.Surface3).Children(func() {
						diffScope := func(staged bool, label string) {
							active := a.diffStagedOnly == staged
							bg := ui.Transparent
							fg := p.TextControl
							if active {
								bg = p.Surface
								if p.IsDark {
									bg = p.Surface2
								}
								fg = p.TextPrimary
							}
							el := ui.Row(c.Key("dscope-"+label)).Padding(3, 8).Radius(4).Background(bg).Cursor(ui.CursorPointer)
							if el.Clicked() && a.diffStagedOnly != staged {
								a.diffStagedOnly = staged
								a.selectChangedFile(cf)
							}
							el.Children(func() {
								ui.Text(c, label).FontSize(11.5).FontWeight(weightMedium).TextColor(fg)
							})
						}
						diffScope(false, "工作区")
						diffScope(true, "暂存区")
					})
				}

				// Split vs Unified switch
				ui.Row(c).Padding(2).Gap(2).Radius(6).Background(p.Surface3).Children(func() {
					splitBtn := func(sideBySide bool, label string) {
						active := a.cfg.DiffSideBySide == sideBySide
						bg := ui.Transparent
						fg := p.TextControl
						if active {
							bg = p.Surface
							if p.IsDark {
								bg = p.Surface2
							}
							fg = p.TextPrimary
						}
						el := ui.Row(c.Key("dlayout-"+label)).Padding(3, 8).Radius(4).Background(bg).Cursor(ui.CursorPointer)
						if el.Clicked() {
							a.cfg.DiffSideBySide = sideBySide
							if a.diffState != nil {
								a.diffState.Layout = diffLayout(sideBySide)
							}
						}
						el.Children(func() {
							ui.Text(c, label).FontSize(11.5).FontWeight(weightMedium).TextColor(fg)
						})
					}
					splitBtn(true, "双栏")
					splitBtn(false, "单栏")
				})

				editLabel := "编辑"
				if a.diffEditable {
					editLabel = "退出编辑"
				}
				iconPill(c, p, icoEdit, editLabel, a.diffEditable, func() {
					a.diffEditable = !a.diffEditable
					if a.diffState != nil {
						a.diffState.Editable = a.diffEditable
					}
				})

				if a.diffEditable || isDirty {
					pill(c, p, "保存 (⌘S)", true, func() {
						a.saveDiffMicroEdit()
					})
				}

				miniBtn(c, p, "打开源文件", func() {
					a.selectCodeFile(gitops.WorkspaceFile{
						RepoID:   cf.RepoID,
						RepoName: cf.RepoName,
						RepoPath: cf.RepoPath,
						RelPath:  cf.Path,
					})
					a.activeTab = TabCode
				})

				// Collapsible Display Options
				ghostBtn(c, p, icoMore, "", a.showDiffOptions, func() {
					a.showDiffOptions = !a.showDiffOptions
				})
			})
		})

		// Collapsible Diff Display Options Bar ("不常用操作折叠起来，必要时候也能用")
		if a.showDiffOptions {
			ui.Row(c).Padding(6, 14).Gap(10).Background(p.RailBg).BorderWidth(0, 0, 1, 0).BorderColor(p.Border).AlignItems(ui.Center).Children(func() {
				ui.Text(c, "显示选项：").FontSize(11.5).TextColor(p.TextMuted)
				wrapLabel := "自动换行：关"
				if a.cfg.EditorSoftWrap {
					wrapLabel = "自动换行：开"
				}
				miniBtn(c, p, wrapLabel, func() {
					a.cfg.EditorSoftWrap = !a.cfg.EditorSoftWrap
					if a.diffState != nil {
						a.diffState.Wrap = a.cfg.EditorSoftWrap
					}
				})

				inlineLabel := "行内高亮：按词"
				if a.cfg.DiffInlineMode == "char" {
					inlineLabel = "行内高亮：按字符"
				} else if a.cfg.DiffInlineMode == "none" {
					inlineLabel = "行内高亮：关闭"
				}
				miniBtn(c, p, inlineLabel, func() {
					switch a.cfg.DiffInlineMode {
					case "word":
						a.cfg.DiffInlineMode = "char"
					case "char":
						a.cfg.DiffInlineMode = "none"
					default:
						a.cfg.DiffInlineMode = "word"
					}
					if a.diffState != nil {
						a.diffState.Inline = parseInlineMode(a.cfg.DiffInlineMode)
					}
				})

				miniBtn(c, p, fmt.Sprintf("字号：%.0fpt", a.cfg.EditorFontSize), func() {
					a.cfg.EditorFontSize += 1
					if a.cfg.EditorFontSize > 17 {
						a.cfg.EditorFontSize = 12
					}
					if a.diffState != nil {
						a.diffState.Font = ui.Font{Size: a.cfg.EditorFontSize}
					}
				})
			})
		}

		// DiffView
		editor.DiffView(c, a.diffState).Grow(1).FillWidth()
	})
}

// changeParentCheck renders the tri-state checkbox shared by the changes root,
// repositories and directories. A partial group shows a dash; clicking it
// selects every child, as IDEA's commit tree does.
func (a *App) changeParentCheck(c *ui.Context, p Palette, key, label string, all, some bool, onClick func()) {
	kc := c.Key(key)
	box := ui.Box(kc).Size(14, 14).Radius(3).Cursor(ui.CursorPointer).Center().Shrink(0).Label(label)
	if all {
		box.Background(p.Accent)
	} else if some {
		box.Background(p.Accent)
	} else {
		box.Background(p.Surface2).Border(1, p.BorderStrong)
	}
	if box.Clicked() && onClick != nil {
		onClick()
	}
	box.Children(func() {
		if all {
			drawIcon(kc, icoCheck, 10, p.AccentText)
		} else if some {
			drawIcon(kc, icoMinus, 10, p.AccentText)
		}
	})
}

// setChangeFilesChecked applies one parent checkbox to every child file.
func (a *App) setChangeFilesChecked(files []gitops.ChangedFile, checked bool) {
	for _, f := range files {
		a.checkedChanges[f.Key()] = checked
	}
}

// renderedChangeFiles walks directories before files, skipping collapsed folders,
// so shift ranges and arrow keys only touch what is on screen.
func (a *App) renderedChangeFiles(prefixKey string, node *changeTreeNode) []gitops.ChangedFile {
	var out []gitops.ChangedFile
	for _, dir := range node.dirs {
		if a.collapsedChangeDirs[prefixKey+"::"+dir.relPath] {
			continue
		}
		out = append(out, a.renderedChangeFiles(prefixKey, dir)...)
	}
	out = append(out, node.files...)
	return out
}

// visibleChangeFilesForSection returns one repository's visible files in render order for either staged or unstaged.
func (a *App) visibleChangeFilesForSection(section string, repoID string, isStaged bool) []gitops.ChangedFile {
	st, ok := a.statusByID[repoID]
	if !ok || len(st.Files) == 0 {
		return nil
	}
	allFiles := a.filterChangeFiles(st.Repo.Name, st.Files)
	if len(allFiles) == 0 {
		return nil
	}
	var files []gitops.ChangedFile
	for _, f := range allFiles {
		if isStaged && f.Staged {
			files = append(files, f)
		} else if !isStaged && (f.Unstaged || f.Untracked || !f.Staged) {
			files = append(files, f)
		}
	}
	if len(files) == 0 {
		return nil
	}
	treeKey := section + "|" + a.changeFilterKey(repoID)
	tree, ok := a.changeTreeCache[treeKey]
	if !ok {
		tree = buildChangeTree(files)
		a.changeTreeCache[treeKey] = tree
	}
	return a.renderedChangeFiles(section+"::"+repoID, tree)
}

// visibleChangeFileList flattens every visible repository tree in display order.
func (a *App) visibleChangeFileList() []gitops.ChangedFile {
	var out []gitops.ChangedFile
	if !a.collapsedStagedSection {
		for _, st := range a.statuses {
			if a.collapsedChangeRepos["staged::"+st.Repo.ID] {
				continue
			}
			out = append(out, a.visibleChangeFilesForSection("staged", st.Repo.ID, true)...)
		}
	}
	if !a.collapsedUnstagedSection {
		for _, st := range a.statuses {
			if a.collapsedChangeRepos["unstaged::"+st.Repo.ID] {
				continue
			}
			out = append(out, a.visibleChangeFilesForSection("unstaged", st.Repo.ID, false)...)
		}
	}
	return out
}

// toggleChangeRange applies a Shift-click across the visible rows of one repository.
func (a *App) toggleChangeRange(ordered []gitops.ChangedFile, targetKey string) {
	from, to := -1, -1
	for i, f := range ordered {
		if f.Key() == a.lastCheckedKey {
			from = i
		}
		if f.Key() == targetKey {
			to = i
		}
	}
	if from < 0 || to < 0 || from == to {
		a.checkedChanges[targetKey] = !a.checkedChanges[targetKey]
		a.lastCheckedKey = targetKey
		return
	}
	if from > to {
		from, to = to, from
	}
	value := !a.checkedChanges[targetKey]
	for i := from; i <= to; i++ {
		a.checkedChanges[ordered[i].Key()] = value
	}
	a.lastCheckedKey = targetKey
}

// moveChangeSelection moves the diff selection with the arrow keys.
func (a *App) moveChangeSelection(delta int) {
	list := a.visibleChangeFileList()
	if len(list) == 0 {
		return
	}
	idx := -1
	for i, f := range list {
		if f.Key() == a.selectedChangeKey {
			idx = i
			break
		}
	}
	next := 0
	switch {
	case idx < 0 && delta < 0:
		next = len(list) - 1
	case idx >= 0:
		next = (idx + delta + len(list)) % len(list)
	}
	a.selectChangedFile(list[next])
}

// toggleSelectedChange flips the commit checkbox of the selected file.
func (a *App) toggleSelectedChange() {
	if a.selectedChangeFile == nil {
		return
	}
	key := a.selectedChangeFile.Key()
	a.checkedChanges[key] = !a.checkedChanges[key]
	a.lastCheckedKey = key
}

// stageSelectedChange stages or unstages the selected file.
func (a *App) stageSelectedChange(stage bool) {
	if a.selectedChangeFile == nil {
		return
	}
	a.stageChangeFiles([]gitops.ChangedFile{*a.selectedChangeFile}, stage)
}

// textFocusActive reports whether a text field owns typing in the main view.
func (a *App) textFocusActive(c *ui.Context) bool {
	return a.diffEditable || a.commitInputHandle.Focused(c) || a.changesFilterHandle.Focused(c) || a.codeFilterHandle.Focused(c)
}
