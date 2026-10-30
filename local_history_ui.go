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
		width, height := float32(980), float32(700)
		if a.win != nil {
			if w, h := a.win.ContentSize(); w > 0 && h > 0 {
				width = min(width, float32(w)-50)
				height = min(height, float32(h)-50)
			}
		}
		panel.Width(width).Height(height).Padding(16).Gap(8).Radius(12).Background(p.Surface).Border(1, p.Border)
		ui.Row(c).Justify(ui.SpaceBetween).AlignItems(ui.Center).Children(func() {
			ui.Row(c).Gap(8).AlignItems(ui.Center).Children(func() {
				drawIcon(c, icoClock, 16, p.Accent)
				ui.Text(c, "本地历史").FontSize(14).FontWeight(weightBold).TextColor(p.TextPrimary)
				ui.Text(c, a.historyRepoID+"/"+a.historyRelPath).Font("monospace").FontSize(10.5).TextColor(p.TextMuted).MaxLines(1)
			})
			ui.Row(c).Gap(6).Children(func() {
				if a.historySelected != "" {
					if a.historyRestoreConfirm {
						pill(c, p, "确认恢复（覆盖当前文件）", true, func() { a.restoreLocalHistoryEntry() })
						miniBtn(c, p, "取消", func() { a.historyRestoreConfirm = false })
					} else {
						pill(c, p, "恢复此版本", false, func() { a.historyRestoreConfirm = true })
					}
					if a.historyDiffMode {
						pill(c, p, "查看快照", false, func() { a.historyDiffMode = false })
					} else {
						pill(c, p, "对比当前", false, func() { a.historyDiffMode = true })
					}
				}
				miniBtn(c, p, "关闭", func() { a.showLocalHistory = false })
			})
		})
		if a.historyErr != "" {
			ui.Text(c, a.historyErr).FontSize(10.5).TextColor(p.Danger)
		}
		if a.historyNotice != "" {
			ui.Text(c, a.historyNotice).FontSize(10.5).TextColor(p.Green)
		}
		ui.Row(c).Grow(1).FillWidth().Gap(10).Children(func() {
			ui.Column(c).Width(250).FillHeight().Gap(4).Children(func() {
				ui.Text(c, "快照").FontSize(11).FontWeight(weightSemibold).TextColor(p.TextSecondary)
				ui.Scroll(c).Grow(1).Children(func() {
					if len(a.historyEntries) == 0 {
						ui.Text(c, "还没有快照。保存、提交或还原文件后会记录。").FontSize(10.5).TextColor(p.TextMuted)
					}
					for _, entry := range a.historyEntries {
						entry := entry
						kc := c.Key("lh-" + entry.ID)
						row := ui.Row(kc).Padding(5, 7).Gap(6).Radius(4).Cursor(ui.CursorPointer).AlignItems(ui.Center)
						if a.historySelected == entry.ID {
							row.Background(p.AccentSoft)
						}
						if row.Clicked() {
							a.selectLocalHistoryEntry(entry.ID)
						}
						row.Children(func() {
							ui.Text(kc, entry.At.Local().Format("01-02 15:04:05")).FontSize(10).TextColor(p.TextSecondary).Width(105)
							ui.Text(kc, entry.Reason).FontSize(10).TextColor(p.TextMuted).Grow(1).MaxLines(1)
						})
					}
				})
			})
			ui.Box(c).Width(1).FillHeight().Background(p.Border)
			ui.Column(c).Grow(1).FillHeight().Children(func() {
				if a.historyLoading && a.historyEditor == nil {
					ui.Text(c, "读取快照…").FontSize(11).TextColor(p.TextMuted)
					return
				}
				if a.historyEditor == nil {
					ui.Text(c, "选择左侧快照查看内容。").FontSize(11).TextColor(p.TextMuted)
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
