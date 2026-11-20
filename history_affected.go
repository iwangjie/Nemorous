package main

import (
	"context"
	"time"

	"github.com/egoist/mygo/ui"
	"github.com/iwangjie/nemorous/internal/gitops"
)

type affectedCommitView struct {
	detail  gitops.CommitDetail
	loading bool
	err     error
	cancel  context.CancelFunc
}

func (a *App) openHistoryAffected(detail gitops.CommitDetail) {
	a.closeHistoryAffected()
	ctx, cancel := context.WithTimeout(context.Background(), 8*time.Second)
	view := &affectedCommitView{detail: detail, loading: true, cancel: cancel}
	a.historyAffected = view
	load := func() {
		defer cancel()
		files, err := gitops.CommitFiles(ctx, detail.RepoPath, detail.Hash)
		a.runUI(func() {
			if a.historyAffected == view {
				view.detail.Files, view.err, view.loading = files, err, false
			}
		})
	}
	if a.win == nil {
		load()
	} else {
		go load()
	}
}

func (a *App) closeHistoryAffected() {
	if view := a.historyAffected; view != nil {
		view.cancel()
		a.historyAffected = nil
	}
}

func (a *App) buildHistoryAffected(c *ui.Context, p Palette) {
	view := a.historyAffected
	ui.Column(c.Key("affected-paths")).Fill().Background(p.RailBg).Children(func() {
		ui.Row(c).Height(36).Padding(0, 8).AlignItems(ui.Center).Children(func() {
			label := "受提交 " + view.detail.ShortHash + " 影响的路径"
			ui.Text(c, label).SingleLine().Grow(1).Tooltip(label)
			toolButton(c, p, "close-affected-paths", icoClose, "关闭受影响的文件", false, true, a.closeHistoryAffected)
		})
		switch {
		case view.loading:
			emptyState(c, p, icoLog, "正在读取提交文件…", "")
		case view.err != nil:
			emptyState(c, p, icoLog, "无法读取提交文件", view.err.Error())
		default:
			ui.Scroll(c).Grow(1).FillWidth().Children(func() {
				a.renderGitCommitTree(c, p, view.detail, buildGitCommitTree(view.detail.Files), 0)
			})
		}
	})
}
