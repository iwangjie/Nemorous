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
