package main

import (
	"context"
	"fmt"

	"github.com/egoist/mygo/ui"
	"github.com/iwangjie/nemorous/internal/gitops"
	"github.com/iwangjie/nemorous/internal/mani"
)

func (a *App) buildOverviewView(c *ui.Context, p Palette) {
	domBranch, alignedCnt, totalChild := a.DominantBranch()
	dirtyRepos, _, aheadRepos, behindRepos, _ := a.SummaryCounts()

	var filtered []gitops.RepoStatus
	for _, st := range a.statuses {
		if a.repoFilterID != "" && st.Repo.ID != a.repoFilterID {
			continue
		}
		switch a.overviewFilter {
		case "dirty":
			if !st.Dirty() {
				continue
			}
		case "ahead":
			if st.Ahead == 0 {
				continue
			}
		case "behind":
			if st.Behind == 0 {
				continue
			}
		case "unaligned":
			if st.Branch == domBranch {
				continue
			}
		}
		filtered = append(filtered, st)
	}

	var selectedStatus *gitops.RepoStatus
	for i := range a.statuses {
		if a.statuses[i].Repo.ID == a.selectedRepoID {
			selectedStatus = &a.statuses[i]
			break
		}
	}

	ui.Row(c).Grow(1).Fill().AlignItems(ui.Stretch).Children(func() {
		// Main Repository Table Canvas
		ui.Column(c).Grow(1).FillHeight().Padding(18, 22).Gap(12).Background(p.AppBg).Children(func() {
			// Top Filter & Action Bar
			a.buildOverviewFilterBar(c, p, len(a.statuses), dirtyRepos, aheadRepos, behindRepos, totalChild-alignedCnt, domBranch)

			// Elevated Repository Card Table
			a.buildRepoMatrixTable(c, p, filtered, domBranch)

			// Collapsible Operation Result Notice (only shown right after a batch operation)
			a.buildBatchResultBanner(c, p)
		})

		// Progressive Disclosure: Repository Detail Drawer opens only when toggled
		if a.showRepoDetailDrawer && selectedStatus != nil {
			a.buildRightInspector(c, p, selectedStatus)
		}
	})
}

func (a *App) buildOverviewFilterBar(c *ui.Context, p Palette, total, dirty, ahead, behind, unaligned int, domBranch string) {
	fChip := func(key, label string, count int) {
		active := a.overviewFilter == key
		bg := p.Surface2
		fg := p.TextControl
		weight := weightMedium
		if active {
			bg = p.AccentSoft
			fg = p.Accent
			weight = weightSemibold
		}
		kc := c.Key("oflt-" + key)
		el := ui.Row(kc).Padding(5, 11).Gap(5).Radius(6).Background(bg).Cursor(ui.CursorPointer).AlignItems(ui.Center)
		if el.Hovered() && !active {
			el.Background(p.SurfaceHover)
		}
		if el.Clicked() {
			a.overviewFilter = key
		}
		el.Children(func() {
			ui.Text(kc, label).FontSize(12).FontWeight(weight).TextColor(fg)
			if count > 0 || key == "all" {
				ui.Text(kc, fmt.Sprintf("%d", count)).FontSize(11).TextColor(p.TextMuted)
			}
		})
	}

	ui.Row(c).Justify(ui.SpaceBetween).AlignItems(ui.Center).Children(func() {
		ui.Row(c).Gap(6).AlignItems(ui.Center).Children(func() {
			fChip("all", "全部仓库", total)
			fChip("dirty", "有改动", dirty)
			if ahead > 0 {
				fChip("ahead", "待推送", ahead)
			}
			if behind > 0 {
				fChip("behind", "待拉取", behind)
			}
			if unaligned > 0 {
				fChip("unaligned", "分支不一致", unaligned)
			}
		})

		ui.Row(c).Gap(8).AlignItems(ui.Center).Children(func() {
			if unaligned > 0 {
				pill(c, p, fmt.Sprintf("统一切换到 %s", domBranch), false, func() {
					a.TriggerBatchSwitch(domBranch, false)
				})
			}
			iconPill(c, p, icoBranch, "切换分支 (⌘B)", false, func() {
				a.openBranchSwitchModal()
			})
			iconPill(c, p, icoMerge, "合并分支...", false, func() {
				a.activeTab = TabMerge
				a.TriggerMergePreflight(false)
			})
		})
	})
}

func (a *App) buildRepoMatrixTable(c *ui.Context, p Palette, list []gitops.RepoStatus, domBranch string) {
	ui.Column(c).Grow(1).FillWidth().Radius(10).Background(p.Surface).Border(1, p.Border).Children(func() {
		// Table Header
		ui.Row(c).Padding(10, 16).Background(p.RailBg).BorderWidth(0, 0, 1, 0).BorderColor(p.Border).AlignItems(ui.Center).Children(func() {
			ui.Box(c).Width(250).Children(func() {
				ui.Text(c, "仓库").FontSize(11.5).FontWeight(weightSemibold).TextColor(p.TextMuted)
			})
			ui.Box(c).Width(200).Children(func() {
				ui.Text(c, "当前分支").FontSize(11.5).FontWeight(weightSemibold).TextColor(p.TextMuted)
			})
			ui.Box(c).Width(130).Children(func() {
				ui.Text(c, "本地变更").FontSize(11.5).FontWeight(weightSemibold).TextColor(p.TextMuted)
			})
			ui.Box(c).Width(120).Children(func() {
				ui.Text(c, "远端同步").FontSize(11.5).FontWeight(weightSemibold).TextColor(p.TextMuted)
			})
			ui.Box(c).Grow(1).Children(func() {
				ui.Text(c, "最近提交").FontSize(11.5).FontWeight(weightSemibold).TextColor(p.TextMuted)
			})
			ui.Box(c).Width(170).AlignItems(ui.End).Children(func() {
				ui.Text(c, "操作").FontSize(11.5).FontWeight(weightSemibold).TextColor(p.TextMuted)
			})
		})

		ui.Scroll(c).Grow(1).Children(func() {
			if len(list) == 0 {
				ui.Column(c).Padding(36).Center().Gap(6).Children(func() {
					ui.Text(c, "没有符合筛选条件的仓库").FontSize(13).FontWeight(weightSemibold).TextColor(p.TextSecondary)
					miniBtn(c, p, "显示全部仓库", func() {
						a.overviewFilter = "all"
						a.repoFilterID = ""
					})
				})
				return
			}

			for idx, st := range list {
				stCopy := st
				selected := a.selectedRepoID == st.Repo.ID && a.showRepoDetailDrawer
				badge := repoBadgeColors(st.Repo.ColorIndex, p.IsDark)
				rowBg := p.Surface
				if selected {
					rowBg = p.ActiveRowBg
				}

				kc := c.Key("row-" + st.Repo.ID)
				row := ui.Row(kc).Padding(11, 16).Background(rowBg).Cursor(ui.CursorPointer).AlignItems(ui.Center)
				if idx < len(list)-1 {
					row.BorderWidth(0, 0, 1, 0).BorderColor(p.BorderSubtle)
				}
				if row.Hovered() && !selected {
					row.Background(p.RailBg)
				}
				if row.Clicked() {
					if a.selectedRepoID == stCopy.Repo.ID && a.showRepoDetailDrawer {
						a.showRepoDetailDrawer = false
					} else {
						a.selectedRepoID = stCopy.Repo.ID
						a.showRepoDetailDrawer = true
					}
				}
				row.Children(func() {
					// 1. Repo Name & Description
					ui.Row(kc).Width(250).Gap(10).AlignItems(ui.Center).Shrink(0).Children(func() {
						ui.Box(kc).Size(28, 28).Radius(6).Background(badge.Bg).Center().Shrink(0).Children(func() {
							ui.Text(kc, stCopy.Repo.Monogram).FontSize(11).FontWeight(weightBold).TextColor(badge.Text)
						})
						ui.Column(kc).Grow(1).Gap(2).Children(func() {
							ui.Text(kc, stCopy.Repo.Name).FontSize(13).FontWeight(weightSemibold).TextColor(p.TextPrimary).MaxLines(1)
							if stCopy.Repo.Description != "" && stCopy.Repo.Description != stCopy.Repo.Name && stCopy.Repo.Description != "." {
								ui.Text(kc, stCopy.Repo.Description).FontSize(11).TextColor(p.TextMuted).MaxLines(1)
							}
						})
					})

					// 2. Branch
					ui.Row(kc).Width(200).Gap(6).AlignItems(ui.Center).Shrink(0).Children(func() {
						bCol := p.TextPrimary
						bBg := p.Surface2
						if stCopy.Branch != domBranch && !stCopy.Repo.IsRoot {
							bCol = p.Amber
							bBg = p.AmberSoft
						}
						badgeBox(kc, stCopy.Branch, bCol, bBg, ui.Color{}, 11.5, true)
					})

					// 3. Working Tree Changes
					ui.Row(kc).Width(130).Gap(6).AlignItems(ui.Center).Shrink(0).Children(func() {
						if stCopy.ConflictCount > 0 {
							badgeBox(kc, fmt.Sprintf("%d 个冲突", stCopy.ConflictCount), p.Danger, p.DangerSoft, ui.Color{}, 11, true)
						} else if len(stCopy.Files) > 0 {
							badgeBox(kc, fmt.Sprintf("%d 个文件改动", len(stCopy.Files)), p.Amber, p.AmberSoft, ui.Color{}, 11, true)
						} else {
							ui.Text(kc, "无改动").FontSize(12).TextColor(p.TextMuted)
						}
					})

					// 4. Remote Sync
					ui.Row(kc).Width(120).Gap(6).AlignItems(ui.Center).Shrink(0).Children(func() {
						if stCopy.Ahead == 0 && stCopy.Behind == 0 {
							ui.Text(kc, "已同步").FontSize(12).TextColor(p.TextMuted)
						} else {
							if stCopy.Ahead > 0 {
								badgeBox(kc, fmt.Sprintf("↑%d", stCopy.Ahead), p.Accent, p.AccentSoft, ui.Color{}, 11, true)
							}
							if stCopy.Behind > 0 {
								badgeBox(kc, fmt.Sprintf("↓%d", stCopy.Behind), p.Amber, p.AmberSoft, ui.Color{}, 11, true)
							}
						}
					})

					// 5. Last Commit
					ui.Column(kc).Grow(1).Gap(2).Padding(0, 10, 0, 0).Children(func() {
						if len(stCopy.RecentCommits) > 0 {
							cm := stCopy.RecentCommits[0]
							ui.Text(kc, cm.Subject).FontSize(12).TextColor(p.TextSecondary).MaxLines(1)
							ui.Text(kc, fmt.Sprintf("%s · %s", cm.Hash, cm.RelTime)).FontSize(10.5).TextColor(p.TextMuted).MaxLines(1)
						} else {
							ui.Text(kc, "暂无提交").FontSize(12).TextColor(p.TextMuted)
						}
					})

					// 6. Quick Actions
					ui.Row(kc).Width(170).Justify(ui.End).Gap(6).Shrink(0).Children(func() {
						if len(stCopy.Files) > 0 {
							miniBtn(kc, p, "查看改动", func() {
								a.repoFilterID = stCopy.Repo.ID
								a.selectChangedFile(stCopy.Files[0])
								a.activeTab = TabChanges
							})
						}
						miniBtn(kc, p, "浏览文件", func() {
							a.repoFilterID = stCopy.Repo.ID
							a.activeTab = TabCode
						})
						miniBtn(kc, p, "详情", func() {
							a.selectedRepoID = stCopy.Repo.ID
							a.showRepoDetailDrawer = true
						})
					})
				})
			}
		})
	})
}

func (a *App) buildBatchResultBanner(c *ui.Context, p Palette) {
	if len(a.lastBatchResults) == 0 {
		return
	}
	okCnt, skipCnt, failCnt := countOpResults(a.lastBatchResults)
	ui.Column(c).Padding(10, 14).Gap(6).Radius(8).Background(p.Surface).Border(1, p.Border).Children(func() {
		ui.Row(c).Justify(ui.SpaceBetween).AlignItems(ui.Center).Children(func() {
			ui.Text(c, fmt.Sprintf("操作结果：%d 成功 · %d 跳过 · %d 失败", okCnt, skipCnt, failCnt)).
				FontSize(12).FontWeight(weightSemibold).TextColor(p.TextPrimary)
			miniBtn(c, p, "收起", func() {
				a.lastBatchResults = nil
			})
		})
		ui.Row(c).Wrap().Gap(6).Children(func() {
			for _, r := range a.lastBatchResults {
				rCopy := r
				col := p.Green
				bg := p.GreenSoft
				if rCopy.Status == "SKIP" {
					col = p.Amber
					bg = p.AmberSoft
				} else if rCopy.Status == "FAIL" {
					col = p.Danger
					bg = p.DangerSoft
				}
				badgeBox(c.Key("res-"+rCopy.RepoName), fmt.Sprintf("%s: %s", rCopy.RepoName, rCopy.Message), col, bg, ui.Color{}, 11, false)
			}
		})
	})
}

func (a *App) buildRightInspector(c *ui.Context, p Palette, st *gitops.RepoStatus) {
	stCopy := *st
	badge := repoBadgeColors(stCopy.Repo.ColorIndex, p.IsDark)

	ui.Column(c).Width(330).FillHeight().Padding(14).Gap(12).Background(p.Surface).BorderWidth(0, 0, 0, 1).BorderColor(p.Border).Shrink(0).Children(func() {
		// Header with close button
		ui.Row(c).Justify(ui.SpaceBetween).AlignItems(ui.Center).Children(func() {
			ui.Row(c).Gap(8).AlignItems(ui.Center).Children(func() {
				ui.Box(c).Size(28, 28).Radius(6).Background(badge.Bg).Center().Children(func() {
					ui.Text(c, stCopy.Repo.Monogram).FontSize(11).FontWeight(weightBold).TextColor(badge.Text)
				})
				ui.Column(c).Gap(1).Children(func() {
					ui.Text(c, stCopy.Repo.Name).FontSize(13.5).FontWeight(weightBold).TextColor(p.TextPrimary)
					ui.Text(c, "当前分支："+stCopy.Branch).FontSize(11).TextColor(p.TextMuted)
				})
			})
			miniBtn(c, p, "关闭", func() {
				a.showRepoDetailDrawer = false
			})
		})

		// Single-repo quick actions
		ui.Row(c).Wrap().Gap(6).Children(func() {
			miniBtn(c, p, "拉取 (Pull)", func() {
				go func(r mani.DiscoveredRepo) {
					res := gitops.PullAll(context.Background(), []mani.DiscoveredRepo{r}, 1)
					a.runUI(func() {
						a.lastBatchResults = res
						a.RefreshAsync()
					})
				}(stCopy.Repo)
			})
			miniBtn(c, p, "推送 (Push)", func() {
				go func(r mani.DiscoveredRepo) {
					res := gitops.BatchPush(context.Background(), []mani.DiscoveredRepo{r}, 1)
					a.runUI(func() {
						a.lastBatchResults = res
						a.RefreshAsync()
					})
				}(stCopy.Repo)
			})
			miniBtn(c, p, "切回 "+stCopy.DefaultBranch, func() {
				go func(r mani.DiscoveredRepo) {
					res := gitops.SwitchBranchAll(context.Background(), []mani.DiscoveredRepo{r}, stCopy.DefaultBranch, false, 1)
					a.runUI(func() {
						a.lastBatchResults = res
						a.RefreshAsync()
					})
				}(stCopy.Repo)
			})
		})

		// Local branch switch: one recent-first hierarchy instead of a flat pill wall.
		if len(stCopy.LocalBranches) > 1 {
			ui.Column(c).Gap(6).Children(func() {
				ui.Row(c).Justify(ui.SpaceBetween).AlignItems(ui.Center).Children(func() {
					ui.Text(c, "切换分支").FontSize(11.5).FontWeight(weightSemibold).TextColor(p.TextMuted)
					ui.Text(c, "最近提交优先").FontSize(10.5).TextColor(p.TextMuted)
				})
				ui.Row(c).Wrap().Gap(6).AlignItems(ui.Center).Children(func() {
					tree := a.repoBranchTrees[stCopy.Repo.ID]
					if tree == nil {
						tree = buildBranchTree(stCopy.BranchInfos)
					}
					branchMenuPill(c, p, stCopy.Branch, true, func(m *ui.Menu) {
						renderBranchMenu(m, tree, stCopy.Branch, func(target string) {
							a.switchRepoBranch(stCopy.Repo, target)
						})
						m.Separator()
						if m.Item("全部分支与分支组合…").Chosen() {
							a.openBranchSwitchModal()
						}
					})
					ui.Text(c, fmt.Sprintf("本地 %d · 远端 %d", len(stCopy.LocalBranches), len(stCopy.RemoteBranches))).FontSize(10.5).TextColor(p.TextMuted)
				})
			})
		}

		ui.Divider(c)

		// Recent Commit History
		ui.Text(c, "最近提交记录").FontSize(11.5).FontWeight(weightSemibold).TextColor(p.TextMuted)
		ui.Scroll(c).Grow(1).Gap(6).Children(func() {
			for _, cm := range stCopy.RecentCommits {
				cmCopy := cm
				ck := c.Key("rcm-" + cmCopy.Hash)
				ui.Column(ck).Padding(8, 10).Gap(3).Radius(6).Background(p.RailBg).Children(func() {
					ui.Text(ck, cmCopy.Subject).FontSize(12).FontWeight(weightMedium).TextColor(p.TextPrimary).MaxLines(2)
					ui.Row(ck).Justify(ui.SpaceBetween).Children(func() {
						ui.Text(ck, cmCopy.Author).FontSize(10.5).TextColor(p.TextMuted)
						ui.Text(ck, cmCopy.Hash+" · "+cmCopy.RelTime).FontSize(10.5).TextColor(p.TextMuted)
					})
				})
			}
		})
	})
}
