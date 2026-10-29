package main

import (
	"fmt"
	"path/filepath"
	"strings"

	"github.com/egoist/mygo/ui"
	"github.com/egoist/mygo/ui/editor"
	"github.com/iwangjie/nemorous/internal/gitops"
)

func (a *App) buildChangesView(c *ui.Context, p Palette) {
	ui.Split(c, &a.changesSplit,
		func() { a.buildChangesLeftPane(c, p) },
		func() { a.buildDiffRightPane(c, p) },
	).Grow(1).Fill()
}

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
		return p.Accent
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
		return p.AccentSoft
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
	kindCounts := make(map[string]int)
	var dirtyStatuses []gitops.RepoStatus
	var cleanStatuses []gitops.RepoStatus
	filteredByRepo := make(map[string][]gitops.ChangedFile, len(a.statuses))

	for _, st := range a.statuses {
		if a.repoFilterID != "" && st.Repo.ID != a.repoFilterID {
			continue
		}
		if len(st.Files) == 0 {
			cleanStatuses = append(cleanStatuses, st)
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
			if !a.changeFileMatchesQuery(st.Repo.Name, f) {
				continue
			}
			kind := f.Kind
			if kind == "?" {
				kind = "A"
			}
			kindCounts[kind]++
		}
		if repoHasChecked {
			checkedRepos++
		}
		files := a.filterChangeFiles(st.Repo.Name, st.Files)
		filteredByRepo[st.Repo.ID] = files
		visibleTotal += len(files)
	}
	filtered := a.changesKindFilter != "" || strings.TrimSpace(a.changesFilterQuery) != ""
	aheadRepos := 0
	for _, st := range a.statuses {
		if st.Ahead > 0 {
			aheadRepos++
		}
	}

	ui.Column(c).Fill().Padding(12).Gap(10).Background(p.RailBg).Children(func() {
		ui.Row(c).Justify(ui.SpaceBetween).AlignItems(ui.Center).Children(func() {
			title := "本地变更"
			if allTotal > 0 {
				title = fmt.Sprintf("本地变更 (%d/%d)", checkedFiles, allTotal)
			}
			ui.Row(c).Gap(6).AlignItems(ui.Center).Children(func() {
				ui.Text(c, title).FontSize(12.5).FontWeight(weightSemibold).TextColor(p.TextPrimary)
				if filtered {
					badgeBox(c, fmt.Sprintf("筛出 %d", visibleTotal), p.Accent, p.AccentSoft, ui.Color{}, 10, false)
				}
			})
			if allTotal > 0 {
				ui.Row(c).Gap(4).AlignItems(ui.Center).Children(func() {
					miniBtn(c, p, "全选", func() {
						for _, st := range dirtyStatuses {
							for _, f := range filteredByRepo[st.Repo.ID] {
								a.checkedChanges[f.Key()] = true
							}
						}
					})
					miniBtn(c, p, "清空", func() {
						for k := range a.checkedChanges {
							a.checkedChanges[k] = false
						}
					})
					menuPill(c, p, "Stash ▾", false, func(m *ui.Menu) {
						if m.Item("贮藏全部改动 (Stash)").Chosen() {
							a.TriggerStashAll()
						}
						if m.Item("恢复最近贮藏 (Stash Pop)").Chosen() {
							a.TriggerStashPopAll()
						}
					})
				})
			}
		})

		ui.Row(c).Gap(6).AlignItems(ui.Center).Children(func() {
			ui.SearchField(c.Key("changes-search"), &a.changesFilterQuery).Bind(&a.changesFilterHandle).Placeholder("搜索改动文件...").Grow(1)
			repoLabel := "全部仓库"
			if a.repoFilterID != "" {
				if st, ok := a.statusByID[a.repoFilterID]; ok {
					repoLabel = st.Repo.Name
				}
			}
			menuPill(c, p, repoLabel+" ▾", a.repoFilterID != "", func(m *ui.Menu) {
				if m.Item("全部仓库").Checked(a.repoFilterID == "").Chosen() {
					a.repoFilterID = ""
				}
				for _, st := range a.statuses {
					stCopy := st
					if m.Item(stCopy.Repo.Name).Checked(a.repoFilterID == stCopy.Repo.ID).Chosen() {
						a.repoFilterID = stCopy.Repo.ID
					}
				}
			})
		})

		if allTotal > 0 {
			a.buildChangeFilterRow(c, p, kindCounts)
		}

		ui.Scroll(c).Grow(1).Gap(8).Children(func() {
			for _, st := range dirtyStatuses {
				a.buildChangeRepoGroup(c, p, st, filteredByRepo[st.Repo.ID])
			}

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
							a.activeTab = TabMerge
							a.TriggerMergePreflight(false)
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
						a.changesKindFilter = ""
						a.changesFilterQuery = ""
						a.repoFilterID = ""
					})
				})
			}

			if len(cleanStatuses) > 0 {
				a.buildCleanRepoList(c, p, cleanStatuses)
			}
		})

		a.buildCommitBox(c, p, checkedRepos, checkedFiles)
	})
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
		if a.changesKindFilter != "" {
			kind := f.Kind
			if kind == "?" {
				kind = "A"
			}
			if kind != a.changesKindFilter {
				continue
			}
		}
		out = append(out, f)
	}
	return out
}

// changeFilterKey identifies the cached tree for the active kind + text filters.
func (a *App) changeFilterKey(repoID string) string {
	return repoID + "|" + a.changesKindFilter + "|" + strings.ToLower(strings.TrimSpace(a.changesFilterQuery))
}

func (a *App) buildChangeFilterRow(c *ui.Context, p Palette, counts map[string]int) {
	chip := func(key, label string, count int) {
		active := a.changesKindFilter == key
		bg := p.Surface2
		fg := p.TextControl
		if active {
			bg = p.AccentSoft
			fg = p.Accent
		}
		kc := c.Key("cfkind-" + key)
		el := ui.Row(kc).Padding(3, 8).Gap(4).Radius(5).Background(bg).Cursor(ui.CursorPointer).AlignItems(ui.Center)
		if el.Hovered() && !active {
			el.Background(p.SurfaceHover)
		}
		if el.Clicked() {
			a.changesKindFilter = key
		}
		el.Children(func() {
			ui.Text(kc, label).FontSize(11).FontWeight(weightMedium).TextColor(fg)
			if count > 0 {
				ui.Text(kc, fmt.Sprintf("%d", count)).FontSize(10).TextColor(p.TextMuted)
			}
		})
	}
	ui.Row(c).Wrap().Gap(4).AlignItems(ui.Center).Children(func() {
		chip("", "全部", 0)
		chip("A", "新增", counts["A"])
		chip("M", "修改", counts["M"])
		chip("D", "删除", counts["D"])
		chip("R", "重命名", counts["R"])
		chip("U", "冲突", counts["U"])
	})
}

func (a *App) buildChangeRepoGroup(c *ui.Context, p Palette, st gitops.RepoStatus, files []gitops.ChangedFile) {
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
	stagedCount := 0
	for _, f := range visible {
		if f.Staged {
			stagedCount++
		}
	}
	collapsed := a.collapsedChangeRepos[stCopy.Repo.ID]

	gkc := c.Key("grp-" + stCopy.Repo.ID)
	ui.Column(gkc).Radius(8).Background(p.Surface).Border(1, p.Border).Children(func() {
		hdr := ui.Row(gkc).Padding(8, 10).Gap(7).Background(p.Surface).AlignItems(ui.Center)
		if !collapsed || len(visible) == 0 {
			hdr.BorderWidth(0, 0, 1, 0).BorderColor(p.BorderSubtle)
		}
		repoMenu := func(m *ui.Menu) {
			if m.Item("全选本仓").Chosen() {
				for _, f := range stCopy.Files {
					a.checkedChanges[f.Key()] = true
				}
			}
			if m.Item("清空本仓勾选").Chosen() {
				for _, f := range stCopy.Files {
					a.checkedChanges[f.Key()] = false
				}
			}
			m.Separator()
			if m.Item("暂存本仓全部").Chosen() {
				a.stageChangeFiles(stCopy.Files, true)
			}
			if m.Item("取消暂存本仓全部").Chosen() {
				a.stageChangeFiles(stCopy.Files, false)
			}
			m.Separator()
			if m.Item("勾选本仓并生成提交说明").Chosen() {
				for _, f := range stCopy.Files {
					a.checkedChanges[f.Key()] = true
				}
				a.generateCommitMessage()
			}
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
			m.Separator()
			if m.Item("在文件管理器中显示").Chosen() {
				_ = revealPath(stCopy.Repo.AbsPath)
			}
			if m.Item("撤销上一次提交…").Chosen() {
				a.pendingUndo = &PendingUndo{RepoPath: stCopy.Repo.AbsPath, RepoName: stCopy.Repo.Name}
			}
		}
		hdr.PopoverMenu(repoMenu)
		hdr.Children(func() {
			chkBox := ui.Box(gkc).Size(15, 15).Radius(4).Cursor(ui.CursorPointer).Center().Shrink(0)
			if repoAllChecked {
				chkBox.Background(p.Accent)
			} else {
				chkBox.Background(p.Surface2).Border(1, p.BorderStrong)
			}
			if chkBox.Clicked() {
				next := !repoAllChecked
				for _, f := range visible {
					a.checkedChanges[f.Key()] = next
				}
			}
			chkBox.Children(func() {
				if repoAllChecked {
					drawIcon(gkc, icoCheck, 11, p.AccentText)
				}
			})

			chev := icoChevronDown
			if collapsed {
				chev = icoChevronRight
			}
			chevEl := ui.Row(gkc).Padding(1, 0).Cursor(ui.CursorPointer).Shrink(0)
			chevEl.Children(func() { drawIcon(gkc, chev, 11, p.TextMuted) })
			if chevEl.Clicked() {
				a.collapsedChangeRepos[stCopy.Repo.ID] = !collapsed
			}

			ui.Box(gkc).Size(18, 18).Radius(4).Background(badge.Bg).Center().Shrink(0).Children(func() {
				ui.Text(gkc, stCopy.Repo.Monogram).FontSize(9.5).FontWeight(weightBold).TextColor(badge.Text)
			})
			ui.Text(gkc, stCopy.Repo.Name).FontSize(12.5).FontWeight(weightSemibold).TextColor(p.TextPrimary).SingleLine()
			badgeBox(gkc, stCopy.Branch, p.TextControl, p.Surface2, ui.Color{}, 10.5, false)
			ui.Spacer(gkc)
			if stagedCount > 0 {
				badgeBox(gkc, fmt.Sprintf("已暂存 %d", stagedCount), p.Green, p.GreenSoft, ui.Color{}, 10, true)
			}
			ui.Text(gkc, fmt.Sprintf("%d", len(visible))).FontSize(11).TextColor(p.TextMuted)
			more := ui.Row(gkc).Padding(1, 3).Radius(4).Cursor(ui.CursorPointer).Shrink(0)
			if more.Hovered() {
				more.Background(p.Surface2)
			}
			more.PopoverMenu(repoMenu)
			more.Children(func() {
				drawIcon(gkc, icoMore, 13, p.TextMuted)
			})
		})

		if !collapsed {
			if len(visible) == 0 {
				ui.Row(gkc).Padding(8, 10).Children(func() {
					ui.Text(gkc, "当前筛选无匹配文件").FontSize(11).TextColor(p.TextMuted)
				})
			} else {
				treeKey := a.changeFilterKey(stCopy.Repo.ID)
				tree, ok := a.changeTreeCache[treeKey]
				if !ok {
					tree = buildChangeTree(visible)
					a.changeTreeCache[treeKey] = tree
				}
				ordered := a.renderedChangeFiles(stCopy.Repo.ID, tree)
				a.renderChangeTreeNode(c, p, stCopy.Repo.ID, tree, 0, ordered)
			}
		}
	})
}

func (a *App) renderChangeTreeNode(c *ui.Context, p Palette, repoID string, node *changeTreeNode, depth int, ordered []gitops.ChangedFile) {
	indent := float32(10 + depth*14)
	for _, dir := range node.dirs {
		dirCopy := dir
		dirKey := repoID + "::" + dirCopy.relPath
		collapsed := a.collapsedChangeDirs[dirKey]
		children := collectChangeFiles(dirCopy)

		dkc := c.Key("cdir-" + dirKey)
		dRow := ui.Row(dkc).Padding(5, 10, 5, indent).Gap(6).Cursor(ui.CursorPointer).AlignItems(ui.Center).BorderWidth(0, 0, 1, 0).BorderColor(p.BorderSubtle)
		if dRow.Hovered() {
			dRow.Background(p.RailBg)
		}
		dRow.PopoverMenu(func(m *ui.Menu) {
			if m.Item("勾选此目录").Chosen() {
				for _, f := range children {
					a.checkedChanges[f.Key()] = true
				}
			}
			if m.Item("取消勾选此目录").Chosen() {
				for _, f := range children {
					a.checkedChanges[f.Key()] = false
				}
			}
			m.Separator()
			if m.Item("暂存此目录").Chosen() {
				a.stageChangeFiles(children, true)
			}
			if m.Item("取消暂存此目录").Chosen() {
				a.stageChangeFiles(children, false)
			}
		})
		if dRow.Clicked() {
			a.collapsedChangeDirs[dirKey] = !collapsed
		}
		dRow.Children(func() {
			chev := icoChevronRight
			if !collapsed {
				chev = icoChevronDown
			}
			drawIcon(dkc, chev, 11, p.TextMuted)
			drawIcon(dkc, icoFolder, 13, p.TextMuted)
			ui.Text(dkc, dirCopy.name).FontSize(11.5).FontWeight(weightMedium).TextColor(p.TextSecondary).MaxLines(1).Grow(1)
			ui.Text(dkc, fmt.Sprintf("%d", len(children))).FontSize(10.5).TextColor(p.TextMuted)
		})

		if !collapsed {
			a.renderChangeTreeNode(c, p, repoID, dirCopy, depth+1, ordered)
		}
	}
	for _, cf := range node.files {
		a.buildChangeFileRow(c, p, cf, indent+14, ordered)
	}
}

func (a *App) buildChangeFileRow(c *ui.Context, p Palette, cf gitops.ChangedFile, indent float32, ordered []gitops.ChangedFile) {
	cfCopy := cf
	key := cfCopy.Key()
	isSelected := a.selectedChangeKey == key
	isChecked := a.checkedChanges[key]

	rowBg := p.Surface
	if isSelected {
		rowBg = p.ActiveRowBg
	}
	kindColor := kindTextColor(cfCopy.Kind, p)
	kindBg := kindChipBg(cfCopy.Kind, p)

	fkc := c.Key("cf-" + key)
	fRow := ui.Row(fkc).Padding(6, 10, 6, indent).Gap(6).Background(rowBg).AlignItems(ui.Center).Cursor(ui.CursorPointer)
	fRow.BorderWidth(0, 0, 1, 0).BorderColor(p.BorderSubtle)
	if fRow.Hovered() && !isSelected {
		fRow.Background(p.RailBg)
	}
	if fRow.RightClicked() {
		a.selectChangedFile(cfCopy)
	}
	fRow.PopoverMenu(func(m *ui.Menu) {
		if cfCopy.Staged {
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
			a.commitScope = "checked"
			a.selectChangedFile(cfCopy)
		}
		if m.Item("查看差异").Chosen() {
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
		fChk := ui.Box(fkc).Size(15, 15).Radius(4).Cursor(ui.CursorPointer).Center().Shrink(0)
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
				drawIcon(fkc, icoCheck, 11, p.AccentText)
			}
		})

		ui.Box(fkc).Size(17, 17).Radius(3).Background(kindBg).Center().Shrink(0).Children(func() {
			ui.Text(fkc, cfCopy.Kind).FontSize(10).FontWeight(weightBold).TextColor(kindColor)
		})

		baseName := filepath.Base(cfCopy.Path)
		dirName := filepath.Dir(cfCopy.Path)

		info := ui.Row(fkc).Grow(1).Gap(6).AlignItems(ui.Center).Cursor(ui.CursorPointer)
		if info.Clicked() {
			if info.ClickModifiers()&ui.Shift != 0 {
				a.toggleChangeRange(ordered, key)
			} else {
				a.selectChangedFile(cfCopy)
			}
		}
		info.Children(func() {
			ui.Text(fkc, baseName).FontSize(12).FontWeight(weightMedium).TextColor(p.TextPrimary).MaxLines(1)
			if dirName != "." && uiColumnDepthHint(dirName) {
				ui.Text(fkc, dirName).FontSize(10.5).TextColor(p.TextMuted).MaxLines(1)
			}
			if cfCopy.Staged {
				stageBadge := "已暂存"
				if cfCopy.Unstaged {
					stageBadge = "部分暂存"
				}
				badgeBox(fkc, stageBadge, p.Green, p.GreenSoft, ui.Color{}, 10, false)
			}
			ui.Spacer(fkc)
			if !isSelected && (cfCopy.AddedLines > 0 || cfCopy.DeletedLines > 0) {
				ui.Text(fkc, fmt.Sprintf("+%d -%d", cfCopy.AddedLines, cfCopy.DeletedLines)).FontSize(10.5).TextColor(p.TextMuted).Shrink(0)
			}
		})

		if isSelected || fRow.Hovered() {
			ui.Row(fkc).Gap(4).Shrink(0).Children(func() {
				stageLabel := "暂存"
				if cfCopy.Staged {
					stageLabel = "取消暂存"
				}
				miniBtn(fkc, p, stageLabel, func() {
					a.stageChangeFiles([]gitops.ChangedFile{cfCopy}, !cfCopy.Staged)
				})
				miniBtn(fkc, p, "还原", func() {
					a.pendingDiscard = &PendingDiscard{
						RepoPath:  cfCopy.RepoPath,
						RepoName:  cfCopy.RepoName,
						RelPath:   cfCopy.Path,
						Untracked: cfCopy.Untracked,
					}
				})
			})
		}
	})
}

// uiColumnDepthHint keeps the muted directory hint only for shallow trees; deep
// paths are already obvious from the indentation.
func uiColumnDepthHint(dirName string) bool {
	return strings.Count(dirName, "/") <= 1
}

func (a *App) buildCleanRepoList(c *ui.Context, p Palette, cleanStatuses []gitops.RepoStatus) {
	toggleRow := ui.Row(c.Key("clean-repos-toggle")).Padding(6, 8).Gap(6).Radius(6).Cursor(ui.CursorPointer).AlignItems(ui.Center)
	if toggleRow.Hovered() {
		toggleRow.Background(p.Surface2)
	}
	if toggleRow.Clicked() {
		a.showCleanReposInChanges = !a.showCleanReposInChanges
	}
	toggleRow.Children(func() {
		chev := icoChevronRight
		if a.showCleanReposInChanges {
			chev = icoChevronDown
		}
		drawIcon(c, chev, 12, p.TextMuted)
		ui.Text(c, fmt.Sprintf("无改动仓库 (%d)", len(cleanStatuses))).FontSize(11.5).FontWeight(weightMedium).TextColor(p.TextMuted)
	})

	if !a.showCleanReposInChanges {
		return
	}
	ui.Column(c).Radius(8).Background(p.Surface).Border(1, p.Border).Children(func() {
		for idx, st := range cleanStatuses {
			stCopy := st
			badge := repoBadgeColors(st.Repo.ColorIndex, p.IsDark)
			ck := c.Key("clean-repo-" + st.Repo.ID)
			r := ui.Row(ck).Padding(7, 10).Gap(8).Cursor(ui.CursorPointer).AlignItems(ui.Center)
			if idx < len(cleanStatuses)-1 {
				r.BorderWidth(0, 0, 1, 0).BorderColor(p.BorderSubtle)
			}
			if r.Hovered() {
				r.Background(p.RailBg)
			}
			if r.Clicked() {
				a.repoFilterID = stCopy.Repo.ID
				a.activeTab = TabCode
			}
			r.Children(func() {
				ui.Box(ck).Size(18, 18).Radius(4).Background(badge.Bg).Center().Children(func() {
					ui.Text(ck, stCopy.Repo.Monogram).FontSize(9.5).FontWeight(weightBold).TextColor(badge.Text)
				})
				ui.Text(ck, stCopy.Repo.Name).FontSize(12).TextColor(p.TextSecondary).Grow(1)
				ui.Text(ck, stCopy.Branch).FontSize(11).TextColor(p.TextMuted)
			})
		}
	})
}

func (a *App) buildCommitBox(c *ui.Context, p Palette, checkedRepos, checkedFiles int) {
	stagedTotal := 0
	for _, st := range a.statuses {
		if a.repoFilterID != "" && st.Repo.ID != a.repoFilterID {
			continue
		}
		for _, f := range st.Files {
			if f.Staged {
				stagedTotal++
			}
		}
	}

	commitBtnLabel := "提交"
	switch a.commitScope {
	case "staged":
		commitBtnLabel = fmt.Sprintf("提交暂存 (%d)", stagedTotal)
	case "current":
		if a.selectedChangeFile != nil {
			commitBtnLabel = fmt.Sprintf("仅提交 %s", a.selectedChangeFile.RepoName)
		}
	default:
		if checkedFiles > 0 {
			if checkedRepos > 1 {
				commitBtnLabel = fmt.Sprintf("提交 (%d 个仓库)", checkedRepos)
			} else {
				commitBtnLabel = fmt.Sprintf("提交 (%d 个文件)", checkedFiles)
			}
		}
	}

	canCommit, commitHint := a.commitAvailability(checkedFiles, stagedTotal)
	perRepoActive := a.perRepoMessagesActive()

	ui.Column(c).Padding(10).Gap(8).Radius(8).Background(p.Surface).Border(1, p.Border).Children(func() {
		ui.TextArea(c.Key("commit-msg"), &a.commitMessage).Bind(&a.commitInputHandle).Placeholder("输入提交说明，或点「生成说明」...").Lines(2, 6).FillWidth()

		ui.Row(c).Justify(ui.SpaceBetween).AlignItems(ui.Center).Children(func() {
			ui.Row(c).Gap(6).AlignItems(ui.Center).Children(func() {
				miniBtn(c, p, "生成说明 ⌘G", func() {
					a.generateCommitMessage()
				})
				switch {
				case perRepoActive:
					badgeBox(c, fmt.Sprintf("逐仓说明 · %d", len(a.generatedCommitByRepo)), p.Green, p.GreenSoft, ui.Color{}, 10, true)
					previewLabel := "预览"
					if a.showCommitPreview {
						previewLabel = "收起预览"
					}
					miniBtn(c, p, previewLabel, func() {
						a.showCommitPreview = !a.showCommitPreview
					})
				case a.generatedCommitRoot != "" && a.commitMessage != a.generatedCommitRoot:
					badgeBox(c, "统一说明（已手动编辑）", p.Amber, p.AmberSoft, ui.Color{}, 10, false)
				case a.generatedCommitRoot != "":
					badgeBox(c, "选择已变化 · 重新生成", p.Amber, p.AmberSoft, ui.Color{}, 10, false)
				}
			})
			optToggle := ui.Row(c.Key("commit-opt-toggle")).Padding(2, 4).Gap(4).Radius(4).Cursor(ui.CursorPointer).AlignItems(ui.Center).Shrink(0)
			if optToggle.Clicked() {
				a.showCommitOptions = !a.showCommitOptions
			}
			optToggle.Children(func() {
				chev := icoChevronRight
				if a.showCommitOptions {
					chev = icoChevronDown
				}
				drawIcon(c, chev, 11, p.TextMuted)
				ui.Text(c, "更多选项").FontSize(11).TextColor(p.TextMuted).SingleLine()
			})
		})

		if perRepoActive && a.showCommitPreview {
			ui.Column(c).Padding(8).Gap(6).Radius(6).Background(p.RailBg).Children(func() {
				for _, st := range a.statuses {
					msg, ok := a.generatedCommitByRepo[st.Repo.ID]
					if !ok {
						continue
					}
					kc := c.Key("cmp-" + st.Repo.ID)
					ui.Column(kc).Gap(2).Children(func() {
						ui.Text(kc, st.Repo.Name).FontSize(10.5).FontWeight(weightSemibold).TextColor(p.Accent)
						ui.Text(kc, msg).FontSize(10.5).TextColor(p.TextSecondary).MaxLines(4)
					})
				}
			})
		}

		ui.Row(c).Wrap().Gap(6).AlignItems(ui.Center).Children(func() {
			if canCommit {
				pill(c, p, commitBtnLabel, true, func() {
					a.TriggerCommit(false)
				})
				pill(c, p, "提交并推送", false, func() {
					a.TriggerCommit(true)
				})
				ui.Text(c, "⌘Enter / ⌘⇧Enter").FontSize(10.5).TextColor(p.TextMuted).SingleLine()
			} else {
				pillDisabled(c, p, commitBtnLabel)
				pillDisabled(c, p, "提交并推送")
				ui.Text(c, "⌘Enter / ⌘⇧Enter").FontSize(10.5).TextColor(p.TextMuted).SingleLine()
			}
		})
		if !canCommit && commitHint != "" {
			ui.Text(c, commitHint).FontSize(10.5).TextColor(p.TextMuted)
		}

		if a.showCommitOptions {
			ui.Column(c).Padding(6, 0, 0, 0).Gap(6).BorderWidth(1, 0, 0, 0).BorderColor(p.BorderSubtle).Children(func() {
				ui.Row(c).Wrap().Gap(4).AlignItems(ui.Center).Children(func() {
					ui.Text(c, "常用前缀：").FontSize(11).TextColor(p.TextMuted)
					for _, pfx := range []string{"feat: ", "fix: ", "refactor: ", "chore: "} {
						pfxCopy := pfx
						miniBtn(c.Key("pfx-"+pfxCopy), p, pfxCopy, func() {
							a.commitMessage = pfxCopy + a.commitMessage
						})
					}
				})
				ui.Column(c).Gap(4).Children(func() {
					ui.Text(c, "提交范围").FontSize(11).TextColor(p.TextMuted)
					ui.Row(c).Wrap().Gap(6).Children(func() {
						pill(c, p, "所有勾选文件", a.commitScope == "checked", func() {
							a.commitScope = "checked"
						})
						pill(c, p, "仅当前仓库", a.commitScope == "current", func() {
							a.commitScope = "current"
						})
						pill(c, p, "仅已暂存", a.commitScope == "staged", func() {
							a.commitScope = "staged"
						})
					})
				})
				ui.Row(c).Wrap().Gap(10).AlignItems(ui.Center).Children(func() {
					ui.Checkbox(c, &a.commitAmend, "修正上一次提交 (--amend)")
				})
			})
		}
	})
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
		ui.Row(c).Height(42).FillWidth().Padding(0, 14).Background(p.Surface).BorderWidth(0, 0, 1, 0).BorderColor(p.Border).Justify(ui.SpaceBetween).AlignItems(ui.Center).Children(func() {
			ui.Row(c).Gap(8).AlignItems(ui.Center).Grow(1).Children(func() {
				badgeBox(c, cf.RepoName, p.Accent, p.AccentSoft, ui.Color{}, 11, true)
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

// renderedChangeFiles walks directories before files, skipping collapsed folders,
// so shift ranges and arrow keys only touch what is on screen.
func (a *App) renderedChangeFiles(repoID string, node *changeTreeNode) []gitops.ChangedFile {
	var out []gitops.ChangedFile
	for _, dir := range node.dirs {
		if a.collapsedChangeDirs[repoID+"::"+dir.relPath] {
			continue
		}
		out = append(out, a.renderedChangeFiles(repoID, dir)...)
	}
	out = append(out, node.files...)
	return out
}

// visibleChangeFilesFor returns one repository's visible files in render order.
func (a *App) visibleChangeFilesFor(repoID string) []gitops.ChangedFile {
	st, ok := a.statusByID[repoID]
	if !ok || len(st.Files) == 0 {
		return nil
	}
	files := a.filterChangeFiles(st.Repo.Name, st.Files)
	if len(files) == 0 {
		return nil
	}
	tree, ok := a.changeTreeCache[a.changeFilterKey(repoID)]
	if !ok {
		tree = buildChangeTree(files)
	}
	return a.renderedChangeFiles(repoID, tree)
}

// visibleChangeFileList flattens every visible repository tree in display order.
func (a *App) visibleChangeFileList() []gitops.ChangedFile {
	var out []gitops.ChangedFile
	for _, st := range a.statuses {
		if a.repoFilterID != "" && st.Repo.ID != a.repoFilterID {
			continue
		}
		if a.collapsedChangeRepos[st.Repo.ID] {
			continue
		}
		out = append(out, a.visibleChangeFilesFor(st.Repo.ID)...)
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
