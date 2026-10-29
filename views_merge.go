package main

import (
	"context"
	"fmt"
	"path/filepath"
	"strings"

	"github.com/egoist/mygo/ui"
	"github.com/egoist/mygo/ui/markdown"
	"github.com/iwangjie/nemorous/internal/aimerge"
	"github.com/iwangjie/nemorous/internal/gitops"
)

func (a *App) buildMergeView(c *ui.Context, p Palette) {
	// Preflight is deferred until the merge view is actually opened.
	if len(a.mergePlans) == 0 && !a.mergeRunning && !a.mergePreflightAttempted {
		a.mergePreflightAttempted = true
		a.TriggerMergePreflight(false)
	}
	conflictCnt := 0
	selectedClean := 0
	selectedAny := 0
	for _, pl := range a.mergePlans {
		checked := a.mergePlanChecked(pl.Repo.ID)
		if checked {
			selectedAny++
		}
		switch pl.Kind {
		case aimerge.PlanClean:
			if checked {
				selectedClean++
			}
		case aimerge.PlanConflict:
			conflictCnt++
		}
	}

	var selectedPlan *aimerge.RepoMergePlan
	for i := range a.mergePlans {
		if a.mergePlans[i].Repo.ID == a.selectedMergeRepoID {
			selectedPlan = &a.mergePlans[i]
			break
		}
	}

	targetPreset := func(b string) {
		active := a.mergeTargetBranch == b
		pill(c.Key("preset-"+b), p, b, active, func() {
			a.mergeTargetBranch = b
			a.TriggerMergePreflight(false)
		})
	}

	ui.Column(c).Grow(1).Fill().Padding(18, 22).Gap(12).Background(p.AppBg).Children(func() {
		// Top Merge Control Card
		ui.Column(c).Padding(12, 16).Gap(10).Radius(10).Background(p.Surface).Border(1, p.Border).Children(func() {
			ui.Row(c).Justify(ui.SpaceBetween).AlignItems(ui.Center).Children(func() {
				ui.Row(c).Gap(8).AlignItems(ui.Center).Children(func() {
					drawIcon(c, icoMerge, 15, p.Accent)
					ui.Text(c, "合并到目标分支：").FontSize(13).FontWeight(weightSemibold).TextColor(p.TextPrimary)
					for _, target := range a.mergeTargetOptions() {
						targetPreset(target)
					}
					miniBtn(c, p, "检查冲突", func() {
						a.TriggerMergePreflight(a.mergeFetchFirst)
					})
				})

				ui.Row(c).Gap(8).AlignItems(ui.Center).Children(func() {
					iconPill(c, p, icoBranch, "回到开发分支", false, func() {
						a.TriggerReturnToDevelopmentBranches()
					})
					if selectedClean > 0 {
						pill(c, p, fmt.Sprintf("合并选中 (%d)", selectedClean), false, func() {
							a.TriggerExecuteCleanMerges(false)
						})
						pill(c, p, fmt.Sprintf("合并并推送 (%d)", selectedClean), true, func() {
							a.TriggerExecuteCleanMerges(true)
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

			// Merge plans are user-defined: pick a target and a repository set, save it, reuse it next time.
			ui.Row(c).Wrap().Gap(6).AlignItems(ui.Center).Children(func() {
				ui.Text(c, fmt.Sprintf("合并方案（已选 %d/%d 个仓库）：", selectedAny, len(a.mergePlans))).FontSize(11).TextColor(p.TextMuted)
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
						ui.Checkbox(c, &a.mergeFetchFirst, "检查前先从远端拉取最新分支 (git fetch)")
						ui.Spacer(c)
						ui.Text(c, fmt.Sprintf("AI 冲突解决助手：%s", a.aiAgentName)).FontSize(11.5).TextColor(p.TextMuted)
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
								a.TriggerMergePreflight(false)
							},
						)
					})
				})
			}
		})

		// Main Content: Merge Table + Contextual Detail Panel
		showDetailPane := a.mergeRunning || a.aiVerifyReport != nil || (selectedPlan != nil && (selectedPlan.Kind == aimerge.PlanConflict || selectedPlan.Kind == aimerge.PlanClean))

		ui.Row(c).Grow(1).FillWidth().Gap(14).AlignItems(ui.Stretch).Children(func() {
			a.buildMergePlanList(c, p, conflictCnt)
			if showDetailPane {
				a.buildAIMergeRightPane(c, p, selectedPlan)
			}
		})
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

func (a *App) buildMergePlanList(c *ui.Context, p Palette, conflictCnt int) {
	allChecked := len(a.mergePlans) > 0
	for _, pl := range a.mergePlans {
		if !a.mergePlanChecked(pl.Repo.ID) {
			allChecked = false
			break
		}
	}
	ui.Column(c).Grow(1).FillHeight().Radius(10).Background(p.Surface).Border(1, p.Border).Children(func() {
		// Table Header
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
					for _, pl := range a.mergePlans {
						a.mergeCheckedRepos[pl.Repo.ID] = next
					}
				}
				chk.Children(func() {
					if allChecked {
						drawIcon(c, icoCheck, 11, p.AccentText)
					}
				})
			})
			ui.Box(c).Width(220).Children(func() {
				ui.Text(c, "仓库").FontSize(11.5).FontWeight(weightSemibold).TextColor(p.TextMuted)
			})
			ui.Box(c).Width(210).Children(func() {
				ui.Text(c, "合并方向").FontSize(11.5).FontWeight(weightSemibold).TextColor(p.TextMuted)
			})
			ui.Box(c).Grow(1).Children(func() {
				ui.Text(c, "状态说明").FontSize(11.5).FontWeight(weightSemibold).TextColor(p.TextMuted)
			})
			ui.Box(c).Width(230).AlignItems(ui.End).Children(func() {
				ui.Text(c, "操作").FontSize(11.5).FontWeight(weightSemibold).TextColor(p.TextMuted)
			})
		})

		ui.Scroll(c).Grow(1).Children(func() {
			for idx, pl := range a.mergePlans {
				if a.repoFilterID != "" && pl.Repo.ID != a.repoFilterID {
					continue
				}
				plCopy := pl
				selected := a.selectedMergeRepoID == pl.Repo.ID
				checked := a.mergePlanChecked(pl.Repo.ID)
				rowBg := p.Surface
				if selected {
					rowBg = p.ActiveRowBg
				}
				badge := repoBadgeColors(pl.Repo.ColorIndex, p.IsDark)

				kc := c.Key("mp-" + pl.Repo.ID)
				row := ui.Row(kc).Padding(11, 16).Background(rowBg).Cursor(ui.CursorPointer).AlignItems(ui.Center)
				if idx < len(a.mergePlans)-1 {
					row.BorderWidth(0, 0, 1, 0).BorderColor(p.BorderSubtle)
				}
				if row.Hovered() && !selected {
					row.Background(p.RailBg)
				}
				if row.Clicked() {
					a.selectedMergeRepoID = plCopy.Repo.ID
				}
				row.Children(func() {
					// Include in the merge batch
					ui.Box(kc).Width(30).Shrink(0).Children(func() {
						chk := ui.Box(kc).Size(15, 15).Radius(4).Cursor(ui.CursorPointer).Center()
						if checked {
							chk.Background(p.Accent)
						} else {
							chk.Background(p.Surface2).Border(1, p.BorderStrong)
						}
						if chk.Clicked() {
							a.mergeCheckedRepos[plCopy.Repo.ID] = !checked
						}
						chk.Children(func() {
							if checked {
								drawIcon(kc, icoCheck, 11, p.AccentText)
							}
						})
					})

					// Repo
					ui.Row(kc).Width(220).Gap(10).AlignItems(ui.Center).Shrink(0).Children(func() {
						ui.Box(kc).Size(26, 26).Radius(6).Background(badge.Bg).Center().Shrink(0).Children(func() {
							ui.Text(kc, plCopy.Repo.Monogram).FontSize(10.5).FontWeight(weightBold).TextColor(badge.Text)
						})
						ui.Text(kc, plCopy.Repo.Name).FontSize(13).FontWeight(weightSemibold).TextColor(p.TextPrimary).MaxLines(1)
					})

					// Branch direction
					ui.Row(kc).Width(210).Gap(6).AlignItems(ui.Center).Shrink(0).Children(func() {
						badgeBox(kc, fmt.Sprintf("%s → %s", plCopy.SourceBranch, plCopy.TargetBranch), p.TextControl, p.Surface2, ui.Color{}, 11, false)
					})

					// Summary
					ui.Column(kc).Grow(1).Padding(0, 10, 0, 0).Children(func() {
						ui.Text(kc, plCopy.Summary).FontSize(12).TextColor(p.TextSecondary).MaxLines(1)
					})

					// Status & Action
					ui.Row(kc).Width(230).Justify(ui.End).Gap(8).AlignItems(ui.Center).Shrink(0).Children(func() {
						switch plCopy.Kind {
						case aimerge.PlanClean:
							badgeBox(kc, fmt.Sprintf("可合并 (%d)", plCopy.AheadCommits), p.Green, p.GreenSoft, ui.Color{}, 11, true)
							pill(kc, p, "合并", true, func() {
								go func(plan aimerge.RepoMergePlan) {
									res := aimerge.ExecuteCleanMerge(context.Background(), plan, false)
									a.runUI(func() {
										a.lastBatchResults = []gitops.OpResult{res}
										a.showToast("ok", res.RepoName+": "+res.Message)
										a.RefreshAsync()
									})
								}(plCopy)
							})
						case aimerge.PlanConflict:
							badgeBox(kc, fmt.Sprintf("冲突 (%d)", len(plCopy.ConflictFiles)), p.Danger, p.DangerSoft, ui.Color{}, 11, true)
							miniBtn(kc, p, "手动", func() {
								a.triggerManualMerge(plCopy)
							})
							iconPill(kc, p, icoSparkles, "AI 解决冲突", true, func() {
								a.TriggerAIMergeConflict(plCopy)
							})
						case aimerge.PlanSkipDirty:
							badgeBox(kc, "有未提交改动", p.Amber, p.AmberSoft, ui.Color{}, 11, false)
						default:
							ui.Text(kc, "无变更").FontSize(11.5).TextColor(p.TextMuted)
						}
					})
				})
			}
		})
	})
}

func (a *App) buildAIMergeRightPane(c *ui.Context, p Palette, plan *aimerge.RepoMergePlan) {
	ui.Column(c).Width(440).FillHeight().Radius(10).Background(p.Surface).Border(1, p.Border).Shrink(0).Children(func() {
		if plan != nil {
			plCopy := *plan
			ui.Column(c).Padding(14).Gap(10).BorderWidth(0, 0, 1, 0).BorderColor(p.Border).Children(func() {
				ui.Row(c).Justify(ui.SpaceBetween).AlignItems(ui.Center).Children(func() {
					ui.Row(c).Gap(8).AlignItems(ui.Center).Children(func() {
						ui.Text(c, plCopy.Repo.Name).FontSize(14).FontWeight(weightBold).TextColor(p.TextPrimary)
						badgeBox(c, fmt.Sprintf("%s → %s", plCopy.SourceBranch, plCopy.TargetBranch), p.Accent, p.AccentSoft, ui.Color{}, 11, true)
					})
					if plCopy.Kind == aimerge.PlanConflict {
						miniBtn(c, p, "手动合并", func() {
							a.triggerManualMerge(plCopy)
						})
						iconPill(c, p, icoSparkles, "AI 解决冲突", true, func() {
							a.TriggerAIMergeConflict(plCopy)
						})
					} else if plCopy.Kind == aimerge.PlanClean {
						pill(c, p, "合并并推送", true, func() {
							go func(pr aimerge.RepoMergePlan) {
								res := aimerge.ExecuteCleanMerge(context.Background(), pr, true)
								a.runUI(func() {
									a.lastBatchResults = []gitops.OpResult{res}
									a.showToast("ok", res.RepoName+": "+res.Message)
									a.RefreshAsync()
								})
							}(plCopy)
						})
					}
				})

				if len(plCopy.ConflictFiles) > 0 {
					ui.Column(c).Padding(8, 10).Gap(4).Radius(6).Background(p.DangerSoft).Children(func() {
						ui.Text(c, "冲突文件：").FontSize(11).FontWeight(weightSemibold).TextColor(p.Danger)
						ui.Text(c, strings.Join(plCopy.ConflictFiles, "\n")).FontSize(11.5).TextColor(p.TextPrimary)
					})
				}

				if mm, ok := a.manualMerges[plCopy.Repo.ID]; ok {
					ui.Column(c).Padding(8, 10).Gap(6).Radius(6).Background(p.RailBg).Children(func() {
						ui.Text(c, "手动合并工作区").FontSize(11).FontWeight(weightSemibold).TextColor(p.TextSecondary)
						ui.Text(c, mm.Path).FontSize(10.5).TextColor(p.TextMuted).MaxLines(2)
						ui.Row(c).Wrap().Gap(6).Children(func() {
							miniBtn(c, p, "打开目录", func() { _ = revealPath(mm.Path) })
							miniBtn(c, p, "复制路径", func() { a.pendingClipboard = mm.Path })
							miniBtn(c, p, "清理工作区", func() { a.cleanupManualMerge(plCopy.Repo.ID) })
						})
						for _, rel := range mm.Conflicts {
							relCopy := rel
							ck := c.Key("mc-" + plCopy.Repo.ID + "-" + relCopy)
							ui.Row(ck).Gap(6).AlignItems(ui.Center).Children(func() {
								ui.Text(ck, relCopy).FontSize(11).TextColor(p.TextPrimary).Grow(1).MaxLines(1)
								miniBtn(ck, p, "打开", func() {
									_ = openPath(filepath.Join(mm.Path, relCopy))
								})
							})
						}
						ui.Text(c, "解决冲突后在该工作区提交即可；这里不会自动推送。").FontSize(10.5).TextColor(p.TextMuted)
					})
				}

				if len(plCopy.CommitPreview) > 0 {
					ui.Column(c).Padding(8, 10).Gap(4).Radius(6).Background(p.RailBg).Children(func() {
						ui.Text(c, "待合并的提交：").FontSize(11).FontWeight(weightSemibold).TextColor(p.TextMuted)
						ui.Text(c, strings.Join(plCopy.CommitPreview, "\n")).FontSize(11.5).TextColor(p.TextSecondary)
					})
				}
			})
		}

		// Verification & Confirmation Card (with collapsible check details)
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

				// Collapsible 6 verification checks
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
