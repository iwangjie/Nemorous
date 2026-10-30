package main

import (
	"context"
	"path/filepath"

	"github.com/egoist/mygo/ui"
	"github.com/egoist/mygo/ui/editor"
	"github.com/iwangjie/nemorous/internal/config"
	"github.com/iwangjie/nemorous/internal/gitops"
	"github.com/iwangjie/nemorous/internal/localhistory"
)

func (a *App) historyStore() *localhistory.Store {
	if a.localHistoryStore != nil {
		return a.localHistoryStore
	}
	root := filepath.Join(filepath.Dir(config.ConfigPath()), "localhistory")
	store, err := localhistory.New(root)
	if err != nil {
		a.showToast("err", "本地历史不可用："+err.Error())
		return nil
	}
	a.localHistoryStore = store
	return store
}

func (a *App) openLocalHistory(repoID, repoPath, relPath string) {
	store := a.historyStore()
	if store == nil {
		return
	}
	a.showLocalHistory = true
	a.historyRepoID = repoID
	a.historyRepoPath = repoPath
	a.historyRelPath = relPath
	a.historySelected = ""
	a.historyContent = ""
	a.historyEditor = nil
	a.historyDiff = nil
	a.historyDiffMode = false
	a.historyErr = ""
	a.historyNotice = ""
	a.historyRestoreConfirm = false
	a.historyLoading = true
	go func() {
		entries, err := store.List(repoID, relPath)
		a.runUI(func() {
			a.historyLoading = false
			if err != nil {
				a.historyErr = err.Error()
				return
			}
			a.historyEntries = entries
			if len(entries) > 0 {
				a.selectLocalHistoryEntry(entries[0].ID)
			}
		})
	}()
}

func (a *App) selectLocalHistoryEntry(id string) {
	store := a.historyStore()
	if store == nil {
		return
	}
	a.historySelected = id
	a.historyErr = ""
	a.historyRestoreConfirm = false
	a.historyLoading = true
	repoID, relPath := a.historyRepoID, a.historyRelPath
	repoPath := a.historyRepoPath
	go func() {
		entry, err := store.Read(repoID, relPath, id)
		if err != nil {
			a.runUI(func() {
				a.historyLoading = false
				a.historyErr = err.Error()
			})
			return
		}
		current, _, _ := gitops.ReadRepoFile(context.Background(), repoPath, relPath)
		a.runUI(func() {
			a.historyLoading = false
			a.historyContent = entry.Content
			buf := editor.NewBuffer(entry.Content)
			buf.SetLanguage(editor.LanguageFor(relPath))
			a.historyEditor = &editor.State{
				Buffer:   buf,
				ReadOnly: true,
				SoftWrap: true,
				Font:     ui.Font{Size: a.cfg.EditorFontSize},
			}
			oldBuf := editor.NewBuffer(entry.Content)
			newBuf := editor.NewBuffer(current)
			oldBuf.SetLanguage(editor.LanguageFor(relPath))
			newBuf.SetLanguage(editor.LanguageFor(relPath))
			a.historyDiff = &editor.DiffState{
				Old: oldBuf, New: newBuf,
				Layout:   diffLayout(a.cfg.DiffSideBySide),
				Inline:   parseInlineMode(a.cfg.DiffInlineMode),
				Wrap:     a.cfg.EditorSoftWrap,
				Editable: false,
				Font:     ui.Font{Size: a.cfg.EditorFontSize},
			}
		})
	}()
}

func (a *App) restoreLocalHistoryEntry() {
	store := a.historyStore()
	if store == nil || a.historySelected == "" {
		return
	}
	repoID, repoPath, relPath, id := a.historyRepoID, a.historyRepoPath, a.historyRelPath, a.historySelected
	a.historyLoading = true
	go func() {
		err := store.Restore(repoID, repoPath, relPath, id)
		a.runUI(func() {
			a.historyLoading = false
			if err != nil {
				a.historyErr = err.Error()
				return
			}
			if tab := a.activeCodeTab(); tab != nil && tab.file.RepoID == repoID && tab.file.RelPath == relPath {
				a.reloadCodeTab(tab)
			}
			a.historyNotice = "已恢复快照到 " + relPath
			a.historyRestoreConfirm = false
			a.RefreshAsync()
		})
	}()
}

func (a *App) buildLocalHistoryModal(c *ui.Context, p Palette) {
	ui.DialogBase(c, &a.showLocalHistory, func(backdrop, panel ui.Element) {
		backdrop.Background(ui.RGBA(20, 22, 30, 0.40))
		width, height := float32(960), float32(640)
		if a.win != nil {
			if w, h := a.win.ContentSize(); w > 0 && h > 0 {
				width = min(width, float32(w)-50)
				height = min(height, float32(h)-50)
			}
		}
		panel.Width(width).Height(height).Padding(14).Gap(8).Radius(12).Background(p.Surface).Border(1, p.Border)
		fullPath := a.historyRepoID + "/" + a.historyRelPath
		modalHeader(c, p, icoClock, "本地历史", filepath.Base(a.historyRelPath), func() {
			if a.historySelected == "" {
				return
			}
			active := a.historyDiffMode
			toolButton(c, p, "lh-diff", icoColumns, "对比当前", active, true, func() { a.historyDiffMode = !a.historyDiffMode })
			toolButton(c, p, "lh-restore", icoRestore, "恢复此版本", false, true, func() { a.historyRestoreConfirm = !a.historyRestoreConfirm })
		}, func() { a.showLocalHistory = false })
		if a.historyErr != "" {
			ui.Text(c, a.historyErr).FontSize(10).TextColor(p.Danger)
		}
		if a.historyNotice != "" {
			ui.Text(c, a.historyNotice).FontSize(10).TextColor(p.Green)
		}
		if a.historyRestoreConfirm {
			ui.Row(c).FillWidth().Padding(5, 8).Gap(7).AlignItems(ui.Center).Radius(4).Background(p.AmberSoft).Children(func() {
				drawIcon(c, icoAlert, 12, p.Amber)
				ui.Text(c, "覆盖当前文件？").FontSize(10).TextColor(p.Amber).Grow(1).Tooltip(fullPath)
				toolButton(c, p, "lh-restore-yes", icoCheck, "确认恢复", false, true, func() { a.restoreLocalHistoryEntry() })
				toolButton(c, p, "lh-restore-no", icoClose, "取消", false, true, func() { a.historyRestoreConfirm = false })
			})
		}
		ui.Row(c).Grow(1).FillWidth().Gap(8).Children(func() {
			ui.Column(c).Width(260).FillHeight().Gap(2).Children(func() {
				ui.Scroll(c).Grow(1).Children(func() {
					if len(a.historyEntries) == 0 {
						emptyState(c, p, icoClock, "暂无快照", "")
					}
					for _, entry := range a.historyEntries {
						entry := entry
						kc := c.Key("lh-" + entry.ID)
						row := ui.Row(kc).Padding(5, 7).Gap(6).Radius(4).Cursor(ui.CursorPointer).AlignItems(ui.Center)
						if a.historySelected == entry.ID {
							row.Background(p.AccentSoft)
						}
						if row.Hovered() && a.historySelected != entry.ID {
							row.Background(p.SurfaceHover)
						}
						if row.Clicked() {
							a.selectLocalHistoryEntry(entry.ID)
						}
						row.Children(func() {
							ui.Text(kc, entry.At.Local().Format("01-02 15:04")).FontSize(9.5).TextColor(p.TextMuted).Width(82)
							ui.Text(kc, entry.Reason).FontSize(10.5).TextColor(p.TextSecondary).Grow(1).MaxLines(1)
						})
					}
				})
			})
			ui.Box(c).Width(1).FillHeight().Background(p.BorderSubtle)
			ui.Column(c).Grow(1).FillHeight().Children(func() {
				if a.historyLoading && a.historyEditor == nil {
					ui.Column(c).Grow(1).Center().Children(func() {
						ui.Spinner(c).Size(16, 16).Label("读取快照").TextColor(p.TextMuted)
					})
					return
				}
				if a.historyEditor == nil {
					emptyState(c, p, icoClock, "选择快照查看内容", "")
					return
				}
				if a.historyDiffMode && a.historyDiff != nil {
					editor.DiffView(c, a.historyDiff).Grow(1).FillWidth()
					return
				}
				editor.View(c, a.historyEditor).Grow(1).FillWidth()
			})
		})
	})
}
