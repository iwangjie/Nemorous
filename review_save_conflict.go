package main

import (
	"errors"
	"os"

	"github.com/egoist/mygo/ui"
	"github.com/egoist/mygo/ui/editor"
	"github.com/iwangjie/nemorous/internal/gitops"
)

type reviewSaveConflict struct {
	file        gitops.WorkspaceFile
	diff        *editor.DiffState
	diskErr     error
	diskVersion gitops.ReviewVersion
	reload      func(string, error)
}

func (a *App) handleReviewSaveError(err error, file gitops.WorkspaceFile, local string, reload func(string, error)) {
	if !errors.Is(err, gitops.ErrReviewFileChanged) {
		a.showToast("err", "保存失败："+err.Error())
		return
	}
	disk, diskErr := gitops.ReadReviewWorktree(file.RepoPath, file.RelPath)
	lang := editor.LanguageFor(file.RelPath)
	old, new := editor.NewBuffer(disk), editor.NewBuffer(local)
	old.SetLanguage(lang)
	new.SetLanguage(lang)
	a.reviewSaveConflict = &reviewSaveConflict{
		file: file, diskErr: diskErr, diskVersion: gitops.ReviewTextVersion(disk), reload: reload,
		diff: &editor.DiffState{Old: old, New: new, Layout: editor.DiffSplit, ExpandUnchanged: true},
	}
}

func (a *App) acceptReviewDiskVersion(conflict *reviewSaveConflict) {
	disk, err := gitops.ReadReviewWorktree(conflict.file.RepoPath, conflict.file.RelPath)
	missing := errors.Is(err, os.ErrNotExist)
	if err != nil && !missing {
		conflict.diskErr = err
		return
	}
	version := gitops.ReviewTextVersion(disk)
	if missing != errors.Is(conflict.diskErr, os.ErrNotExist) || version != conflict.diskVersion {
		conflict.diskErr = err
		conflict.diskVersion = version
		conflict.diff.Old.Set(disk)
		a.showToast("warn", "磁盘内容再次变化，请核对最新版本")
		return
	}
	a.recordLocalHistoryContent(conflict.file.RepoID, conflict.file.RepoPath, conflict.file.RelPath, reviewBufferText(conflict.diff.New), "外部改动前的编辑")
	a.reviewSaveConflict = nil
	conflict.reload(disk, err)
}

func (a *App) buildReviewSaveConflict(c *ui.Context, p Palette) {
	conflict := a.reviewSaveConflict
	if conflict == nil {
		return
	}
	open := true
	close := func() { a.reviewSaveConflict = nil }
	ui.DialogBase(c.Key("review-save-conflict"), &open, func(backdrop, panel ui.Element) {
		backdrop.Background(ui.RGBA(20, 22, 30, 0.36))
		panel.Width(900).Height(620).Padding(14).Gap(8).Radius(10).Background(p.Surface).Border(1, p.Border)
		modalHeader(c, p, icoColumns, "磁盘文件已变化，编辑仍保留", conflict.file.RelPath, nil, close)
		ui.Row(c).Gap(12).Children(func() {
			label := "磁盘版本"
			if conflict.diskErr != nil {
				if errors.Is(conflict.diskErr, os.ErrNotExist) {
					label = "磁盘文件已删除"
				} else {
					label = "无法读取磁盘版本：" + conflict.diskErr.Error()
				}
			}
			ui.Text(c, label).Grow(1).MaxLines(1)
			ui.Text(c, "你的编辑").Grow(1).MaxLines(1)
		})
		editor.DiffView(c, conflict.diff).Grow(1).FillWidth()
		ui.Row(c).Gap(8).AlignItems(ui.Center).Children(func() {
			miniBtn(c, p, "复制我的编辑", func() { c.Services().WriteClipboard(reviewBufferText(conflict.diff.New)) })
			ui.Spacer(c)
			if conflict.diskErr == nil {
				miniBtn(c, p, "使用磁盘版本", func() { a.acceptReviewDiskVersion(conflict) })
			} else if errors.Is(conflict.diskErr, os.ErrNotExist) {
				miniBtn(c, p, "放弃编辑并关闭文件", func() { a.acceptReviewDiskVersion(conflict) })
			}
			pill(c, p, "继续编辑", true, close)
		})
	})
	if !open {
		close()
	}
}
