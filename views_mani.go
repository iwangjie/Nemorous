package main

import (
	"fmt"
	"sort"

	"github.com/egoist/mygo/ui"
)

func (a *App) buildManiView(c *ui.Context, p Palette) {
	if a.maniCfg == nil || len(a.maniCfg.Tasks) == 0 {
		ui.Column(c).Grow(1).Fill().Center().Padding(24).Gap(8).Background(p.AppBg).Children(func() {
			ui.Text(c, "未找到 mani.yaml 配置文件").FontSize(14).FontWeight(weightSemibold).TextColor(p.TextPrimary)
			ui.Text(c, "在工作区根目录放置 mani.yaml 后，可以在这里运行批量脚本。").FontSize(12).TextColor(p.TextMuted)
		})
		return
	}

	taskNames := make([]string, 0, len(a.maniCfg.Tasks))
	for k := range a.maniCfg.Tasks {
		taskNames = append(taskNames, k)
	}
	sort.Strings(taskNames)
	selectedSpec := a.maniCfg.Tasks[a.selectedManiTask]

	ui.Row(c).Grow(1).Fill().Padding(18, 22).Gap(14).Background(p.AppBg).AlignItems(ui.Stretch).Children(func() {
		// Left Task List Card
		ui.Column(c).Width(320).FillHeight().Radius(10).Background(p.Surface).Border(1, p.Border).Shrink(0).Children(func() {
			ui.Row(c).Padding(12, 14).Background(p.RailBg).BorderWidth(0, 0, 1, 0).BorderColor(p.Border).Justify(ui.SpaceBetween).AlignItems(ui.Center).Children(func() {
				ui.Text(c, "批量脚本 (mani.yaml)").FontSize(12.5).FontWeight(weightSemibold).TextColor(p.TextPrimary)
				ui.Text(c, fmt.Sprintf("%d 个任务", len(taskNames))).FontSize(11).TextColor(p.TextMuted)
			})

			ui.Scroll(c).Grow(1).Children(func() {
				for idx, name := range taskNames {
					tName := name
					spec := a.maniCfg.Tasks[tName]
					selected := a.selectedManiTask == tName
					rowBg := p.Surface
					if selected {
						rowBg = p.ActiveRowBg
					}

					kc := c.Key("mt-" + tName)
					row := ui.Column(kc).Padding(10, 14).Gap(4).Background(rowBg).Cursor(ui.CursorPointer)
					if idx < len(taskNames)-1 {
						row.BorderWidth(0, 0, 1, 0).BorderColor(p.BorderSubtle)
					}
					if row.Hovered() && !selected {
						row.Background(p.RailBg)
					}
					if row.Clicked() {
						a.selectedManiTask = tName
					}
					row.Children(func() {
						ui.Row(kc).Justify(ui.SpaceBetween).AlignItems(ui.Center).Children(func() {
							ui.Text(kc, tName).FontSize(12.5).FontWeight(weightSemibold).TextColor(p.TextPrimary)
							iconPill(kc, p, icoPlay, "运行", selected, func() {
								extra := map[string]string{}
								if a.maniParamBranch != "" {
									extra["B"] = a.maniParamBranch
								}
								if a.maniParamTarget != "" {
									extra["TARGET"] = a.maniParamTarget
								}
								a.TriggerManiTask(tName, extra)
							})
						})
						if spec.Desc != "" {
							ui.Text(kc, spec.Desc).FontSize(11.5).TextColor(p.TextMuted).MaxLines(2)
						}
					})
				}
			})
		})

		// Right Script Details & Output Card
		ui.Column(c).Grow(1).FillHeight().Radius(10).Background(p.Surface).Border(1, p.Border).Children(func() {
			ui.Column(c).Padding(14, 16).Gap(10).BorderWidth(0, 0, 1, 0).BorderColor(p.Border).Children(func() {
				ui.Row(c).Justify(ui.SpaceBetween).AlignItems(ui.Center).Children(func() {
					ui.Column(c).Gap(2).Children(func() {
						ui.Text(c, a.selectedManiTask).FontSize(14).FontWeight(weightBold).TextColor(p.TextPrimary)
						if selectedSpec.Desc != "" {
							ui.Text(c, selectedSpec.Desc).FontSize(12).TextColor(p.TextSecondary)
						}
					})
					iconPill(c, p, icoPlay, "运行脚本", true, func() {
						extra := map[string]string{}
						if a.maniParamBranch != "" {
							extra["B"] = a.maniParamBranch
						}
						if a.maniParamTarget != "" {
							extra["TARGET"] = a.maniParamTarget
						}
						a.TriggerManiTask(a.selectedManiTask, extra)
					})
				})

				// Optional environment variables
				allCands := a.BranchCandidatesForRepo("")
				ui.Row(c).Gap(10).AlignItems(ui.Center).Children(func() {
					ui.Text(c, "分支参数 (B)：").FontSize(11.5).TextColor(p.TextMuted)
					branchFuzzyInput(c.Key("mani-b"), p, &a.maniParamBranch, allCands, "可选，模糊搜索分支...", 175, 0, false, nil, nil)
					ui.Text(c, "目标分支 (TARGET)：").FontSize(11.5).TextColor(p.TextMuted)
					branchFuzzyInput(c.Key("mani-t"), p, &a.maniParamTarget, allCands, "可选，如 test", 145, 0, false, nil, nil)
				})

				if selectedSpec.Cmd != "" {
					ui.Scroll(c).MaxHeight(100).Padding(8, 10).Radius(6).Background(p.RailBg).Children(func() {
						ui.Text(c, selectedSpec.Cmd).FontSize(11.5).TextColor(p.TextSecondary)
					})
				}
			})

			ui.Row(c).Padding(8, 16).Background(p.RailBg).BorderWidth(0, 0, 1, 0).BorderColor(p.Border).Children(func() {
				ui.Text(c, fmt.Sprintf("执行结果 (%d)", len(a.maniResults))).FontSize(11.5).FontWeight(weightSemibold).TextColor(p.TextMuted)
			})

			ui.Scroll(c).Grow(1).Children(func() {
				if len(a.maniResults) == 0 {
					ui.Column(c).Padding(32).Center().Children(func() {
						ui.Text(c, "点击「运行脚本」查看各仓库执行输出").FontSize(12).TextColor(p.TextMuted)
					})
					return
				}
				for idx, r := range a.maniResults {
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
					kc := c.Key("mres-" + rCopy.RepoID)
					row := ui.Row(kc).Padding(9, 16).Gap(10).Background(p.Surface).AlignItems(ui.Center)
					if idx < len(a.maniResults)-1 {
						row.BorderWidth(0, 0, 1, 0).BorderColor(p.BorderSubtle)
					}
					row.Children(func() {
						badgeBox(kc, rCopy.Status, col, bg, ui.Color{}, 10.5, true)
						ui.Text(kc, rCopy.RepoName).FontSize(12.5).FontWeight(weightSemibold).TextColor(p.TextPrimary).Width(150)
						ui.Text(kc, rCopy.Output).FontSize(12).TextColor(p.TextSecondary).Grow(1)
						ui.Text(kc, fmt.Sprintf("%dms", rCopy.Duration.Milliseconds())).FontSize(11).TextColor(p.TextMuted)
					})
				}
			})
		})
	})
}
