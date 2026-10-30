package main

import (
	"context"
	"fmt"
	"path/filepath"
	"strings"
	"time"

	"github.com/egoist/mygo/ui"
	"github.com/egoist/mygo/ui/markdown"
	"github.com/iwangjie/nemorous/internal/aimerge"
)

func (a *App) buildMergeView(c *ui.Context, p Palette) {
	// A scan reads refs and counts; it never simulates a merge.
	if !a.mergeScanLoading && (len(a.mergeTargets) == 0 || time.Since(a.mergeScanAt) > 30*time.Second) {
		a.TriggerMergeScan()
	}

	selectedClean := 0
	selectedAny := 0
	conflictCnt := 0
	for _, target := range a.mergeTargets {
		checked := a.mergeRepoChecked(target.Repo.ID)
		if checked {
			selectedAny++
		}
		if _, hasManual := a.manualMerges[target.Repo.ID]; checked && mergeableTarget(target) && !hasManual {
			selectedClean++
		}
		if _, ok := a.manualMerges[target.Repo.ID]; ok {
			conflictCnt++
		}
	}
	selectedTarget := a.mergeTargetByID(a.selectedMergeRepoID)

	targetPreset := func(branch string) {
		active := a.mergeTargetBranch == branch
		pill(c.Key("preset-"+branch), p, branch, active, func() {
			a.mergeTargetBranch = branch
			a.mergeTargets = nil
			a.TriggerMergeScan()
		})
	}

	ui.Column(c).Grow(1).Fill().Padding(18, 22).Gap(12).Background(p.AppBg).Children(func() {
		ui.Column(c).Padding(12, 16).Gap(10).Radius(10).Background(p.Surface).Border(1, p.Border).Children(func() {
			ui.Row(c).Wrap().Justify(ui.SpaceBetween).Gap(8).AlignItems(ui.Center).Children(func() {
				ui.Row(c).Gap(8).AlignItems(ui.Center).Children(func() {
					drawIcon(c, icoMerge, 15, p.Accent)
					ui.Text(c, "合并到目标分支：").FontSize(13).FontWeight(weightSemibold).TextColor(p.TextPrimary)
					for _, branch := range a.mergeTargetOptions() {
						targetPreset(branch)
					}
					statusLabel := "刷新状态"
					if a.mergeScanLoading {
						statusLabel = "读取中…"
					}
					miniBtn(c, p, statusLabel, func() {
						a.TriggerMergeScan()
					})
				})

				ui.Row(c).Gap(8).AlignItems(ui.Center).Children(func() {
					iconPill(c, p, icoBranch, "回到开发分支", false, func() {
						a.TriggerReturnToDevelopmentBranches()
					})
					if selectedClean > 0 {
						pill(c, p, fmt.Sprintf("合并选中 (%d)", selectedClean), false, func() {
							a.TriggerExecuteMerges(false)
						})
						pill(c, p, fmt.Sprintf("合并并推送 (%d)", selectedClean), true, func() {
							a.TriggerExecuteMerges(true)
						})
					} else {
						pillDisabled(c, p, "合并选中 (0)")
						pillDisabled(c, p, "合并并推送 (0)")
					}
					ghostBtn(c, p, icoMore, "", a.showMergeAdvanced, func() {
						a.showMergeAdvanced = !a.showMergeAdvanced
					})
				})
			})

			ui.Row(c).Wrap().Gap(6).AlignItems(ui.Center).Children(func() {
				ui.Text(c, fmt.Sprintf("合并方案（已选 %d/%d 个仓库）：", selectedAny, len(a.mergeTargets))).FontSize(11).TextColor(p.TextMuted)
				for _, preset := range a.cfg.MergePresets {
					prCopy := preset
					pill(c.Key("mpreset-"+prCopy.Name), p, fmt.Sprintf("%s · %s (%d)", prCopy.Name, prCopy.TargetBranch, len(prCopy.RepoIDs)), false, func() {
						a.applyMergePreset(prCopy)
					})
					miniBtn(c.Key("mpreset-del-"+prCopy.Name), p, "×", func() {
						a.cfg.DeleteMergePreset(prCopy.Name)
						_ = a.cfg.Save()
					})
				}
				ui.TextInput(c.Key("merge-preset-in"), &a.mergePresetInput).Placeholder("方案名称，如：合并到 test").Width(180)
				miniBtn(c, p, "保存当前勾选", func() {
					a.saveMergePreset()
				})
			})

			if a.showMergeAdvanced {
				ui.Column(c).Padding(8, 0, 0, 0).Gap(8).BorderWidth(1, 0, 0, 0).BorderColor(p.BorderSubtle).Children(func() {
					ui.Row(c).Gap(12).AlignItems(ui.Center).Children(func() {
						ui.Text(c, fmt.Sprintf("直接合并：在临时 worktree 中执行；冲突会保留工作区，绝不 force push。AI 助手：%s", a.aiAgentName)).
							FontSize(11).TextColor(p.TextMuted)
					})
					ui.Row(c).Gap(8).AlignItems(ui.Center).Children(func() {
						ui.Text(c, "自定义目标分支：").FontSize(11.5).TextColor(p.TextMuted)
						branchFuzzyInput(
							c.Key("merge-target-in"),
							p,
							&a.mergeTargetBranch,
							a.BranchCandidatesForRepo(""),
							"搜索或输入其他目标分支...",
							200,
							0,
							false,
							nil,
							func(_ string) {
								a.mergeTargets = nil
								a.TriggerMergeScan()
							},
						)
					})
				})
			}
		})

		showDetail := a.mergeRunning || a.aiVerifyReport != nil || selectedTarget != nil
		if showDetail {
			ui.SplitPercent(c, &a.mergeDetailPct,
				func() { a.buildMergeTable(c, p) },
				func() { a.buildMergeDetail(c, p, selectedTarget) },
			).Grow(1).FillHeight()
		} else {
			a.buildMergeTable(c, p)
		}
	})
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

func (a *App) buildMergeTable(c *ui.Context, p Palette) {
	allChecked := len(a.mergeTargets) > 0
	for _, target := range a.mergeTargets {
		if mergeableTarget(target) && !a.mergeRepoChecked(target.Repo.ID) {
			allChecked = false
			break
		}
	}

	ui.Column(c).Grow(1).FillHeight().Radius(10).Background(p.Surface).Border(1, p.Border).Children(func() {
		ui.Row(c).Padding(10, 16).Background(p.RailBg).BorderWidth(0, 0, 1, 0).BorderColor(p.Border).AlignItems(ui.Center).Children(func() {
			ui.Box(c).Width(30).Children(func() {
				chk := ui.Box(c.Key("merge-all")).Size(15, 15).Radius(4).Cursor(ui.CursorPointer).Center()
				if allChecked {
					chk.Background(p.Accent)
				} else {
					chk.Background(p.Surface2).Border(1, p.BorderStrong)
				}
				if chk.Clicked() {
					next := !allChecked
					for _, target := range a.mergeTargets {
						if mergeableTarget(target) {
							a.mergeCheckedRepos[target.Repo.ID] = next
						}
					}
				}
				chk.Children(func() {
					if allChecked {
						drawIcon(c, icoCheck, 11, p.AccentText)
					}
				})
			})
			ui.Box(c).Width(150).Children(func() {
				ui.Text(c, "仓库").FontSize(11.5).FontWeight(weightSemibold).TextColor(p.TextMuted).SingleLine()
			})
			ui.Box(c).Width(160).Children(func() {
				ui.Text(c, "合并方向").FontSize(11.5).FontWeight(weightSemibold).TextColor(p.TextMuted).SingleLine()
			})
			ui.Box(c).Grow(1).Children(func() {
				ui.Text(c, "状态说明").FontSize(11.5).FontWeight(weightSemibold).TextColor(p.TextMuted).SingleLine()
			})
			ui.Box(c).Width(190).AlignItems(ui.End).Children(func() {
				ui.Text(c, "操作").FontSize(11.5).FontWeight(weightSemibold).TextColor(p.TextMuted).SingleLine()
			})
		})

		ui.Scroll(c).Grow(1).Children(func() {
			for idx, target := range a.mergeTargets {
				targetCopy := target
				selected := a.selectedMergeRepoID == targetCopy.Repo.ID
				checked := a.mergeRepoChecked(targetCopy.Repo.ID)
				badge := repoBadgeColors(targetCopy.Repo.ColorIndex, p.IsDark)
				rowBg := p.Surface
				if selected {
					rowBg = p.ActiveRowBg
				}
				manual, hasManual := a.manualMerges[targetCopy.Repo.ID]

				kc := c.Key("mt-" + targetCopy.Repo.ID)
				row := ui.Row(kc).Padding(11, 16).Background(rowBg).Cursor(ui.CursorPointer).AlignItems(ui.Center)
				if idx < len(a.mergeTargets)-1 {
					row.BorderWidth(0, 0, 1, 0).BorderColor(p.BorderSubtle)
				}
				if row.Hovered() && !selected {
					row.Background(p.RailBg)
				}
				if row.Clicked() {
					a.selectedMergeRepoID = targetCopy.Repo.ID
				}
				row.Children(func() {
					ui.Box(kc).Width(30).Shrink(0).Children(func() {
						chk := ui.Box(kc).Size(15, 15).Radius(4).Cursor(ui.CursorPointer).Center()
						if checked {
							chk.Background(p.Accent)
						} else {
							chk.Background(p.Surface2).Border(1, p.BorderStrong)
						}
						if chk.Clicked() {
							a.mergeCheckedRepos[targetCopy.Repo.ID] = !checked
						}
						chk.Children(func() {
							if checked {
								drawIcon(kc, icoCheck, 11, p.AccentText)
							}
						})
					})

					ui.Row(kc).Width(150).Gap(10).AlignItems(ui.Center).Shrink(0).Children(func() {
						ui.Box(kc).Size(26, 26).Radius(6).Background(badge.Bg).Center().Shrink(0).Children(func() {
							ui.Text(kc, targetCopy.Repo.Monogram).FontSize(10.5).FontWeight(weightBold).TextColor(badge.Text)
						})
						ui.Text(kc, targetCopy.Repo.Name).FontSize(13).FontWeight(weightSemibold).TextColor(p.TextPrimary).MaxLines(1)
					})

					ui.Row(kc).Width(160).Gap(6).AlignItems(ui.Center).Shrink(0).Children(func() {
						source := targetCopy.SourceBranch
						if targetCopy.Detached {
							source = "(detached)"
						}
						badgeBox(kc, fmt.Sprintf("%s → %s", source, targetCopy.TargetBranch), p.TextControl, p.Surface2, ui.Color{}, 11, false)
					})

					ui.Column(kc).Grow(1).Padding(0, 10, 0, 0).Children(func() {
						switch {
						case hasManual:
							ui.Text(kc, fmt.Sprintf("冲突待解决（%d 个文件），临时工作区已保留", len(manual.Conflicts))).FontSize(11.5).TextColor(p.Danger)
						case !targetCopy.TargetExists:
							ui.Text(kc, fmt.Sprintf("未找到目标分支 %s", targetCopy.TargetBranch)).FontSize(11.5).TextColor(p.Amber)
						case targetCopy.Detached:
							ui.Text(kc, "detached HEAD，无法合并").FontSize(11.5).TextColor(p.Amber)
						case targetCopy.SourceBranch == targetCopy.TargetBranch:
							ui.Text(kc, "已在目标分支").FontSize(11.5).TextColor(p.TextMuted)
						case targetCopy.AheadCommits == 0:
							ui.Text(kc, "没有需要合并的提交").FontSize(11.5).TextColor(p.TextMuted)
						default:
							ui.Text(kc, fmt.Sprintf("%d 个提交待合并", targetCopy.AheadCommits)).FontSize(12).TextColor(p.TextSecondary)
						}
						if targetCopy.Dirty {
							ui.Text(kc, "未提交改动不会进入合并").FontSize(10.5).TextColor(p.Amber)
						}
					})

					ui.Row(kc).Width(190).Justify(ui.End).Gap(6).AlignItems(ui.Center).Shrink(0).Children(func() {
						if hasManual {
							miniBtn(kc, p, "处理冲突", func() {
								a.selectedMergeRepoID = targetCopy.Repo.ID
							})
							return
						}
						if mergeableTarget(targetCopy) {
							badgeBox(kc, fmt.Sprintf("待合并 %d", targetCopy.AheadCommits), p.Green, p.GreenSoft, ui.Color{}, 10.5, true)
							miniBtn(kc, p, "合并", func() {
								a.TriggerMergeOne(targetCopy, false)
							})
							miniBtn(kc, p, "合并+推", func() {
								a.TriggerMergeOne(targetCopy, true)
							})
						} else if targetCopy.TargetExists && targetCopy.AheadCommits == 0 && targetCopy.SourceBranch != targetCopy.TargetBranch {
							ui.Text(kc, "无新提交").FontSize(11).TextColor(p.TextMuted)
						}
					})
				})
			}
		})
	})
}

func (a *App) buildMergeDetail(c *ui.Context, p Palette, target *aimerge.MergeTarget) {
	ui.Column(c).FillWidth().FillHeight().Radius(10).Background(p.Surface).Border(1, p.Border).Children(func() {
		if target != nil {
			targetCopy := *target
			manual, hasManual := a.manualMerges[targetCopy.Repo.ID]
			ui.Column(c).Padding(14).Gap(10).BorderWidth(0, 0, 1, 0).BorderColor(p.Border).Children(func() {
				ui.Row(c).Justify(ui.SpaceBetween).Wrap().Gap(6).AlignItems(ui.Center).Children(func() {
					ui.Row(c).Gap(8).Wrap().AlignItems(ui.Center).Children(func() {
						ui.Text(c, targetCopy.Repo.Name).FontSize(14).FontWeight(weightBold).TextColor(p.TextPrimary)
						badgeBox(c, fmt.Sprintf("%s → %s", targetCopy.SourceBranch, targetCopy.TargetBranch), p.Accent, p.AccentSoft, ui.Color{}, 11, true)
					})
					ui.Row(c).Gap(6).Children(func() {
						if mergeableTarget(targetCopy) && !hasManual {
							pill(c, p, "合并", false, func() { a.TriggerMergeOne(targetCopy, false) })
							pill(c, p, "合并并推送", true, func() { a.TriggerMergeOne(targetCopy, true) })
							miniBtn(c, p, "AI 合并", func() { a.triggerAIForTarget(targetCopy) })
						}
					})
				})

				ui.Row(c).Wrap().Gap(6).Children(func() {
					if targetCopy.AheadCommits > 0 {
						badgeBox(c, fmt.Sprintf("待合并 %d 个提交", targetCopy.AheadCommits), p.Green, p.GreenSoft, ui.Color{}, 10.5, true)
					}
					if targetCopy.BehindCommits > 0 {
						badgeBox(c, fmt.Sprintf("目标领先 %d", targetCopy.BehindCommits), p.Amber, p.AmberSoft, ui.Color{}, 10.5, false)
					}
					if targetCopy.Dirty {
						badgeBox(c, "有未提交改动", p.Amber, p.AmberSoft, ui.Color{}, 10.5, false)
					}
					if hasManual {
						badgeBox(c, "冲突待解决", p.Danger, p.DangerSoft, ui.Color{}, 10.5, true)
					}
				})

				if len(targetCopy.CommitPreview) > 0 {
					ui.Column(c).Padding(8, 10).Gap(4).Radius(6).Background(p.RailBg).Children(func() {
						ui.Text(c, "待合并的提交：").FontSize(11).FontWeight(weightSemibold).TextColor(p.TextMuted)
						ui.Text(c, strings.Join(targetCopy.CommitPreview, "\n")).FontSize(11.5).TextColor(p.TextSecondary).MaxLines(8)
					})
				}

				if hasManual {
					ui.Column(c).Padding(8, 10).Gap(6).Radius(6).Background(p.RailBg).Children(func() {
						ui.Text(c, "手动合并工作区").FontSize(11).FontWeight(weightSemibold).TextColor(p.TextSecondary)
						ui.Text(c, manual.Path).FontSize(10.5).TextColor(p.TextMuted).MaxLines(2)
						ui.Row(c).Wrap().Gap(6).Children(func() {
							pill(c, p, "完成并推送", true, func() { a.finishManualMerge(targetCopy.Repo.ID, true) })
							miniBtn(c, p, "完成（本地）", func() { a.finishManualMerge(targetCopy.Repo.ID, false) })
							miniBtn(c, p, "放弃", func() { a.cleanupManualMerge(targetCopy.Repo.ID) })
							miniBtn(c, p, "打开目录", func() { _ = revealPath(manual.Path) })
							miniBtn(c, p, "复制路径", func() { a.pendingClipboard = manual.Path })
						})
						for _, rel := range manual.Conflicts {
							relCopy := rel
							ck := c.Key("mc-" + targetCopy.Repo.ID + "-" + relCopy)
							ui.Row(ck).Gap(6).AlignItems(ui.Center).Children(func() {
								ui.Text(ck, relCopy).FontSize(11).TextColor(p.TextPrimary).Grow(1).MaxLines(1)
								miniBtn(ck, p, "打开", func() {
									_ = openPath(filepath.Join(manual.Path, relCopy))
								})
							})
						}
						ui.Text(c, "在临时分支解决冲突后点“完成”：工具会提交、更新本地目标分支，推送需明确点击。").FontSize(10.5).TextColor(p.TextMuted)
					})
				}
			})
		}

		if a.aiVerifyReport != nil && a.aiVerifyRepo != nil {
			rep := a.aiVerifyReport
			repo := *a.aiVerifyRepo
			headerCol := p.Green
			headerTitle := "冲突已解决并通过安全检查"
			if !rep.Passed {
				headerCol = p.Danger
				headerTitle = "安全检查未通过：" + rep.FailureReason
			}
			ui.Column(c).Padding(14).Gap(10).Background(p.RailBg).BorderWidth(0, 0, 1, 0).BorderColor(p.Border).Children(func() {
				ui.Text(c, headerTitle).FontSize(13).FontWeight(weightSemibold).TextColor(headerCol)
				ui.Row(c).Wrap().Gap(6).Children(func() {
					if rep.Passed {
						pill(c, p, "确认合并并推送", true, func() {
							err := aimerge.ConfirmAndPushAIMerge(context.Background(), repo.AbsPath, a.mergeTargetBranch, rep.OrigBranch, true)
							if err != nil {
								a.showToast("err", err.Error())
							} else {
								a.showToast("ok", fmt.Sprintf("%s 已推送合并结果并切回 %s", repo.Name, rep.OrigBranch))
								a.aiVerifyReport = nil
								a.RefreshAsync()
								a.TriggerMergeScan()
							}
						})
					}
					pill(c, p, "仅保留本地合并", false, func() {
						_ = aimerge.ConfirmAndPushAIMerge(context.Background(), repo.AbsPath, a.mergeTargetBranch, rep.OrigBranch, false)
						a.showToast("ok", fmt.Sprintf("%s 已保留本地合并并切回 %s", repo.Name, rep.OrigBranch))
						a.aiVerifyReport = nil
						a.RefreshAsync()
					})
					pill(c, p, "撤销合并", false, func() {
						_ = aimerge.RollbackAIMerge(context.Background(), repo.AbsPath, a.mergeTargetBranch, rep.OrigBranch, rep.PreTargetSHA)
						a.showToast("warn", fmt.Sprintf("%s 已恢复到合并前状态", repo.Name))
						a.aiVerifyReport = nil
						a.RefreshAsync()
					})
				})
				chkToggle := ui.Row(c.Key("mchk-toggle")).Padding(4, 0).Gap(4).Cursor(ui.CursorPointer).AlignItems(ui.Center)
				if chkToggle.Clicked() {
					a.showMergeCheckDetails = !a.showMergeCheckDetails
				}
				chkToggle.Children(func() {
					chev := icoChevronRight
					if a.showMergeCheckDetails {
						chev = icoChevronDown
					}
					drawIcon(c, chev, 11, p.TextMuted)
					ui.Text(c, fmt.Sprintf("查看检查明细 (%d 项)", len(rep.Checks))).FontSize(11).TextColor(p.TextMuted)
				})
				if a.showMergeCheckDetails {
					for _, chk := range rep.Checks {
						chkCopy := chk
						col := p.Green
						label := "通过"
						if !chkCopy.Passed {
							col = p.Danger
							label = "失败"
						}
						ck := c.Key("chk-" + chkCopy.Name)
						ui.Row(ck).Padding(4, 8).Gap(6).Radius(4).Background(p.Surface).AlignItems(ui.Center).Children(func() {
							ui.Text(ck, label).FontSize(10.5).FontWeight(weightBold).TextColor(col).Width(32)
							ui.Text(ck, chkCopy.Name).FontSize(11).FontWeight(weightMedium).TextColor(p.TextPrimary).Width(140)
							ui.Text(ck, chkCopy.Detail).FontSize(11).TextColor(p.TextMuted).Grow(1)
						})
					}
				}
			})
		}

		mdText := a.aiStreamRaw.String()
		if mdText != "" {
			ui.Scroll(c).Grow(1).Padding(14).Children(func() {
				if a.mergeRunning {
					markdown.Stream(c, mdText, &a.mdOpts).FillWidth()
				} else {
					markdown.View(c, mdText, &a.mdOpts).FillWidth()
				}
			})
		}
	})
}
