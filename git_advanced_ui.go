package main

import (
	"context"
	"fmt"
	"strconv"
	"strings"
	"time"

	"github.com/egoist/mygo/ui"
	"github.com/iwangjie/nemorous/internal/gitops"
)

func (a *App) gitStatusByID(id string) *gitops.RepoStatus {
	for i := range a.statuses {
		if a.statuses[i].Repo.ID == id {
			return &a.statuses[i]
		}
	}
	return nil
}

func (a *App) ensureGitOpsRepo() *gitops.RepoStatus {
	if st := a.gitStatusByID(a.gitOpsRepo); st != nil {
		return st
	}
	if len(a.statuses) == 0 {
		return nil
	}
	a.gitOpsRepo = a.statuses[0].Repo.ID
	return &a.statuses[0]
}

func (a *App) openGitOps(tab string) {
	if len(a.statuses) == 0 {
		a.showToast("warn", "请先加载工作区")
		return
	}
	if tab == "" {
		tab = "stash"
	}
	a.gitOpsTab = tab
	a.showGitOps = true
	a.gitOpsErr = ""
	a.gitOpsNotice = ""
	a.gitOpsConfirm = ""
	a.ensureGitOpsRepo()
	if a.gitSelectedCommit != "" {
		a.gitOpsHash = a.gitSelectedCommit
	}
	a.loadGitOpsData()
}

func (a *App) loadGitOpsData() {
	repo := a.ensureGitOpsRepo()
	if repo == nil {
		return
	}
	path := repo.Repo.AbsPath
	repoID := repo.Repo.ID
	a.gitOpsLoading = true
	go func() {
		ctx, cancel := context.WithTimeout(context.Background(), 20*time.Second)
		defer cancel()
		stashes, _ := gitops.ListStashes(ctx, path)
		tags, _ := gitops.ListTags(ctx, path)
		remotes, _ := gitops.ListRemotes(ctx, path)
		submodules, _ := gitops.ListSubmodules(ctx, path)
		state := gitops.RepoOperationState(ctx, path)
		a.runUI(func() {
			if a.gitOpsRepo != repoID {
				return
			}
			a.gitOpsStashes = stashes
			a.gitOpsTags = tags
			a.gitOpsRemotes = remotes
			a.gitOpsSubmodules = submodules
			a.gitOpsState = state
			a.gitOpsLoading = false
		})
	}()
}

// runGitOpsAction runs one operation on the selected repository, then
// reloads its data.
func (a *App) runGitOpsAction(fn func(ctx context.Context, repoPath string) (string, error)) {
	repo := a.ensureGitOpsRepo()
	if repo == nil {
		return
	}
	if a.gitOpsLoading {
		return
	}
	a.gitOpsLoading = true
	a.gitOpsErr = ""
	a.gitOpsNotice = ""
	path := repo.Repo.AbsPath
	go func() {
		ctx, cancel := context.WithTimeout(context.Background(), 90*time.Second)
		defer cancel()
		msg, err := fn(ctx, path)
		a.runUI(func() {
			a.gitOpsLoading = false
			if err != nil {
				a.gitOpsErr = err.Error()
				if msg != "" {
					a.gitOpsErr = strings.TrimSpace(msg) + "\n" + a.gitOpsErr
				}
			} else {
				a.gitOpsNotice = msg
			}
			a.loadGitOpsData()
			a.RefreshAsync()
		})
	}()
}

func (a *App) gitOpsRepoPills(c *ui.Context, p Palette) {
	ui.Row(c).Wrap().Gap(4).AlignItems(ui.Center).Children(func() {
		ui.Text(c, "仓库").FontSize(10.5).TextColor(p.TextMuted)
		for i := range a.statuses {
			st := a.statuses[i]
			id := st.Repo.ID
			pill(c.Key("gops-repo-"+id), p, st.Repo.Name, a.gitOpsRepo == id, func() {
				a.gitOpsRepo = id
				a.gitOpsConfirm = ""
				a.loadGitOpsData()
			})
		}
	})
}

func (a *App) buildGitOpsModal(c *ui.Context, p Palette) {
	ui.DialogBase(c, &a.showGitOps, func(backdrop, panel ui.Element) {
		backdrop.Background(ui.RGBA(20, 22, 30, 0.40))
		panelWidth := float32(900)
		panelHeight := float32(700)
		if a.win != nil {
			if w, h := a.win.ContentSize(); w > 0 && h > 0 {
				panelWidth = min(panelWidth, float32(w)-60)
				panelHeight = min(panelHeight, float32(h)-48)
			}
		}
		panel.Width(panelWidth).Height(panelHeight).Padding(18).Gap(10).Radius(12).Background(p.Surface).Border(1, p.Border)
		ui.Row(c).Justify(ui.SpaceBetween).AlignItems(ui.Center).Children(func() {
			ui.Row(c).Gap(8).AlignItems(ui.Center).Children(func() {
				drawIcon(c, icoBranch, 16, p.Accent)
				ui.Text(c, "Git 高级操作").FontSize(15).FontWeight(weightBold).TextColor(p.TextPrimary)
				if st := a.ensureGitOpsRepo(); st != nil {
					ui.Text(c, st.Repo.Name+" · "+st.Branch).FontSize(10.5).TextColor(p.TextMuted)
				}
			})
			miniBtn(c, p, "关闭", func() { a.showGitOps = false })
		})
		a.gitOpsRepoPills(c, p)
		ui.Row(c).Wrap().Gap(4).Children(func() {
			for _, tab := range []struct{ id, label string }{
				{"stash", "Stash"}, {"rebase", "变基"}, {"commit", "Cherry-pick / Revert"},
				{"tags", "标签"}, {"remotes", "Remotes"}, {"submodules", "子模块"},
			} {
				tab := tab
				pill(c.Key("gops-tab-"+tab.id), p, tab.label, a.gitOpsTab == tab.id, func() {
					a.gitOpsTab = tab.id
					a.gitOpsConfirm = ""
				})
			}
			ui.Spacer(c)
			miniBtn(c, p, "刷新", func() { a.loadGitOpsData() })
			if a.gitOpsLoading {
				ui.Text(c, "处理中…").FontSize(10).TextColor(p.AccentSoftText)
			}
		})

		if a.gitOpsState.Busy() {
			ui.Column(c).FillWidth().Padding(8, 10).Gap(5).Radius(6).Background(p.AmberSoft).Children(func() {
				op := "合并"
				switch {
				case a.gitOpsState.Rebase:
					op = "Rebase"
				case a.gitOpsState.CherryPick:
					op = "Cherry-pick"
				case a.gitOpsState.Revert:
					op = "Revert"
				}
				ui.Text(c, fmt.Sprintf("%s 进行中 · %d 个冲突文件", op, len(a.gitOpsState.Conflicts))).FontSize(11.5).FontWeight(weightSemibold).TextColor(p.Amber)
				ui.Row(c).Gap(6).Wrap().Children(func() {
					if len(a.gitOpsState.Conflicts) > 0 {
						pill(c, p, "解决冲突", true, func() {
							a.showGitOps = false
							a.openConflictResolver(a.gitOpsRepo)
						})
					}
					pill(c, p, "继续", false, func() {
						a.runGitOpsAction(func(ctx context.Context, path string) (string, error) {
							return gitops.ContinueOperation(ctx, path)
						})
					})
					miniBtn(c, p, "中止", func() {
						a.runGitOpsAction(func(ctx context.Context, path string) (string, error) {
							return "已中止", gitops.AbortOperation(ctx, path)
						})
					})
				})
			})
		}
		if a.gitOpsErr != "" {
			ui.Column(c).FillWidth().Padding(7, 9).Gap(2).Radius(5).Background(p.DangerSoft).Children(func() {
				ui.Text(c, a.gitOpsErr).FontSize(10.5).TextColor(p.Danger).MaxLines(6)
			})
		}
		if a.gitOpsNotice != "" {
			ui.Column(c).FillWidth().Padding(7, 9).Radius(5).Background(p.GreenSoft).Children(func() {
				ui.Text(c, a.gitOpsNotice).FontSize(10.5).TextColor(p.Green).MaxLines(4)
			})
		}

		ui.Scroll(c).Grow(1).FillWidth().Children(func() {
			ui.Column(c).FillWidth().Gap(8).Children(func() {
				switch a.gitOpsTab {
				case "rebase":
					a.buildGitOpsRebase(c, p)
				case "commit":
					a.buildGitOpsCommitOps(c, p)
				case "tags":
					a.buildGitOpsTags(c, p)
				case "remotes":
					a.buildGitOpsRemotes(c, p)
				case "submodules":
					a.buildGitOpsSubmodules(c, p)
				default:
					a.buildGitOpsStash(c, p)
				}
			})
		})
	})
}

func (a *App) buildGitOpsStash(c *ui.Context, p Palette) {
	ui.Column(c).FillWidth().Gap(6).Children(func() {
		ui.Text(c, "贮藏当前改动").FontSize(12.5).FontWeight(weightSemibold).TextColor(p.TextPrimary)
		ui.Row(c).Gap(6).AlignItems(ui.Center).Children(func() {
			ui.TextInput(c.Key("stash-msg"), &a.stashMessage).Placeholder("说明（可选）").Grow(1)
			ui.Checkbox(c, &a.stashUntracked, "包含未跟踪文件")
			pill(c, p, "贮藏", true, func() {
				msg := a.stashMessage
				untracked := a.stashUntracked
				a.stashMessage = ""
				a.runGitOpsAction(func(ctx context.Context, path string) (string, error) {
					return gitops.StashCreate(ctx, path, msg, untracked)
				})
			})
		})
	})
	ui.Box(c).Height(1).FillWidth().Background(p.Border)
	ui.Text(c, fmt.Sprintf("Stash 列表（%d）", len(a.gitOpsStashes))).FontSize(11.5).FontWeight(weightSemibold).TextColor(p.TextSecondary)
	if len(a.gitOpsStashes) == 0 {
		ui.Text(c, "没有贮藏条目。").FontSize(11).TextColor(p.TextMuted)
		return
	}
	for _, entry := range a.gitOpsStashes {
		entry := entry
		kc := c.Key("stash-" + entry.Ref)
		ui.Column(kc).FillWidth().Padding(8, 10).Gap(4).Radius(6).Background(p.RailBg).Border(1, p.BorderSubtle).Children(func() {
			ui.Row(kc).Gap(8).AlignItems(ui.Center).Children(func() {
				badgeBox(kc, entry.Ref, p.AccentSoftText, p.AccentSoft, p.Border, 10, true)
				ui.Text(kc, entry.Message).FontSize(11.5).TextColor(p.TextPrimary).Grow(1).MaxLines(2)
				ui.Text(kc, entry.Date).FontSize(9.5).TextColor(p.TextMuted).Shrink(0)
			})
			if a.gitOpsConfirm == "stash:"+entry.Ref {
				ui.Row(kc).Gap(6).Children(func() {
					pill(kc, p, "确认删除", true, func() {
						a.gitOpsConfirm = ""
						ref := entry.Ref
						a.runGitOpsAction(func(ctx context.Context, path string) (string, error) {
							return "已删除 " + ref, gitops.StashDrop(ctx, path, ref)
						})
					})
					miniBtn(kc, p, "取消", func() { a.gitOpsConfirm = "" })
				})
			} else {
				ui.Row(kc).Gap(6).Wrap().Children(func() {
					if a.stashPreviewRef == entry.Ref {
						miniBtn(kc, p, "收起差异", func() { a.stashPreviewRef = ""; a.stashPreview = "" })
					} else {
						miniBtn(kc, p, "查看差异", func() {
							ref := entry.Ref
							repoPath := a.gitOpsRepoPath()
							go func() {
								ctx, cancel := context.WithTimeout(context.Background(), 20*time.Second)
								defer cancel()
								diff, err := gitops.StashDiff(ctx, repoPath, ref)
								a.runUI(func() {
									if err != nil {
										a.gitOpsErr = err.Error()
										return
									}
									if len(diff) > 200<<10 {
										diff = diff[len(diff)-(200<<10):]
									}
									a.stashPreviewRef = ref
									a.stashPreview = diff
								})
							}()
						})
					}
					miniBtn(kc, p, "应用", func() {
						ref := entry.Ref
						a.runGitOpsAction(func(ctx context.Context, path string) (string, error) {
							return "已应用 " + ref, gitops.StashApply(ctx, path, ref)
						})
					})
					miniBtn(kc, p, "弹出", func() {
						ref := entry.Ref
						a.runGitOpsAction(func(ctx context.Context, path string) (string, error) {
							return "已弹出 " + ref, gitops.StashPop(ctx, path, ref)
						})
					})
					miniBtn(kc, p, "复制引用", func() { a.pendingClipboard = entry.Ref })
					miniBtn(kc, p, "删除", func() { a.gitOpsConfirm = "stash:" + entry.Ref })
				})
			}
			if a.stashPreviewRef == entry.Ref && a.stashPreview != "" {
				ui.Scroll(kc).MaxHeight(150).Padding(6).Background(p.Surface).Radius(5).Children(func() {
					ui.Text(kc, a.stashPreview).Font("monospace").FontSize(9).TextColor(p.TextSecondary)
				})
			}
		})
	}
}

func (a *App) gitOpsRepoPath() string {
	if st := a.ensureGitOpsRepo(); st != nil {
		return st.Repo.AbsPath
	}
	return ""
}

func (a *App) buildGitOpsRebase(c *ui.Context, p Palette) {
	ui.Text(c, "将当前分支变基到目标分支").FontSize(12.5).FontWeight(weightSemibold).TextColor(p.TextPrimary)
	ui.Text(c, "变基会重写当前分支历史；冲突可在解决后继续。").FontSize(10.5).TextColor(p.TextMuted)
	ui.Row(c).Gap(6).AlignItems(ui.Center).Children(func() {
		ui.TextInput(c.Key("rebase-branch"), &a.gitOpsBranch).Placeholder("目标分支，如 main / origin/main").Grow(1)
		label := "开始变基"
		if a.gitOpsConfirm == "rebase" {
			label = "确认变基（重写历史）"
		}
		pill(c, p, label, true, func() {
			branch := strings.TrimSpace(a.gitOpsBranch)
			if a.gitOpsConfirm != "rebase" {
				a.gitOpsConfirm = "rebase"
				return
			}
			a.gitOpsConfirm = ""
			a.runGitOpsAction(func(ctx context.Context, path string) (string, error) {
				return gitops.Rebase(ctx, path, branch)
			})
		})
	})
	if st := a.ensureGitOpsRepo(); st != nil {
		ui.Text(c, "当前分支："+st.Branch).FontSize(10.5).TextColor(p.TextMuted)
	}
}

func (a *App) buildGitOpsCommitOps(c *ui.Context, p Palette) {
	ui.Text(c, "Cherry-pick / Revert").FontSize(12.5).FontWeight(weightSemibold).TextColor(p.TextPrimary)
	ui.Text(c, "在提交图选中提交后打开此面板会自动填入哈希。").FontSize(10.5).TextColor(p.TextMuted)
	ui.Row(c).Gap(6).AlignItems(ui.Center).Children(func() {
		ui.TextInput(c.Key("gops-hash"), &a.gitOpsHash).Placeholder("提交哈希").Grow(1)
		ui.TextInput(c.Key("gops-mainline"), &a.gitOpsMainline).Placeholder("mainline").Width(90)
		pill(c, p, "Cherry-pick", true, func() {
			hash := strings.TrimSpace(a.gitOpsHash)
			a.runGitOpsAction(func(ctx context.Context, path string) (string, error) {
				return gitops.CherryPick(ctx, path, hash)
			})
		})
		pill(c, p, "Revert", false, func() {
			hash := strings.TrimSpace(a.gitOpsHash)
			mainline := 0
			if n, err := strconv.Atoi(strings.TrimSpace(a.gitOpsMainline)); err == nil && n > 0 {
				mainline = n
			}
			a.runGitOpsAction(func(ctx context.Context, path string) (string, error) {
				return gitops.Revert(ctx, path, hash, mainline)
			})
		})
	})
	ui.Text(c, "Revert 合并提交时在 mainline 填 1（保留第一父提交）；Cherry-pick 合并提交请使用提交图/CLI 指定 -m。").FontSize(9.5).TextColor(p.TextMuted)
}

func (a *App) buildGitOpsTags(c *ui.Context, p Palette) {
	ui.Text(c, "标签").FontSize(12.5).FontWeight(weightSemibold).TextColor(p.TextPrimary)
	ui.Row(c).Wrap().Gap(6).AlignItems(ui.Center).Children(func() {
		ui.TextInput(c.Key("tag-name"), &a.gitOpsTagName).Placeholder("标签名，如 v0.4.0").Width(160)
		ui.TextInput(c.Key("tag-msg"), &a.gitOpsTagMessage).Placeholder("说明（留空为轻量标签）").Grow(1)
		pill(c, p, "创建", true, func() {
			name := strings.TrimSpace(a.gitOpsTagName)
			msg := a.gitOpsTagMessage
			a.runGitOpsAction(func(ctx context.Context, path string) (string, error) {
				return "已创建标签 " + name, gitops.CreateTag(ctx, path, name, msg, "HEAD", false)
			})
		})
	})
	for _, tag := range a.gitOpsTags {
		tag := tag
		kc := c.Key("tag-" + tag.Name)
		ui.Row(kc).FillWidth().Padding(6, 8).Gap(8).AlignItems(ui.Center).Radius(5).Background(p.RailBg).Children(func() {
			ui.Text(kc, tag.Name).FontSize(11.5).FontWeight(weightSemibold).TextColor(p.TextPrimary).Width(140).MaxLines(1)
			ui.Text(kc, tag.Hash).Font("monospace").FontSize(10).TextColor(p.TextMuted).Width(80)
			ui.Text(kc, tag.Subject).FontSize(10.5).TextColor(p.TextSecondary).Grow(1).MaxLines(1)
			if a.gitOpsConfirm == "tag:"+tag.Name {
				pill(kc, p, "确认删除", true, func() {
					a.gitOpsConfirm = ""
					name := tag.Name
					a.runGitOpsAction(func(ctx context.Context, path string) (string, error) {
						return "已删除标签 " + name, gitops.DeleteTag(ctx, path, name)
					})
				})
				miniBtn(kc, p, "取消", func() { a.gitOpsConfirm = "" })
			} else {
				miniBtn(kc, p, "推送", func() {
					name := tag.Name
					a.runGitOpsAction(func(ctx context.Context, path string) (string, error) {
						return "已推送标签 " + name, gitops.PushTag(ctx, path, name)
					})
				})
				miniBtn(kc, p, "删除", func() { a.gitOpsConfirm = "tag:" + tag.Name })
			}
		})
	}
}

func (a *App) buildGitOpsRemotes(c *ui.Context, p Palette) {
	ui.Text(c, "Remotes").FontSize(12.5).FontWeight(weightSemibold).TextColor(p.TextPrimary)
	ui.Row(c).Wrap().Gap(6).AlignItems(ui.Center).Children(func() {
		ui.TextInput(c.Key("remote-name"), &a.gitOpsRemoteName).Placeholder("名称，如 origin").Width(120)
		ui.TextInput(c.Key("remote-url"), &a.gitOpsRemoteURL).Placeholder("git@github.com:owner/repo.git").Grow(1)
		pill(c, p, "添加", true, func() {
			name, url := strings.TrimSpace(a.gitOpsRemoteName), strings.TrimSpace(a.gitOpsRemoteURL)
			a.runGitOpsAction(func(ctx context.Context, path string) (string, error) {
				return "已添加 remote " + name, gitops.AddRemote(ctx, path, name, url)
			})
		})
		pill(c, p, "设置地址", false, func() {
			name, url := strings.TrimSpace(a.gitOpsRemoteName), strings.TrimSpace(a.gitOpsRemoteURL)
			a.runGitOpsAction(func(ctx context.Context, path string) (string, error) {
				return "已更新 remote " + name, gitops.SetRemoteURL(ctx, path, name, url)
			})
		})
	})
	for _, remote := range a.gitOpsRemotes {
		remote := remote
		kc := c.Key("remote-" + remote.Name)
		ui.Column(kc).FillWidth().Padding(6, 8).Gap(3).Radius(5).Background(p.RailBg).Children(func() {
			ui.Row(kc).Gap(8).AlignItems(ui.Center).Children(func() {
				ui.Text(kc, remote.Name).FontSize(11.5).FontWeight(weightSemibold).TextColor(p.TextPrimary).Width(100)
				ui.Text(kc, remote.FetchURL).Font("monospace").FontSize(10).TextColor(p.TextSecondary).Grow(1).MaxLines(1)
				if a.gitOpsConfirm == "remote:"+remote.Name {
					pill(kc, p, "确认删除", true, func() {
						a.gitOpsConfirm = ""
						name := remote.Name
						a.runGitOpsAction(func(ctx context.Context, path string) (string, error) {
							return "已删除 remote " + name, gitops.RemoveRemote(ctx, path, name)
						})
					})
					miniBtn(kc, p, "取消", func() { a.gitOpsConfirm = "" })
				} else {
					miniBtn(kc, p, "删除", func() { a.gitOpsConfirm = "remote:" + remote.Name })
				}
			})
		})
	}
}

func (a *App) buildGitOpsSubmodules(c *ui.Context, p Palette) {
	ui.Text(c, "子模块").FontSize(12.5).FontWeight(weightSemibold).TextColor(p.TextPrimary)
	ui.Row(c).Wrap().Gap(6).AlignItems(ui.Center).Children(func() {
		pill(c, p, "初始化 / 更新", true, func() {
			a.runGitOpsAction(func(ctx context.Context, path string) (string, error) {
				return gitops.UpdateSubmodules(ctx, path)
			})
		})
		pill(c, p, "同步 URL", false, func() {
			a.runGitOpsAction(func(ctx context.Context, path string) (string, error) {
				return gitops.SyncSubmodules(ctx, path)
			})
		})
	})
	ui.Row(c).Wrap().Gap(6).AlignItems(ui.Center).Children(func() {
		ui.TextInput(c.Key("sub-url"), &a.gitOpsSubURL).Placeholder("仓库地址").Grow(1)
		ui.TextInput(c.Key("sub-path"), &a.gitOpsSubPath).Placeholder("目录，如 libs/foo").Width(180)
		pill(c, p, "添加", false, func() {
			url, dest := strings.TrimSpace(a.gitOpsSubURL), strings.TrimSpace(a.gitOpsSubPath)
			a.runGitOpsAction(func(ctx context.Context, path string) (string, error) {
				return gitops.AddSubmodule(ctx, path, url, dest)
			})
		})
	})
	if len(a.gitOpsSubmodules) == 0 {
		ui.Text(c, "没有子模块。").FontSize(10.5).TextColor(p.TextMuted)
		return
	}
	for _, mod := range a.gitOpsSubmodules {
		mod := mod
		kc := c.Key("sub-" + mod.Path)
		ui.Column(kc).FillWidth().Padding(6, 8).Gap(3).Radius(5).Background(p.RailBg).Children(func() {
			ui.Row(kc).Gap(8).AlignItems(ui.Center).Children(func() {
				ui.Text(kc, mod.Path).FontSize(11.5).FontWeight(weightSemibold).TextColor(p.TextPrimary).Width(180).MaxLines(1)
				ui.Text(kc, mod.Hash).Font("monospace").FontSize(10).TextColor(p.TextMuted).Width(90)
				tone, label := "ok", "已初始化"
				if !mod.Initialized {
					tone, label = "warn", "未初始化"
				} else if mod.Modified {
					tone, label = "accent", "有改动"
				}
				badgeBox(kc, label, pluginToneColor(tone, p), pluginToneBg(tone, p), p.Border, 9, true)
				ui.Text(kc, mod.URL).FontSize(9.5).TextColor(p.TextMuted).Grow(1).MaxLines(1)
				if a.gitOpsConfirm == "sub:"+mod.Path {
					pill(kc, p, "确认移除", true, func() {
						a.gitOpsConfirm = ""
						subPath := mod.Path
						a.runGitOpsAction(func(ctx context.Context, path string) (string, error) {
							return "已移除子模块 " + subPath, gitops.RemoveSubmodule(ctx, path, subPath)
						})
					})
					miniBtn(kc, p, "取消", func() { a.gitOpsConfirm = "" })
				} else {
					miniBtn(kc, p, "移除", func() { a.gitOpsConfirm = "sub:" + mod.Path })
				}
			})
		})
	}
}
