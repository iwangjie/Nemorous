package main

import (
	"context"
	"fmt"
	"path"
	"regexp"
	"sort"
	"strings"
	"time"

	"github.com/egoist/mygo/ui"
	"github.com/egoist/mygo/ui/editor"
	"github.com/iwangjie/nemorous/internal/gitops"
	"github.com/iwangjie/nemorous/internal/mani"
)

// gitGraphLaneLimit keeps the multi-repo forest readable: hidden lanes collapse
// into the rightmost column instead of drawing connectors across the text.
const gitGraphLaneLimit = 10

type gitCommitRow struct {
	Entry   gitops.CommitEntry
	Lane    int
	Before  []int
	After   []int
	Parents []int
}

type gitRepoLog struct {
	Status  gitops.RepoStatus
	Rows    []gitCommitRow
	Fetched int
}

// gitGraphLane maps a real lane to the bounded display column.
func gitGraphLane(lane, limit int) int {
	if limit <= 0 {
		return 0
	}
	if lane < 0 {
		return 0
	}
	if lane >= limit {
		return limit - 1
	}
	return lane
}

// buildGitView is the Git management hub: commit graph first, merge workflow
// one click away, so the merge screen no longer has to be a separate world.
func (a *App) buildGitView(c *ui.Context, p Palette) {
	ui.Column(c).Fill().Children(func() {
		ui.Row(c).Height(40).Padding(0, 12).Gap(4).AlignItems(ui.Center).Background(p.RailBg).BorderWidth(0, 0, 1, 0).BorderColor(p.Border).Children(func() {
			gitTab := func(id, label string, icon *ui.SVG) {
				active := a.gitSubTab == id
				bg := ui.Transparent
				fg := p.TextSecondary
				if active {
					bg = p.AccentSoft
					fg = p.AccentSoftText
				}
				kc := c.Key("git-tab-" + id)
				el := ui.Row(kc).Padding(5, 10).Gap(5).Radius(6).Background(bg).Cursor(ui.CursorPointer).AlignItems(ui.Center)
				if el.Hovered() && !active {
					el.Background(p.SurfaceHover)
					fg = p.TextPrimary
				}
				if el.Clicked() {
					if id == "merge" {
						a.enterGitMerge()
					} else {
						a.gitSubTab = id
					}
				}
				el.Children(func() {
					drawIcon(kc, icon, 12, fg)
					ui.Text(kc, label).FontSize(11.5).FontWeight(weightMedium).TextColor(fg)
				})
			}
			gitTab("history", "提交图", icoBranch)
			gitTab("merge", "合并", icoMerge)
			ui.Spacer(c)
			if a.gitSubTab == "history" {
				ui.Text(c, fmt.Sprintf("已加载 %d 个提交 · %d 个仓库", a.gitLogTotal, len(a.gitLogTargets))).FontSize(10.5).TextColor(p.TextMuted)
			}
			closeKc := c.Key("git-close")
			closeBtn := ui.Column(closeKc).Size(24, 24).Radius(4).Center().Cursor(ui.CursorPointer).Tooltip("隐藏 Git 面板").Label("关闭 Git")
			if closeBtn.Hovered() {
				closeBtn.Background(p.SurfaceHover)
			}
			if closeBtn.Clicked() {
				a.showGitPanel = false
			}
			closeBtn.Children(func() {
				drawIcon(closeKc, icoClose, 11, p.TextMuted)
			})
		})
		if a.gitSubTab == "merge" {
			a.buildMergeView(c, p)
			return
		}
		a.ensureGitLog()
		ui.Column(c).Grow(1).Fill().Children(func() {
			a.buildGitFilterBar(c, p)
			ui.Split(c, &a.gitLeftWidth,
				func() { a.buildGitBranchTree(c, p) },
				func() {
					ui.SplitPercent(c, &a.gitCenterPct,
						func() { a.buildGitCommitList(c, p) },
						func() { a.buildGitDetails(c, p) },
					).FillHeight()
				},
			).Grow(1).Fill()
		})
	})
}

// enterGitMerge opens the merge workflow and gives the docked Git window enough
// height on small screens; history keeps the normal editor/tool-window split.
func (a *App) enterGitMerge() {
	a.showGitPanel = true
	a.gitSubTab = "merge"
	a.expandGitForMerge()
}

// expandGitForMerge only grows a short Git window, preserving a taller split
// the user chose by dragging.
func (a *App) expandGitForMerge() {
	if a.gitSubTab != "merge" {
		return
	}
	if a.gitEditorPct > 50 {
		a.gitEditorPct = 40
	}
}

func (a *App) ensureGitLog() {
	if !a.gitAuthorsLoaded && !a.gitAuthorsLoading {
		a.TriggerGitAuthors()
	}
	if !a.gitLogLoaded {
		a.TriggerGitLog()
	}
}

// TriggerGitAuthors fills the author autocomplete once per workspace.
func (a *App) TriggerGitAuthors() {
	if a.gitAuthorsLoading || a.gitAuthorsLoaded || len(a.statuses) == 0 {
		return
	}
	a.gitAuthorsLoading = true
	statuses := append([]gitops.RepoStatus(nil), a.statuses...)
	ws := a.cfg.WorkspacePath
	go func() {
		seen := make(map[string]bool)
		names := make([]string, 0, 32)
		for _, st := range statuses {
			list, _ := gitops.RepoAuthors(context.Background(), st, 500)
			for _, name := range list {
				if !seen[name] {
					seen[name] = true
					names = append(names, name)
				}
			}
		}
		sort.Strings(names)
		if len(names) > 200 {
			names = names[:200]
		}
		a.runUI(func() {
			if a.cfg.WorkspacePath != ws {
				a.gitAuthorsLoading = false
				return
			}
			a.gitAuthorsLoading = false
			a.gitAuthorsLoaded = true
			a.gitAuthorSuggestions = names
		})
	}()
}

func (a *App) gitAuthorChoices() []string {
	out := make([]string, 0, len(a.gitAuthorSuggestions)+1)
	out = append(out, "我")
	for _, name := range a.gitAuthorSuggestions {
		if name != "我" {
			out = append(out, name)
		}
	}
	return out
}

// gitPathSuggestions lists directories in the selected repository for the path
// filter's autocomplete. Paths are repo-relative, matching git log's -- path.
func (a *App) gitPathSuggestions() []string {
	repoID := a.gitFilterRepoID
	if repoID == "" {
		return nil
	}
	key := fmt.Sprintf("%s|%d|%d", repoID, len(a.workspaceFiles), a.lastFullRefresh.UnixNano())
	if a.gitPathCacheKey == key {
		return a.gitPathCache
	}
	seen := make(map[string]bool)
	dirs := make([]string, 0, 64)
	for _, wf := range a.workspaceFiles {
		if wf.RepoID != repoID {
			continue
		}
		dir := path.Dir(strings.ReplaceAll(wf.RelPath, "\\", "/"))
		for dir != "." && dir != "/" && dir != "" {
			if !seen[dir] {
				seen[dir] = true
				dirs = append(dirs, dir+"/")
			}
			dir = path.Dir(dir)
		}
	}
	sort.Strings(dirs)
	if len(dirs) > 300 {
		dirs = dirs[:300]
	}
	a.gitPathCacheKey = key
	a.gitPathCache = dirs
	return dirs
}

func (a *App) gitPathChoices() []string {
	if a.gitFilterRepoID != "" {
		return a.gitPathSuggestions()
	}
	out := make([]string, 0, len(a.statuses))
	for _, st := range a.statuses {
		out = append(out, st.Repo.Name+"/")
	}
	sort.Strings(out)
	return out
}

func (a *App) gitTopLevelDirs() []string {
	seen := make(map[string]bool)
	out := make([]string, 0, 12)
	for _, dir := range a.gitPathSuggestions() {
		trimmed := strings.TrimSuffix(dir, "/")
		if idx := strings.IndexByte(trimmed, '/'); idx >= 0 {
			trimmed = trimmed[:idx]
		}
		if trimmed == "" || seen[trimmed] {
			continue
		}
		seen[trimmed] = true
		out = append(out, trimmed+"/")
	}
	sort.Strings(out)
	return out
}

func (a *App) gitFiltersActive() bool {
	return strings.TrimSpace(a.gitFilterText) != "" || strings.TrimSpace(a.gitFilterAuthor) != "" || a.gitAuthorMe ||
		strings.TrimSpace(a.gitFilterPath) != "" || a.gitFilterRepoID != "" || a.gitFilterBranch != "" || a.gitSinceDays != 0
}

func (a *App) reloadGitLog() {
	a.gitUnifiedLimit = 80
	a.gitLogLoaded = false
	a.TriggerGitLog()
}

func (a *App) setGitRepoFilter(repoID string) {
	a.gitFilterRepoID = repoID
	a.gitFilterBranch = ""
	a.gitFilterPath = ""
	a.reloadGitLog()
}

func (a *App) setGitBranchFilter(repoID, branch string) {
	if repoID != a.gitFilterRepoID {
		a.gitFilterPath = ""
	}
	a.gitFilterRepoID = repoID
	a.gitFilterBranch = branch
	a.reloadGitLog()
}

func (a *App) clearGitFilters() {
	a.gitFilterText = ""
	a.gitFilterAuthor = ""
	a.gitAuthorMe = false
	a.gitFilterPath = ""
	a.gitFilterRepoID = ""
	a.gitFilterBranch = ""
	a.gitSinceDays = 0
	a.reloadGitLog()
}

func gitFilterPill(c *ui.Context, p Palette, key, label string, active bool, build func(m *ui.Menu)) {
	bg := p.Surface2
	fg := p.TextSecondary
	if active {
		bg = p.AccentSoft
		fg = p.AccentSoftText
	}
	kc := c.Key(key)
	el := ui.Row(kc).Padding(5, 9).Gap(4).Radius(6).Background(bg).Cursor(ui.CursorPointer).AlignItems(ui.Center).Shrink(0)
	if el.Hovered() {
		el.Background(p.SurfaceHover)
	}
	el.PopoverMenuButton(build)
	el.Children(func() {
		ui.Text(kc, label).FontSize(11).FontWeight(weightMedium).TextColor(fg).MaxLines(1)
		drawIcon(kc, icoChevronDown, 9, p.TextMuted)
	})
}

// gitIconButton is a compact icon-only toolbar action; the tooltip carries the
// label so the filter row stays scannable.
func gitIconButton(c *ui.Context, p Palette, key string, icon *ui.SVG, tooltip string, onClick func()) {
	kc := c.Key(key)
	fg := p.TextControl
	el := ui.Row(kc).Size(24, 24).Radius(5).Background(ui.Transparent).Cursor(ui.CursorPointer).
		AlignItems(ui.Center).Justify(ui.Center).Tooltip(tooltip).Label(tooltip)
	if el.Hovered() {
		el.Background(p.SurfaceHover)
		fg = p.TextPrimary
	}
	if el.Clicked() && onClick != nil {
		onClick()
	}
	el.Children(func() {
		drawIcon(kc, icon, 13, fg)
	})
}

func (a *App) buildGitFilterBar(c *ui.Context, p Palette) {
	repoLabel := "全部仓库"
	if a.gitFilterRepoID != "" {
		if st, ok := a.statusByID[a.gitFilterRepoID]; ok {
			repoLabel = st.Repo.Name
		}
	}
	branchLabel := "全部分支"
	if a.gitFilterBranch != "" {
		branchLabel = a.gitFilterBranch
	}

	ui.Row(c).Padding(8, 10).Gap(6).Wrap().Background(p.RailBg).BorderWidth(0, 0, 1, 0).BorderColor(p.Border).Children(func() {
		sf := ui.SearchField(c.Key("git-filter-q"), &a.gitFilterText).Bind(&a.gitFilterHandle).Placeholder("提交信息 / 哈希 / 正则...").Width(220)
		if sf.Changed() {
			a.reloadGitLog()
		}
		gitFilterPill(c, p, "git-repo-filter", repoLabel, a.gitFilterRepoID != "", func(m *ui.Menu) {
			if m.Item("全部仓库").Radio(a.gitFilterRepoID == "").Chosen() {
				a.setGitRepoFilter("")
			}
			for _, st := range a.statuses {
				stCopy := st
				if m.Item(stCopy.Repo.Name).Radio(a.gitFilterRepoID == stCopy.Repo.ID).Chosen() {
					a.setGitRepoFilter(stCopy.Repo.ID)
				}
			}
		})
		gitFilterPill(c, p, "git-branch-filter", branchLabel, a.gitFilterBranch != "", func(m *ui.Menu) {
			if m.Item("全部分支").Radio(a.gitFilterBranch == "").Chosen() {
				a.gitFilterBranch = ""
				a.reloadGitLog()
			}
			if a.gitFilterRepoID != "" {
				st := a.statusByID[a.gitFilterRepoID]
				for _, bi := range st.BranchInfos {
					biCopy := bi
					if m.Item(biCopy.Name).Radio(a.gitFilterBranch == biCopy.Name).Chosen() {
						a.setGitBranchFilter(a.gitFilterRepoID, biCopy.Name)
					}
				}
				return
			}
			for _, st := range a.statuses {
				stCopy := st
				m.Submenu(stCopy.Repo.Name, func(sm *ui.Menu) {
					for _, bi := range stCopy.BranchInfos {
						biCopy := bi
						if sm.Item(biCopy.Name).Radio(a.gitFilterBranch == biCopy.Name).Chosen() {
							a.setGitBranchFilter(stCopy.Repo.ID, biCopy.Name)
						}
					}
				})
			}
		})
		author := ui.Autocomplete(c.Key("git-filter-author"), &a.gitFilterAuthor, a.gitAuthorChoices()).Placeholder("作者 / 我").Width(150)
		if author.Changed() {
			a.gitAuthorMe = strings.TrimSpace(a.gitFilterAuthor) == "我"
			a.reloadGitLog()
		}
		pathIn := ui.Autocomplete(c.Key("git-filter-path"), &a.gitFilterPath, a.gitPathChoices()).Placeholder("路径 / 目录 / glob").Width(170)
		if pathIn.Changed() {
			if a.gitFilterRepoID == "" {
				chosen := strings.TrimSuffix(strings.TrimSpace(a.gitFilterPath), "/")
				for _, st := range a.statuses {
					if st.Repo.Name == chosen || st.Repo.ID == chosen {
						a.setGitRepoFilter(st.Repo.ID)
						return
					}
				}
			}
			a.reloadGitLog()
		}
		sinceLabel := "全部"
		switch a.gitSinceDays {
		case 7:
			sinceLabel = "7 天"
		case 30:
			sinceLabel = "30 天"
		case 365:
			sinceLabel = "1 年"
		}
		gitFilterPill(c, p, "git-since-filter", sinceLabel, a.gitSinceDays != 0, func(m *ui.Menu) {
			for _, opt := range []struct {
				label string
				days  int
			}{{"全部", 0}, {"7 天", 7}, {"30 天", 30}, {"1 年", 365}} {
				optCopy := opt
				if m.Item(optCopy.label).Radio(a.gitSinceDays == optCopy.days).Chosen() {
					if a.gitSinceDays != optCopy.days {
						a.gitSinceDays = optCopy.days
						a.reloadGitLog()
					}
				}
			}
		})
		gitIconButton(c, p, "git-refresh", icoRefresh, "刷新提交记录", func() {
			a.reloadGitLog()
		})
		if a.gitFiltersActive() {
			miniBtn(c.Key("git-clear"), p, "清除筛选", func() {
				a.clearGitFilters()
			})
		}
	})
}

func (a *App) buildGitBranchTree(c *ui.Context, p Palette) {
	rootName := a.cfg.WorkspacePath
	if rootName != "" {
		if idx := strings.LastIndexAny(rootName, "/\\"); idx >= 0 {
			rootName = rootName[idx+1:]
		}
	}
	if rootName == "" {
		rootName = "workspace"
	}
	ui.Column(c).FillHeight().Background(p.RailBg).Children(func() {
		ui.Scroll(c).Grow(1).Children(func() {
			ui.Row(c.Key("git-tree-root")).Height(30).Padding(0, 10).Gap(6).AlignItems(ui.Center).Children(func() {
				drawIcon(c, icoFolder, 13, p.TextControl)
				ui.Text(c, rootName).FontSize(12).FontWeight(weightSemibold).TextColor(p.TextPrimary)
				ui.Text(c, fmt.Sprintf("%d", len(a.statuses))).FontSize(10).TextColor(p.TextMuted)
			})
			for _, st := range a.statuses {
				stCopy := st
				repoCollapsed := a.gitCollapsedRepos[stCopy.Repo.ID]
				if a.gitFilterRepoID == stCopy.Repo.ID {
					repoCollapsed = false
				}
				repoSelected := a.gitFilterRepoID == stCopy.Repo.ID && a.gitFilterBranch == ""
				rkc := c.Key("git-repo-" + stCopy.Repo.ID)
				repoRow := ui.Row(rkc).Height(26).Padding(0, 10, 0, 22).Gap(5).Cursor(ui.CursorPointer).AlignItems(ui.Center)
				if repoSelected {
					repoRow.Background(p.ActiveRowBg)
				} else if repoRow.Hovered() {
					repoRow.Background(p.SurfaceHover)
				}
				if repoRow.Clicked() {
					a.gitCollapsedRepos[stCopy.Repo.ID] = !repoCollapsed
				}
				repoRow.PopoverMenu(func(m *ui.Menu) {
					if m.Item("仅看此仓库").Chosen() {
						a.setGitRepoFilter(stCopy.Repo.ID)
					}
					if m.Item("显示全部仓库").Chosen() {
						a.setGitRepoFilter("")
					}
					if m.Item("复制仓库路径").Chosen() {
						a.pendingClipboard = stCopy.Repo.AbsPath
					}
				})
				repoRow.Children(func() {
					chev := icoChevronDown
					if repoCollapsed {
						chev = icoChevronRight
					}
					drawIcon(rkc, chev, 9, p.TextMuted)
					badge := repoBadgeColors(stCopy.Repo.ColorIndex, p.IsDark)
					ui.Box(rkc).Size(15, 15).Radius(3).Background(badge.Bg).Center().Children(func() {
						ui.Text(rkc, stCopy.Repo.Monogram).FontSize(8.5).FontWeight(weightBold).TextColor(badge.Text)
					})
					ui.Text(rkc, stCopy.Repo.Name).FontSize(11.5).FontWeight(weightMedium).TextColor(p.TextPrimary).MaxLines(1).Grow(1)
					if stCopy.Branch != "" {
						ui.Text(rkc, stCopy.Branch).FontSize(9.5).TextColor(p.TextMuted).MaxLines(1).Shrink(0)
					}
				})
				if repoCollapsed {
					continue
				}
				localCount := 0
				for _, bi := range stCopy.BranchInfos {
					if !bi.IsRemote {
						localCount++
					}
				}
				if localCount > 0 {
					ui.Row(c.Key("git-local-hdr-"+stCopy.Repo.ID)).Height(20).Padding(0, 10, 0, 34).AlignItems(ui.Center).Children(func() {
						ui.Text(c, "本地").FontSize(9.5).FontWeight(weightSemibold).TextColor(p.TextMuted)
						ui.Text(c, fmt.Sprintf("%d", localCount)).FontSize(9).TextColor(p.TextMuted)
					})
				}
				for _, bi := range stCopy.BranchInfos {
					if bi.IsRemote {
						continue
					}
					biCopy := bi
					a.buildGitBranchRow(c, p, stCopy, biCopy, 34)
				}
				remoteCount := 0
				for _, bi := range stCopy.BranchInfos {
					if bi.IsRemote {
						remoteCount++
					}
				}
				if remoteCount > 0 {
					open := a.gitRemoteOpen[stCopy.Repo.ID]
					rhkc := c.Key("git-remote-hdr-" + stCopy.Repo.ID)
					rhRow := ui.Row(rhkc).Height(20).Padding(0, 10, 0, 34).Gap(4).Cursor(ui.CursorPointer).AlignItems(ui.Center)
					if rhRow.Hovered() {
						rhRow.Background(p.SurfaceHover)
					}
					if rhRow.Clicked() {
						a.gitRemoteOpen[stCopy.Repo.ID] = !open
					}
					rhRow.Children(func() {
						chev := icoChevronRight
						if open {
							chev = icoChevronDown
						}
						drawIcon(rhkc, chev, 8, p.TextMuted)
						ui.Text(rhkc, "远程").FontSize(9.5).FontWeight(weightSemibold).TextColor(p.TextMuted)
						ui.Text(rhkc, fmt.Sprintf("%d", remoteCount)).FontSize(9).TextColor(p.TextMuted)
					})
					if open {
						shown := 0
						for _, bi := range stCopy.BranchInfos {
							if !bi.IsRemote {
								continue
							}
							if shown >= 100 {
								break
							}
							shown++
							a.buildGitBranchRow(c, p, stCopy, bi, 46)
						}
					}
				}
			}
		})
	})
}

func (a *App) buildGitBranchRow(c *ui.Context, p Palette, st gitops.RepoStatus, bi gitops.BranchInfo, indent float32) {
	biCopy := bi
	selected := a.gitFilterRepoID == st.Repo.ID && a.gitFilterBranch == biCopy.Name
	kc := c.Key("git-br-" + st.Repo.ID + "-" + biCopy.Name)
	row := ui.Row(kc).Height(22).Padding(0, 10, 0, indent).Gap(4).Cursor(ui.CursorPointer).AlignItems(ui.Center)
	if selected {
		row.Background(p.AccentSoft)
	} else if row.Hovered() {
		row.Background(p.SurfaceHover)
	}
	if row.Clicked() {
		a.setGitBranchFilter(st.Repo.ID, biCopy.Name)
	}
	row.PopoverMenu(func(m *ui.Menu) {
		if !biCopy.IsRemote && st.Branch != biCopy.Name {
			if m.Item("切换到此分支").Chosen() {
				a.checkoutGitBranch(st.Repo.ID, biCopy.Name)
			}
		}
		if m.Item("查看此分支历史").Chosen() {
			a.setGitBranchFilter(st.Repo.ID, biCopy.Name)
		}
		if m.Item("复制分支名").Chosen() {
			a.pendingClipboard = biCopy.Name
		}
	})
	row.Children(func() {
		drawIcon(kc, icoBranch, 10, p.TextMuted)
		nameCol := p.TextSecondary
		if selected {
			nameCol = p.TextPrimary
		}
		ui.Text(kc, biCopy.Name).FontSize(10.8).TextColor(nameCol).MaxLines(1).Grow(1)
		if !biCopy.IsRemote && st.Branch == biCopy.Name {
			badgeBox(kc, "HEAD", p.Green, p.GreenSoft, ui.Color{}, 8.5, true)
		}
		if rel := gitRelTime(biCopy.CommitUnix); rel != "" {
			ui.Text(kc, rel).FontSize(9).TextColor(p.TextMuted).MaxLines(1)
		}
	})
}

func (a *App) buildGitCommitList(c *ui.Context, p Palette) {
	ui.Column(c).FillHeight().Background(p.RailBg).Children(func() {
		if len(a.gitLogTargets) == 0 {
			ui.Column(c).Grow(1).Center().Gap(4).Children(func() {
				ui.Text(c, "没有匹配的提交").FontSize(12).FontWeight(weightSemibold).TextColor(p.TextSecondary)
				ui.Text(c, "调整筛选条件，或刷新仓库状态").FontSize(10.5).TextColor(p.TextMuted)
			})
			return
		}
		if len(a.gitUnifiedRows) == 0 && len(a.gitGroupLoading) == 0 {
			ui.Column(c).Grow(1).Center().Gap(4).Children(func() {
				ui.Text(c, "没有匹配的提交").FontSize(12).FontWeight(weightSemibold).TextColor(p.TextSecondary)
				ui.Text(c, "调整筛选条件，或刷新仓库状态").FontSize(10.5).TextColor(p.TextMuted)
			})
			return
		}
		ui.Scroll(c).Grow(1).Children(func() {
			laneLimit := min(a.gitUnifiedMaxLanes, gitGraphLaneLimit)
			if laneLimit < 1 {
				laneLimit = 1
			}
			gutterW := float32(laneLimit*12 + 8)
			for _, row := range a.gitUnifiedRows {
				a.buildGitCommitRow(c, p, row, gutterW, laneLimit)
			}
			if len(a.gitGroupLoading) > 0 {
				ui.Row(c.Key("git-unified-loading")).Height(28).Padding(0, 16).AlignItems(ui.Center).Children(func() {
					ui.Text(c, "正在加载其余仓库的提交...").FontSize(10.5).TextColor(p.TextMuted)
				})
			}
			more := false
			for _, g := range a.gitLogGroups {
				if g.Fetched >= a.gitUnifiedLimit {
					more = true
					break
				}
			}
			if more {
				kc := c.Key("git-load-more")
				ui.Row(kc).Height(28).Padding(0, 16).AlignItems(ui.Center).Children(func() {
					label := "加载更多提交"
					if len(a.gitUnifiedRows) == 0 && strings.TrimSpace(a.gitFilterText) != "" {
						label = "继续搜索更早提交"
					}
					miniBtn(kc, p, label, func() {
						a.gitUnifiedLimit += 80
						a.TriggerGitLog()
					})
				})
			}
		})
	})
}

func (a *App) buildGitCommitRow(c *ui.Context, p Palette, row gitCommitRow, gutterW float32, laneLimit int) {
	entry := row.Entry
	key := entry.RepoID + "|" + entry.Hash
	selected := a.gitSelectedCommit == key
	kc := c.Key("git-commit-" + key)
	rowEl := ui.Row(kc).Height(28).Padding(0, 8).Gap(6).Cursor(ui.CursorPointer).AlignItems(ui.Center)
	if a.gitRevealCommit == key {
		rowEl.ScrollIntoView()
		a.gitRevealCommit = ""
	}
	if selected {
		rowEl.Background(p.ActiveRowBg)
	} else if rowEl.Hovered() {
		rowEl.Background(p.SurfaceHover)
	}
	if rowEl.Clicked() {
		a.gitAutoSelected = false
		a.TriggerGitDetail(entry)
	}
	rowEl.PopoverMenu(func(m *ui.Menu) {
		if m.Item("复制提交哈希").Chosen() {
			a.pendingClipboard = entry.Hash
		}
		if m.Item("复制提交信息").Chosen() {
			a.pendingClipboard = entry.Subject
		}
		if m.Item("查看变更文件").Chosen() {
			a.TriggerGitDetail(entry)
		}
		if m.Item("复制仓库路径").Chosen() {
			a.pendingClipboard = entry.RepoPath
		}
	})
	rowEl.Children(func() {
		a.drawGitGraphCell(c, p, key, row, gutterW, laneLimit)
		if st, ok := a.statusByID[entry.RepoID]; ok {
			badge := repoBadgeColors(st.Repo.ColorIndex, p.IsDark)
			ui.Box(kc).Size(15, 15).Radius(3).Background(badge.Bg).Center().Shrink(0).Children(func() {
				ui.Text(kc, st.Repo.Monogram).FontSize(8).FontWeight(weightBold).TextColor(badge.Text)
			})
		}
		ui.Text(kc, entry.Subject).FontSize(11.3).TextColor(p.TextPrimary).MaxLines(1).Grow(1)
		if label, fg, bg := gitPrimaryRef(entry.Refs, p); label != "" {
			badgeBox(kc, label, fg, bg, ui.Color{}, 9, false)
			if len(entry.Refs) > 1 {
				ui.Text(kc, fmt.Sprintf("+%d", len(entry.Refs)-1)).FontSize(8.5).TextColor(p.TextMuted)
			}
		}
		ui.Text(kc, gitRelTime(entry.AuthorUnix)).FontSize(9.3).TextColor(p.TextMuted).Width(62).MaxLines(1)
	})
}

func (a *App) drawGitGraphCell(c *ui.Context, p Palette, key string, row gitCommitRow, gutterW float32, laneLimit int) {
	gutter := ui.Box(c.Key("git-gutter-" + key)).Width(gutterW).Height(28).Shrink(0)
	if a.gitUnifiedMaxLanes > laneLimit {
		gutter.Tooltip(fmt.Sprintf("活跃泳道 %d 条，仅显示前 %d 条；按仓库筛选可查看完整拓扑", a.gitUnifiedMaxLanes, laneLimit))
	}
	gutter.Children(func() {
		center := float32(14)
		lane := gitGraphLane(row.Lane, laneLimit)
		for _, active := range row.Before {
			if active >= laneLimit {
				continue
			}
			ui.Box(c).Absolute().Left(float32(4 + active*12)).Top(0).Width(2).Height(center).Background(gitLaneColor(active, p))
		}
		for _, active := range row.After {
			if active >= laneLimit {
				continue
			}
			ui.Box(c).Absolute().Left(float32(4 + active*12)).Top(center).Width(2).Height(center).Background(gitLaneColor(active, p))
		}
		for _, parent := range row.Parents {
			parentLane := gitGraphLane(parent, laneLimit)
			if parentLane == lane {
				continue
			}
			x1 := float32(4 + lane*12)
			x2 := float32(4 + parentLane*12)
			if x1 > x2 {
				x1, x2 = x2, x1
			}
			ui.Box(c).Absolute().Left(x1).Top(center - 1).Width(x2 - x1).Height(2).Background(gitLaneColor(parent, p))
		}
		ui.Box(c).Absolute().Left(float32(4+lane*12-3)).Top(center-4).Size(8, 8).Radius(4).Background(gitLaneColor(row.Lane, p))
	})
}

func gitLaneColor(lane int, p Palette) ui.Color {
	colors := [...]ui.Color{p.Blue, p.Green, p.Amber, p.Purple, p.Danger, p.Accent}
	return colors[lane%len(colors)]
}

func gitPrimaryRef(refs []string, p Palette) (string, ui.Color, ui.Color) {
	for _, ref := range refs {
		if strings.HasPrefix(ref, "HEAD -> ") {
			return strings.TrimPrefix(ref, "HEAD -> "), p.AccentSoftText, p.AccentSoft
		}
	}
	for _, ref := range refs {
		if strings.HasPrefix(ref, "tag: ") {
			return strings.TrimPrefix(ref, "tag: "), p.Amber, p.AmberSoft
		}
	}
	for _, ref := range refs {
		if !strings.Contains(ref, "/") {
			return ref, p.AccentSoftText, p.AccentSoft
		}
	}
	if len(refs) > 0 {
		return refs[0], p.TextMuted, p.Surface2
	}
	return "", ui.Color{}, ui.Color{}
}

func gitRelTime(unix int64) string {
	if unix <= 0 {
		return ""
	}
	t := time.Unix(unix, 0)
	d := time.Since(t)
	switch {
	case d < time.Minute:
		return "刚刚"
	case d < time.Hour:
		return fmt.Sprintf("%d 分钟前", int(d.Minutes()))
	case d < 24*time.Hour:
		return fmt.Sprintf("%d 小时前", int(d.Hours()))
	case d < 30*24*time.Hour:
		return fmt.Sprintf("%d 天前", int(d.Hours()/24))
	default:
		return t.Format("2006-01-02")
	}
}

func (a *App) buildGitDetails(c *ui.Context, p Palette) {
	ui.Column(c).FillHeight().Background(p.RailBg).Children(func() {
		if a.gitDetail == nil && !a.gitDetailLoading {
			ui.Column(c).Grow(1).Center().Gap(4).Children(func() {
				ui.Text(c, "选择提交查看详情").FontSize(12).FontWeight(weightSemibold).TextColor(p.TextSecondary)
				ui.Text(c, "提交信息、作者与变更文件会显示在这里").FontSize(10.5).TextColor(p.TextMuted)
			})
			return
		}
		ui.Scroll(c).Grow(1).Children(func() {
			if a.gitDetailLoading {
				ui.Row(c).Padding(14).Children(func() {
					ui.Text(c, "正在加载提交详情...").FontSize(10.5).TextColor(p.TextMuted)
				})
				return
			}
			d := a.gitDetail
			ui.Column(c).Padding(14).Gap(8).Children(func() {
				ui.Row(c).Gap(6).Wrap().AlignItems(ui.Center).Children(func() {
					badgeBox(c, d.RepoName, p.AccentSoftText, p.AccentSoft, ui.Color{}, 9.5, true)
					ui.Text(c, d.ShortHash).Font("SF Mono, Menlo, monospace").FontSize(11).FontWeight(weightSemibold).TextColor(p.AccentSoftText)
					gitIconButton(c, p, "git-copy-detail", icoCopy, "复制提交哈希", func() {
						a.pendingClipboard = d.Hash
					})
				})
				ui.Text(c, d.Subject).FontSize(13).FontWeight(weightBold).TextColor(p.TextPrimary)
				if d.Body != "" {
					ui.Text(c, d.Body).FontSize(10.5).TextColor(p.TextSecondary).MaxLines(8)
				}
				ui.Row(c).Gap(6).Wrap().Children(func() {
					ui.Text(c, d.Author).FontSize(10.5).TextColor(p.TextSecondary)
					ui.Text(c, "<"+d.AuthorEmail+">").FontSize(9.5).TextColor(p.TextMuted)
					ui.Text(c, time.Unix(d.AuthorUnix, 0).Format("2006-01-02 15:04")).FontSize(9.5).TextColor(p.TextMuted)
				})
				if len(d.Refs) > 0 {
					ui.Row(c).Gap(4).Wrap().Children(func() {
						for _, ref := range d.Refs {
							label, fg, bg := gitPrimaryRef([]string{ref}, p)
							if label != "" {
								badgeBox(c, label, fg, bg, ui.Color{}, 9, false)
							}
						}
					})
				}
				ui.Text(c, fmt.Sprintf("变更文件 (%d)", len(d.Files))).FontSize(11).FontWeight(weightSemibold).TextColor(p.TextMuted)
				if len(d.Files) > 0 {
					a.renderGitCommitTree(c, p, *d, buildGitCommitTree(d.Files), 0)
				}
			})
		})
	})
}

// gitTargetStatuses applies repository/branch filters without loading any
// commit data, so the tree can stay lazy.
func (a *App) gitTargetStatuses() []gitops.RepoStatus {
	repoID := a.gitFilterRepoID
	branch := a.gitFilterBranch
	statuses := make([]gitops.RepoStatus, 0, len(a.statuses))
	for _, st := range a.statuses {
		if repoID != "" && st.Repo.ID != repoID {
			continue
		}
		if branch != "" {
			has := false
			for _, bi := range st.BranchInfos {
				if bi.Name == branch {
					has = true
					break
				}
			}
			if !has {
				continue
			}
		}
		statuses = append(statuses, st)
	}
	sort.Slice(statuses, func(i, j int) bool {
		return strings.ToLower(statuses[i].Repo.Name) < strings.ToLower(statuses[j].Repo.Name)
	})
	return statuses
}

// TriggerGitLog starts one page for every target repository and merges them
// into a single forest: all projects share one commit list, while lane keys are
// repo-qualified so histories never connect across repositories.
func (a *App) TriggerGitLog() {
	if a.cfg.WorkspacePath == "" {
		return
	}
	a.gitLogSeq++
	a.gitLogGroups = nil
	a.gitGroupLoading = make(map[string]bool)
	a.gitUnifiedRows = nil
	a.gitUnifiedMaxLanes = 0
	a.gitLogTotal = 0
	a.gitLogLoaded = true
	a.gitSelectedCommit = ""
	a.gitAutoSelected = false
	a.gitRevealCommit = ""
	a.gitPinnedCommit = nil
	a.gitDetail = nil
	a.gitDetailLoading = false
	a.gitLogTargets = a.gitTargetStatuses()
	if len(a.gitLogTargets) == 0 {
		return
	}
	for _, st := range a.gitLogTargets {
		a.TriggerGitRepoLog(st.Repo.ID)
	}
}

// TriggerGitRepoLog loads one repository's page of commits.
func (a *App) TriggerGitRepoLog(repoID string) {
	if a.gitGroupLoading[repoID] {
		return
	}
	var target *gitops.RepoStatus
	for i := range a.gitLogTargets {
		if a.gitLogTargets[i].Repo.ID == repoID {
			target = &a.gitLogTargets[i]
			break
		}
	}
	if target == nil {
		return
	}
	limit := a.gitUnifiedLimit
	if limit <= 0 {
		limit = 80
	}
	a.gitGroupLoading[repoID] = true
	gen := a.gitLogSeq
	text := strings.TrimSpace(a.gitFilterText)
	lowerText := strings.ToLower(text)
	var pattern *regexp.Regexp
	if text != "" {
		if compiled, err := regexp.Compile("(?i)" + text); err == nil {
			pattern = compiled
		}
	}
	authorText := strings.TrimSpace(a.gitFilterAuthor)
	if a.gitAuthorMe {
		authorText = ""
	}
	opts := gitops.LogOptions{
		Branch:    a.gitFilterBranch,
		Author:    authorText,
		AuthorMe:  a.gitAuthorMe,
		Path:      strings.TrimSpace(a.gitFilterPath),
		SinceDays: a.gitSinceDays,
		Limit:     limit,
	}
	stCopy := *target
	go func() {
		commits, err := gitops.RepoLog(context.Background(), stCopy, opts)
		fetched := len(commits)
		if err == nil && text != "" {
			filtered := commits[:0]
			for _, c := range commits {
				hay := c.Subject + "\n" + c.Body + "\n" + c.Hash + "\n" + c.Author + "\n" + strings.Join(c.Refs, " ")
				match := false
				if pattern != nil {
					match = pattern.MatchString(hay)
				} else {
					match = strings.Contains(strings.ToLower(hay), lowerText)
				}
				if match {
					filtered = append(filtered, c)
				}
			}
			commits = filtered
		}
		if err != nil {
			commits = nil
			fetched = 0
		}
		rows, _ := buildGitRows(commits)
		a.runUI(func() {
			if gen != a.gitLogSeq {
				return
			}
			delete(a.gitGroupLoading, repoID)
			group := gitRepoLog{Status: stCopy, Rows: rows, Fetched: fetched}
			replaced := false
			for i := range a.gitLogGroups {
				if a.gitLogGroups[i].Status.Repo.ID == repoID {
					a.gitLogGroups[i] = group
					replaced = true
					break
				}
			}
			if !replaced {
				a.gitLogGroups = append(a.gitLogGroups, group)
			}
			a.rebuildGitUnified()
			delete(a.gitGroupLoading, repoID)
		})
	}()
}

// rebuildGitUnified merges every loaded repo page by commit time, then recomputes
// the forest lanes over the interleaved rows.
func (a *App) rebuildGitUnified() {
	groups := append([]gitRepoLog(nil), a.gitLogGroups...)
	sort.Slice(groups, func(i, j int) bool {
		return strings.ToLower(groups[i].Status.Repo.Name) < strings.ToLower(groups[j].Status.Repo.Name)
	})
	idxs := make([]int, len(groups))
	entries := make([]gitops.CommitEntry, 0, a.gitLogTotal)
	for {
		best := -1
		var bestTime int64
		for i, g := range groups {
			if idxs[i] >= len(g.Rows) {
				continue
			}
			e := g.Rows[idxs[i]].Entry
			t := gitCommitTime(e)
			if best < 0 || t > bestTime || (t == bestTime && e.RepoName < groups[best].Status.Repo.Name) {
				best = i
				bestTime = t
			}
		}
		if best < 0 {
			break
		}
		entries = append(entries, groups[best].Rows[idxs[best]].Entry)
		idxs[best]++
	}
	if pinned := a.gitPinnedCommit; pinned != nil && !gitLogContains(entries, pinned.RepoID+"|"+pinned.Hash) {
		t := gitCommitTime(*pinned)
		pos := sort.Search(len(entries), func(i int) bool { return gitCommitTime(entries[i]) < t })
		entries = append(entries, gitops.CommitEntry{})
		copy(entries[pos+1:], entries[pos:])
		entries[pos] = *pinned
	}
	rows, maxLanes := buildGitRows(entries)
	a.gitUnifiedRows = rows
	a.gitUnifiedMaxLanes = maxLanes
	a.gitLogTotal = len(rows)
	if a.gitSelectedCommit != "" && !a.gitAutoSelected {
		if a.findGitCommit(a.gitSelectedCommit) || a.gitRevealCommit == a.gitSelectedCommit {
			return
		}
	}
	a.gitSelectedCommit = ""
	a.gitDetail = nil
	if len(rows) > 0 {
		a.gitAutoSelected = true
		a.TriggerGitDetail(rows[0].Entry)
	}
}

func gitCommitTime(e gitops.CommitEntry) int64 {
	if e.CommitterUnix != 0 {
		return e.CommitterUnix
	}
	return e.AuthorUnix
}

func gitLogContains(entries []gitops.CommitEntry, key string) bool {
	for _, e := range entries {
		if e.RepoID+"|"+e.Hash == key {
			return true
		}
	}
	return false
}

func (a *App) findGitCommit(key string) bool {
	for _, row := range a.gitUnifiedRows {
		if row.Entry.RepoID+"|"+row.Entry.Hash == key {
			return true
		}
	}
	return false
}

func buildGitRows(commits []gitops.CommitEntry) ([]gitCommitRow, int) {
	rows := make([]gitCommitRow, 0, len(commits))
	active := make([]string, 0, 8)
	maxLanes := 0
	for _, c := range commits {
		commitKey := c.RepoID + "\x00" + c.Hash
		lane := -1
		for i, h := range active {
			if h == commitKey {
				lane = i
				break
			}
		}
		existed := lane >= 0
		var before []int
		if existed {
			before = activeLanes(active)
		} else {
			before = activeLanes(active)
			lane = firstFreeLane(active)
			if lane == len(active) {
				active = append(active, "")
			}
		}
		active[lane] = commitKey

		active[lane] = ""
		parents := make([]int, 0, len(c.Parents))
		for pi, parent := range c.Parents {
			parentKey := c.RepoID + "\x00" + parent
			pl := -1
			for i, h := range active {
				if h == parentKey {
					pl = i
					break
				}
			}
			if pl < 0 {
				if pi == 0 {
					pl = lane
				} else {
					pl = firstFreeLane(active)
					if pl == len(active) {
						active = append(active, "")
					}
				}
				active[pl] = parentKey
			}
			parents = append(parents, pl)
		}
		after := activeLanes(active)
		if len(active) > maxLanes {
			maxLanes = len(active)
		}
		rows = append(rows, gitCommitRow{Entry: c, Lane: lane, Before: before, After: after, Parents: parents})
	}
	return rows, maxLanes
}

func activeLanes(active []string) []int {
	out := make([]int, 0, len(active))
	for i, h := range active {
		if h != "" {
			out = append(out, i)
		}
	}
	return out
}

func firstFreeLane(active []string) int {
	for i, h := range active {
		if h == "" {
			return i
		}
	}
	return len(active)
}

// openGitCommit reveals one commit in the Git history: it clears filters that
// could hide the commit, pins it into the list even when it is older than the
// current page, and loads its detail pane.
func (a *App) openGitCommit(entry gitops.CommitEntry) {
	if entry.RepoID == "" || entry.Hash == "" {
		return
	}
	// Prefer the richer entry already in the log (email, refs, body) over the
	// minimal metadata carried by a blame line.
	key := entry.RepoID + "|" + entry.Hash
	for _, row := range a.gitUnifiedRows {
		if row.Entry.RepoID+"|"+row.Entry.Hash == key {
			entry = row.Entry
			break
		}
	}
	a.showGitPanel = true
	a.showLogPanel = false
	a.showTerminalPanel = false
	a.gitSubTab = "history"
	a.gitFilterRepoID = entry.RepoID
	a.gitFilterBranch = ""
	a.gitFilterText = ""
	a.gitFilterAuthor = ""
	a.gitAuthorMe = false
	a.gitFilterPath = ""
	a.gitSinceDays = 0
	a.gitAutoSelected = false
	a.TriggerGitLog()
	pinned := entry
	a.gitPinnedCommit = &pinned
	a.gitSelectedCommit = key
	a.gitRevealCommit = key
	a.TriggerGitDetail(entry)
	a.invalidateUI()
}

func (a *App) TriggerGitDetail(entry gitops.CommitEntry) {
	key := entry.RepoID + "|" + entry.Hash
	if a.gitSelectedCommit == key && (a.gitDetail != nil || a.gitDetailLoading) {
		return
	}
	a.gitSelectedCommit = key
	a.gitDetailSeq++
	seq := a.gitDetailSeq
	a.gitDetailLoading = true
	a.gitDetail = nil
	go func() {
		files, err := gitops.CommitFiles(context.Background(), entry.RepoPath, entry.Hash)
		if err != nil {
			files = nil
		}
		a.runUI(func() {
			if seq != a.gitDetailSeq || a.gitSelectedCommit != key {
				return
			}
			a.gitDetailLoading = false
			a.gitDetail = &gitops.CommitDetail{CommitEntry: entry, Files: files}
		})
	}()
}

func (a *App) TriggerGitFileDiff(detail gitops.CommitDetail, file gitops.CommitFile) {
	a.gitDiffOpen = true
	a.gitDiffLoading = true
	a.gitDiffSeq++
	seq := a.gitDiffSeq
	a.gitDiffRepo = detail.RepoName
	a.gitDiffPath = file.Path
	a.gitDiffOldPath = file.OldPath
	a.gitDiffHash = detail.Hash
	a.gitDiffErr = ""
	a.gitCommitDiff = nil
	layout := diffLayout(a.cfg.DiffSideBySide)
	inline := parseInlineMode(a.cfg.DiffInlineMode)
	wrap := a.cfg.EditorSoftWrap
	font := ui.Font{Size: a.cfg.EditorFontSize}
	go func() {
		oldText, newText, err := gitops.CommitFileVersions(context.Background(), detail.RepoPath, detail.Hash, file.Path, file.OldPath, file.Status)
		a.runUI(func() {
			if seq != a.gitDiffSeq || !a.gitDiffOpen {
				return
			}
			a.gitDiffLoading = false
			if err != nil {
				a.gitDiffErr = err.Error()
				return
			}
			lang := editor.LanguageFor(file.Path)
			oldBuf := editor.NewBuffer(oldText)
			newBuf := editor.NewBuffer(newText)
			oldBuf.SetLanguage(lang)
			newBuf.SetLanguage(lang)
			a.gitCommitDiff = &editor.DiffState{
				Old:           oldBuf,
				New:           newBuf,
				Layout:        layout,
				Inline:        inline,
				Wrap:          wrap,
				Editable:      false,
				HunkActions:   false,
				LineSelection: true,
				Font:          font,
			}
		})
	}()
}

func (a *App) closeGitCommitDiff() {
	a.gitDiffOpen = false
	a.gitDiffLoading = false
	a.gitCommitDiff = nil
	a.gitDiffErr = ""
}

func (a *App) buildGitCommitDiffPane(c *ui.Context, p Palette) {
	ui.Column(c).Grow(1).Fill().Background(p.AppBg).Children(func() {
		ui.Row(c).Height(44).FillWidth().Padding(0, 14).Background(p.RailBg).BorderWidth(0, 0, 1, 0).BorderColor(p.Border).Justify(ui.SpaceBetween).AlignItems(ui.Center).Children(func() {
			ui.Row(c).Gap(8).AlignItems(ui.Center).Grow(1).Children(func() {
				miniBtn(c, p, "← 返回提交图", func() {
					a.closeGitCommitDiff()
				})
				badgeBox(c, a.gitDiffRepo, p.AccentSoftText, p.AccentSoft, ui.Color{}, 10.5, true)
				pathLabel := a.gitDiffPath
				if a.gitDiffOldPath != "" && a.gitDiffOldPath != a.gitDiffPath {
					pathLabel = a.gitDiffOldPath + " → " + a.gitDiffPath
				}
				ui.Text(c, pathLabel).FontSize(12.5).FontWeight(weightSemibold).TextColor(p.TextPrimary).MaxLines(1).Grow(1)
				short := a.gitDiffHash
				if len(short) > 8 {
					short = short[:8]
				}
				ui.Text(c, short).Font("SF Mono, Menlo, monospace").FontSize(10.5).TextColor(p.TextMuted).Shrink(0)
				gitIconButton(c, p, "git-copy-diff", icoCopy, "复制提交哈希", func() {
					a.pendingClipboard = a.gitDiffHash
				})
			})
			ui.Row(c).Gap(6).AlignItems(ui.Center).Shrink(0).Children(func() {
				ui.Row(c).Padding(2).Gap(2).Radius(6).Background(p.Surface3).Children(func() {
					layoutBtn := func(sideBySide bool, label string) {
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
						el := ui.Row(c.Key("git-dlayout-"+label)).Padding(3, 8).Radius(4).Background(bg).Cursor(ui.CursorPointer)
						if el.Clicked() {
							a.cfg.DiffSideBySide = sideBySide
							if a.gitCommitDiff != nil {
								a.gitCommitDiff.Layout = diffLayout(sideBySide)
							}
							if a.diffState != nil {
								a.diffState.Layout = diffLayout(sideBySide)
							}
						}
						el.Children(func() {
							ui.Text(c, label).FontSize(11.5).FontWeight(weightMedium).TextColor(fg)
						})
					}
					layoutBtn(true, "双栏")
					layoutBtn(false, "单栏")
				})
				ghostBtn(c, p, icoMore, "", a.showDiffOptions, func() {
					a.showDiffOptions = !a.showDiffOptions
				})
			})
		})
		if a.showDiffOptions {
			ui.Row(c).Padding(6, 14).Gap(10).Background(p.RailBg).BorderWidth(0, 0, 1, 0).BorderColor(p.Border).AlignItems(ui.Center).Children(func() {
				ui.Text(c, "显示选项：").FontSize(11.5).TextColor(p.TextMuted)
				wrapLabel := "自动换行：关"
				if a.cfg.EditorSoftWrap {
					wrapLabel = "自动换行：开"
				}
				miniBtn(c, p, wrapLabel, func() {
					a.cfg.EditorSoftWrap = !a.cfg.EditorSoftWrap
					if a.gitCommitDiff != nil {
						a.gitCommitDiff.Wrap = a.cfg.EditorSoftWrap
					}
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
					if a.gitCommitDiff != nil {
						a.gitCommitDiff.Inline = parseInlineMode(a.cfg.DiffInlineMode)
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
					if a.gitCommitDiff != nil {
						a.gitCommitDiff.Font = ui.Font{Size: a.cfg.EditorFontSize}
					}
					if a.diffState != nil {
						a.diffState.Font = ui.Font{Size: a.cfg.EditorFontSize}
					}
				})
			})
		}
		switch {
		case a.gitDiffLoading:
			ui.Column(c).Grow(1).Center().Gap(4).Children(func() {
				ui.Text(c, "正在加载提交差异...").FontSize(12).TextColor(p.TextMuted)
			})
		case a.gitDiffErr != "":
			ui.Column(c).Grow(1).Center().Gap(4).Children(func() {
				ui.Text(c, "无法显示差异").FontSize(12).FontWeight(weightSemibold).TextColor(p.Danger)
				ui.Text(c, a.gitDiffErr).FontSize(10.5).TextColor(p.TextMuted).MaxLines(3)
			})
		case a.gitCommitDiff != nil:
			editor.DiffView(c, a.gitCommitDiff).Grow(1).FillWidth()
		}
	})
}

func (a *App) checkoutGitBranch(repoID, branch string) {
	st, ok := a.statusByID[repoID]
	if !ok || branch == "" || st.Branch == branch {
		return
	}
	repo := st.Repo
	go func() {
		res := gitops.SwitchBranchAll(context.Background(), []mani.DiscoveredRepo{repo}, branch, false, 1)
		a.runUI(func() {
			if len(res) > 0 && res[0].Status == "OK" {
				a.showToast("ok", fmt.Sprintf("已切换 %s 到 %s", repo.Name, branch))
			} else if len(res) > 0 {
				a.showToast("warn", res[0].Message)
			}
			a.RefreshAsync()
			a.reloadGitLog()
		})
	}()
}
