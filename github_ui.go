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

func (a *App) buildGitHubModal(c *ui.Context, p Palette) {
	ui.DialogBase(c, &a.showGitHub, func(backdrop, panel ui.Element) {
		backdrop.Background(ui.RGBA(20, 22, 30, 0.40))
		width, height := float32(1080), float32(740)
		if a.win != nil {
			if w, h := a.win.ContentSize(); w > 0 && h > 0 {
				width = min(width, float32(w)-50)
				height = min(height, float32(h)-50)
			}
		}
		panel.Width(width).Height(height).Padding(16).Gap(8).Radius(12).Background(p.Surface).Border(1, p.Border)
		ui.Row(c).Justify(ui.SpaceBetween).AlignItems(ui.Center).Children(func() {
			ui.Row(c).Gap(8).AlignItems(ui.Center).Children(func() {
				drawIcon(c, icoPR, 16, p.Accent)
				ui.Text(c, "GitHub PR / CI").FontSize(15).FontWeight(weightBold).TextColor(p.TextPrimary)
				if st := a.githubStatus(); st != nil {
					ui.Text(c, st.Repo.Name).FontSize(10.5).TextColor(p.TextMuted)
				}
			})
			ui.Row(c).Gap(6).Children(func() {
				miniBtn(c, p, "新建 PR", func() { a.githubCreating = !a.githubCreating })
				miniBtn(c, p, "刷新", func() { a.loadGitHub() })
				miniBtn(c, p, "关闭", func() { a.showGitHub = false })
			})
		})
		ui.Row(c).Wrap().Gap(4).AlignItems(ui.Center).Children(func() {
			for i := range a.statuses {
				st := a.statuses[i]
				id := st.Repo.ID
				pill(c.Key("gh-repo-"+id), p, st.Repo.Name, a.githubRepo == id, func() {
					a.githubRepo = id
					a.githubNumber = 0
					a.githubDetails = nil
					a.loadGitHub()
				})
			}
			ui.Spacer(c)
			for _, state := range []struct{ id, label string }{{"open", "Open"}, {"closed", "Closed"}, {"all", "All"}} {
				state := state
				pill(c.Key("gh-state-"+state.id), p, state.label, a.githubState == state.id, func() {
					a.githubState = state.id
					a.loadGitHub()
				})
			}
			if a.githubLoading {
				ui.Text(c, "读取中…").FontSize(10).TextColor(p.AccentSoftText)
			}
		})
		if a.githubErr != "" {
			ui.Column(c).FillWidth().Padding(6, 8).Radius(5).Background(p.DangerSoft).Children(func() {
				ui.Text(c, a.githubErr).FontSize(10).TextColor(p.Danger).MaxLines(4)
			})
		}
		if a.githubCreating {
			ui.Column(c).FillWidth().Padding(8, 10).Gap(5).Radius(6).Background(p.RailBg).Children(func() {
				st := a.githubStatus()
				defaultTitle := ""
				defaultBase := ""
				if st != nil {
					defaultTitle = st.Branch
					defaultBase = "main"
				}
				ui.Row(c).Gap(6).AlignItems(ui.Center).Children(func() {
					ui.TextInput(c.Key("gh-title"), &a.githubTitle).Placeholder("PR 标题（默认分支名）").Grow(1)
					ui.TextInput(c.Key("gh-base"), &a.githubBase).Placeholder("base：" + defaultBase).Width(140)
					ui.Checkbox(c, &a.githubDraft, "草稿")
					pill(c, p, "创建 PR", true, func() {
						if strings.TrimSpace(a.githubTitle) == "" {
							a.githubTitle = defaultTitle
						}
						a.createGitHubPR()
					})
				})
				ui.TextInput(c.Key("gh-body"), &a.githubBody).Placeholder("描述（可选）")
			})
		}
		ui.Row(c).Grow(1).FillWidth().Gap(10).Children(func() {
			ui.Column(c).Width(330).FillHeight().Gap(4).Children(func() {
				ui.Scroll(c).Grow(1).Children(func() {
					if len(a.githubPRs) == 0 && !a.githubLoading {
						ui.Text(c, "没有 PR。").FontSize(11).TextColor(p.TextMuted)
					}
					for _, pr := range a.githubPRs {
						pr := pr
						kc := c.Key(fmt.Sprintf("gh-pr-%d", pr.Number))
						row := ui.Column(kc).FillWidth().Padding(6, 8).Gap(3).Radius(5).Cursor(ui.CursorPointer)
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
								ui.Text(kc, fmt.Sprintf("#%d", pr.Number)).Font("monospace").FontSize(10).TextColor(p.TextMuted)
								ui.Text(kc, pr.Title).FontSize(11.5).FontWeight(weightSemibold).TextColor(p.TextPrimary).Grow(1).MaxLines(2)
								if pr.IsDraft {
									badgeBox(kc, "Draft", p.TextMuted, p.Surface2, p.Border, 8.5, false)
								}
							})
							ui.Text(kc, pr.HeadRefName+" → "+pr.BaseRefName).FontSize(9.5).TextColor(p.TextMuted).MaxLines(1)
							ui.Row(kc).Gap(4).Children(func() {
								if pr.Author != "" {
									ui.Text(kc, "@"+pr.Author).FontSize(9).TextColor(p.TextMuted)
								}
								if pr.ReviewDecision != "" {
									tone := "accent"
									switch pr.ReviewDecision {
									case "APPROVED":
										tone = "ok"
									case "CHANGES_REQUESTED":
										tone = "err"
									}
									badgeBox(kc, pr.ReviewDecision, pluginToneColor(tone, p), pluginToneBg(tone, p), p.Border, 8.5, true)
								}
							})
						})
					}
				})
			})
			ui.Box(c).Width(1).FillHeight().Background(p.Border)
			ui.Column(c).Grow(1).FillHeight().Gap(6).Children(func() {
				details := a.githubDetails
				if details == nil {
					ui.Text(c, "选择一个 PR 查看详情、CI 检查与评论。").FontSize(11).TextColor(p.TextMuted)
					return
				}
				ui.Row(c).Gap(6).AlignItems(ui.Center).Children(func() {
					ui.Text(c, fmt.Sprintf("#%d %s", details.Number, details.Title)).FontSize(13).FontWeight(weightBold).TextColor(p.TextPrimary).Grow(1).MaxLines(1)
					if details.URL != "" {
						miniBtn(c, p, "在浏览器打开", func() { _ = openPath(details.URL) })
					}
				})
				ui.Row(c).Gap(6).Wrap().Children(func() {
					badgeBox(c, details.State, p.TextSecondary, p.Surface2, p.Border, 9, false)
					ui.Text(c, details.HeadRefName+" → "+details.BaseRefName).FontSize(10).TextColor(p.TextMuted)
					ui.Text(c, "@"+details.Author).FontSize(10).TextColor(p.TextMuted)
				})
				if strings.TrimSpace(details.Body) != "" {
					ui.Column(c).FillWidth().Padding(6, 8).Radius(5).Background(p.RailBg).Children(func() {
						ui.Text(c, details.Body).FontSize(10.5).TextColor(p.TextSecondary).MaxLines(6)
					})
				}
				ui.Text(c, fmt.Sprintf("Checks (%d)", len(details.Checks))).FontSize(11).FontWeight(weightSemibold).TextColor(p.TextSecondary)
				ui.Scroll(c).MaxHeight(150).Children(func() {
					for i, check := range details.Checks {
						check := check
						kc := c.Key(fmt.Sprintf("gh-check-%d", i))
						row := ui.Row(kc).Padding(3, 5).Gap(6).AlignItems(ui.Center)
						if check.Link != "" {
							row.Cursor(ui.CursorPointer)
							if row.Clicked() {
								_ = openPath(check.Link)
							}
						}
						row.Children(func() {
							tone := "warn"
							if check.Bucket == "success" {
								tone = "ok"
							} else if check.Bucket == "failure" {
								tone = "err"
							}
							badgeBox(kc, check.State, pluginToneColor(tone, p), pluginToneBg(tone, p), p.Border, 8.5, true)
							ui.Text(kc, check.Name).FontSize(10.5).TextColor(p.TextPrimary).Grow(1).MaxLines(1)
						})
					}
					if len(details.Checks) == 0 {
						ui.Text(c, "没有检查结果。").FontSize(10).TextColor(p.TextMuted)
					}
				})
				ui.Text(c, fmt.Sprintf("评论 (%d)", len(details.Comments))).FontSize(11).FontWeight(weightSemibold).TextColor(p.TextSecondary)
				ui.Scroll(c).Grow(1).Children(func() {
					for i, comment := range details.Comments {
						comment := comment
						kc := c.Key(fmt.Sprintf("gh-comment-%d", i))
						ui.Column(kc).FillWidth().Padding(5, 7).Gap(2).Radius(5).Background(p.RailBg).Children(func() {
							ui.Row(kc).Gap(6).Children(func() {
								ui.Text(kc, "@"+comment.Author).FontSize(9.5).FontWeight(weightSemibold).TextColor(p.AccentSoftText)
								if comment.Path != "" {
									loc := comment.Path
									if comment.Line > 0 {
										loc += fmt.Sprintf(":%d", comment.Line)
									}
									badgeBox(kc, loc, p.TextMuted, p.Surface2, p.Border, 8.5, false)
								}
								ui.Text(kc, comment.CreatedAt).FontSize(9).TextColor(p.TextMuted)
							})
							ui.Text(kc, comment.Body).FontSize(10.5).TextColor(p.TextSecondary).MaxLines(5)
						})
					}
					if len(details.Comments) == 0 {
						ui.Text(c, "还没有评论。").FontSize(10).TextColor(p.TextMuted)
					}
				})
				ui.Row(c).Gap(6).AlignItems(ui.Center).Children(func() {
					ui.TextInput(c.Key("gh-comment"), &a.githubComment).Grow(1).Placeholder("添加评论…")
					pill(c, p, "发送", true, func() { a.postGitHubComment() })
				})
			})
		})
	})
}
