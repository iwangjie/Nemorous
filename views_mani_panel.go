package main

import (
	"fmt"
	"path/filepath"
	"sort"
	"strings"

	"github.com/egoist/mygo/ui"
)

// maniTaskNames returns the workspace task names in stable alphabetical order.
func (a *App) maniTaskNames() []string {
	if a.maniCfg == nil {
		return nil
	}
	names := make([]string, 0, len(a.maniCfg.Tasks))
	for name := range a.maniCfg.Tasks {
		names = append(names, name)
	}
	sort.Strings(names)
	return names
}

// buildRightToolRail renders the secondary tool stripe on the far right. It is
// only present when the Mani integration is enabled and a config was found.
func (a *App) buildRightToolRail(c *ui.Context, p Palette) {
	ui.Column(c).Width(40).FillHeight().Padding(4).Gap(4).Background(p.RailBg).BorderWidth(0, 0, 0, 1).BorderColor(p.Border).Children(func() {
		active := a.showManiPanel
		bg := ui.Transparent
		fg := p.TextControl
		if active {
			bg = p.Accent
			fg = p.AccentText
		}
		kc := c.Key("rail-mani")
		btn := ui.Column(kc).Height(32).FillWidth().Radius(6).Background(bg).Center().Cursor(ui.CursorPointer).Tooltip("Mani 任务").Label("Mani")
		if btn.Hovered() && !active {
			btn.Background(p.SurfaceHover)
			fg = p.TextPrimary
		}
		if btn.Clicked() {
			a.showManiPanel = !a.showManiPanel
		}
		btn.Children(func() {
			drawIcon(kc, icoTerminal, 15, fg)
		})
	})
}

func (a *App) maniToolButton(c *ui.Context, p Palette, key string, icon *ui.SVG, tip string, enabled, accent bool, onClick func()) {
	fg := p.TextControl
	if accent {
		fg = p.Accent
	}
	if !enabled {
		fg = p.TextMuted
	}
	kc := c.Key(key)
	btn := ui.Box(kc).Size(24, 24).Radius(4).Center()
	if enabled {
		btn.Cursor(ui.CursorPointer)
		if btn.Hovered() {
			btn.Background(p.SurfaceHover)
			if !accent {
				fg = p.TextPrimary
			}
		}
		if btn.Clicked() && onClick != nil {
			onClick()
		}
	}
	btn.Tooltip(tip)
	btn.Children(func() {
		drawIcon(kc, icon, 13, fg)
	})
}

// buildManiPanel is the Maven-style tool window: run tasks workspace-wide or
// target one project from its context menu.
func (a *App) buildManiPanel(c *ui.Context, p Palette) {
	if !a.maniAvailable() {
		return
	}
	names := a.maniTaskNames()
	if _, ok := a.maniCfg.Tasks[a.selectedManiTask]; !ok && len(names) > 0 {
		a.selectedManiTask = names[0]
	}
	spec, hasSpec := a.maniCfg.Tasks[a.selectedManiTask]
	runSelected := func() {
		if a.selectedManiTask != "" && !a.maniTaskRunning {
			a.TriggerManiTask(a.selectedManiTask, a.maniExtraEnv())
		}
	}

	ui.Column(c).Grow(1).FillHeight().Background(p.Surface).BorderWidth(0, 0, 0, 1).BorderColor(p.Border).Children(func() {
		// Header
		ui.Row(c).Height(40).Padding(0, 12).Justify(ui.SpaceBetween).AlignItems(ui.Center).BorderWidth(0, 0, 1, 0).BorderColor(p.Border).Children(func() {
			ui.Row(c).Gap(6).AlignItems(ui.Center).Grow(1).Children(func() {
				drawIcon(c, icoTerminal, 14, p.Accent)
				ui.Text(c, "Mani").FontSize(12.5).FontWeight(weightBold).TextColor(p.TextPrimary)
				ui.Text(c, fmt.Sprintf("%d 个任务", len(names))).FontSize(10.5).TextColor(p.TextMuted).MaxLines(1)
			})
			xb := ui.Box(c.Key("mani-close")).Size(20, 20).Radius(4).Center().Cursor(ui.CursorPointer).Tooltip("关闭")
			if xb.Hovered() {
				xb.Background(p.SurfaceHover)
			}
			if xb.Clicked() {
				a.showManiPanel = false
			}
			xb.Children(func() {
				drawIcon(c, icoClose, 10, p.TextMuted)
			})
		})

		// Toolbar
		ui.Row(c).Height(34).Padding(0, 10).Gap(2).AlignItems(ui.Center).BorderWidth(0, 0, 1, 0).BorderColor(p.Border).Children(func() {
			a.maniToolButton(c, p, "mani-refresh", icoRefresh, "重新加载 mani 配置", !a.maniTaskRunning, false, func() {
				a.RefreshAsync()
			})
			a.maniToolButton(c, p, "mani-collapse", icoChevronDown, "全部收起", !a.maniTaskRunning, false, func() {
				a.maniRootCollapsed = true
			})
			a.maniToolButton(c, p, "mani-run", icoPlay, "运行选中任务（全部项目）", !a.maniTaskRunning && hasSpec, true, runSelected)
			ui.Spacer(c)
			a.maniToolButton(c, p, "mani-settings", icoSettings, "设置", true, false, func() {
				a.showSettings = true
			})
			if a.maniTaskRunning {
				ui.Text(c, "运行中…").FontSize(10).TextColor(p.Accent)
			}
		})

		// Run parameters (collapsed by default, IDEA-style progressive disclosure)
		ui.Column(c).Padding(8, 10).Gap(6).BorderWidth(0, 0, 1, 0).BorderColor(p.Border).Children(func() {
			head := ui.Row(c.Key("mani-params-head")).Gap(4).AlignItems(ui.Center).Cursor(ui.CursorPointer)
			if head.Clicked() {
				a.maniParamsOpen = !a.maniParamsOpen
			}
			head.Children(func() {
				chev := icoChevronRight
				if a.maniParamsOpen {
					chev = icoChevronDown
				}
				drawIcon(c, chev, 9, p.TextMuted)
				ui.Text(c, "运行参数").FontSize(10.5).FontWeight(weightSemibold).TextColor(p.TextMuted)
				if !a.maniParamsOpen {
					parts := make([]string, 0, 2)
					if a.maniParamBranch != "" {
						parts = append(parts, "B="+a.maniParamBranch)
					}
					if a.maniParamTarget != "" {
						parts = append(parts, "TARGET="+a.maniParamTarget)
					}
					summary := "B / TARGET 可选"
					if len(parts) > 0 {
						summary = strings.Join(parts, " · ")
					}
					ui.Text(c, summary).FontSize(10).TextColor(p.TextMuted).MaxLines(1)
				}
			})
			if a.maniParamsOpen {
				ui.Row(c).Gap(6).Children(func() {
					ui.TextInput(c.Key("mani-b"), &a.maniParamBranch).Placeholder("B 分支（可选）").Grow(1)
					ui.TextInput(c.Key("mani-target"), &a.maniParamTarget).Placeholder("TARGET（可选）").Grow(1)
				})
				if hasSpec && spec.Cmd != "" {
					ui.Scroll(c).MaxHeight(54).Padding(6, 8).Radius(5).Background(p.RailBg).Children(func() {
						ui.Text(c, spec.Cmd).FontSize(10.5).TextColor(p.TextSecondary).MaxLines(2)
					})
				}
			}
		})

		// Maven-style tree
		ui.Scroll(c).Grow(1).Children(func() {
			rootName := filepath.Base(a.cfg.WorkspacePath)
			if rootName == "" || rootName == "." || rootName == string(filepath.Separator) {
				rootName = "workspace"
			}
			rkc := c.Key("mani-root")
			rootRow := ui.Row(rkc).Height(28).Padding(0, 10).Gap(5).Cursor(ui.CursorPointer).AlignItems(ui.Center)
			if rootRow.Hovered() {
				rootRow.Background(p.RailBg)
			}
			if rootRow.Clicked() {
				a.maniRootCollapsed = !a.maniRootCollapsed
			}
			rootRow.Children(func() {
				chev := icoChevronDown
				if a.maniRootCollapsed {
					chev = icoChevronRight
				}
				drawIcon(c, chev, 10, p.TextMuted)
				drawIcon(c, icoFolder, 13, p.TextControl)
				ui.Text(c, rootName).FontSize(12).FontWeight(weightSemibold).TextColor(p.TextPrimary).MaxLines(1)
				ui.Text(c, "mani.yaml").FontSize(10).TextColor(p.TextMuted)
			})
			if a.maniRootCollapsed {
				return
			}

			section := func(key, title string, count int, collapsed *bool, rows func()) {
				kc := c.Key("mani-sec-" + key)
				hdr := ui.Row(kc).Height(24).Padding(0, 10, 0, 22).Gap(4).Cursor(ui.CursorPointer).AlignItems(ui.Center)
				if hdr.Hovered() {
					hdr.Background(p.RailBg)
				}
				if hdr.Clicked() {
					*collapsed = !*collapsed
				}
				hdr.Children(func() {
					chev := icoChevronDown
					if *collapsed {
						chev = icoChevronRight
					}
					drawIcon(c, chev, 9, p.TextMuted)
					ui.Text(c, title).FontSize(10.5).FontWeight(weightSemibold).TextColor(p.TextMuted)
					ui.Text(c, fmt.Sprintf("%d", count)).FontSize(9.5).TextColor(p.TextMuted)
				})
				if !*collapsed {
					rows()
				}
			}

			section("tasks", "任务", len(names), &a.maniTasksCollapsed, func() {
				for _, name := range names {
					taskName := name
					selected := a.selectedManiTask == taskName
					tkc := c.Key("mani-task-" + taskName)
					row := ui.Row(tkc).Height(26).Padding(0, 10, 0, 34).Gap(5).Background(p.Surface).Cursor(ui.CursorPointer).AlignItems(ui.Center)
					if selected {
						row.Background(p.ActiveRowBg)
					} else if row.Hovered() {
						row.Background(p.RailBg)
					}
					if row.Clicked() {
						a.selectedManiTask = taskName
					}
					row.Children(func() {
						ts := a.maniCfg.Tasks[taskName]
						ui.Text(tkc, taskName).FontSize(11.5).FontWeight(weightMedium).TextColor(p.TextPrimary).MaxLines(1).MaxWidth(150).Shrink(0)
						if ts.Desc != "" {
							ui.Text(tkc, ts.Desc).FontSize(10).TextColor(p.TextMuted).MaxLines(1).Grow(1)
						} else {
							ui.Spacer(tkc)
						}
						playCol := p.TextMuted
						if selected {
							playCol = p.Accent
						}
						pb := ui.Box(c.Key("mani-run-"+taskName)).Size(16, 16).Radius(3).Center()
						if !a.maniTaskRunning {
							pb.Cursor(ui.CursorPointer)
							if pb.Hovered() {
								pb.Background(p.SurfaceHover)
								playCol = p.Accent
							}
							if pb.Clicked() {
								a.TriggerManiTask(taskName, a.maniExtraEnv())
							}
						} else {
							playCol = p.TextMuted
						}
						pb.Children(func() {
							drawIcon(c, icoPlay, 9, playCol)
						})
					})
				}
			})

			section("projects", "项目", len(a.repos), &a.maniProjectsCollapsed, func() {
				for _, repo := range a.repos {
					rCopy := repo
					badge := repoBadgeColors(rCopy.ColorIndex, p.IsDark)
					pkc := c.Key("mani-proj-" + rCopy.ID)
					row := ui.Row(pkc).Height(26).Padding(0, 10, 0, 34).Gap(5).Background(p.Surface).Cursor(ui.CursorPointer).AlignItems(ui.Center)
					if row.Hovered() {
						row.Background(p.RailBg)
					}
					row.PopoverMenu(func(m *ui.Menu) {
						m.Submenu("执行 Mani 任务", func(sm *ui.Menu) {
							for _, taskName := range names {
								nameCopy := taskName
								if sm.Item(nameCopy).Chosen() {
									a.TriggerManiTaskForRepo(nameCopy, rCopy.ID)
								}
							}
						})
						if m.Item("复制项目路径").Chosen() {
							a.pendingClipboard = rCopy.AbsPath
						}
					})
					row.Children(func() {
						ui.Box(pkc).Size(15, 15).Radius(3).Background(badge.Bg).Center().Children(func() {
							ui.Text(pkc, rCopy.Monogram).FontSize(8.5).FontWeight(weightBold).TextColor(badge.Text)
						})
						ui.Text(pkc, rCopy.Name).FontSize(11.5).FontWeight(weightMedium).TextColor(p.TextPrimary).MaxLines(1).Grow(1)
						if a.selectedManiTask != "" {
							pb := ui.Box(c.Key("mani-proj-run-"+rCopy.ID)).Size(16, 16).Radius(3).Center()
							if !a.maniTaskRunning {
								pb.Cursor(ui.CursorPointer)
								col := p.TextMuted
								if pb.Hovered() {
									pb.Background(p.SurfaceHover)
									col = p.Accent
								}
								if pb.Clicked() {
									a.TriggerManiTaskForRepo(a.selectedManiTask, rCopy.ID)
								}
								pb.Children(func() {
									drawIcon(c, icoPlay, 9, col)
								})
							}
						}
					})
				}
			})
		})

		// Results
		if a.maniTaskRunning || len(a.maniResults) > 0 {
			resultH := float32(28 + 24*len(a.maniResults))
			if resultH < 60 {
				resultH = 60
			}
			if resultH > 172 {
				resultH = 172
			}
			progressH := float32(0)
			if a.maniTaskRunning {
				progressH = 2
			}
			ui.Column(c).Height(resultH+progressH).Shrink(0).Background(p.Surface).BorderWidth(1, 0, 0, 0).BorderColor(p.Border).Children(func() {
				ui.Row(c).Height(28).Padding(0, 10).Justify(ui.SpaceBetween).AlignItems(ui.Center).Children(func() {
					title := a.maniLastTask
					if title == "" {
						title = "执行结果"
					}
					if a.maniLastScope != "" {
						title += " · " + a.maniLastScope
					}
					ui.Row(c).Gap(5).AlignItems(ui.Center).Children(func() {
						ui.Text(c, title).FontSize(10.5).FontWeight(weightSemibold).TextColor(p.TextMuted)
						ui.Text(c, fmt.Sprintf("%d", len(a.maniResults))).FontSize(9.5).TextColor(p.TextMuted)
					})
					if len(a.maniResults) > 0 && !a.maniTaskRunning {
						miniBtn(c, p, "清空", func() {
							a.maniResults = nil
						})
					}
				})
				if a.maniTaskRunning {
					ui.Progress(c, -1).Height(2).FillWidth()
				}
				ui.Scroll(c).Height(resultH - 28).Children(func() {
					for _, res := range a.maniResults {
						rCopy := res
						col, bg := p.Green, p.GreenSoft
						if rCopy.Status == "SKIP" {
							col, bg = p.Amber, p.AmberSoft
						} else if rCopy.Status == "FAIL" {
							col, bg = p.Danger, p.DangerSoft
						}
						rkc := c.Key("mani-result-" + rCopy.RepoID)
						row := ui.Row(rkc).Height(24).Padding(0, 10).Gap(6).Cursor(ui.CursorPointer).AlignItems(ui.Center)
						if row.Hovered() {
							row.Background(p.RailBg)
						}
						if row.Clicked() {
							a.maniResultTitle = a.maniLastTask + " · " + rCopy.RepoName
							a.maniResultOutput = rCopy.Output
							a.showManiResult = true
						}
						row.Children(func() {
							badgeBox(rkc, rCopy.Status, col, bg, ui.Color{}, 9.5, true)
							ui.Text(rkc, rCopy.RepoName).FontSize(10.5).TextColor(p.TextPrimary).MaxLines(1).Grow(1)
							ui.Text(rkc, fmt.Sprintf("%dms", rCopy.Duration.Milliseconds())).FontSize(9.5).TextColor(p.TextMuted)
						})
					}
				})
			})
		}
	})
}

func (a *App) buildManiResultModal(c *ui.Context, p Palette) {
	open := a.showManiResult
	ui.DialogBase(c, &open, func(backdrop, panel ui.Element) {
		if !open {
			a.showManiResult = false
			return
		}
		backdrop.Background(ui.RGBA(20, 22, 30, 0.42))
		panel.Width(760).MaxHeight(600).Padding(16).Gap(10).Radius(12).Background(p.Surface).Border(1, p.Border)
		ui.Row(c).Justify(ui.SpaceBetween).AlignItems(ui.Center).Children(func() {
			ui.Text(c, a.maniResultTitle).FontSize(12.5).FontWeight(weightSemibold).TextColor(p.TextPrimary).MaxLines(1).Grow(1)
			miniBtn(c, p, "关闭", func() {
				a.showManiResult = false
			})
		})
		ui.Scroll(c).MaxHeight(500).Padding(10).Radius(6).Background(p.RailBg).Children(func() {
			output := a.maniResultOutput
			if output == "" {
				output = "(无输出)"
			}
			ui.Text(c, output).Font("SF Mono, Menlo, monospace").FontSize(11).TextColor(p.TextSecondary)
		})
	})
}
