package main

import (
	"context"
	"fmt"
	"path/filepath"

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

func (a *App) buildChangesLeftPane(c *ui.Context, p Palette) {
	totalFiles := 0
	checkedFiles := 0
	checkedRepos := 0
	var dirtyStatuses []gitops.RepoStatus
	var cleanStatuses []gitops.RepoStatus

	for _, st := range a.statuses {
		if a.repoFilterID != "" && st.Repo.ID != a.repoFilterID {
			continue
		}
		if len(st.Files) > 0 {
			dirtyStatuses = append(dirtyStatuses, st)
			repoHasChecked := false
			for _, f := range st.Files {
				totalFiles++
				if a.checkedChanges[f.Key()] {
					checkedFiles++
					repoHasChecked = true
				}
			}
			if repoHasChecked {
				checkedRepos++
			}
		} else {
			cleanStatuses = append(cleanStatuses, st)
		}
	}

	ui.Column(c).Fill().Padding(12).Gap(10).Background(p.RailBg).Children(func() {
		// Top Section Header
		ui.Row(c).Justify(ui.SpaceBetween).AlignItems(ui.Center).Children(func() {
			title := "本地变更"
			if totalFiles > 0 {
				title = fmt.Sprintf("本地变更 (%d/%d)", checkedFiles, totalFiles)
			}
			ui.Text(c, title).FontSize(12.5).FontWeight(weightSemibold).TextColor(p.TextPrimary)
			if totalFiles > 0 {
				ui.Row(c).Gap(4).Children(func() {
					miniBtn(c, p, "全选", func() {
						for _, st := range dirtyStatuses {
							for _, f := range st.Files {
								a.checkedChanges[f.Key()] = true
							}
						}
					})
					miniBtn(c, p, "清空", func() {
						for k := range a.checkedChanges {
							a.checkedChanges[k] = false
						}
					})
				})
			}
		})

		// Scrollable Repo Changes + Collapsed Clean Repos
		ui.Scroll(c).Grow(1).Gap(8).Children(func() {
			for _, st := range dirtyStatuses {
				stCopy := st
				badge := repoBadgeColors(st.Repo.ColorIndex, p.IsDark)

				repoAllChecked := true
				for _, f := range st.Files {
					if !a.checkedChanges[f.Key()] {
						repoAllChecked = false
						break
					}
				}

				gkc := c.Key("grp-" + st.Repo.ID)
				ui.Column(gkc).Radius(8).Background(p.Surface).Border(1, p.Border).Children(func() {
					// Repo Group Header
					hdr := ui.Row(gkc).Padding(8, 10).Gap(7).Background(p.Surface).BorderWidth(0, 0, 1, 0).BorderColor(p.BorderSubtle).AlignItems(ui.Center)
					hdr.Children(func() {
						chkBox := ui.Box(gkc).Size(15, 15).Radius(4).Cursor(ui.CursorPointer).Center()
						if repoAllChecked {
							chkBox.Background(p.Accent)
						} else {
							chkBox.Background(p.Surface2).Border(1, p.BorderStrong)
						}
						if chkBox.Clicked() {
							next := !repoAllChecked
							for _, f := range stCopy.Files {
								a.checkedChanges[f.Key()] = next
							}
						}
						chkBox.Children(func() {
							if repoAllChecked {
								drawIcon(gkc, icoCheck, 11, p.AccentText)
							}
						})

						ui.Box(gkc).Size(18, 18).Radius(4).Background(badge.Bg).Center().Children(func() {
							ui.Text(gkc, stCopy.Repo.Monogram).FontSize(9.5).FontWeight(weightBold).TextColor(badge.Text)
						})
						ui.Text(gkc, stCopy.Repo.Name).FontSize(12.5).FontWeight(weightSemibold).TextColor(p.TextPrimary).SingleLine()
						badgeBox(gkc, stCopy.Branch, p.TextControl, p.Surface2, ui.Color{}, 10.5, false)
						ui.Spacer(gkc)
						ui.Text(gkc, fmt.Sprintf("%d", len(stCopy.Files))).FontSize(11).TextColor(p.TextMuted)
					})

					// Changed File Rows
					for idx, cf := range stCopy.Files {
						cfCopy := cf
						key := cfCopy.Key()
						isSelected := a.selectedChangeKey == key
						isChecked := a.checkedChanges[key]

						rowBg := p.Surface
						if isSelected {
							rowBg = p.ActiveRowBg
						}

						kindColor := p.Amber
						switch cfCopy.Kind {
						case "A", "?":
							kindColor = p.Green
						case "D", "U":
							kindColor = p.Danger
						}

						fkc := c.Key("cf-" + key)
						fRow := ui.Row(fkc).Padding(7, 10).Gap(7).Background(rowBg).Cursor(ui.CursorPointer).AlignItems(ui.Center)
						if idx < len(stCopy.Files)-1 {
							fRow.BorderWidth(0, 0, 1, 0).BorderColor(p.BorderSubtle)
						}
						if fRow.Hovered() && !isSelected {
							fRow.Background(p.RailBg)
						}
						fRow.Children(func() {
							fChk := ui.Box(fkc).Size(15, 15).Radius(4).Cursor(ui.CursorPointer).Center().Shrink(0)
							if isChecked {
								fChk.Background(p.Accent)
							} else {
								fChk.Background(p.Surface2).Border(1, p.BorderStrong)
							}
							if fChk.Clicked() {
								a.checkedChanges[key] = !a.checkedChanges[key]
							}
							fChk.Children(func() {
								if isChecked {
									drawIcon(fkc, icoCheck, 11, p.AccentText)
								}
							})

							ui.Text(fkc, cfCopy.Kind).FontSize(10.5).FontWeight(weightBold).TextColor(kindColor).Shrink(0)

							baseName := filepath.Base(cfCopy.Path)
							dirName := filepath.Dir(cfCopy.Path)

							infoCol := ui.Row(fkc).Grow(1).Gap(6).AlignItems(ui.Center).Cursor(ui.CursorPointer)
							if infoCol.Clicked() {
								a.selectChangedFile(cfCopy)
							}
							infoCol.Children(func() {
								ui.Text(fkc, baseName).FontSize(12).FontWeight(weightMedium).TextColor(p.TextPrimary).MaxLines(1)
								if dirName != "." {
									ui.Text(fkc, dirName).FontSize(11).TextColor(p.TextMuted).MaxLines(1)
								}
								ui.Spacer(fkc)
								if !isSelected {
									ui.Text(fkc, fmt.Sprintf("+%d -%d", cfCopy.AddedLines, cfCopy.DeletedLines)).FontSize(10.5).TextColor(p.TextMuted).Shrink(0)
								}
							})

							// Progressive Disclosure: show per-file Stage / Discard buttons only on the active file row
							if isSelected {
								ui.Row(fkc).Gap(4).Shrink(0).Children(func() {
									stageLabel := "暂存"
									if cfCopy.Staged {
										stageLabel = "取消暂存"
									}
									miniBtn(fkc, p, stageLabel, func() {
										ctx := context.Background()
										if cfCopy.Staged {
											_ = gitops.UnstageFile(ctx, cfCopy.RepoPath, cfCopy.Path)
										} else {
											_ = gitops.StageFile(ctx, cfCopy.RepoPath, cfCopy.Path)
										}
										a.RefreshAsync()
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
				})
			}

			if len(dirtyStatuses) == 0 {
				ui.Column(c).Padding(16).Gap(6).Radius(8).Background(p.Surface).Border(1, p.Border).Children(func() {
					ui.Text(c, "所有仓库均无未提交改动").FontSize(13).FontWeight(weightSemibold).TextColor(p.TextPrimary)
					ui.Text(c, "工作区保持干净。你可以随时切换分支、拉取最新代码或浏览文件。").FontSize(11.5).TextColor(p.TextMuted)
				})
			}

			// Collapsible Clean Repositories Section ("不常用操作折叠起来，必要时候也能用")
			if len(cleanStatuses) > 0 {
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

				if a.showCleanReposInChanges {
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
			}
		})

		// Bottom Commit Area
		a.buildCommitBox(c, p, checkedRepos, checkedFiles)
	})
}

func (a *App) buildCommitBox(c *ui.Context, p Palette, checkedRepos, checkedFiles int) {
	commitBtnLabel := "提交"
	if checkedFiles > 0 {
		if checkedRepos > 1 {
			commitBtnLabel = fmt.Sprintf("提交 (%d 个仓库)", checkedRepos)
		} else {
			commitBtnLabel = fmt.Sprintf("提交 (%d 个文件)", checkedFiles)
		}
	}
	if a.commitScope == "current" && a.selectedChangeFile != nil {
		commitBtnLabel = fmt.Sprintf("仅提交 %s", a.selectedChangeFile.RepoName)
	}

	ui.Column(c).Padding(10).Gap(8).Radius(8).Background(p.Surface).Border(1, p.Border).Children(func() {
		if ui.TextInput(c.Key("commit-msg"), &a.commitMessage).Bind(&a.commitInputHandle).Placeholder("输入提交说明...").FillWidth().Submitted() {
			a.TriggerCommit(false)
		}

		// Collapsible commit options (prefixes & scope)
		ui.Row(c).Justify(ui.SpaceBetween).AlignItems(ui.Center).Children(func() {
			optToggle := ui.Row(c.Key("commit-opt-toggle")).Padding(2, 4).Gap(4).Radius(4).Cursor(ui.CursorPointer).AlignItems(ui.Center)
			if optToggle.Clicked() {
				a.showCommitOptions = !a.showCommitOptions
			}
			optToggle.Children(func() {
				chev := icoChevronRight
				if a.showCommitOptions {
					chev = icoChevronDown
				}
				drawIcon(c, chev, 11, p.TextMuted)
				ui.Text(c, "更多选项").FontSize(11).TextColor(p.TextMuted)
			})

			ui.Row(c).Gap(6).Children(func() {
				pill(c, p, commitBtnLabel, true, func() {
					a.TriggerCommit(false)
				})
				pill(c, p, "提交并推送", false, func() {
					a.TriggerCommit(true)
				})
			})
		})

		if a.showCommitOptions {
			ui.Column(c).Padding(6, 0, 0, 0).Gap(6).BorderWidth(1, 0, 0, 0).BorderColor(p.BorderSubtle).Children(func() {
				ui.Row(c).Gap(4).AlignItems(ui.Center).Children(func() {
					ui.Text(c, "常用前缀：").FontSize(11).TextColor(p.TextMuted)
					for _, pfx := range []string{"feat: ", "fix: ", "refactor: ", "chore: "} {
						pfxCopy := pfx
						miniBtn(c.Key("pfx-"+pfxCopy), p, pfxCopy, func() {
							a.commitMessage = pfxCopy + a.commitMessage
						})
					}
				})
				ui.Row(c).Gap(6).AlignItems(ui.Center).Children(func() {
					ui.Text(c, "提交范围：").FontSize(11).TextColor(p.TextMuted)
					pill(c, p, "所有勾选文件", a.commitScope == "checked", func() {
						a.commitScope = "checked"
					})
					pill(c, p, "仅当前仓库", a.commitScope == "current", func() {
						a.commitScope = "current"
					})
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
				ui.Text(c, cf.Path).FontSize(12.5).FontWeight(weightSemibold).TextColor(p.TextPrimary).SingleLine()
				ui.Text(c, fmt.Sprintf("+%d  -%d", cf.AddedLines, cf.DeletedLines)).FontSize(11.5).TextColor(p.TextMuted).SingleLine()
				if isDirty {
					badgeBox(c, "未保存", p.Amber, p.AmberSoft, ui.Color{}, 10.5, true)
				}
			})

			ui.Row(c).Gap(6).AlignItems(ui.Center).Children(func() {
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
