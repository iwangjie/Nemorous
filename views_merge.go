package main

import (
	"context"
	"fmt"
	"strings"
	"time"

	"github.com/egoist/mygo/ui"
	"github.com/iwangjie/nemorous/internal/aimerge"
	"github.com/iwangjie/nemorous/internal/gitops"
)

// mergeableCount is the number of repositories with commits to merge into the
// current target branch.
func (a *App) mergeableCount() int {
	n := 0
	for _, target := range a.mergeTargets {
		if mergeableTarget(target) {
			n++
		}
	}
	return n
}

// mergeSelectionStats counts selected mergeable repositories and active
// conflict worktrees for the compact merge bar.
func (a *App) mergeSelectionStats() (selected, mergeable, conflicts int) {
	for _, target := range a.mergeTargets {
		if mergeableTarget(target) {
			mergeable++
			if a.mergeRepoChecked(target.Repo.ID) {
				if _, hasManual := a.manualMerges[target.Repo.ID]; !hasManual {
					selected++
				}
			}
		}
	}
	for range a.manualMerges {
		conflicts++
	}
	return selected, mergeable, conflicts
}

// toggleReleaseMerge opens the inline merge bar and refreshes its scan.
func (a *App) toggleReleaseMerge() {
	a.mergeBarOpen = !a.mergeBarOpen
	if !a.mergeBarOpen {
		a.mergePickOpen = false
	}
	if a.mergeBarOpen {
		a.ensureMergeScan()
	}
}

// openReleaseMerge opens the Git tool window with the inline merge bar visible.
func (a *App) openReleaseMerge() {
	a.showGitPanel = true
	a.showLogPanel = false
	a.showTerminalPanel = false
	a.showRunPanel = false
	a.mergeBarOpen = true
	a.ensureMergeScan()
}

func (a *App) ensureMergeScan() {
	a.ensureManualMergeRecovery()
	if a.mergeScanLoading {
		return
	}
	if len(a.mergeTargets) == 0 || time.Since(a.mergeScanAt) > 30*time.Second {
		a.TriggerMergeScan()
	}
}

// ensureManualMergeRecovery reconnects to conflict worktrees created before a
// restart. They live outside the working tree, so without this scan a conflict
// would become invisible while its temporary branch kept holding the merge.
func (a *App) ensureManualMergeRecovery() {
	if a.mergeRecovered {
		return
	}
	repos := a.reposSnapshot()
	if len(repos) == 0 {
		return
	}
	a.mergeRecovered = true
	go func() {
		ctx, cancel := context.WithTimeout(context.Background(), 20*time.Second)
		defer cancel()
		found := aimerge.RecoverManualMerges(ctx, repos)
		if len(found) == 0 {
			return
		}
		a.runUI(func() {
			added := 0
			for _, mm := range found {
				if _, exists := a.manualMerges[mm.RepoID]; exists {
					continue
				}
				a.manualMerges[mm.RepoID] = ManualMerge{
					RepoPath:  mm.RepoPath,
					Path:      mm.Worktree,
					Branch:    mm.Branch,
					Conflicts: mm.Conflicts,
				}
				a.mergeCheckedRepos[mm.RepoID] = false
				added++
			}
			if added > 0 {
				a.showToast("warn", fmt.Sprintf("发现 %d 个未完成的合并冲突，点击「解决冲突」继续", added))
			}
		})
	}()
}

func (a *App) mergeTargetOptions() []string {
	seen := make(map[string]bool, len(a.cfg.MergePresets)+2)
	out := make([]string, 0, len(a.cfg.MergePresets)+2)
	add := func(branch string) {
		branch = strings.TrimSpace(branch)
		if branch != "" && !seen[branch] {
			seen[branch] = true
			out = append(out, branch)
		}
	}
	add(a.cfg.DefaultTargetBranch)
	for _, preset := range a.cfg.MergePresets {
		add(preset.TargetBranch)
	}
	add(a.mergeTargetBranch)
	return out
}

// buildReleaseMergeBar renders the whole release-merge workflow as a slim
// strip over the branch tree: target, selection, batch actions and presets.
func (a *App) buildReleaseMergeBar(c *ui.Context, p Palette) {
	if !a.mergeBarOpen {
		return
	}
	a.ensureMergeScan()
	selected, mergeable, conflicts := a.mergeSelectionStats()

	ui.Column(c).FillWidth().BorderWidth(0, 0, 1, 0).BorderColor(p.Border).Children(func() {
		ui.Row(c).Height(38).Padding(0, 10).Gap(7).AlignItems(ui.Center).Background(p.RailBg).Children(func() {
			drawIcon(c, icoMerge, 13, p.Accent)
			menuIconPill(c, p, "merge-target", icoBranch, "→ "+a.mergeTargetBranch, true, func(m *ui.Menu) {
				for _, branch := range a.mergeTargetOptions() {
					branch := branch
					if m.Item(branch).Radio(a.mergeTargetBranch == branch).Chosen() {
						a.mergeTargetBranch = branch
						a.mergeTargets = nil
						a.TriggerMergeScan()
					}
				}
			})
			a.buildMergePicker(c, p, selected, mergeable, conflicts)
			if conflicts > 0 {
				a.mergeConflictPill(c, p, conflicts)
			}
			if a.mergeScanLoading {
				ui.Spinner(c).Size(12, 12)
			}
			ui.Spacer(c)
			if selected > 0 {
				pill(c, p, fmt.Sprintf("合并 (%d)", selected), false, func() { a.TriggerExecuteMerges(false) })
				pill(c, p, fmt.Sprintf("合并并推送 (%d)", selected), true, func() { a.TriggerExecuteMerges(true) })
			} else {
				pillDisabled(c, p, "合并 (0)")
				pillDisabled(c, p, "合并并推送 (0)")
			}
			menuIconPill(c, p, "merge-more", icoMore, "", false, func(m *ui.Menu) {
				if m.Item("刷新状态").Chosen() {
					a.mergeTargets = nil
					a.TriggerMergeScan()
				}
				if m.Item("全部切回开发分支").Chosen() {
					a.TriggerReturnToDevelopmentBranches()
				}
				if len(a.cfg.MergePresets) > 0 {
					m.Separator()
					for _, preset := range a.cfg.MergePresets {
						presetCopy := preset
						label := fmt.Sprintf("方案：%s · %s (%d)", presetCopy.Name, presetCopy.TargetBranch, len(presetCopy.RepoIDs))
						if m.Item(label).Chosen() {
							a.applyMergePreset(presetCopy)
						}
					}
				}
				m.Separator()
				if m.Item("保存当前选择为方案…").Chosen() {
					if strings.TrimSpace(a.mergePresetInput) == "" {
						a.mergePresetInput = "合并到 " + a.mergeTargetBranch
					}
					a.mergePresetSaveOpen = true
				}
				if conflicts > 0 {
					if m.Item("清理全部临时冲突工作区").Chosen() {
						for id := range a.manualMerges {
							a.cleanupManualMerge(id)
						}
					}
				}
			})
		})
		if a.mergePresetSaveOpen {
			ui.Row(c).Height(34).Padding(0, 10).Gap(6).AlignItems(ui.Center).Background(p.RailBg).Children(func() {
				ui.Text(c, "方案名称").FontSize(10.5).TextColor(p.TextMuted).Shrink(0)
				ui.TextInput(c.Key("merge-preset-in"), &a.mergePresetInput).Placeholder("如：合并到 test").Width(180)
				miniBtn(c, p, "保存", func() {
					a.saveMergePreset()
					a.mergePresetSaveOpen = false
				})
				miniBtn(c, p, "取消", func() { a.mergePresetSaveOpen = false })
			})
		}
		if a.aiVerifyReport != nil && a.aiVerifyRepo != nil {
			a.buildInlineAIMergeReport(c, p)
		} else if a.mergeRunning && a.aiVerifyRepo != nil {
			if last := lastNonEmptyLine(a.aiStreamRaw.String()); last != "" {
				ui.Row(c).Height(26).Padding(0, 10).Gap(6).AlignItems(ui.Center).Background(p.Surface).Children(func() {
					ui.Spinner(c).Size(11, 11)
					ui.Text(c, last).FontSize(10).TextColor(p.TextMuted).MaxLines(1).Grow(1)
				})
			}
		}
	})
}

// firstManualConflictID returns a stable conflict repository, following the
// scan order so the resolver opens the same one every time.
func (a *App) firstManualConflictID() string {
	for _, target := range a.mergeTargets {
		if _, ok := a.manualMerges[target.Repo.ID]; ok {
			return target.Repo.ID
		}
	}
	for id := range a.manualMerges {
		return id
	}
	return ""
}

// mergeConflictPill is the always-visible way back into conflict resolution
// after the merge toast or the selection popover has been dismissed.
func (a *App) mergeConflictPill(c *ui.Context, p Palette, conflicts int) {
	id := a.firstManualConflictID()
	el := ui.Row(c).Padding(4, 9).Gap(5).Radius(5).Background(p.DangerSoft).
		AlignItems(ui.Center).Shrink(0).Cursor(ui.CursorPointer).Tooltip("处理临时合并工作区中的冲突")
	if el.Hovered() {
		el.Background(p.SurfaceHover)
	}
	if el.Clicked() && id != "" {
		a.openConflictResolver(id)
	}
	el.Children(func() {
		drawIcon(c, icoAlert, 12, p.Danger)
		ui.Text(c, fmt.Sprintf("解决冲突 (%d)", conflicts)).
			FontSize(11).FontWeight(weightMedium).TextColor(p.Danger).SingleLine()
	})
}

// buildMergePicker renders the repository selection as an in-window popover.
// Unlike a native menu it stays open while rows are toggled, so a batch of
// changes does not need the picker to be reopened for every click.
func (a *App) buildMergePicker(c *ui.Context, p Palette, selected, mergeable, conflicts int) {
	kc := c.Key("merge-pick")
	trigger := ui.Row(kc).Padding(4, 8).Gap(5).Radius(4).AlignItems(ui.Center).Shrink(0)
	fg := p.TextSecondary
	if a.mergePickOpen {
		trigger.Background(p.AccentSoft)
		fg = p.AccentSoftText
	}
	if trigger.Hovered() {
		trigger.Background(p.SurfaceHover)
		if !a.mergePickOpen {
			fg = p.TextPrimary
		}
	}
	if trigger.Clicked() {
		a.mergePickOpen = !a.mergePickOpen
	}
	trigger.Children(func() {
		drawIcon(kc, icoCheck, 13, fg)
		ui.Text(kc, fmt.Sprintf("已选 %d/%d", selected, mergeable)).
			FontSize(11.5).FontWeight(weightMedium).TextColor(fg).SingleLine()
		drawIcon(kc, icoChevronDown, 10, p.TextMuted)
	})

	ui.Popover(c, trigger, &a.mergePickOpen, func() {
		ui.Column(c).Width(390).Padding(5).Gap(1).Children(func() {
			action := func(label string, icon *ui.SVG, color ui.Color, onClick func()) {
				row := ui.Row(c).FillWidth().Padding(6, 9).Gap(7).Radius(4).
					AlignItems(ui.Center).Cursor(ui.CursorPointer)
				if row.Hovered() {
					row.Background(p.SurfaceHover)
				}
				if row.Clicked() {
					onClick()
				}
				row.Children(func() {
					if icon != nil {
						drawIcon(c, icon, 12, color)
					} else {
						ui.Box(c).Size(12, 12).Shrink(0)
					}
					ui.Text(c, label).FontSize(11.5).TextColor(color).SingleLine()
				})
			}
			if conflicts > 0 {
				action(fmt.Sprintf("解决冲突 (%d)", conflicts), icoAlert, p.Danger, func() {
					if id := a.firstManualConflictID(); id != "" {
						a.mergePickOpen = false
						a.openConflictResolver(id)
					}
				})
			}
			action("全选可合并", nil, p.TextPrimary, func() {
				for _, target := range a.mergeTargets {
					if _, manual := a.manualMerges[target.Repo.ID]; manual {
						continue
					}
					if mergeableTarget(target) {
						a.mergeCheckedRepos[target.Repo.ID] = true
					}
				}
			})
			action("清空", nil, p.TextPrimary, func() {
				for _, target := range a.mergeTargets {
					a.mergeCheckedRepos[target.Repo.ID] = false
				}
			})
			if len(a.mergeTargets) > 0 {
				ui.Divider(c)
			}
			for _, target := range a.mergeTargets {
				targetCopy := target
				_, manual := a.manualMerges[targetCopy.Repo.ID]
				canMerge := mergeableTarget(targetCopy)
				actionable := manual || canMerge
				row := ui.Row(c).FillWidth().Padding(6, 8).Gap(8).Radius(4).AlignItems(ui.Center)
				if actionable {
					row.Cursor(ui.CursorPointer)
				}
				switch {
				case manual:
					if row.Hovered() {
						row.Background(p.DangerSoft)
					}
				case canMerge:
					if row.Hovered() {
						row.Background(p.SurfaceHover)
					}
				}
				if row.Clicked() && actionable {
					if manual {
						a.mergePickOpen = false
						a.openConflictResolver(targetCopy.Repo.ID)
					} else {
						a.mergeCheckedRepos[targetCopy.Repo.ID] = !a.mergeRepoChecked(targetCopy.Repo.ID)
					}
				}
				row.Children(func() {
					switch {
					case manual:
						drawIcon(c, icoAlert, 13, p.Danger)
					case !canMerge:
						ui.Box(c).Size(14, 14).Shrink(0)
					default:
						box := ui.Box(c).Size(14, 14).Radius(3).Center().Shrink(0)
						if a.mergeRepoChecked(targetCopy.Repo.ID) {
							box.Background(p.Accent)
							box.Children(func() { drawIcon(c, icoCheck, 10, p.AccentText) })
						} else {
							box.BorderWidth(1).BorderColor(p.Border)
						}
					}
					ui.Column(c).Grow(1).Gap(1).Children(func() {
						label := targetCopy.Repo.Name + " · " + targetCopy.SourceBranch + " → " + targetCopy.TargetBranch
						textColor := p.TextPrimary
						if !actionable {
							textColor = p.TextMuted
						}
						if manual {
							textColor = p.Danger
						}
						ui.Text(c, label).FontSize(11.5).TextColor(textColor).SingleLine()
						status, tone := "", p.TextMuted
						switch {
						case manual:
							status, tone = "冲突待处理 · 点击解决", p.Danger
						case !targetCopy.TargetExists:
							status = "无目标分支"
						case targetCopy.AheadCommits == 0:
							status = "无新提交"
						}
						if status != "" {
							ui.Text(c, status).FontSize(9.5).TextColor(tone).SingleLine()
						}
					})
				})
			}
		})
	})
}

func lastNonEmptyLine(s string) string {
	lines := strings.Split(strings.TrimSpace(s), "\n")
	for i := len(lines) - 1; i >= 0; i-- {
		if line := strings.TrimSpace(lines[i]); line != "" {
			return line
		}
	}
	return ""
}

// buildMergeRepoStatus adds each repository's release-merge state to its row in
// the branch tree. It only runs while the merge bar is open.
func (a *App) buildMergeRepoStatus(c *ui.Context, p Palette, st gitops.RepoStatus) {
	if !a.mergeBarOpen {
		return
	}
	target := a.mergeTargetByID(st.Repo.ID)
	if target == nil {
		return
	}
	if _, manual := a.manualMerges[st.Repo.ID]; manual {
		badgeBox(c, "冲突待处理", p.Danger, p.DangerSoft, ui.Color{}, 9.5, true)
		return
	}
	switch {
	case target.Detached:
		ui.Text(c, "detached").FontSize(9.5).TextColor(p.Amber)
	case !target.TargetExists:
		ui.Text(c, "无 "+a.mergeTargetBranch).FontSize(9.5).TextColor(p.Amber)
	case target.SourceBranch == target.TargetBranch:
		ui.Text(c, "已在目标分支").FontSize(9.5).TextColor(p.TextMuted)
	case target.AheadCommits == 0:
		ui.Text(c, "无新提交").FontSize(9.5).TextColor(p.TextMuted)
	default:
		tip := fmt.Sprintf("%d 个提交待合并", target.AheadCommits)
		if len(target.CommitPreview) > 0 {
			preview := target.CommitPreview[:min(5, len(target.CommitPreview))]
			tip += "\n" + strings.Join(preview, "\n")
		}
		badge := ui.Row(c).Padding(2, 7).Radius(5).Background(p.GreenSoft).AlignItems(ui.Center).Shrink(0).Tooltip(tip)
		badge.Children(func() {
			ui.Text(c, fmt.Sprintf("+%d", target.AheadCommits)).FontSize(9.5).FontWeight(weightSemibold).TextColor(p.Green).SingleLine()
		})
	}
}

// mergeRepoMenu adds forward release-merge actions to a repository's context
// menu without opening a separate workflow.
func (a *App) mergeRepoMenu(m *ui.Menu, repoID string) {
	target := a.mergeTargetByID(repoID)
	if target == nil {
		return
	}
	targetCopy := *target
	if _, manual := a.manualMerges[repoID]; manual {
		if m.Item("解决冲突").Chosen() {
			a.openConflictResolver(repoID)
		}
		if m.Item("完成并推送").Chosen() {
			a.finishManualMerge(repoID, true)
		}
		if m.Item("完成（本地）").Chosen() {
			a.finishManualMerge(repoID, false)
		}
		if m.Item("放弃临时工作区").Chosen() {
			a.cleanupManualMerge(repoID)
		}
		return
	}
	if !mergeableTarget(targetCopy) {
		return
	}
	if m.Item("合并到 " + targetCopy.TargetBranch).Chosen() {
		a.TriggerMergeOne(targetCopy, false)
	}
	if m.Item("合并并推送到 " + targetCopy.TargetBranch).Chosen() {
		a.TriggerMergeOne(targetCopy, true)
	}
	if m.Item("AI 合并").Chosen() {
		a.triggerAIForTarget(targetCopy)
	}
}

// buildInlineAIMergeReport keeps the AI verification result visible without a
// detail pane: one compact line and an optional check list.
func (a *App) buildInlineAIMergeReport(c *ui.Context, p Palette) {
	rep := a.aiVerifyReport
	repo := *a.aiVerifyRepo
	col := p.Green
	title := "AI 已解决冲突并通过检查 · " + repo.Name
	if !rep.Passed {
		col = p.Danger
		title = "AI 检查未通过 · " + repo.Name + "：" + rep.FailureReason
	}
	ui.Column(c).FillWidth().Background(p.Surface).BorderWidth(0, 0, 1, 0).BorderColor(p.BorderSubtle).Children(func() {
		ui.Row(c).Padding(5, 10).Gap(6).AlignItems(ui.Center).Children(func() {
			drawIcon(c, icoCheck, 12, col)
			ui.Text(c, title).FontSize(10.5).TextColor(col).MaxLines(1).Grow(1)
			if rep.Passed {
				pill(c, p, "确认并推送", true, func() {
					if err := aimerge.ConfirmAndPushAIMerge(context.Background(), repo.AbsPath, a.mergeTargetBranch, rep.OrigBranch, true); err != nil {
						a.showToast("err", err.Error())
						return
					}
					a.showToast("ok", repo.Name+" 已推送合并结果")
					a.aiVerifyReport = nil
					a.RefreshAsync()
					a.TriggerMergeScan()
				})
			}
			miniBtn(c, p, "仅本地", func() {
				_ = aimerge.ConfirmAndPushAIMerge(context.Background(), repo.AbsPath, a.mergeTargetBranch, rep.OrigBranch, false)
				a.showToast("ok", repo.Name+" 已保留本地合并")
				a.aiVerifyReport = nil
				a.RefreshAsync()
			})
			miniBtn(c, p, "撤销", func() {
				_ = aimerge.RollbackAIMerge(context.Background(), repo.AbsPath, a.mergeTargetBranch, rep.OrigBranch, rep.PreTargetSHA)
				a.showToast("warn", repo.Name+" 已恢复到合并前")
				a.aiVerifyReport = nil
				a.RefreshAsync()
			})
			miniBtn(c, p, fmt.Sprintf("检查明细 (%d)", len(rep.Checks)), func() {
				a.showMergeCheckDetails = !a.showMergeCheckDetails
			})
		})
		if a.showMergeCheckDetails {
			ui.Scroll(c).MaxHeight(120).Children(func() {
				for _, chk := range rep.Checks {
					chkCopy := chk
					ck := c.Key("mchk-" + chkCopy.Name)
					ui.Row(ck).Padding(3, 10).Gap(6).Background(p.RailBg).Children(func() {
						col := p.Green
						label := "通过"
						if !chkCopy.Passed {
							col = p.Danger
							label = "失败"
						}
						ui.Text(ck, label).FontSize(9.5).FontWeight(weightBold).TextColor(col).Width(30)
						ui.Text(ck, chkCopy.Name).FontSize(10).FontWeight(weightMedium).TextColor(p.TextPrimary).Width(130)
						ui.Text(ck, chkCopy.Detail).FontSize(10).TextColor(p.TextMuted).Grow(1).MaxLines(1)
					})
				}
			})
		}
	})
}
