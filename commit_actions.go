package main

import (
	"context"
	"fmt"
	"strings"
	"time"

	"github.com/egoist/mygo/ui"
	"github.com/iwangjie/nemorous/internal/gitops"
	"github.com/iwangjie/nemorous/internal/mani"
)

// runCommitRepoAction runs one operation against the repository that owns a
// commit graph row, then refreshes the graph.
func (a *App) runCommitRepoAction(entry gitops.CommitEntry, label string, fn func(context.Context, string) (string, error)) {
	go func() {
		ctx, cancel := context.WithTimeout(context.Background(), 60*time.Second)
		defer cancel()
		out, err := fn(ctx, entry.RepoPath)
		a.runUI(func() {
			if err != nil {
				msg := err.Error()
				if out != "" {
					msg = out
				}
				a.showToast("err", label+"失败："+msg)
				return
			}
			a.showToast("ok", label+"完成")
			a.RefreshAsync()
		})
	}()
}

func (a *App) compareWithCurrent(entry gitops.CommitEntry) {
	a.gitOpsRepo = entry.RepoID
	a.gitOpsCompareOld = entry.Hash
	a.gitOpsCompareNew = "HEAD"
	a.gitOpsCompareThree = false
	a.openGitOps("compare")
	a.loadRefCompare()
}

func (a *App) compareBranchWithCurrent(repoID, branch string) {
	a.gitOpsRepo = repoID
	a.gitOpsCompareOld = branch
	a.gitOpsCompareNew = "HEAD"
	a.gitOpsCompareThree = true
	a.openGitOps("compare")
	a.loadRefCompare()
}

// runRepoSyncAction runs a network Git operation for one repository and
// reports the result without blocking the UI.
func (a *App) runRepoSyncAction(repo mani.DiscoveredRepo, label string, fn func(context.Context) (string, error)) {
	go func() {
		ctx, cancel := context.WithTimeout(context.Background(), 2*time.Minute)
		defer cancel()
		out, err := fn(ctx)
		a.runUI(func() {
			if err != nil {
				a.showToast("err", label+" 失败："+err.Error())
				return
			}
			msg := label + " 完成"
			if strings.TrimSpace(out) != "" {
				msg += "：" + strings.TrimSpace(out)
			}
			a.showToast("ok", msg)
			a.RefreshAsync()
		})
	}()
}

// buildCommitResetModal confirms destructive history operations from the
// commit graph.
func (a *App) buildCommitResetModal(c *ui.Context, p Palette) {
	ui.DialogBase(c, &a.showCommitReset, func(backdrop, panel ui.Element) {
		pending := a.pendingCommitReset
		if pending == nil {
			a.showCommitReset = false
			return
		}
		backdrop.Background(ui.RGBA(20, 22, 30, 0.36))
		panel.Width(440).Padding(18).Gap(10).Radius(12).Background(p.Surface).Border(1, p.Border)
		title := "重置分支"
		detail := fmt.Sprintf("将 %s 当前分支重置到 %s（%s）。", pending.RepoName, pending.Hash[:min(8, len(pending.Hash))], pending.Mode)
		danger := pending.Mode == "hard"
		if pending.Mode == "checkout" {
			title = "检出提交"
			detail = fmt.Sprintf("将在 %s 检出 %s 的 Detached HEAD。", pending.RepoName, pending.Hash[:min(8, len(pending.Hash))])
		}
		ui.Text(c, title).FontSize(15).FontWeight(weightBold).TextColor(p.TextPrimary)
		ui.Text(c, detail).FontSize(12).TextColor(p.TextSecondary)
		if danger {
			ui.Row(c).Padding(6, 8).Gap(6).Radius(4).Background(p.DangerSoft).Children(func() {
				drawIcon(c, icoAlert, 12, p.Danger)
				ui.Text(c, "Hard reset 会丢弃工作区与暂存区改动").FontSize(10).TextColor(p.Danger)
			})
		}
		ui.Row(c).Justify(ui.End).Gap(8).Children(func() {
			pill(c, p, "取消", false, func() { a.pendingCommitReset = nil; a.showCommitReset = false })
			label := "确认重置"
			if pending.Mode == "checkout" {
				label = "确认检出"
			}
			pill(c, p, label, true, func() {
				p := *pending
				a.pendingCommitReset = nil
				a.showCommitReset = false
				go func() {
					ctx, cancel := context.WithTimeout(context.Background(), 60*time.Second)
					defer cancel()
					var err error
					if p.Mode == "checkout" {
						_, err = gitops.CheckoutDetached(ctx, p.RepoPath, p.Hash)
					} else {
						_, err = gitops.ResetTo(ctx, p.RepoPath, p.Hash, p.Mode)
					}
					a.runUI(func() {
						if err != nil {
							a.showToast("err", err.Error())
							return
						}
						a.showToast("ok", "操作完成")
						a.RefreshAsync()
					})
				}()
			})
		})
	})
}

func (a *App) commitUndoRepo() *gitops.RepoStatus {
	if targets := a.commitTargets(); len(targets) > 0 {
		return a.gitStatusByID(targets[0].Repo.ID)
	}
	if len(a.statuses) > 0 {
		return &a.statuses[0]
	}
	return nil
}

// openCommitMsgHistory loads recent subjects from the selected repositories.
func (a *App) openCommitMsgHistory() {
	a.showCommitMsgHistory = true
	a.commitMsgHistoryLoading = true
	a.commitMsgHistoryErr = ""
	a.commitMsgHistory = nil
	var repos []mani.DiscoveredRepo
	for _, t := range a.commitTargets() {
		repos = append(repos, t.Repo)
	}
	if len(repos) == 0 {
		for _, st := range a.statuses {
			repos = append(repos, st.Repo)
		}
	}
	go func() {
		ctx, cancel := context.WithTimeout(context.Background(), 20*time.Second)
		defer cancel()
		seen := make(map[string]bool)
		var out []string
		for _, repo := range repos {
			log, err := gitops.RunGit(ctx, repo.AbsPath, "log", "-20", "--format=%s")
			if err != nil {
				continue
			}
			for _, line := range strings.Split(log, "\n") {
				line = strings.TrimSpace(line)
				if line != "" && !seen[line] {
					seen[line] = true
					out = append(out, line)
				}
			}
		}
		a.runUI(func() {
			a.commitMsgHistoryLoading = false
			if len(out) == 0 {
				a.commitMsgHistoryErr = "所选仓库没有提交记录"
				return
			}
			if len(out) > 100 {
				out = out[:100]
			}
			a.commitMsgHistory = out
		})
	}()
}

func (a *App) buildCommitMsgHistoryModal(c *ui.Context, p Palette) {
	ui.DialogBase(c, &a.showCommitMsgHistory, func(backdrop, panel ui.Element) {
		backdrop.Background(ui.RGBA(20, 22, 30, 0.40))
		panel.Width(640).Height(520).Padding(14).Gap(8).Radius(12).Background(p.Surface).Border(1, p.Border)
		modalHeader(c, p, icoCommit, "提交信息历史", "所选仓库的最近提交", nil, func() { a.showCommitMsgHistory = false })
		if a.commitMsgHistoryLoading {
			ui.Spinner(c).Size(13, 13).Label("加载中").TextColor(p.TextMuted)
		}
		if a.commitMsgHistoryErr != "" {
			ui.Text(c, a.commitMsgHistoryErr).FontSize(10).TextColor(p.TextMuted)
		}
		ui.Scroll(c).Grow(1).FillWidth().Children(func() {
			ui.Column(c).FillWidth().Gap(1).Children(func() {
				for i, msg := range a.commitMsgHistory {
					msg := msg
					kc := c.Key(fmt.Sprintf("commit-history-%d", i))
					row := ui.Row(kc).FillWidth().Padding(6, 4).Radius(4).Cursor(ui.CursorPointer).AlignItems(ui.Center)
					if row.Hovered() {
						row.Background(p.SurfaceHover)
					}
					if row.Clicked() {
						a.commitMessage = msg
						a.showCommitMsgHistory = false
						a.commitInputHandle.Focus()
					}
					row.Children(func() { ui.Text(kc, msg).FontSize(11).TextColor(p.TextPrimary).MaxLines(2) })
				}
			})
		})
	})
}
