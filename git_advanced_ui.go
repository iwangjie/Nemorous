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
		if a.gitOpsTab == "" {
			tab = "stash"
		} else {
			tab = a.gitOpsTab
		}
	}
	a.gitOpsTab = tab
	a.showGitOps = true
	a.gitOpsErr = ""
	a.gitOpsNotice = ""
	a.gitOpsConfirm = ""
	a.gitOpsRebaseCommits = nil
	a.gitOpsRebaseActions = make(map[string]string)
	a.gitOpsRebaseRewordAt = ""
	a.gitOpsRebaseErr = ""
	a.ensureGitOpsRepo()
	if a.gitOpsCompareOld == "" {
		a.gitOpsCompareOld = "origin/main"
	}
	if a.gitOpsCompareNew == "" {
		a.gitOpsCompareNew = "HEAD"
	}
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
		reflog, _ := gitops.ListReflog(ctx, path, 100)
		state := gitops.RepoOperationState(ctx, path)
		a.runUI(func() {
			if a.gitOpsRepo != repoID {
				return
			}
			a.gitOpsStashes = stashes
			a.gitOpsTags = tags
			a.gitOpsRemotes = remotes
			a.gitOpsSubmodules = submodules
			a.gitOpsReflog = reflog
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

func (a *App) buildGitOpsModal(c *ui.Context, p Palette) {
	ui.DialogBase(c, &a.showGitOps, func(backdrop, panel ui.Element) {
		backdrop.Background(ui.RGBA(20, 22, 30, 0.40))
		panelWidth := float32(980)
		panelHeight := float32(680)
		if a.win != nil {
			if w, h := a.win.ContentSize(); w > 0 && h > 0 {
				panelWidth = min(panelWidth, float32(w)-60)
				panelHeight = min(panelHeight, float32(h)-48)
			}
		}
		panel.Width(panelWidth).Height(panelHeight).Padding(14).Gap(8).Radius(12).Background(p.Surface).Border(1, p.Border)
		st := a.ensureGitOpsRepo()
		branchContext := ""
		if st != nil {
			branchContext = st.Branch
		}
		modalHeader(c, p, icoBranch, "Git", branchContext, func() {
			if st != nil {
				menuIconPill(c, p, "gops-repo", icoFolder, st.Repo.Name, true, func(m *ui.Menu) {
					for i := range a.statuses {
						rst := a.statuses[i]
						id := rst.Repo.ID
						if m.Item(rst.Repo.Name).Radio(a.gitOpsRepo == id).Chosen() {
							a.gitOpsRepo = id
							a.gitOpsConfirm = ""
							a.gitOpsRebaseCommits = nil
							a.gitOpsRebaseActions = make(map[string]string)
							a.gitOpsRebaseRewordAt = ""
							a.loadGitOpsData()
						}
					}
				})
			}
			if a.gitOpsLoading {
				ui.Spinner(c).Size(14, 14).Label("加载中").TextColor(p.TextMuted)
			} else {
				toolButton(c, p, "gops-refresh", icoRefresh, "刷新", false, true, func() { a.loadGitOpsData() })
			}
		}, func() { a.showGitOps = false })

		ui.Row(c).Grow(1).FillWidth().Gap(10).Children(func() {
			buildModalNav(c, p, 166, []modalNavItem{
				{ID: "stash", Label: "Stash", Group: "常用", Icon: icoArchive},
				{ID: "rebase", Label: "Rebase", Group: "历史", Icon: icoBranch},
				{ID: "reflog", Label: "Reflog", Group: "历史", Icon: icoRestore},
				{ID: "commit", Label: "Cherry-pick", Group: "历史", Icon: icoCommit},
				{ID: "compare", Label: "Ref 对比", Group: "历史", Icon: icoColumns},
				{ID: "merge", Label: "合并", Group: "分支", Icon: icoMerge},
				{ID: "remotes", Label: "远端", Group: "分支", Icon: icoCloud},
				{ID: "tags", Label: "标签", Group: "仓库", Icon: icoTag},
				{ID: "submodules", Label: "子模块", Group: "仓库", Icon: icoBox},
			}, a.gitOpsTab, func(id string) {
				a.gitOpsTab = id
				a.gitOpsConfirm = ""
			})

			ui.Column(c).Grow(1).FillHeight().Gap(6).Children(func() {
				if a.gitOpsState.Busy() {
					ui.Row(c).FillWidth().Padding(6, 8).Gap(8).AlignItems(ui.Center).Radius(4).Background(p.AmberSoft).Children(func() {
						drawIcon(c, icoAlert, 13, p.Amber)
						op := "合并"
						switch {
						case a.gitOpsState.Rebase:
							op = "Rebase"
						case a.gitOpsState.CherryPick:
							op = "Cherry-pick"
						case a.gitOpsState.Revert:
							op = "Revert"
						}
						ui.Text(c, fmt.Sprintf("%s · %d 个冲突", op, len(a.gitOpsState.Conflicts))).FontSize(10.5).FontWeight(weightMedium).TextColor(p.Amber).Grow(1)
						if len(a.gitOpsState.Conflicts) > 0 {
							iconAction(c, p, "gops-resolve", icoMerge, "解决冲突", true, func() {
								a.showGitOps = false
								a.openConflictResolver(a.gitOpsRepo)
							})
						}
						toolButton(c, p, "gops-continue", icoPlay, "继续", false, true, func() {
							a.runGitOpsAction(func(ctx context.Context, path string) (string, error) {
								return gitops.ContinueOperation(ctx, path)
							})
						})
						toolButton(c, p, "gops-abort", icoStop, "中止", false, true, func() {
							a.runGitOpsAction(func(ctx context.Context, path string) (string, error) {
								return "已中止", gitops.AbortOperation(ctx, path)
							})
						})
					})
				}
				if a.gitOpsErr != "" {
					ui.Row(c).FillWidth().Padding(5, 7).Gap(6).Radius(4).Background(p.DangerSoft).Children(func() {
						drawIcon(c, icoAlert, 12, p.Danger)
						ui.Text(c, a.gitOpsErr).FontSize(10).TextColor(p.Danger).MaxLines(5)
					})
				}
				if a.gitOpsNotice != "" {
					ui.Row(c).FillWidth().Padding(5, 7).Gap(6).Radius(4).Background(p.GreenSoft).Children(func() {
						drawIcon(c, icoCheck, 12, p.Green)
						ui.Text(c, a.gitOpsNotice).FontSize(10).TextColor(p.Green).MaxLines(3)
					})
				}

				ui.Scroll(c).Grow(1).FillWidth().Children(func() {
					ui.Column(c).FillWidth().Gap(6).Children(func() {
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
						case "reflog":
							a.buildGitOpsReflog(c, p)
						case "merge":
							a.buildGitOpsMerge(c, p)
						case "compare":
							a.buildGitOpsCompare(c, p)
						default:
							a.buildGitOpsStash(c, p)
						}
					})
				})
			})
		})
	})
}

func shortGitDate(raw string) string {
	for _, layout := range []string{"2006-01-02 15:04:05 -0700", time.RFC3339} {
		if t, err := time.Parse(layout, raw); err == nil {
			return t.Local().Format("01-02 15:04")
		}
	}
	return raw
}

func cleanStashMessage(raw string) string {
	if strings.HasPrefix(raw, "On ") {
		if i := strings.Index(raw, ": "); i > 0 {
			return raw[i+2:]
		}
	}
	return raw
}

func (a *App) buildGitOpsStash(c *ui.Context, p Palette) {
	ui.Row(c).FillWidth().Gap(6).AlignItems(ui.Center).Children(func() {
		ui.TextInput(c.Key("stash-msg"), &a.stashMessage).Placeholder("Stash 说明（可选）").Grow(1)
		ui.Checkbox(c, &a.stashUntracked, "含未跟踪")
		iconAction(c, p, "stash-create", icoArchive, "创建 Stash", true, func() {
			msg := a.stashMessage
			untracked := a.stashUntracked
			a.stashMessage = ""
			a.runGitOpsAction(func(ctx context.Context, path string) (string, error) {
				return gitops.StashCreate(ctx, path, msg, untracked)
			})
		})
	})
	if len(a.gitOpsStashes) == 0 {
		emptyState(c, p, icoArchive, "没有 Stash", "")
		return
	}
	for _, entry := range a.gitOpsStashes {
		entry := entry
		kc := c.Key("stash-" + entry.Ref)
		row := ui.Column(kc).FillWidth().Padding(4, 3).Gap(2)
		if row.Hovered() {
			row.Background(p.SurfaceHover)
		}
		row.Children(func() {
			ui.Row(kc).FillWidth().Gap(6).AlignItems(ui.Center).Children(func() {
				drawIcon(kc, icoArchive, 12, p.TextMuted)
				ui.Text(kc, entry.Ref).Font("monospace").FontSize(9.5).TextColor(p.TextMuted).Width(72).MaxLines(1)
				ui.Text(kc, cleanStashMessage(entry.Message)).FontSize(11).TextColor(p.TextPrimary).Grow(1).MaxLines(1)
				ui.Text(kc, shortGitDate(entry.Date)).FontSize(9).TextColor(p.TextMuted).Shrink(0)
				toolButton(kc, p, "stash-diff-"+entry.Ref, icoColumns, "查看差异", a.stashPreviewRef == entry.Ref, true, func() {
					if a.stashPreviewRef == entry.Ref {
						a.stashPreviewRef = ""
						a.stashPreview = ""
						return
					}
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
				toolButton(kc, p, "stash-apply-"+entry.Ref, icoFetch, "应用 Stash", false, true, func() {
					ref := entry.Ref
					a.runGitOpsAction(func(ctx context.Context, path string) (string, error) {
						return "已应用 " + ref, gitops.StashApply(ctx, path, ref)
					})
				})
				menuIconAction(kc, p, "stash-more-"+entry.Ref, icoMore, "更多", false, func(m *ui.Menu) {
					if m.Item("弹出 Stash").Chosen() {
						ref := entry.Ref
						a.runGitOpsAction(func(ctx context.Context, path string) (string, error) {
							return "已弹出 " + ref, gitops.StashPop(ctx, path, ref)
						})
					}
					if m.Item("复制引用").Chosen() {
						a.pendingClipboard = entry.Ref
					}
					m.Separator()
					if m.Item("删除 Stash").Chosen() {
						a.gitOpsConfirm = "stash:" + entry.Ref
					}
				})
				if a.gitOpsConfirm == "stash:"+entry.Ref {
					ui.Text(kc, "删除？").FontSize(9.5).TextColor(p.Danger)
					toolButton(kc, p, "stash-del-yes-"+entry.Ref, icoCheck, "确认删除", false, true, func() {
						a.gitOpsConfirm = ""
						ref := entry.Ref
						a.runGitOpsAction(func(ctx context.Context, path string) (string, error) {
							return "已删除 " + ref, gitops.StashDrop(ctx, path, ref)
						})
					})
					toolButton(kc, p, "stash-del-no-"+entry.Ref, icoClose, "取消", false, true, func() { a.gitOpsConfirm = "" })
				}
			})
			if a.stashPreviewRef == entry.Ref && a.stashPreview != "" {
				ui.Scroll(kc).MaxHeight(140).Padding(6).Background(p.RailBg).Radius(4).Children(func() {
					ui.Text(kc, a.stashPreview).Font("monospace").FontSize(9).TextColor(p.TextSecondary)
				})
			}
		})
		ui.Box(c).Height(1).FillWidth().Background(p.BorderSubtle)
	}
}

func (a *App) gitOpsRepoPath() string {
	if st := a.ensureGitOpsRepo(); st != nil {
		return st.Repo.AbsPath
	}
	return ""
}

func (a *App) buildGitOpsRebase(c *ui.Context, p Palette) {
	ui.Row(c).FillWidth().Gap(6).AlignItems(ui.Center).Children(func() {
		ui.Text(c, "上游").FontSize(10.5).TextColor(p.TextMuted).Width(40).Shrink(0)
		ui.TextInput(c.Key("rebase-branch"), &a.gitOpsBranch).Placeholder("main / origin/main").Grow(1)
		iconPill(c, p, icoRefresh, "加载提交", false, func() {
			a.loadRebasePlan()
		})
		label := "普通变基"
		if a.gitOpsConfirm == "rebase" {
			label = "确认重写"
		}
		iconPill(c, p, icoBranch, label, false, func() {
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
	if a.gitOpsConfirm == "rebase" {
		ui.Text(c, "普通变基将重写当前分支历史").FontSize(9.5).TextColor(p.Amber)
	}
	ui.Box(c).Height(1).FillWidth().Background(p.Border).MarginY(4)
	ui.Row(c).FillWidth().Gap(8).AlignItems(ui.Center).Children(func() {
		ui.Text(c, "交互式变基计划").FontSize(11).FontWeight(weightSemibold).TextColor(p.TextPrimary)
		ui.Text(c, "从旧到新；可拖序（↑/↓）、pick、squash、reword、drop").FontSize(9).TextColor(p.TextMuted).Grow(1)
		if a.gitOpsRebaseLoading {
			ui.Spinner(c).Size(12, 12).Label("加载").TextColor(p.TextMuted)
		}
	})
	if a.gitOpsRebaseErr != "" {
		ui.Text(c, a.gitOpsRebaseErr).FontSize(9.5).TextColor(p.Danger).MaxLines(3)
	}
	if len(a.gitOpsRebaseCommits) == 0 {
		if !a.gitOpsRebaseLoading {
			emptyState(c, p, icoCommit, "尚未加载提交", "填写上游分支后点击「加载提交」")
		}
		return
	}
	for i, commit := range a.gitOpsRebaseCommits {
		commit := commit
		action := a.gitOpsRebaseActions[commit.Hash]
		if action == "" {
			action = "pick"
		}
		kc := c.Key("rb-" + commit.Hash)
		row := ui.Row(kc).FillWidth().Padding(4, 3).Gap(6).AlignItems(ui.Center).Radius(4)
		if row.Hovered() {
			row.Background(p.SurfaceHover)
		}
		row.Children(func() {
			ui.Text(kc, fmt.Sprintf("%02d", i+1)).Font("monospace").FontSize(9).TextColor(p.TextMuted).Width(20)
			tone, bg := "muted", p.Surface2
			switch action {
			case "squash":
				tone, bg = "accent", p.AccentSoft
			case "reword":
				tone, bg = "warn", p.AmberSoft
			case "drop":
				tone, bg = "danger", p.DangerSoft
			}
			badgeBox(kc, action, pluginToneColor(tone, p), bg, p.Border, 9, false)
			ui.Text(kc, commit.Hash[:min(8, len(commit.Hash))]).Font("monospace").FontSize(9).TextColor(p.TextMuted).Width(66)
			ui.Text(kc, commit.Subject).FontSize(10.5).TextColor(p.TextPrimary).Grow(1).MaxLines(1)
			toolButton(kc, p, "rb-up-"+commit.Hash, icoChevronUp, "上移", false, i > 0, func() {
				a.gitOpsRebaseCommits[i], a.gitOpsRebaseCommits[i-1] = a.gitOpsRebaseCommits[i-1], a.gitOpsRebaseCommits[i]
			})
			toolButton(kc, p, "rb-down-"+commit.Hash, icoChevronDown, "下移", false, i < len(a.gitOpsRebaseCommits)-1, func() {
				a.gitOpsRebaseCommits[i], a.gitOpsRebaseCommits[i+1] = a.gitOpsRebaseCommits[i+1], a.gitOpsRebaseCommits[i]
			})
			menuIconAction(kc, p, "rb-menu-"+commit.Hash, icoMore, "动作", false, func(m *ui.Menu) {
				for _, candidate := range []string{"pick", "squash", "reword", "drop"} {
					candidate := candidate
					if m.Item(candidate).Radio(action == candidate).Chosen() {
						if candidate == "reword" {
							for h := range a.gitOpsRebaseActions {
								if a.gitOpsRebaseActions[h] == "reword" {
									a.gitOpsRebaseActions[h] = "pick"
								}
							}
							a.gitOpsRebaseRewordAt = commit.Hash
							a.gitOpsRebaseMessage = commit.Subject
						}
						if action == "reword" && candidate != "reword" {
							a.gitOpsRebaseRewordAt = ""
						}
						a.gitOpsRebaseActions[commit.Hash] = candidate
					}
				}
			})
		})
		if action == "reword" && a.gitOpsRebaseRewordAt == commit.Hash {
			ui.Row(c).FillWidth().Padding(0, 28).Gap(6).AlignItems(ui.Center).Children(func() {
				ui.Text(c, "新提交信息").FontSize(9).TextColor(p.TextMuted)
				ui.TextInput(c.Key("rb-msg"), &a.gitOpsRebaseMessage).Grow(1)
			})
		}
	}
	ui.Row(c).FillWidth().Gap(8).AlignItems(ui.Center).Margin(4, 0, 0, 0).Children(func() {
		label := "执行交互式变基"
		if a.gitOpsConfirm == "interactive-rebase" {
			label = "确认重写历史"
		}
		pill(c, p, label, true, func() {
			if a.gitOpsConfirm != "interactive-rebase" {
				a.gitOpsConfirm = "interactive-rebase"
				return
			}
			a.gitOpsConfirm = ""
			a.executeInteractiveRebase()
		})
		if a.gitOpsConfirm == "interactive-rebase" {
			ui.Text(c, "将重写当前分支历史").FontSize(9.5).TextColor(p.Amber)
		}
	})
}

func (a *App) loadRebasePlan() {
	repo := a.ensureGitOpsRepo()
	if repo == nil {
		return
	}
	upstream := strings.TrimSpace(a.gitOpsBranch)
	if upstream == "" {
		a.gitOpsRebaseErr = "请先填写上游分支"
		return
	}
	a.gitOpsRebaseLoading = true
	a.gitOpsRebaseErr = ""
	repoID := repo.Repo.ID
	path := repo.Repo.AbsPath
	go func() {
		ctx, cancel := context.WithTimeout(context.Background(), 30*time.Second)
		defer cancel()
		commits, err := gitops.ListRebaseCommits(ctx, path, upstream)
		a.runUI(func() {
			if a.gitOpsRepo != repoID {
				return
			}
			a.gitOpsRebaseLoading = false
			if err != nil {
				a.gitOpsRebaseErr = err.Error()
				return
			}
			a.gitOpsRebaseCommits = commits
			a.gitOpsRebaseActions = make(map[string]string, len(commits))
			a.gitOpsRebaseRewordAt = ""
			for _, commit := range commits {
				a.gitOpsRebaseActions[commit.Hash] = "pick"
			}
		})
	}()
}

func (a *App) executeInteractiveRebase() {
	repo := a.ensureGitOpsRepo()
	if repo == nil || len(a.gitOpsRebaseCommits) == 0 || a.gitOpsLoading {
		return
	}
	steps := make([]gitops.RebaseStep, 0, len(a.gitOpsRebaseCommits))
	for _, commit := range a.gitOpsRebaseCommits {
		action := a.gitOpsRebaseActions[commit.Hash]
		msg := ""
		if action == "reword" {
			msg = a.gitOpsRebaseMessage
		}
		steps = append(steps, gitops.RebaseStep{Hash: commit.Hash, Action: action, Message: msg})
	}
	upstream := strings.TrimSpace(a.gitOpsBranch)
	repoID := repo.Repo.ID
	path := repo.Repo.AbsPath
	a.gitOpsLoading = true
	a.gitOpsErr, a.gitOpsNotice = "", ""
	go func() {
		ctx, cancel := context.WithTimeout(context.Background(), 10*time.Minute)
		defer cancel()
		out, err := gitops.InteractiveRebase(ctx, path, upstream, steps)
		a.runUI(func() {
			a.gitOpsLoading = false
			if err != nil {
				a.gitOpsErr = strings.TrimSpace(out)
				if a.gitOpsErr == "" {
					a.gitOpsErr = err.Error()
				}
				a.loadGitOpsData()
				return
			}
			a.gitOpsNotice = "交互式变基完成"
			a.gitOpsRebaseCommits = nil
			a.gitOpsRebaseActions = make(map[string]string)
			a.loadGitOpsData()
			a.RefreshAsync()
		})
	}()
	_ = repoID
}

func (a *App) buildGitOpsReflog(c *ui.Context, p Palette) {
	ui.Row(c).FillWidth().Gap(6).AlignItems(ui.Center).Children(func() {
		ui.Text(c, "Reset 模式").FontSize(10).TextColor(p.TextMuted)
		menuIconPill(c, p, "reflog-mode", icoRestore, a.gitOpsResetMode, true, func(m *ui.Menu) {
			for _, mode := range []string{"soft", "mixed", "hard", "keep"} {
				mode := mode
				if m.Item(mode).Radio(a.gitOpsResetMode == mode).Chosen() {
					a.gitOpsResetMode = mode
					a.gitOpsConfirm = ""
				}
			}
		})
		ui.Text(c, "hard 会丢弃工作区修改，操作前会二次确认").FontSize(9).TextColor(p.TextMuted).Grow(1)
	})
	for _, entry := range a.gitOpsReflog {
		entry := entry
		kc := c.Key("reflog-" + entry.Hash + entry.Ref)
		row := ui.Row(kc).FillWidth().Padding(4, 3).Gap(6).AlignItems(ui.Center).Radius(4)
		if row.Hovered() {
			row.Background(p.SurfaceHover)
		}
		row.Children(func() {
			ui.Text(kc, entry.Hash[:min(8, len(entry.Hash))]).Font("monospace").FontSize(9).TextColor(p.TextMuted).Width(66)
			ui.Text(kc, entry.Ref).Font("monospace").FontSize(9).TextColor(p.Accent).Width(90).MaxLines(1)
			ui.Text(kc, entry.Subject).FontSize(10).TextColor(p.TextPrimary).Grow(1).MaxLines(1)
			ui.Text(kc, shortGitDate(entry.Date)).FontSize(9).TextColor(p.TextMuted).Width(80)
			if a.gitOpsConfirm == "reset:"+entry.Hash {
				ui.Text(kc, "Reset？").FontSize(9).TextColor(p.Danger)
				toolButton(kc, p, "reflog-yes-"+entry.Hash, icoCheck, "确认 reset", false, true, func() {
					a.gitOpsConfirm = ""
					hash := entry.Hash
					mode := a.gitOpsResetMode
					a.runGitOpsAction(func(ctx context.Context, path string) (string, error) {
						out, err := gitops.ResetTo(ctx, path, hash, mode)
						return "已 reset --" + mode + " 到 " + hash[:min(8, len(hash))] + "\n" + out, err
					})
				})
				toolButton(kc, p, "reflog-no-"+entry.Hash, icoClose, "取消", false, true, func() { a.gitOpsConfirm = "" })
			} else {
				toolButton(kc, p, "reflog-reset-"+entry.Hash, icoRestore, "Reset 到此记录", false, true, func() {
					a.gitOpsConfirm = "reset:" + entry.Hash
				})
			}
		})
		ui.Box(c).Height(1).FillWidth().Background(p.BorderSubtle)
	}
	if len(a.gitOpsReflog) == 0 {
		emptyState(c, p, icoRestore, "没有 Reflog", "仓库还没有引用变更记录")
	}
}

func (a *App) buildGitOpsMerge(c *ui.Context, p Palette) {
	ui.Text(c, "将分支合并到当前分支").FontSize(11).FontWeight(weightSemibold).TextColor(p.TextPrimary)
	ui.Row(c).FillWidth().Gap(6).AlignItems(ui.Center).Children(func() {
		ui.TextInput(c.Key("merge-branch"), &a.gitOpsMergeBranch).Placeholder("feature / origin/feature").Grow(1)
		menuIconPill(c, p, "merge-strategy", icoMerge, a.gitOpsMergeStrategy, true, func(m *ui.Menu) {
			for _, strategy := range []string{"default", "no-ff", "ff-only", "squash"} {
				strategy := strategy
				if m.Item(strategy).Radio(a.gitOpsMergeStrategy == strategy).Chosen() {
					a.gitOpsMergeStrategy = strategy
					a.gitOpsConfirm = ""
				}
			}
		})
		if a.gitOpsMergeStrategy == "no-ff" {
			ui.TextInput(c.Key("merge-msg"), &a.gitOpsMergeMessage).Placeholder("合并提交信息（可选）").Width(220)
		}
		label := "合并"
		if a.gitOpsConfirm == "merge" {
			label = "确认合并"
		}
		pill(c, p, label, true, func() {
			if a.gitOpsConfirm != "merge" {
				a.gitOpsConfirm = "merge"
				return
			}
			a.gitOpsConfirm = ""
			branch := strings.TrimSpace(a.gitOpsMergeBranch)
			strategy := gitops.MergeStrategy(a.gitOpsMergeStrategy)
			msg := a.gitOpsMergeMessage
			a.runGitOpsAction(func(ctx context.Context, path string) (string, error) {
				out, err := gitops.MergeBranch(ctx, path, branch, strategy, msg)
				if err != nil {
					return out, err
				}
				if strings.TrimSpace(out) == "" {
					out = "合并完成"
				}
				return out, nil
			})
		})
	})
	ui.Text(c, "squash 会创建一个压缩提交；ff-only 在不能快进时失败，不会改写历史。").FontSize(9).TextColor(p.TextMuted)
	ui.Box(c).Height(1).FillWidth().Background(p.Border).MarginY(6)
	ui.Text(c, "安全强制推送").FontSize(11).FontWeight(weightSemibold).TextColor(p.TextPrimary)
	ui.Text(c, "只在远端仍是上次 fetch 的提交时覆盖，防止覆盖别人的工作。").FontSize(9).TextColor(p.TextMuted)
	ui.Row(c).Gap(6).AlignItems(ui.Center).Children(func() {
		if a.gitOpsConfirm == "force-lease" {
			ui.Text(c, "确认 force-with-lease？").FontSize(9.5).TextColor(p.Danger)
			pill(c, p, "确认推送", true, func() {
				a.gitOpsConfirm = ""
				a.runGitOpsAction(func(ctx context.Context, path string) (string, error) {
					return gitops.ForcePushWithLease(ctx, path)
				})
			})
			miniBtn(c, p, "取消", func() { a.gitOpsConfirm = "" })
		} else {
			iconPill(c, p, icoPush, "Force with lease", false, func() { a.gitOpsConfirm = "force-lease" })
		}
	})
}

func (a *App) buildGitOpsCompare(c *ui.Context, p Palette) {
	ui.Row(c).FillWidth().Gap(6).AlignItems(ui.Center).Children(func() {
		ui.TextInput(c.Key("cmp-old"), &a.gitOpsCompareOld).Placeholder("旧 ref，如 origin/main").Grow(1)
		ui.TextInput(c.Key("cmp-new"), &a.gitOpsCompareNew).Placeholder("新 ref，如 HEAD").Grow(1)
		ui.Checkbox(c, &a.gitOpsCompareThree, "三点（merge-base）")
		iconPill(c, p, icoColumns, "比较", true, func() {
			a.loadRefCompare()
		})
	})
	if a.gitOpsCompareLoading {
		ui.Spinner(c).Size(12, 12).Label("比较中").TextColor(p.TextMuted)
	}
	if a.gitOpsCompareErr != "" {
		ui.Text(c, a.gitOpsCompareErr).FontSize(9.5).TextColor(p.Danger)
	}
	if a.gitOpsCompareDiff != "" {
		ui.Scroll(c).MaxHeight(380).FillWidth().Padding(8).Background(p.RailBg).Radius(4).Children(func() {
			ui.Text(c, a.gitOpsCompareDiff).Font("monospace").FontSize(9).TextColor(p.TextSecondary)
		})
	} else if !a.gitOpsCompareLoading && a.gitOpsCompareErr == "" {
		emptyState(c, p, icoColumns, "选择两个 ref", "展示任意分支、标签或提交之间的差异")
	}
}

func (a *App) loadRefCompare() {
	repo := a.ensureGitOpsRepo()
	if repo == nil {
		return
	}
	oldRef, newRef := strings.TrimSpace(a.gitOpsCompareOld), strings.TrimSpace(a.gitOpsCompareNew)
	three := a.gitOpsCompareThree
	a.gitOpsCompareLoading = true
	a.gitOpsCompareErr = ""
	a.gitOpsCompareDiff = ""
	repoID := repo.Repo.ID
	path := repo.Repo.AbsPath
	go func() {
		ctx, cancel := context.WithTimeout(context.Background(), 30*time.Second)
		defer cancel()
		diff, err := gitops.RefDiff(ctx, path, oldRef, newRef, three)
		a.runUI(func() {
			if a.gitOpsRepo != repoID {
				return
			}
			a.gitOpsCompareLoading = false
			if err != nil {
				a.gitOpsCompareErr = err.Error()
				return
			}
			if len(diff) > 400<<10 {
				diff = diff[:400<<10] + "\n\n[差异过大，已截断]"
			}
			a.gitOpsCompareDiff = diff
		})
	}()
}

func (a *App) buildGitOpsCommitOps(c *ui.Context, p Palette) {
	ui.Row(c).FillWidth().Gap(6).AlignItems(ui.Center).Children(func() {
		ui.Text(c, "提交").FontSize(10.5).TextColor(p.TextMuted).Width(64).Shrink(0)
		ui.TextInput(c.Key("gops-hash"), &a.gitOpsHash).Placeholder("提交哈希或引用").Grow(1)
		ui.TextInput(c.Key("gops-mainline"), &a.gitOpsMainline).Placeholder("mainline").Width(84).Tooltip("合并提交的父编号，通常为 1")
		iconPill(c, p, icoCommit, "Cherry-pick", true, func() {
			hash := strings.TrimSpace(a.gitOpsHash)
			a.runGitOpsAction(func(ctx context.Context, path string) (string, error) {
				return gitops.CherryPick(ctx, path, hash)
			})
		})
		iconPill(c, p, icoRestore, "Revert", false, func() {
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
}

func (a *App) buildGitOpsTags(c *ui.Context, p Palette) {
	ui.Row(c).FillWidth().Gap(6).AlignItems(ui.Center).Children(func() {
		ui.TextInput(c.Key("tag-name"), &a.gitOpsTagName).Placeholder("标签名").Width(180)
		ui.TextInput(c.Key("tag-msg"), &a.gitOpsTagMessage).Placeholder("说明（可选）").Grow(1)
		iconAction(c, p, "tag-create", icoPlus, "创建标签", true, func() {
			name := strings.TrimSpace(a.gitOpsTagName)
			msg := a.gitOpsTagMessage
			a.runGitOpsAction(func(ctx context.Context, path string) (string, error) {
				return "已创建标签 " + name, gitops.CreateTag(ctx, path, name, msg, "HEAD", false)
			})
		})
	})
	if len(a.gitOpsTags) == 0 {
		emptyState(c, p, icoTag, "没有标签", "")
		return
	}
	for _, tag := range a.gitOpsTags {
		tag := tag
		kc := c.Key("tag-" + tag.Name)
		row := ui.Row(kc).FillWidth().Padding(4, 3).Gap(6).AlignItems(ui.Center)
		if row.Hovered() {
			row.Background(p.SurfaceHover)
		}
		row.Children(func() {
			drawIcon(kc, icoTag, 12, p.TextMuted)
			ui.Text(kc, tag.Name).FontSize(11).FontWeight(weightMedium).TextColor(p.TextPrimary).Width(150).MaxLines(1)
			ui.Text(kc, tag.Hash).Font("monospace").FontSize(9.5).TextColor(p.TextMuted).Width(70)
			ui.Text(kc, tag.Subject).FontSize(10).TextColor(p.TextSecondary).Grow(1).MaxLines(1)
			if a.gitOpsConfirm == "tag:"+tag.Name {
				ui.Text(kc, "删除？").FontSize(9.5).TextColor(p.Danger)
				toolButton(kc, p, "tag-del-yes-"+tag.Name, icoCheck, "确认删除", false, true, func() {
					a.gitOpsConfirm = ""
					name := tag.Name
					a.runGitOpsAction(func(ctx context.Context, path string) (string, error) {
						return "已删除标签 " + name, gitops.DeleteTag(ctx, path, name)
					})
				})
				toolButton(kc, p, "tag-del-no-"+tag.Name, icoClose, "取消", false, true, func() { a.gitOpsConfirm = "" })
			} else {
				toolButton(kc, p, "tag-push-"+tag.Name, icoPush, "推送标签", false, true, func() {
					name := tag.Name
					a.runGitOpsAction(func(ctx context.Context, path string) (string, error) {
						return "已推送标签 " + name, gitops.PushTag(ctx, path, name)
					})
				})
				toolButton(kc, p, "tag-del-"+tag.Name, icoTrash, "删除标签", false, true, func() { a.gitOpsConfirm = "tag:" + tag.Name })
			}
		})
		ui.Box(c).Height(1).FillWidth().Background(p.BorderSubtle)
	}
}

func (a *App) buildGitOpsRemotes(c *ui.Context, p Palette) {
	ui.Row(c).FillWidth().Gap(6).AlignItems(ui.Center).Children(func() {
		ui.TextInput(c.Key("remote-name"), &a.gitOpsRemoteName).Placeholder("名称").Width(130)
		ui.TextInput(c.Key("remote-url"), &a.gitOpsRemoteURL).Placeholder("git@github.com:owner/repo.git").Grow(1)
		iconAction(c, p, "remote-add", icoPlus, "添加远端", true, func() {
			name, url := strings.TrimSpace(a.gitOpsRemoteName), strings.TrimSpace(a.gitOpsRemoteURL)
			a.runGitOpsAction(func(ctx context.Context, path string) (string, error) {
				return "已添加远端 " + name, gitops.AddRemote(ctx, path, name, url)
			})
		})
		iconAction(c, p, "remote-set", icoEdit, "更新远端地址", false, func() {
			name, url := strings.TrimSpace(a.gitOpsRemoteName), strings.TrimSpace(a.gitOpsRemoteURL)
			a.runGitOpsAction(func(ctx context.Context, path string) (string, error) {
				return "已更新远端 " + name, gitops.SetRemoteURL(ctx, path, name, url)
			})
		})
	})
	if len(a.gitOpsRemotes) == 0 {
		emptyState(c, p, icoCloud, "没有远端", "")
		return
	}
	for _, remote := range a.gitOpsRemotes {
		remote := remote
		kc := c.Key("remote-" + remote.Name)
		row := ui.Row(kc).FillWidth().Padding(4, 3).Gap(6).AlignItems(ui.Center)
		if row.Hovered() {
			row.Background(p.SurfaceHover)
		}
		row.Children(func() {
			drawIcon(kc, icoCloud, 12, p.TextMuted)
			ui.Text(kc, remote.Name).FontSize(11).FontWeight(weightMedium).TextColor(p.TextPrimary).Width(100)
			ui.Text(kc, remote.FetchURL).Font("monospace").FontSize(9.5).TextColor(p.TextSecondary).Grow(1).MaxLines(1)
			if a.gitOpsConfirm == "remote:"+remote.Name {
				ui.Text(kc, "删除？").FontSize(9.5).TextColor(p.Danger)
				toolButton(kc, p, "remote-del-yes-"+remote.Name, icoCheck, "确认删除", false, true, func() {
					a.gitOpsConfirm = ""
					name := remote.Name
					a.runGitOpsAction(func(ctx context.Context, path string) (string, error) {
						return "已删除远端 " + name, gitops.RemoveRemote(ctx, path, name)
					})
				})
				toolButton(kc, p, "remote-del-no-"+remote.Name, icoClose, "取消", false, true, func() { a.gitOpsConfirm = "" })
			} else {
				toolButton(kc, p, "remote-del-"+remote.Name, icoTrash, "删除远端", false, true, func() { a.gitOpsConfirm = "remote:" + remote.Name })
			}
		})
		ui.Box(c).Height(1).FillWidth().Background(p.BorderSubtle)
	}
}

func (a *App) buildGitOpsSubmodules(c *ui.Context, p Palette) {
	ui.Row(c).FillWidth().Gap(6).AlignItems(ui.Center).Children(func() {
		iconPill(c, p, icoRefresh, "更新", true, func() {
			a.runGitOpsAction(func(ctx context.Context, path string) (string, error) {
				return gitops.UpdateSubmodules(ctx, path)
			})
		})
		iconPill(c, p, icoCloud, "同步", false, func() {
			a.runGitOpsAction(func(ctx context.Context, path string) (string, error) {
				return gitops.SyncSubmodules(ctx, path)
			})
		})
		ui.TextInput(c.Key("sub-url"), &a.gitOpsSubURL).Placeholder("仓库地址").Grow(1)
		ui.TextInput(c.Key("sub-path"), &a.gitOpsSubPath).Placeholder("目录").Width(160)
		iconAction(c, p, "sub-add", icoPlus, "添加子模块", false, func() {
			url, dest := strings.TrimSpace(a.gitOpsSubURL), strings.TrimSpace(a.gitOpsSubPath)
			a.runGitOpsAction(func(ctx context.Context, path string) (string, error) {
				return gitops.AddSubmodule(ctx, path, url, dest)
			})
		})
	})
	if len(a.gitOpsSubmodules) == 0 {
		emptyState(c, p, icoBox, "没有子模块", "")
		return
	}
	for _, mod := range a.gitOpsSubmodules {
		mod := mod
		kc := c.Key("sub-" + mod.Path)
		row := ui.Row(kc).FillWidth().Padding(4, 3).Gap(6).AlignItems(ui.Center)
		if row.Hovered() {
			row.Background(p.SurfaceHover)
		}
		row.Children(func() {
			drawIcon(kc, icoBox, 12, p.TextMuted)
			ui.Text(kc, mod.Path).FontSize(11).FontWeight(weightMedium).TextColor(p.TextPrimary).Width(180).MaxLines(1)
			ui.Text(kc, mod.Hash).Font("monospace").FontSize(9.5).TextColor(p.TextMuted).Width(76)
			tone, label := "ok", "已初始化"
			if !mod.Initialized {
				tone, label = "warn", "未初始化"
			} else if mod.Modified {
				tone, label = "accent", "有改动"
			}
			badgeBox(kc, label, pluginToneColor(tone, p), pluginToneBg(tone, p), p.Border, 8.5, false)
			ui.Text(kc, mod.URL).FontSize(9).TextColor(p.TextMuted).Grow(1).MaxLines(1)
			if a.gitOpsConfirm == "sub:"+mod.Path {
				ui.Text(kc, "移除？").FontSize(9.5).TextColor(p.Danger)
				toolButton(kc, p, "sub-del-yes-"+mod.Path, icoCheck, "确认移除", false, true, func() {
					a.gitOpsConfirm = ""
					subPath := mod.Path
					a.runGitOpsAction(func(ctx context.Context, path string) (string, error) {
						return "已移除子模块 " + subPath, gitops.RemoveSubmodule(ctx, path, subPath)
					})
				})
				toolButton(kc, p, "sub-del-no-"+mod.Path, icoClose, "取消", false, true, func() { a.gitOpsConfirm = "" })
			} else {
				toolButton(kc, p, "sub-del-"+mod.Path, icoTrash, "移除子模块", false, true, func() { a.gitOpsConfirm = "sub:" + mod.Path })
			}
		})
		ui.Box(c).Height(1).FillWidth().Background(p.BorderSubtle)
	}
}
