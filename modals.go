package main

import (
	"context"
	"fmt"
	"sort"
	"strings"
	"time"

	"github.com/egoist/mygo"
	"github.com/egoist/mygo/ui"
	"github.com/iwangjie/nemorous/internal/gitops"
)

func isWordBoundaryByte(b byte) bool {
	return b == '/' || b == '-' || b == '_' || b == '.'
}

func scoreTokenFuzzy(nameLower string, tok string) (int, bool) {
	if tok == "" {
		return 1, true
	}
	if nameLower == tok {
		return 2000, true
	}
	if strings.HasPrefix(nameLower, tok) {
		return 1200 - min(len(nameLower), 100), true
	}
	// Check segment boundary prefix (e.g. after '/', '-', '_', '.')
	for i := 1; i < len(nameLower); i++ {
		if isWordBoundaryByte(nameLower[i-1]) && strings.HasPrefix(nameLower[i:], tok) {
			return 950 - i*2 - min(len(nameLower), 80), true
		}
	}
	if idx := strings.Index(nameLower, tok); idx >= 0 {
		return 750 - idx*2 - min(len(nameLower), 80), true
	}

	// Subsequence fuzzy match (e.g. "f28c" -> "fix/20261028-cors")
	tRunes := []rune(tok)
	nRunes := []rune(nameLower)
	ti := 0
	score := 300
	firstMatch := -1
	lastMatch := -1

	for ni, nr := range nRunes {
		if nr == tRunes[ti] {
			if firstMatch < 0 {
				firstMatch = ni
			}
			if ni == 0 || (ni > 0 && isWordBoundaryByte(byte(nRunes[ni-1]))) {
				score += 35
			}
			if lastMatch == ni-1 {
				score += 25
			}
			lastMatch = ni
			ti++
			if ti == len(tRunes) {
				break
			}
		}
	}
	if ti < len(tRunes) {
		return 0, false
	}
	span := lastMatch - firstMatch + 1
	gaps := span - len(tRunes)
	score -= gaps * 3
	return max(score, 10), true
}

// fuzzyMatchBranches filters and ranks branches by fuzzy match quality and recent commit activity.
func fuzzyMatchBranches(branches []gitops.BranchInfo, query string, maxResults int) []gitops.BranchInfo {
	if maxResults <= 0 {
		maxResults = 10
	}
	q := strings.ToLower(strings.TrimSpace(query))
	if q == "" {
		if len(branches) <= maxResults {
			return branches
		}
		return branches[:maxResults]
	}

	tokens := strings.Fields(q)
	type scoredBranch struct {
		info  gitops.BranchInfo
		score int
	}
	now := time.Now()
	var matched []scoredBranch

	for _, bi := range branches {
		nameLower := strings.ToLower(bi.Name)
		totalScore := 0
		okAll := true
		for _, tok := range tokens {
			sc, ok := scoreTokenFuzzy(nameLower, tok)
			if !ok {
				okAll = false
				break
			}
			totalScore += sc
		}
		if !okAll {
			continue
		}
		if bi.IsRecent(now) {
			totalScore += 40
		}
		matched = append(matched, scoredBranch{info: bi, score: totalScore})
	}

	// Fallback: if user typed directly after an existing branch name (e.g. "maincors" or "master261028"),
	// strip the leading branch name prefix and fuzzy-match the newly typed suffix.
	if len(matched) == 0 && len(tokens) == 1 {
		for _, pfxBranch := range branches {
			pfx := strings.ToLower(pfxBranch.Name)
			if len(q) > len(pfx) && strings.HasPrefix(q, pfx) {
				rem := q[len(pfx):]
				if len(rem) >= 2 {
					return fuzzyMatchBranches(branches, rem, maxResults)
				}
			}
		}
	}

	sort.Slice(matched, func(i, j int) bool {
		if matched[i].score != matched[j].score {
			return matched[i].score > matched[j].score
		}
		if matched[i].info.CommitUnix != matched[j].info.CommitUnix {
			return matched[i].info.CommitUnix > matched[j].info.CommitUnix
		}
		return matched[i].info.Name < matched[j].info.Name
	})

	limit := min(len(matched), maxResults)
	out := make([]gitops.BranchInfo, 0, limit)
	for i := 0; i < limit; i++ {
		out = append(out, matched[i].info)
	}
	return out
}

// branchFuzzyInput renders a text input with fuzzy search autocomplete popup for branch names.
func branchFuzzyInput(
	c *ui.Context,
	p Palette,
	value *string,
	candidates []gitops.BranchInfo,
	placeholder string,
	width float32,
	grow float32,
	autoFocus bool,
	onChanged func(string),
	onSubmitted func(string),
) {
	cb := ui.ComboboxBase(c, value)
	borderCol := p.Border
	if cb.Input.Focused() {
		borderCol = p.Accent
	}
	cb.Input.Padding(6, 10).Radius(6).Background(p.Surface).Border(1, borderCol).Placeholder(placeholder)
	if width > 0 {
		cb.Input.Width(width)
	}
	if grow > 0 {
		cb.Input.Grow(grow)
	}
	if autoFocus {
		cb.Input.AutoFocus()
	}

	query := ""
	if cb.Filtering() {
		query = strings.TrimSpace(*value)
	}
	matches := fuzzyMatchBranches(candidates, query, 10)

	cb.Popup(func(panel ui.Element) {
		panel.Padding(4).Radius(8).Background(p.Surface).Border(1, p.Border).MaxHeight(260)
		for _, bi := range matches {
			biCopy := bi
			item := cb.Item(biCopy.Name).Padding(6, 10).Radius(6).Gap(8).AlignItems(ui.Center).Cursor(ui.CursorPointer)
			hi := item.Highlighted()
			if hi {
				item.Background(p.Accent)
			}
			item.Children(func() {
				icCol := p.Accent
				txtCol := p.TextPrimary
				metaCol := p.TextMuted
				if hi {
					icCol = p.AccentText
					txtCol = p.AccentText
					metaCol = p.AccentText.Alpha(0.82)
				}
				drawIcon(c, icoBranch, 12, icCol)
				ui.Text(c, biCopy.Name).FontSize(12).FontWeight(weightMedium).TextColor(txtCol).SingleLine().Grow(1)
				if biCopy.RelTime != "" {
					ui.Text(c, biCopy.RelTime).FontSize(10.5).TextColor(metaCol).SingleLine()
				}
			})
		}
	})

	changed := cb.Input.Changed()
	if chosen, ok := cb.Chosen(); ok {
		if *value != chosen {
			*value = chosen
			changed = true
		}
	}
	if changed && onChanged != nil {
		onChanged(*value)
	}
	if cb.Input.Submitted() && onSubmitted != nil {
		onSubmitted(*value)
	}
}

func (a *App) buildCommandPaletteModal(c *ui.Context, p Palette) {
	ui.DialogBase(c, &a.showCmdPalette, func(backdrop, panel ui.Element) {
		backdrop.Background(ui.RGBA(20, 22, 30, 0.36))
		panel.Width(600).Padding(14).Gap(10).Radius(12).Background(p.Surface).Border(1, p.Border)

		q := strings.ToLower(strings.TrimSpace(a.cmdPaletteQuery))

		ui.Row(c).Gap(8).AlignItems(ui.Center).Children(func() {
			drawIcon(c, icoSearch, 15, p.TextMuted)
			ui.TextInput(c.Key("cmd-q"), &a.cmdPaletteQuery).Bind(&a.cmdInputHandle).AutoFocus().
				Placeholder("搜索文件、仓库、分支组合或操作...").Grow(1)
			miniBtn(c, p, "关闭", func() {
				a.showCmdPalette = false
			})
		})

		ui.Scroll(c).MaxHeight(360).Gap(2).Children(func() {
			shown := 0
			addEntry := func(key, category, title, subtitle string, catColor ui.Color, action func()) {
				if shown >= 24 {
					return
				}
				if q != "" {
					hay := strings.ToLower(category + " " + title + " " + subtitle)
					if !strings.Contains(hay, q) {
						return
					}
				}
				shown++
				kc := c.Key("cp-" + key)
				row := ui.Row(kc).Padding(8, 10).Gap(10).Radius(6).Background(p.Surface).Cursor(ui.CursorPointer).AlignItems(ui.Center)
				if row.Hovered() {
					row.Background(p.RailBg)
				}
				if row.Clicked() {
					a.showCmdPalette = false
					action()
				}
				row.Children(func() {
					badgeBox(kc, category, catColor, p.Surface2, ui.Color{}, 10, true)
					ui.Text(kc, title).FontSize(12.5).FontWeight(weightMedium).TextColor(p.TextPrimary)
					ui.Text(kc, subtitle).FontSize(11.5).TextColor(p.TextMuted).Grow(1).MaxLines(1)
				})
			}

			// Saved Branch Presets in Command Palette
			for _, pr := range a.cfg.BranchPresets {
				prCopy := pr
				addEntry("preset-"+prCopy.Name, "分支组合", "切换到组合："+prCopy.Name, formatPresetSummary(prCopy.RepoBranches), p.Purple, func() {
					a.ApplyBranchPreset(prCopy, true)
				})
			}

			addEntry("act-branch", "操作", "切换或新建分支 / 管理分支组合", "⌘B · 支持多项目同时切换与记住分支组合", p.Accent, func() {
				a.openBranchSwitchModal()
			})
			addEntry("act-pull", "操作", "拉取全部仓库更新 (Pull)", "自动跳过有未提交改动的仓库", p.Accent, func() {
				a.TriggerBatchPull()
			})
			addEntry("act-push", "操作", "推送全部待推送仓库 (Push)", "将本地已提交改动推送到远端", p.Accent, func() {
				a.TriggerPushAllAhead()
			})
			addEntry("act-fetch", "操作", "获取远端分支信息 (Fetch)", "git fetch --prune", p.Accent, func() {
				a.TriggerBatchFetch()
			})
			addEntry("act-main", "操作", "全部切回默认分支", "切回 main/master 并拉取最新", p.Accent, func() {
				a.TriggerSwitchToMain()
			})
			addEntry("act-merge", "操作", "合并分支到 "+a.mergeTargetBranch, "检查冲突并合并", p.Purple, func() {
				a.activeTab = TabMerge
				a.TriggerMergePreflight(false)
			})
			addEntry("act-refresh", "操作", "刷新仓库状态", "⌘R", p.Accent, func() {
				a.RefreshAsync()
			})
			addEntry("act-theme", "操作", "切换浅色 / 深色外观", "", p.Amber, func() {
				a.toggleTheme()
			})

			for _, st := range a.statuses {
				stCopy := st
				sub := "分支 " + st.Branch
				if st.Repo.Description != "" && st.Repo.Description != st.Repo.Name && st.Repo.Description != "." {
					sub += " · " + st.Repo.Description
				}
				addEntry("repo-"+st.Repo.ID, "仓库", st.Repo.Name, sub, p.Blue, func() {
					a.selectedRepoID = stCopy.Repo.ID
					a.activeTab = TabOverview
				})
			}

			for _, st := range a.statuses {
				for _, cf := range st.Files {
					cfCopy := cf
					addEntry("chg-"+cf.Key(), "变更", cf.RepoName+"/"+cf.Path, fmt.Sprintf("+%d -%d", cf.AddedLines, cf.DeletedLines), p.Amber, func() {
						a.selectChangedFile(cfCopy)
						a.activeTab = TabChanges
					})
				}
			}

			for _, wf := range a.workspaceFiles {
				wfCopy := wf
				addEntry("file-"+wf.RepoID+"::"+wf.RelPath, "文件", wf.RepoName+"/"+wf.RelPath, "打开代码文件", p.Green, func() {
					a.selectCodeFile(wfCopy)
					a.activeTab = TabCode
				})
			}
		})
	})
}

func formatPresetSummary(repoBranches map[string]string) string {
	keys := make([]string, 0, len(repoBranches))
	for k := range repoBranches {
		keys = append(keys, k)
	}
	sort.Strings(keys)
	parts := make([]string, 0, len(keys))
	for _, k := range keys {
		parts = append(parts, fmt.Sprintf("%s→%s", k, repoBranches[k]))
	}
	return strings.Join(parts, " · ")
}

func (a *App) buildBranchSwitchModal(c *ui.Context, p Palette) {
	ui.DialogBase(c, &a.showBranchModal, func(backdrop, panel ui.Element) {
		backdrop.Background(ui.RGBA(20, 22, 30, 0.36))
		panel.Width(660).Padding(18).Gap(12).Radius(12).Background(p.Surface).Border(1, p.Border)

		checkedCount := 0
		for _, st := range a.statuses {
			if a.branchCheckedRepos[st.Repo.ID] {
				checkedCount++
			}
		}

		// Modal Header
		ui.Row(c).Justify(ui.SpaceBetween).AlignItems(ui.Center).Children(func() {
			ui.Column(c).Gap(2).Children(func() {
				ui.Text(c, "切换分支与分支组合").FontSize(15).FontWeight(weightBold).TextColor(p.TextPrimary)
				ui.Text(c, "支持多项目同时切换到同一分支，或为不同项目分别指定分支并保存为需求组合（有未提交改动的仓库会自动跳过）。").FontSize(11.5).TextColor(p.TextMuted)
			})
			miniBtn(c, p, "关闭", func() { a.showBranchModal = false })
		})

		// 1. Saved Branch Combinations (e.g. "AAA 需求")
		if len(a.cfg.BranchPresets) > 0 {
			ui.Column(c).Padding(10, 12).Gap(6).Radius(8).Background(p.RailBg).Border(1, p.BorderSubtle).Children(func() {
				ui.Text(c, "已保存的分支组合").FontSize(11.5).FontWeight(weightSemibold).TextColor(p.TextSecondary)
				for _, preset := range a.cfg.BranchPresets {
					prCopy := preset
					pkc := c.Key("preset-row-" + prCopy.Name)
					ui.Row(pkc).Padding(6, 10).Gap(8).Radius(6).Background(p.Surface).Border(1, p.BorderSubtle).AlignItems(ui.Center).Children(func() {
						badgeBox(pkc, prCopy.Name, p.Accent, p.AccentSoft, ui.Color{}, 11.5, true)
						ui.Text(pkc, formatPresetSummary(prCopy.RepoBranches)).FontSize(11.5).TextColor(p.TextSecondary).Grow(1).MaxLines(1)
						miniBtn(pkc, p, "填入下方", func() {
							a.ApplyBranchPreset(prCopy, false)
						})
						pill(pkc, p, "立即切换", true, func() {
							a.ApplyBranchPreset(prCopy, true)
						})
						miniBtn(pkc, p, "删除", func() {
							a.cfg.DeleteBranchPreset(prCopy.Name)
							_ = a.cfg.Save()
						})
					})
				}
			})
		}

		// 2. Unified Input + Activity-Sorted Branches (<=7 days visible, >7 days collapsed)
		recentBrs, staleBrs := a.BranchesByActivity()
		allWorkspaceCandidates := a.BranchCandidatesForRepo("")

		ui.Column(c).Gap(6).Children(func() {
			ui.Row(c).Gap(8).AlignItems(ui.Center).Children(func() {
				ui.Text(c, "批量设置勾选仓库分支：").FontSize(12).FontWeight(weightMedium).TextColor(p.TextPrimary).Shrink(0)
				branchFuzzyInput(
					c.Key("branch-in"),
					p,
					&a.branchInput,
					allWorkspaceCandidates,
					"输入关键字模糊搜索分支（如 cors / 261028），回车切换...",
					0,
					1,
					true,
					func(val string) {
						a.SetUnifiedBranchForChecked(val)
					},
					func(_ string) {
						a.TriggerCheckedBranchSwitch()
					},
				)
			})

			// Recent Active Branches (within 7 days) — show top 8 directly, scroll if more
			if len(recentBrs) > 0 {
				ui.Row(c).Wrap().Gap(6).AlignItems(ui.Center).Children(func() {
					ui.Text(c, fmt.Sprintf("近 7 天活跃 (%d)：", len(recentBrs))).FontSize(11).TextColor(p.TextMuted)
					recentTree, _ := a.workspaceBranchTrees()
					branchMenuPill(c, p, "按层级选择近期分支", false, func(m *ui.Menu) {
						renderBranchMenu(m, recentTree, a.branchInput, func(target string) {
							a.SetUnifiedBranchForChecked(target)
						})
					})
				})
			}

			// Collapsible Stale Branches (> 7 days without changes)
			if len(staleBrs) > 0 {
				staleToggle := ui.Row(c.Key("modal-stale-toggle")).Padding(2, 0).Gap(4).Cursor(ui.CursorPointer).AlignItems(ui.Center)
				if staleToggle.Clicked() {
					a.showStaleBranchesInModal = !a.showStaleBranchesInModal
				}
				staleToggle.Children(func() {
					chev := icoChevronRight
					if a.showStaleBranchesInModal {
						chev = icoChevronDown
					}
					drawIcon(c, chev, 11, p.TextMuted)
					ui.Text(c, fmt.Sprintf("一周前无变更分支 (%d)", len(staleBrs))).FontSize(11).TextColor(p.TextMuted)
				})

				if a.showStaleBranchesInModal {
					_, staleTree := a.workspaceBranchTrees()
					branchMenuPill(c, p, fmt.Sprintf("按层级选择更早分支 (%d)", len(staleBrs)), false, func(m *ui.Menu) {
						renderBranchMenu(m, staleTree, a.branchInput, func(target string) {
							a.SetUnifiedBranchForChecked(target)
						})
					})
				}
			}
		})

		// 3. Multi-Project Selection & Per-Project Branch Table
		ui.Column(c).Radius(8).Background(p.Surface).Border(1, p.Border).Children(func() {
			ui.Row(c).Padding(7, 12).Background(p.RailBg).BorderWidth(0, 0, 1, 0).BorderColor(p.Border).Justify(ui.SpaceBetween).AlignItems(ui.Center).Children(func() {
				ui.Text(c, fmt.Sprintf("选择要切换的仓库 (%d/%d) — 可单独修改每个仓库的目标分支", checkedCount, len(a.statuses))).FontSize(11.5).FontWeight(weightSemibold).TextColor(p.TextSecondary)
				ui.Row(c).Gap(6).Children(func() {
					miniBtn(c, p, "全选", func() {
						for _, st := range a.statuses {
							a.branchCheckedRepos[st.Repo.ID] = true
						}
					})
					miniBtn(c, p, "清空", func() {
						for _, st := range a.statuses {
							a.branchCheckedRepos[st.Repo.ID] = false
						}
					})
				})
			})

			ui.Scroll(c).MaxHeight(190).Children(func() {
				for idx, st := range a.statuses {
					stCopy := st
					repoID := stCopy.Repo.ID
					checked := a.branchCheckedRepos[repoID]
					badge := repoBadgeColors(stCopy.Repo.ColorIndex, p.IsDark)
					if _, ok := a.branchPerRepoTarget[repoID]; !ok || a.branchPerRepoTarget[repoID] == nil {
						b := stCopy.Branch
						a.branchPerRepoTarget[repoID] = &b
					}
					targetPtr := a.branchPerRepoTarget[repoID]

					rkc := c.Key("mbr-" + repoID)
					row := ui.Row(rkc).Padding(6, 12).Gap(8).Background(p.Surface).AlignItems(ui.Center)
					if idx < len(a.statuses)-1 {
						row.BorderWidth(0, 0, 1, 0).BorderColor(p.BorderSubtle)
					}
					row.Children(func() {
						chk := ui.Box(rkc).Size(15, 15).Radius(4).Cursor(ui.CursorPointer).Center().Shrink(0)
						if checked {
							chk.Background(p.Accent)
						} else {
							chk.Background(p.Surface2).Border(1, p.BorderStrong)
						}
						if chk.Clicked() {
							a.branchCheckedRepos[repoID] = !a.branchCheckedRepos[repoID]
						}
						chk.Children(func() {
							if checked {
								drawIcon(rkc, icoCheck, 11, p.AccentText)
							}
						})

						ui.Box(rkc).Size(18, 18).Radius(4).Background(badge.Bg).Center().Shrink(0).Children(func() {
							ui.Text(rkc, stCopy.Repo.Monogram).FontSize(9.5).FontWeight(weightBold).TextColor(badge.Text)
						})
						ui.Text(rkc, stCopy.Repo.Name).FontSize(12.5).FontWeight(weightMedium).TextColor(p.TextPrimary).Width(130).MaxLines(1)
						badgeBox(rkc, "当前: "+stCopy.Branch, p.TextControl, p.Surface2, ui.Color{}, 10.5, false)
						ui.Spacer(rkc)
						ui.Text(rkc, "目标分支：").FontSize(11).TextColor(p.TextMuted)
						repoCandidates := a.BranchCandidatesForRepo(repoID)
						branchFuzzyInput(
							rkc.Key("in"),
							p,
							targetPtr,
							repoCandidates,
							"搜索或输入目标分支...",
							185,
							0,
							false,
							func(_ string) {
								a.branchCheckedRepos[repoID] = true
							},
							nil,
						)
					})
				}
			})
		})

		ui.Checkbox(c, &a.branchCreateMissing, "如果目标分支不存在，自动基于当前提交新建分支")

		// 4. Bottom Bar: Save Preset ("记住分支组合") & Execute Switch
		ui.Row(c).Justify(ui.SpaceBetween).AlignItems(ui.Center).Gap(8).Children(func() {
			ui.Row(c).Gap(6).AlignItems(ui.Center).Children(func() {
				ui.TextInput(c.Key("preset-name-in"), &a.presetNameInput).Placeholder("组合名称，如：AAA 需求").Width(170)
				pill(c, p, "保存为分支组合", false, func() {
					a.SaveCurrentSelectionAsPreset()
				})
			})

			ui.Row(c).Gap(8).AlignItems(ui.Center).Children(func() {
				pill(c, p, "取消", false, func() {
					a.showBranchModal = false
				})
				pill(c, p, fmt.Sprintf("切换勾选仓库 (%d)", checkedCount), true, func() {
					a.TriggerCheckedBranchSwitch()
				})
			})
		})
	})
}

func (a *App) buildSettingsModal(c *ui.Context, p Palette) {
	ui.DialogBase(c, &a.showSettings, func(backdrop, panel ui.Element) {
		backdrop.Background(ui.RGBA(20, 22, 30, 0.36))
		panel.Width(560).Padding(18).Gap(14).Radius(12).Background(p.Surface).Border(1, p.Border)

		ui.Row(c).Justify(ui.SpaceBetween).AlignItems(ui.Center).Children(func() {
			ui.Text(c, "设置").FontSize(15).FontWeight(weightBold).TextColor(p.TextPrimary)
			miniBtn(c, p, "完成", func() {
				_ = a.cfg.Save()
				a.showSettings = false
			})
		})

		// Section 1: Workspace Directory
		ui.Column(c).Padding(12).Gap(8).Radius(8).Background(p.RailBg).Border(1, p.BorderSubtle).Children(func() {
			ui.Text(c, "工作区目录").FontSize(12.5).FontWeight(weightSemibold).TextColor(p.TextPrimary)
			ui.Row(c).Gap(6).AlignItems(ui.Center).Children(func() {
				ui.TextInput(c.Key("ws-path-in"), &a.cfg.WorkspacePath).Grow(1)
				pill(c, p, "选择目录...", false, func() {
					go func() {
						paths, err := mygo.Dialog.Open(mygo.OpenDialogOptions{
							Parent:    a.win,
							Title:     "选择多仓库工作区目录",
							Directory: true,
						})
						if err == nil && len(paths) > 0 {
							a.runUI(func() {
								a.LoadWorkspaceSync(paths[0])
								_ = a.cfg.Save()
							})
						}
					}()
				})
				pill(c, p, "加载", true, func() {
					a.LoadWorkspaceSync(a.cfg.WorkspacePath)
					_ = a.cfg.Save()
				})
			})

			if ui.Checkbox(c, &a.cfg.IncludeRootRepo, "包含工作区根目录自身的 Git 仓库").Changed() {
				_ = a.cfg.Save()
				a.RefreshAsync()
			}

			if len(a.cfg.RecentWorkspaces) > 1 {
				for _, ws := range a.cfg.RecentWorkspaces {
					wsCopy := ws
					kc := c.Key("rec-" + wsCopy)
					ui.Row(kc).Padding(4, 8).Gap(8).Radius(5).Background(p.Surface).AlignItems(ui.Center).Children(func() {
						ui.Text(kc, wsCopy).FontSize(11.5).TextColor(p.TextSecondary).Grow(1)
						miniBtn(kc, p, "切换", func() {
							a.LoadWorkspaceSync(wsCopy)
							_ = a.cfg.Save()
						})
					})
				}
			}
		})

		// Section 2: Appearance & Editor
		ui.Column(c).Padding(12).Gap(8).Radius(8).Background(p.RailBg).Border(1, p.BorderSubtle).Children(func() {
			ui.Text(c, "外观与显示").FontSize(12.5).FontWeight(weightSemibold).TextColor(p.TextPrimary)
			ui.Row(c).Gap(8).AlignItems(ui.Center).Children(func() {
				ui.Text(c, "主题：").FontSize(12).TextColor(p.TextSecondary)
				pill(c, p, "浅色", a.cfg.Theme == "light", func() {
					a.setTheme("light")
				})
				pill(c, p, "深色", a.cfg.Theme == "dark", func() {
					a.setTheme("dark")
				})
				ui.Spacer(c)
				ui.Text(c, "代码字号：").FontSize(12).TextColor(p.TextSecondary)
				miniBtn(c, p, fmt.Sprintf("%.0fpt", a.cfg.EditorFontSize), func() {
					a.cfg.EditorFontSize += 1
					if a.cfg.EditorFontSize > 17 {
						a.cfg.EditorFontSize = 12
					}
					if a.codeEditorState != nil {
						a.codeEditorState.Font = ui.Font{Size: a.cfg.EditorFontSize}
					}
					if a.diffState != nil {
						a.diffState.Font = ui.Font{Size: a.cfg.EditorFontSize}
					}
					_ = a.cfg.Save()
				})
			})
		})

		// Section 3: Merge & AI Assistant (supports omp, pi, claude, codex)
		ui.Column(c).Padding(12).Gap(8).Radius(8).Background(p.RailBg).Border(1, p.BorderSubtle).Children(func() {
			ui.Text(c, "分支合并与 AI 冲突解决").FontSize(12.5).FontWeight(weightSemibold).TextColor(p.TextPrimary)
			ui.Row(c).Gap(6).AlignItems(ui.Center).Children(func() {
				ui.Text(c, "AI 命令行工具：").FontSize(12).TextColor(p.TextSecondary)
				for _, id := range []string{"auto", "omp", "pi", "claude", "codex"} {
					idCopy := id
					label := idCopy
					if idCopy == "auto" {
						label = "自动检测"
					}
					pill(c.Key("ag-"+idCopy), p, label, a.cfg.AIAgent == idCopy, func() {
						a.cfg.AIAgent = idCopy
						a.aiAgentName, a.aiAgentBin = a.cfg.ResolvedAIAgent()
						_ = a.cfg.Save()
					})
				}
			})
			ui.Text(c, fmt.Sprintf("当前使用：%s (%s)", a.aiAgentName, a.aiAgentBin)).FontSize(11).TextColor(p.TextMuted)
			ui.Row(c).Gap(8).AlignItems(ui.Center).Children(func() {
				ui.Text(c, "默认合并目标分支：").FontSize(12).TextColor(p.TextSecondary)
				if ui.TextInput(c.Key("def-target"), &a.cfg.DefaultTargetBranch).Width(120).Changed() {
					a.mergeTargetBranch = a.cfg.DefaultTargetBranch
					_ = a.cfg.Save()
				}
			})
		})
	})
}

func (a *App) buildDiscardConfirmModal(c *ui.Context, p Palette) {
	open := a.pendingDiscard != nil
	ui.DialogBase(c, &open, func(backdrop, panel ui.Element) {
		if !open || a.pendingDiscard == nil {
			a.pendingDiscard = nil
			return
		}
		pd := *a.pendingDiscard
		backdrop.Background(ui.RGBA(20, 22, 30, 0.36))
		panel.Width(420).Padding(18).Gap(10).Radius(12).Background(p.Surface).Border(1, p.Border)

		ui.Text(c, "确认还原改动？").FontSize(15).FontWeight(weightBold).TextColor(p.Danger)
		ui.Text(c, fmt.Sprintf("将丢弃 %s/%s 的未提交修改，此操作不可撤销。", pd.RepoName, pd.RelPath)).FontSize(12.5).TextColor(p.TextPrimary)
		ui.Row(c).Justify(ui.End).Gap(8).Children(func() {
			pill(c, p, "取消", false, func() {
				a.pendingDiscard = nil
			})
			pill(c, p, "确认还原", true, func() {
				err := gitops.DiscardFile(context.Background(), pd.RepoPath, pd.RelPath, pd.Untracked)
				a.pendingDiscard = nil
				if err != nil {
					a.showToast("err", "还原失败: "+err.Error())
				} else {
					a.showToast("ok", fmt.Sprintf("已还原 %s/%s", pd.RepoName, pd.RelPath))
				}
				a.RefreshAsync()
			})
		})
	})
	if !open {
		a.pendingDiscard = nil
	}
}

func (a *App) toggleTheme() {
	if a.cfg.Theme == "dark" {
		a.setTheme("light")
	} else {
		a.setTheme("dark")
	}
}

func (a *App) setTheme(t string) {
	a.cfg.Theme = t
	_ = a.cfg.Save()
}
