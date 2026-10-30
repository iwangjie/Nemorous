package main

import (
	"context"
	"fmt"
	"strings"
	"time"

	"github.com/egoist/mygo/ui"
	"github.com/iwangjie/nemorous/internal/github"
	"github.com/iwangjie/nemorous/internal/gitops"
)

func (a *App) githubStatus() *gitops.RepoStatus {
	if st := a.gitStatusByID(a.githubRepo); st != nil {
		return st
	}
	if len(a.statuses) > 0 {
		a.githubRepo = a.statuses[0].Repo.ID
		return &a.statuses[0]
	}
	return nil
}

func (a *App) openGitHub() {
	if !github.Available() {
		a.showToast("warn", "未安装 gh CLI；请安装 GitHub CLI 并执行 gh auth login")
		return
	}
	if len(a.statuses) == 0 {
		a.showToast("warn", "请先加载工作区")
		return
	}
	a.showGitHub = true
	a.githubErr = ""
	a.githubState = "open"
	a.githubPRs = nil
	a.githubDetails = nil
	a.githubNumber = 0
	a.githubCreating = false
	if a.githubRepo == "" {
		a.githubRepo = a.statuses[0].Repo.ID
	}
	a.loadGitHub()
}

func (a *App) loadGitHub() {
	st := a.githubStatus()
	if st == nil {
		return
	}
	repoID := a.githubRepo
	path := st.Repo.AbsPath
	state := a.githubState
	if state == "" {
		state = "open"
	}
	a.githubLoading = true
	a.githubErr = ""
	go func() {
		ctx, cancel := context.WithTimeout(context.Background(), 45*time.Second)
		defer cancel()
		prs, err := github.ListPRs(ctx, path, state)
		a.runUI(func() {
			if a.githubRepo != repoID {
				return
			}
			a.githubLoading = false
			if err != nil {
				a.githubErr = err.Error()
				return
			}
			a.githubPRs = prs
			if len(prs) > 0 {
				number := a.githubNumber
				if number == 0 {
					number = prs[0].Number
				}
				a.loadGitHubDetails(number)
			}
		})
	}()
}

func (a *App) loadGitHubDetails(number int) {
	st := a.githubStatus()
	if st == nil {
		return
	}
	repoID := a.githubRepo
	path := st.Repo.AbsPath
	a.githubNumber = number
	a.githubLoading = true
	go func() {
		ctx, cancel := context.WithTimeout(context.Background(), 45*time.Second)
		defer cancel()
		details, err := github.ViewPR(ctx, path, number)
		if err == nil {
			if repo, derr := github.Detect(ctx, path); derr == nil {
				if inline, ierr := github.InlineComments(ctx, path, repo, number); ierr == nil {
					details.Comments = append(details.Comments, inline...)
				}
			}
		}
		a.runUI(func() {
			if a.githubRepo != repoID || a.githubNumber != number {
				return
			}
			a.githubLoading = false
			if err != nil {
				a.githubErr = err.Error()
				return
			}
			a.githubDetails = &details
		})
	}()
}

func (a *App) postGitHubComment() {
	body := strings.TrimSpace(a.githubComment)
	if body == "" || a.githubNumber == 0 {
		return
	}
	st := a.githubStatus()
	if st == nil {
		return
	}
	number := a.githubNumber
	path := st.Repo.AbsPath
	a.githubComment = ""
	go func() {
		ctx, cancel := context.WithTimeout(context.Background(), 45*time.Second)
		defer cancel()
		err := github.AddComment(ctx, path, number, body)
		a.runUI(func() {
			if err != nil {
				a.githubErr = err.Error()
				return
			}
			a.loadGitHubDetails(number)
		})
	}()
}

func (a *App) createGitHubPR() {
	st := a.githubStatus()
	if st == nil {
		return
	}
	title := strings.TrimSpace(a.githubTitle)
	if title == "" {
		title = st.Branch
	}
	path := st.Repo.AbsPath
	base := strings.TrimSpace(a.githubBase)
	body := a.githubBody
	draft := a.githubDraft
	a.githubLoading = true
	a.githubErr = ""
	go func() {
		ctx, cancel := context.WithTimeout(context.Background(), 90*time.Second)
		defer cancel()
		url, err := github.CreatePR(ctx, path, title, body, base, draft)
		a.runUI(func() {
			a.githubLoading = false
			if err != nil {
				a.githubErr = err.Error()
				return
			}
			a.githubCreating = false
			a.githubTitle = ""
			a.githubBody = ""
			a.showToast("ok", "已创建 PR："+url)
			a.loadGitHub()
		})
	}()
}

func githubStateIcon(state string) *ui.SVG {
	switch state {
	case "MERGED":
		return icoMerge
	case "CLOSED":
		return icoMinus
	default:
		return icoPR
	}
}

func githubCheckIcon(bucket string) *ui.SVG {
	switch bucket {
	case "success":
		return icoCheck
	case "failure":
		return icoAlert
	default:
		return icoClock
	}
}

func shortGitHubTime(raw string) string {
	if raw == "" {
		return ""
	}
	if t, err := time.Parse(time.RFC3339, raw); err == nil {
		return t.Local().Format("01-02 15:04")
	}
	return raw
}

func (a *App) buildGitHubModal(c *ui.Context, p Palette) {
	ui.DialogBase(c, &a.showGitHub, func(backdrop, panel ui.Element) {
		backdrop.Background(ui.RGBA(20, 22, 30, 0.40))
		width, height := float32(1060), float32(660)
		if a.win != nil {
			if w, h := a.win.ContentSize(); w > 0 && h > 0 {
				width = min(width, float32(w)-50)
				height = min(height, float32(h)-50)
			}
		}
		panel.Width(width).Height(height).Padding(14).Gap(8).Radius(12).Background(p.Surface).Border(1, p.Border)
		modalHeader(c, p, icoPR, "GitHub", "PR · CI", func() {
			if st := a.githubStatus(); st != nil {
				menuIconPill(c, p, "gh-repo-menu", icoFolder, st.Repo.Name, true, func(m *ui.Menu) {
					for i := range a.statuses {
						rst := a.statuses[i]
						id := rst.Repo.ID
						if m.Item(rst.Repo.Name).Radio(a.githubRepo == id).Chosen() {
							a.githubRepo = id
							a.githubNumber = 0
							a.githubDetails = nil
							a.loadGitHub()
						}
					}
				})
			}
			if a.githubLoading {
				ui.Spinner(c).Size(14, 14).Label("读取中").TextColor(p.TextMuted)
			} else {
				toolButton(c, p, "gh-refresh", icoRefresh, "刷新", false, true, func() { a.loadGitHub() })
			}
			toolButton(c, p, "gh-new", icoPlus, "新建 PR", a.githubCreating, true, func() { a.githubCreating = !a.githubCreating })
		}, func() { a.showGitHub = false })

		ui.Row(c).Height(26).Gap(2).AlignItems(ui.Center).Children(func() {
			for _, state := range []struct{ id, label string }{{"open", "Open"}, {"closed", "Closed"}, {"all", "All"}} {
				state := state
				active := a.githubState == state.id
				kc := c.Key("gh-state-" + state.id)
				el := ui.Row(kc).Padding(3, 8).Radius(4).Cursor(ui.CursorPointer).AlignItems(ui.Center)
				fg := p.TextMuted
				if active {
					el.Background(p.AccentSoft)
					fg = p.AccentSoftText
				}
				if el.Hovered() && !active {
					el.Background(p.SurfaceHover)
					fg = p.TextSecondary
				}
				if el.Clicked() {
					a.githubState = state.id
					a.loadGitHub()
				}
				el.Children(func() {
					ui.Text(kc, state.label).FontSize(10.5).FontWeight(weightMedium).TextColor(fg)
				})
			}
			ui.Spacer(c)
		})

		if a.githubErr != "" {
			ui.Row(c).FillWidth().Padding(5, 7).Gap(6).Radius(4).Background(p.DangerSoft).Children(func() {
				drawIcon(c, icoAlert, 12, p.Danger)
				ui.Text(c, a.githubErr).FontSize(10).TextColor(p.Danger).MaxLines(3)
			})
		}
		if a.githubCreating {
			st := a.githubStatus()
			defaultTitle := ""
			defaultBase := "main"
			if st != nil {
				defaultTitle = st.Branch
			}
			ui.Column(c).FillWidth().Gap(5).Padding(3, 0).Children(func() {
				ui.Row(c).FillWidth().Gap(6).AlignItems(ui.Center).Children(func() {
					ui.TextInput(c.Key("gh-title"), &a.githubTitle).Placeholder("PR 标题（默认分支名）").Grow(1)
					ui.TextInput(c.Key("gh-base"), &a.githubBase).Placeholder("base：" + defaultBase).Width(120)
					ui.Checkbox(c, &a.githubDraft, "Draft")
					iconAction(c, p, "gh-create", icoCheck, "创建 PR", true, func() {
						if strings.TrimSpace(a.githubTitle) == "" {
							a.githubTitle = defaultTitle
						}
						a.createGitHubPR()
					})
					toolButton(c, p, "gh-create-cancel", icoClose, "取消", false, true, func() { a.githubCreating = false })
				})
				ui.TextInput(c.Key("gh-body"), &a.githubBody).Placeholder("描述（可选）")
			})
			ui.Box(c).Height(1).FillWidth().Background(p.BorderSubtle)
		}

		ui.Row(c).Grow(1).FillWidth().Gap(8).Children(func() {
			ui.Column(c).Width(320).FillHeight().Gap(0).Children(func() {
				ui.Scroll(c).Grow(1).Children(func() {
					if len(a.githubPRs) == 0 && !a.githubLoading {
						emptyState(c, p, icoPR, "没有 Pull Request", "")
					}
					for _, pr := range a.githubPRs {
						pr := pr
						kc := c.Key(fmt.Sprintf("gh-pr-%d", pr.Number))
						row := ui.Column(kc).FillWidth().Padding(6, 7).Gap(2).Radius(4).Cursor(ui.CursorPointer)
						if a.githubNumber == pr.Number {
							row.Background(p.AccentSoft)
						}
						if row.Hovered() && a.githubNumber != pr.Number {
							row.Background(p.SurfaceHover)
						}
						if row.Clicked() {
							a.loadGitHubDetails(pr.Number)
						}
						row.Children(func() {
							ui.Row(kc).Gap(5).AlignItems(ui.Center).Children(func() {
								stateColor := p.Green
								if pr.State == "CLOSED" {
									stateColor = p.Danger
								} else if pr.State == "MERGED" {
									stateColor = p.Purple
								}
								drawIcon(kc, githubStateIcon(pr.State), 12, stateColor)
								ui.Text(kc, fmt.Sprintf("#%d", pr.Number)).Font("monospace").FontSize(9.5).TextColor(p.TextMuted)
								ui.Text(kc, pr.Title).FontSize(11).FontWeight(weightMedium).TextColor(p.TextPrimary).Grow(1).MaxLines(2)
								if pr.IsDraft {
									ui.Text(kc, "DRAFT").FontSize(8).FontWeight(weightSemibold).TextColor(p.TextMuted)
								}
							})
							ui.Row(kc).Gap(5).AlignItems(ui.Center).Children(func() {
								ui.Text(kc, pr.HeadRefName+" → "+pr.BaseRefName).FontSize(9).TextColor(p.TextMuted).MaxLines(1)
								if pr.Author != "" {
									ui.Text(kc, "@"+pr.Author).FontSize(9).TextColor(p.TextMuted)
								}
								ui.Spacer(kc)
								if pr.ReviewDecision == "APPROVED" {
									drawIcon(kc, icoCheck, 11, p.Green)
								} else if pr.ReviewDecision == "CHANGES_REQUESTED" {
									drawIcon(kc, icoAlert, 11, p.Danger)
								}
							})
						})
					}
				})
			})
			ui.Box(c).Width(1).FillHeight().Background(p.BorderSubtle)
			ui.Column(c).Grow(1).FillHeight().Gap(6).Children(func() {
				details := a.githubDetails
				if details == nil {
					emptyState(c, p, icoPR, "选择 Pull Request", "")
					return
				}
				ui.Row(c).Gap(6).AlignItems(ui.Center).Children(func() {
					ui.Text(c, fmt.Sprintf("#%d %s", details.Number, details.Title)).FontSize(13).FontWeight(weightBold).TextColor(p.TextPrimary).Grow(1).MaxLines(1)
					if details.URL != "" {
						toolButton(c, p, "gh-open", icoExternal, "在浏览器打开", false, true, func() { _ = openPath(details.URL) })
					}
				})
				ui.Row(c).Gap(6).Wrap().Children(func() {
					drawIcon(c, githubStateIcon(details.State), 12, p.TextMuted)
					ui.Text(c, details.State).FontSize(9.5).TextColor(p.TextSecondary)
					ui.Text(c, details.HeadRefName+" → "+details.BaseRefName).FontSize(9.5).TextColor(p.TextMuted)
					ui.Text(c, "@"+details.Author).FontSize(9.5).TextColor(p.TextMuted)
				})
				if strings.TrimSpace(details.Body) != "" {
					ui.Text(c, details.Body).FontSize(10.5).TextColor(p.TextSecondary).MaxLines(6)
				}
				ui.Row(c).Gap(5).AlignItems(ui.Center).Children(func() {
					drawIcon(c, icoCheck, 12, p.TextSecondary)
					ui.Text(c, "检查").FontSize(10.5).FontWeight(weightSemibold).TextColor(p.TextSecondary)
					ui.Text(c, fmt.Sprint(len(details.Checks))).FontSize(9.5).TextColor(p.TextMuted)
				})
				ui.Scroll(c).MaxHeight(140).Children(func() {
					for i, check := range details.Checks {
						check := check
						kc := c.Key(fmt.Sprintf("gh-check-%d", i))
						row := ui.Row(kc).Padding(2, 3).Gap(6).AlignItems(ui.Center)
						if check.Link != "" {
							row.Cursor(ui.CursorPointer)
							if row.Clicked() {
								_ = openPath(check.Link)
							}
						}
						row.Children(func() {
							tone := p.Amber
							if check.Bucket == "success" {
								tone = p.Green
							} else if check.Bucket == "failure" {
								tone = p.Danger
							}
							ui.Icon(kc, githubCheckIcon(check.Bucket)).Size(12, 12).TextColor(tone).Tooltip(check.State).Label(check.State)
							ui.Text(kc, check.Name).FontSize(10).TextColor(p.TextPrimary).Grow(1).MaxLines(1)
						})
					}
					if len(details.Checks) == 0 {
						ui.Text(c, "没有检查结果").FontSize(9.5).TextColor(p.TextMuted)
					}
				})
				ui.Row(c).Gap(5).AlignItems(ui.Center).Children(func() {
					drawIcon(c, icoComment, 12, p.TextSecondary)
					ui.Text(c, "评论").FontSize(10.5).FontWeight(weightSemibold).TextColor(p.TextSecondary)
					ui.Text(c, fmt.Sprint(len(details.Comments))).FontSize(9.5).TextColor(p.TextMuted)
				})
				ui.Scroll(c).Grow(1).Children(func() {
					for i, comment := range details.Comments {
						comment := comment
						kc := c.Key(fmt.Sprintf("gh-comment-%d", i))
						ui.Column(kc).FillWidth().Padding(2, 4).Gap(2).Children(func() {
							ui.Row(kc).Gap(6).AlignItems(ui.Center).Children(func() {
								ui.Text(kc, "@"+comment.Author).FontSize(9.5).FontWeight(weightMedium).TextColor(p.AccentSoftText)
								if comment.Path != "" {
									loc := comment.Path
									if comment.Line > 0 {
										loc += fmt.Sprintf(":%d", comment.Line)
									}
									ui.Text(kc, loc).Font("monospace").FontSize(8.5).TextColor(p.TextMuted)
								}
								ui.Text(kc, shortGitHubTime(comment.CreatedAt)).FontSize(8.5).TextColor(p.TextMuted)
							})
							ui.Text(kc, comment.Body).FontSize(10.5).TextColor(p.TextSecondary).MaxLines(5)
						})
					}
					if len(details.Comments) == 0 {
						ui.Text(c, "还没有评论").FontSize(9.5).TextColor(p.TextMuted)
					}
				})
				ui.Row(c).Gap(6).AlignItems(ui.Center).Children(func() {
					ui.TextInput(c.Key("gh-comment"), &a.githubComment).Grow(1).Placeholder("添加评论…")
					iconAction(c, p, "gh-send", icoSend, "发送评论", true, func() { a.postGitHubComment() })
				})
			})
		})
	})
}
