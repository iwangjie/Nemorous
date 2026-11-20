package main

import (
	"context"
	"time"

	"github.com/egoist/mygo/ui"
	"github.com/iwangjie/nemorous/internal/gitops"
)

type fileRevisionPicker struct {
	file     gitops.WorkspaceFile
	entries  []gitops.FileRevision
	selected int
	loading  bool
	err      error
	cancel   context.CancelFunc
}

func (a *App) openFileRevisionPicker(wf gitops.WorkspaceFile) {
	wf = a.normalizeWorkspaceFile(wf)
	if wf.RepoPath == "" || wf.RelPath == "" {
		return
	}
	a.closeFileRevisionPicker()
	ctx, cancel := context.WithTimeout(context.Background(), 10*time.Second)
	view := &fileRevisionPicker{file: wf, loading: true, cancel: cancel}
	a.fileRevisionPicker = view
	load := func() {
		defer cancel()
		entries, err := gitops.FileHistoryAtRevision(ctx, wf.RepoPath, "HEAD", wf.RelPath)
		a.runUI(func() {
			if a.fileRevisionPicker == view {
				view.entries, view.err, view.loading = entries, err, false
			}
		})
	}
	if a.win == nil {
		load()
	} else {
		go load()
	}
}

func (a *App) closeFileRevisionPicker() {
	if view := a.fileRevisionPicker; view != nil {
		view.cancel()
		a.fileRevisionPicker = nil
	}
}

func (a *App) comparePickedRevision(view *fileRevisionPicker, revision gitops.FileRevision) {
	wf := view.file
	a.closeFileRevisionPicker()
	detail := gitops.CommitDetail{CommitEntry: gitops.CommitEntry{
		RepoID: wf.RepoID, RepoName: wf.RepoName, RepoPath: wf.RepoPath,
		Hash: revision.Hash, ShortHash: shortRevision(revision.Hash), Subject: revision.Subject,
	}}
	file := gitops.CommitFile{Path: wf.RelPath, Status: revision.File.Status}
	if revision.File.Path != file.Path {
		file.OldPath = revision.File.Path
	}
	detail.Files = []gitops.CommitFile{file}
	a.loadCommittedDiff(detail, file, revision.Hash, "")
}

func (a *App) buildFileRevisionPicker(c *ui.Context, p Palette) {
	view := a.fileRevisionPicker
	if view == nil {
		return
	}
	open := true
	ui.DialogBase(c.Key("file-revision-picker"), &open, func(backdrop, panel ui.Element) {
		panel.Width(555).Padding(12).Gap(8).Radius(8).Background(p.Surface).Border(1, p.Border)
		modalHeader(c, p, icoLog, "文件修订版本", "", nil, a.closeFileRevisionPicker)
		if view.loading || view.err != nil || len(view.entries) == 0 {
			message := "没有历史提交"
			if view.loading {
				message = "正在读取历史…"
			} else if view.err != nil {
				message = view.err.Error()
			}
			emptyState(c, p, icoLog, message, "")
			return
		}
		previous := view.selected
		switch {
		case c.Shortcut(0, ui.KeyUp):
			view.selected = max(0, view.selected-1)
		case c.Shortcut(0, ui.KeyDown):
			view.selected = min(len(view.entries)-1, view.selected+1)
		case c.Shortcut(0, ui.KeyEnter):
			a.comparePickedRevision(view, view.entries[view.selected])
			return
		}
		now := time.Now()
		ui.Scroll(c).MaxHeight(280).Focusable().AutoFocus().Children(func() {
			for i, rev := range view.entries {
				kc := c.Key(rev.Hash)
				row := ui.Row(kc).Height(27).Padding(0, 8).Gap(8).AlignItems(ui.Center).Cursor(ui.CursorPointer)
				if row.Clicked() {
					view.selected = i
				}
				if row.DoubleClicked() {
					a.comparePickedRevision(view, rev)
				}
				if i == view.selected {
					row.Background(p.ActiveRowBg)
					if i != previous {
						row.ScrollIntoView()
					}
				} else if row.Hovered() {
					row.Background(p.SurfaceHover)
				}
				row.Children(func() {
					ui.Text(kc, shortRevision(rev.Hash)).Width(70).Tooltip(rev.Hash)
					ui.Text(kc, fileHistoryDate(rev.AuthorUnix, now)).Width(118).SingleLine()
					ui.Text(kc, rev.Author).Grow(1).SingleLine()
				})
			}
		})
		ui.Text(c, "提交说明").FontSize(11).TextColor(p.TextMuted)
		ui.Text(c, view.entries[view.selected].Subject).SingleLine()
	})
	if !open && a.fileRevisionPicker == view {
		a.closeFileRevisionPicker()
	}
}
