package main

import (
	"context"
	"fmt"
	"path/filepath"
	"slices"
	"strings"
	"time"
	"unicode/utf8"

	"github.com/egoist/mygo/ui"
	"github.com/egoist/mygo/ui/editor"
	"github.com/iwangjie/nemorous/internal/conflict"
	"github.com/iwangjie/nemorous/internal/gitops"
)

// ---------- conflict resolver ----------

func (a *App) openConflictResolver(repoID string) {
	st := a.gitStatusByID(repoID)
	if st == nil {
		a.showToast("warn", "找不到仓库")
		return
	}
	path := st.Repo.AbsPath
	manual := false
	if mm, ok := a.manualMerges[repoID]; ok {
		path = mm.Path
		manual = true
	}
	sameRepo := a.conflictRepoID == repoID && a.conflictRepoPath == path
	a.showConflict = true
	a.conflictRepoID = repoID
	a.conflictRepoPath = path
	a.conflictRepoName = st.Repo.Name
	a.conflictManual = manual
	if !sameRepo {
		a.conflictFiles = nil
		a.conflictSelected = ""
		a.conflictEditor = nil
		a.conflictBaseView = nil
		a.conflictOursView = nil
		a.conflictTheirsView = nil
		a.conflictDoc = nil
		a.conflictHunkIdx = 0
		a.conflictDrafts = make(map[string]*editor.State)
		a.conflictHunkCount = make(map[string]int)
		a.conflictBeforeAuto = ""
		a.conflictBase, a.conflictOurs, a.conflictTheirs = "", "", ""
	}
	a.conflictBinary = false
	a.conflictConfirmAbort = false
	a.conflictConfirmClose = false
	a.conflictOpLabel = "继续操作"
	a.conflictErr = ""
	a.conflictNotice = ""
	a.loadConflictFiles()
	a.refreshConflictOpLabel()
}

func (a *App) loadConflictFiles() {
	if a.conflictRepoPath == "" {
		return
	}
	path := a.conflictRepoPath
	a.conflictLoading = true
	go func() {
		ctx, cancel := context.WithTimeout(context.Background(), 20*time.Second)
		defer cancel()
		files, err := conflict.UnresolvedFiles(ctx, path)
		a.runUI(func() {
			a.conflictLoading = false
			if err != nil {
				a.conflictErr = err.Error()
				return
			}
			a.conflictFiles = files
			if len(files) == 0 {
				a.conflictSelected = ""
				a.conflictEditor = nil
				a.conflictDoc = nil
				return
			}
			if a.conflictSelected == "" || !slices.Contains(files, a.conflictSelected) {
				a.selectConflictFile(files[0])
			}
		})
		// Count hunks of the other files for the file list.
		for _, rel := range files {
			content, err := conflict.ReadFile(path, rel)
			if err != nil {
				continue
			}
			doc, err := conflict.Parse(content)
			count := 0
			if err == nil {
				count = len(doc.Hunks)
			}
			a.runUI(func() {
				if a.conflictRepoPath == path {
					a.conflictHunkCount[rel] = count
				}
			})
		}
	}()
}

// editorText includes the buffer's line endings, so resolving a CRLF
// conflict does not rewrite the whole file.
func editorText(buf *editor.Buffer) string {
	if buf == nil {
		return ""
	}
	var b strings.Builder
	_, _ = buf.WriteTo(&b)
	return b.String()
}

func (a *App) stashConflictDraft() {
	if a.conflictSelected != "" && a.conflictEditor != nil && a.conflictDrafts != nil {
		a.conflictDrafts[a.conflictSelected] = a.conflictEditor
	}
}

func (a *App) selectConflictFile(rel string) {
	if a.conflictSelected == rel && a.conflictEditor != nil {
		return
	}
	a.stashConflictDraft()
	path := a.conflictRepoPath
	a.conflictSelected = rel
	a.conflictHunkIdx = 0
	a.conflictErr = ""
	a.conflictNotice = ""
	a.conflictBeforeAuto = ""
	reuse := a.conflictDrafts[rel]

	go func() {
		ctx, cancel := context.WithTimeout(context.Background(), 20*time.Second)
		defer cancel()
		base, ours, theirs := conflict.Versions(ctx, path, rel)
		content := ""
		fellBack := false
		if reuse != nil {
			content = editorText(reuse.Buffer)
		} else if data, err := conflict.ReadFile(path, rel); err == nil {
			content = data
		} else {
			if ours != "" {
				content = ours
			} else {
				content = theirs
			}
			fellBack = true
		}
		doc, perr := conflict.Parse(content)
		if perr == conflict.ErrNoMarkers {
			doc = nil
		}
		// Re-run a diff3 merge from the index to learn base sides and line
		// ranges even when the worktree used the default conflict style.
		if diff3Text, err := conflict.MergeFile(ctx, path, rel); err == nil {
			if diff3Doc, derr := conflict.Parse(diff3Text); derr == nil {
				conflict.EnrichWithDiff3(doc, diff3Doc)
			}
		}
		a.runUI(func() {
			a.conflictBase, a.conflictOurs, a.conflictTheirs = base, ours, theirs
			a.conflictBinary = isBinaryContent(base) || isBinaryContent(ours) || isBinaryContent(theirs) || isBinaryContent(content)
			a.conflictDoc = doc
			if perr != nil && perr != conflict.ErrNoMarkers {
				a.conflictErr = perr.Error()
			}
			if fellBack {
				a.conflictNotice = "工作区文件不可读（可能是删除/修改冲突），已显示索引版本"
			}
			lang := editor.LanguageFor(rel)
			newPane := func(text string) *editor.State {
				buf := editor.NewBuffer(text)
				buf.SetLanguage(lang)
				return &editor.State{
					Buffer:    buf,
					ReadOnly:  true,
					Font:      ui.Font{Size: a.cfg.EditorFontSize},
					NoFolding: true,
				}
			}
			a.conflictBaseView = newPane(base)
			a.conflictOursView = newPane(ours)
			a.conflictTheirsView = newPane(theirs)
			if reuse != nil {
				a.conflictEditor = reuse
			} else {
				buf := editor.NewBuffer(content)
				buf.SetLanguage(lang)
				diffBase := editor.NewBuffer(ours)
				diffBase.SetLanguage(lang)
				a.conflictEditor = &editor.State{
					Buffer:   buf,
					DiffBase: diffBase,
					SoftWrap: false,
					Font:     ui.Font{Size: a.cfg.EditorFontSize},
				}
			}
			if doc == nil {
				a.conflictHunkCount[rel] = 0
			} else {
				a.conflictHunkCount[rel] = len(doc.Hunks)
			}
			a.updateConflictPanes()
		})
	}()
}

func (a *App) updateConflictPanes() {
	idx := a.conflictHunkIdx
	if a.conflictDoc == nil || len(a.conflictDoc.Hunks) == 0 {
		for _, view := range []*editor.State{a.conflictBaseView, a.conflictOursView, a.conflictTheirsView} {
			if view != nil {
				view.LineBackgrounds = nil
			}
		}
		return
	}
	if idx < 0 {
		idx = 0
	}
	if idx >= len(a.conflictDoc.Hunks) {
		idx = len(a.conflictDoc.Hunks) - 1
	}
	a.conflictHunkIdx = idx
	h := a.conflictDoc.Hunks[idx]
	highlight := func(view *editor.State, start, count int, color ui.Color) {
		if view == nil {
			return
		}
		if count > 0 {
			view.LineBackgrounds = []editor.LineBackground{{First: start, Last: start + count - 1, Color: color}}
			view.ScrollTo(start)
		} else {
			view.LineBackgrounds = nil
		}
	}
	highlight(a.conflictBaseView, h.BaseStart, h.BaseCount, ui.RGBA(90, 120, 200, 0.16))
	highlight(a.conflictOursView, h.OursStart, h.OursCount, ui.RGBA(90, 120, 220, 0.20))
	highlight(a.conflictTheirsView, h.TheirsStart, h.TheirsCount, ui.RGBA(220, 160, 60, 0.20))
	if a.conflictEditor != nil {
		a.conflictEditor.ScrollTo(h.StartLine)
	}
}

func (a *App) refreshConflictDoc() {
	if a.conflictEditor == nil {
		a.conflictDoc = nil
		return
	}
	doc, err := conflict.Parse(editorText(a.conflictEditor.Buffer))
	if err == conflict.ErrNoMarkers {
		a.conflictDoc = nil
		a.conflictHunkCount[a.conflictSelected] = 0
		return
	}
	if err != nil {
		a.conflictErr = err.Error()
		return
	}
	a.conflictDoc = doc
	a.conflictHunkCount[a.conflictSelected] = len(doc.Hunks)
	a.conflictErr = ""
}

func (a *App) conflictNav(delta int) {
	if a.conflictDoc == nil || len(a.conflictDoc.Hunks) == 0 {
		return
	}
	n := len(a.conflictDoc.Hunks)
	a.conflictHunkIdx = (a.conflictHunkIdx + delta + n) % n
	a.updateConflictPanes()
}

func (a *App) conflictApplyCurrent(strategy conflict.Strategy) {
	a.conflictApplyHunk(a.conflictHunkIdx, strategy)
}

func (a *App) conflictApplyHunk(index int, strategy conflict.Strategy) {
	if a.conflictEditor == nil || a.conflictDoc == nil || index < 0 || index >= len(a.conflictDoc.Hunks) {
		return
	}
	content := editorText(a.conflictEditor.Buffer)
	resolved, err := conflict.ApplyTo(content, a.conflictDoc.Hunks[index], strategy)
	if err != nil {
		a.conflictErr = err.Error()
		return
	}
	a.conflictEditor.Buffer.Set(resolved)
	a.conflictBeforeAuto = ""
	a.refreshConflictDoc()
	if a.conflictDoc != nil && a.conflictHunkIdx >= len(a.conflictDoc.Hunks) {
		a.conflictHunkIdx = max(len(a.conflictDoc.Hunks)-1, 0)
	}
	a.updateConflictPanes()
}

func (a *App) conflictApplyAll(strategy conflict.Strategy) {
	if a.conflictEditor == nil {
		return
	}
	content := editorText(a.conflictEditor.Buffer)
	doc, err := conflict.Parse(content)
	if err != nil {
		a.conflictErr = "没有可解决的冲突标记"
		return
	}
	resolved, err := doc.Resolve(strategy)
	if err != nil {
		a.conflictErr = err.Error()
		return
	}
	a.conflictEditor.Buffer.Set(resolved)
	a.conflictBeforeAuto = ""
	a.refreshConflictDoc()
	a.conflictHunkIdx = 0
	a.updateConflictPanes()
}

func (a *App) conflictUseVersion(side conflict.Strategy) {
	if a.conflictEditor == nil {
		return
	}
	content := a.conflictOurs
	if side == conflict.Theirs {
		content = a.conflictTheirs
	} else if side == conflict.Base {
		content = a.conflictBase
	} else if side == conflict.Both {
		content = a.conflictOurs + a.conflictTheirs
	}
	a.conflictEditor.Buffer.Set(content)
	a.refreshConflictDoc()
	a.conflictHunkIdx = 0
	a.conflictNotice = "已采用 " + string(side) + " 版本"
	a.updateConflictPanes()
}

func (a *App) conflictAutoResolve() {
	if a.conflictEditor == nil || a.conflictDoc == nil {
		return
	}
	before := editorText(a.conflictEditor.Buffer)
	resolvedText, resolved, remaining := a.conflictDoc.SmartResolve()
	if resolved == 0 {
		a.conflictNotice = "没有可自动解决的无争议冲突"
		return
	}
	a.conflictEditor.Buffer.Set(resolvedText)
	a.conflictBeforeAuto = before
	a.refreshConflictDoc()
	a.conflictHunkIdx = 0
	a.conflictNotice = fmt.Sprintf("已自动解决 %d 个无争议冲突，剩余 %d 个", resolved, remaining)
	a.updateConflictPanes()
}

func (a *App) conflictUndoAuto() {
	if a.conflictEditor == nil || a.conflictBeforeAuto == "" {
		return
	}
	a.conflictEditor.Buffer.Set(a.conflictBeforeAuto)
	a.conflictBeforeAuto = ""
	a.refreshConflictDoc()
	a.conflictHunkIdx = 0
	a.conflictNotice = "已撤销自动解决"
	a.updateConflictPanes()
}

func (a *App) reloadConflictFile() {
	if a.conflictSelected == "" {
		return
	}
	rel := a.conflictSelected
	delete(a.conflictDrafts, rel)
	a.conflictEditor = nil
	a.conflictSelected = ""
	a.selectConflictFile(rel)
}

func (a *App) saveConflictFile() {
	if a.conflictEditor == nil || a.conflictSelected == "" {
		return
	}
	content := editorText(a.conflictEditor.Buffer)
	if _, err := conflict.Parse(content); err == nil {
		a.conflictErr = "文件仍有冲突标记，请先解决"
		return
	} else if err != conflict.ErrNoMarkers {
		a.conflictErr = err.Error()
		return
	}
	path := a.conflictRepoPath
	rel := a.conflictSelected
	go func() {
		ctx, cancel := context.WithTimeout(context.Background(), 20*time.Second)
		defer cancel()
		var err error
		if content == "" {
			err = conflict.RemoveAndStage(ctx, path, rel)
		} else {
			err = conflict.WriteAndStage(ctx, path, rel, content)
		}
		a.runUI(func() {
			if err != nil {
				a.conflictErr = err.Error()
				return
			}
			delete(a.conflictDrafts, rel)
			delete(a.conflictHunkCount, rel)
			a.conflictNotice = "已解决并暂存：" + rel
			a.conflictEditor = nil
			a.loadConflictFiles()
			a.RefreshAsync()
		})
	}()
}

func (a *App) refreshConflictOpLabel() {
	if a.conflictManual {
		a.conflictOpLabel = "返回合并视图完成"
		return
	}
	path := a.conflictRepoPath
	go func() {
		st := gitops.RepoOperationState(context.Background(), path)
		label := "继续操作"
		switch {
		case st.Rebase:
			label = "继续 Rebase"
		case st.CherryPick:
			label = "继续 Cherry-pick"
		case st.Revert:
			label = "继续 Revert"
		case st.Merge:
			label = "完成合并"
		}
		a.runUI(func() {
			if a.conflictRepoPath == path {
				a.conflictOpLabel = label
			}
		})
	}()
}

func (a *App) conflictOperationLabel() string {
	if a.conflictManual {
		return "返回合并视图完成"
	}
	if a.conflictOpLabel != "" {
		return a.conflictOpLabel
	}
	return "继续操作"
}

func (a *App) continueAfterConflict() {
	if a.conflictManual {
		a.showToast("info", "冲突已解决，回到合并视图点击“完成”")
		a.showConflict = false
		return
	}
	path := a.conflictRepoPath
	go func() {
		ctx, cancel := context.WithTimeout(context.Background(), 60*time.Second)
		defer cancel()
		msg, err := gitops.ContinueOperation(ctx, path)
		a.runUI(func() {
			if err != nil {
				a.conflictErr = err.Error()
				return
			}
			a.showConflict = false
			a.showToast("ok", "操作已继续")
			if msg != "" {
				a.showToast("ok", msg)
			}
			a.RefreshAsync()
		})
	}()
}

func (a *App) conflictDirty() bool {
	if a.conflictEditor != nil && a.conflictEditor.Buffer != nil && a.conflictEditor.Buffer.Modified() {
		return true
	}
	for _, draft := range a.conflictDrafts {
		if draft != nil && draft.Buffer != nil && draft.Buffer.Modified() {
			return true
		}
	}
	return false
}

func (a *App) closeConflictModal() {
	if a.conflictDirty() {
		a.conflictConfirmClose = true
		return
	}
	a.showConflict = false
}

func (a *App) conflictBinaryApply(side conflict.Strategy) {
	if a.conflictSelected == "" {
		return
	}
	path := a.conflictRepoPath
	rel := a.conflictSelected
	content := a.conflictOurs
	if side == conflict.Theirs {
		content = a.conflictTheirs
	} else if side == conflict.Base {
		content = a.conflictBase
	}
	go func() {
		ctx, cancel := context.WithTimeout(context.Background(), 20*time.Second)
		defer cancel()
		var err error
		if content == "" {
			err = conflict.RemoveAndStage(ctx, path, rel)
		} else {
			err = conflict.WriteAndStage(ctx, path, rel, content)
		}
		a.runUI(func() {
			if err != nil {
				a.conflictErr = err.Error()
				return
			}
			delete(a.conflictDrafts, rel)
			delete(a.conflictHunkCount, rel)
			a.conflictNotice = "已解决二进制冲突：" + rel
			a.conflictEditor = nil
			a.loadConflictFiles()
			a.RefreshAsync()
			a.refreshConflictOpLabel()
		})
	}()
}

func (a *App) conflictDeleteFile() {
	if a.conflictSelected == "" {
		return
	}
	path := a.conflictRepoPath
	rel := a.conflictSelected
	go func() {
		ctx, cancel := context.WithTimeout(context.Background(), 20*time.Second)
		defer cancel()
		err := conflict.RemoveAndStage(ctx, path, rel)
		a.runUI(func() {
			if err != nil {
				a.conflictErr = err.Error()
				return
			}
			delete(a.conflictDrafts, rel)
			delete(a.conflictHunkCount, rel)
			a.conflictNotice = "已删除并暂存：" + rel
			a.conflictEditor = nil
			a.loadConflictFiles()
			a.RefreshAsync()
		})
	}()
}

func isBinaryContent(s string) bool {
	if strings.ContainsRune(s, 0) {
		return true
	}
	return !utf8.ValidString(s)
}

func (a *App) conflictSideSubs() (string, string) {
	label := a.conflictOpLabel
	switch {
	case a.conflictManual:
		return "目标分支（目标版本）", "合入分支（来源版本）"
	case strings.Contains(label, "Rebase"):
		return "变基目标（上游）", "你的提交"
	case strings.Contains(label, "Cherry-pick"):
		return "当前分支", "被拣选的提交"
	case strings.Contains(label, "Revert"):
		return "当前分支", "被还原的提交"
	case strings.Contains(label, "完成合并"):
		return "当前分支（目标）", "合入的分支"
	default:
		return "当前版本", "合入版本"
	}
}

func (a *App) conflictPaneSimple(c *ui.Context, p Palette, title, sub string, view *editor.State, tone ui.Color) {
	ui.Column(c).Grow(1).FillHeight().Background(p.Surface).Border(1, p.BorderSubtle).Radius(8).Children(func() {
		ui.Row(c).Height(30).Padding(0, 9).Gap(5).AlignItems(ui.Center).BorderWidth(0, 0, 1, 0).BorderColor(p.BorderSubtle).Children(func() {
			ui.Box(c).Size(6, 6).Radius(3).Background(tone).Shrink(0)
			ui.Text(c, title).FontSize(10.5).FontWeight(weightBold).TextColor(p.TextPrimary)
			if sub != "" {
				ui.Text(c, "· "+sub).FontSize(9.5).TextColor(p.TextMuted).MaxLines(1)
			}
			ui.Spacer(c)
		})
		if view == nil || view.Buffer == nil || view.Buffer.Len() == 0 {
			ui.Column(c).Grow(1).Center().Children(func() {
				ui.Text(c, "无此版本").FontSize(10.5).TextColor(p.TextMuted)
			})
			return
		}
		editor.View(c, view).Grow(1).FillWidth()
	})
}

func (a *App) buildConflictModal(c *ui.Context, p Palette) {
	ui.DialogBase(c, &a.showConflict, func(backdrop, panel ui.Element) {
		backdrop.Background(ui.RGBA(15, 17, 24, 0.50))
		width, height := float32(1240), float32(820)
		if a.win != nil {
			if w, h := a.win.ContentSize(); w > 0 && h > 0 {
				width = min(width, float32(w)-28)
				height = min(height, float32(h)-28)
			}
		}
		panel.Width(width).Height(height).Padding(14).Gap(8).Radius(12).Background(p.AppBg).Border(1, p.Border)

		modalHeader(c, p, icoMerge, "冲突解决", a.conflictRepoName, func() {
			if a.conflictManual {
				badgeBox(c, "手动合并", p.Amber, p.AmberSoft, p.Amber, 9, true)
			}
			if len(a.conflictFiles) > 0 {
				label := filepath.Base(a.conflictSelected)
				if label == "" || label == "." {
					label = "选择文件"
				}
				menuIconPill(c, p, "conflict-file", icoFile, label, a.conflictSelected != "", func(m *ui.Menu) {
					for i, file := range a.conflictFiles {
						file := file
						text := fmt.Sprintf("%d. %s", i+1, filepath.ToSlash(file))
						if n := a.conflictHunkCount[file]; n > 0 {
							text += fmt.Sprintf("  (%d)", n)
						}
						if m.Item(text).Radio(a.conflictSelected == file).Chosen() {
							a.selectConflictFile(file)
						}
					}
				})
				ui.Text(c, fmt.Sprintf("%d/%d", a.currentConflictFileIndex()+1, len(a.conflictFiles))).FontSize(9.5).TextColor(p.TextMuted)
			}
		}, func() { a.closeConflictModal() })

		if a.conflictConfirmClose {
			ui.Row(c).FillWidth().Padding(6, 9).Gap(8).AlignItems(ui.Center).Radius(6).Background(p.AmberSoft).Children(func() {
				ui.Text(c, "有未保存的解决结果，关闭会丢弃这些编辑。").FontSize(10.5).TextColor(p.Amber).Grow(1)
				miniBtn(c, p, "继续编辑", func() { a.conflictConfirmClose = false })
				pill(c, p, "关闭并丢弃", true, func() {
					a.conflictConfirmClose = false
					a.conflictDrafts = make(map[string]*editor.State)
					a.conflictEditor = nil
					a.conflictSelected = ""
					a.conflictDoc = nil
					a.conflictBaseView = nil
					a.conflictOursView = nil
					a.conflictTheirsView = nil
					a.conflictFiles = nil
					a.showConflict = false
				})
			})
		}
		if a.conflictErr != "" {
			ui.Text(c, a.conflictErr).FontSize(10).TextColor(p.Danger).MaxLines(3)
		}
		if a.conflictNotice != "" {
			ui.Text(c, a.conflictNotice).FontSize(10).TextColor(p.Green).MaxLines(2)
		}

		if a.conflictSelected == "" {
			ui.Column(c).Grow(1).Center().Gap(10).Children(func() {
				if len(a.conflictFiles) == 0 {
					drawIcon(c, icoResolve, 34, p.Green)
					ui.Text(c, "所有冲突已解决").FontSize(16).FontWeight(weightBold).TextColor(p.TextPrimary)
					ui.Text(c, "文件已暂存").FontSize(10.5).TextColor(p.TextMuted)
					pill(c, p, a.conflictOperationLabel(), true, func() { a.continueAfterConflict() })
				} else {
					ui.Text(c, "选择要解决的冲突文件。").FontSize(11.5).TextColor(p.TextMuted)
				}
			})
			return
		}

		if a.conflictBinary {
			ui.Column(c).Grow(1).Center().Gap(10).Children(func() {
				drawIcon(c, icoAlert, 32, p.Amber)
				ui.Text(c, "二进制文件冲突").FontSize(16).FontWeight(weightBold).TextColor(p.TextPrimary)
				ui.Text(c, filepath.ToSlash(a.conflictSelected)).Font("monospace").FontSize(10.5).TextColor(p.TextMuted)
				ui.Text(c, "选择保留的版本").FontSize(10.5).TextColor(p.TextMuted)
				ui.Row(c).Gap(8).Children(func() {
					pill(c, p, "保留 Ours", true, func() { a.conflictBinaryApply(conflict.Ours) })
					pill(c, p, "采用 Theirs", false, func() { a.conflictBinaryApply(conflict.Theirs) })
					miniBtn(c, p, "删除文件", func() { a.conflictDeleteFile() })
				})
			})
			return
		}

		// IDEA merge toolbar: navigate, accept from either side, then finish.
		ui.Row(c).Height(32).Padding(0, 2).Gap(2).AlignItems(ui.Center).Children(func() {
			if a.conflictDoc != nil && len(a.conflictDoc.Hunks) > 0 {
				badgeBox(c, fmt.Sprintf("%d/%d", a.conflictHunkIdx+1, len(a.conflictDoc.Hunks)), p.Danger, p.DangerSoft, p.Danger, 9.5, true)
				toolButton(c, p, "conflict-prev", icoChevronUp, "上一个冲突 (⌥←)", false, true, func() { a.conflictNav(-1) })
				toolButton(c, p, "conflict-next", icoChevronDown, "下一个冲突 (⌥→)", false, true, func() { a.conflictNav(1) })
				ui.Box(c).Width(1).Height(16).Background(p.BorderSubtle)
				toolButtonTone(c, p, "conflict-ours", icoAcceptLeft, "采用 Ours (⌥1)", p.Accent, true, func() { a.conflictApplyCurrent(conflict.Ours) })
				toolButtonTone(c, p, "conflict-theirs", icoAcceptRight, "采用 Theirs (⌥2)", p.Amber, true, func() { a.conflictApplyCurrent(conflict.Theirs) })
				toolButtonTone(c, p, "conflict-both", icoBoth, "采用两者 (⌥3)", p.TextSecondary, true, func() { a.conflictApplyCurrent(conflict.Both) })
			} else {
				badgeBox(c, "已解决", p.Green, p.GreenSoft, p.Green, 9.5, true)
			}
			toolButton(c, p, "conflict-auto", icoSparkles, "自动合并不重叠修改", false, true, func() { a.conflictAutoResolve() })
			if a.conflictBeforeAuto != "" {
				toolButton(c, p, "conflict-undo", icoRestore, "撤销自动合并", false, true, func() { a.conflictUndoAuto() })
			}
			ui.Spacer(c)
			menuIconAction(c, p, "conflict-more", icoMore, "更多操作", false, func(m *ui.Menu) {
				if m.Item("全部采用 Ours").Chosen() {
					a.conflictApplyAll(conflict.Ours)
				}
				if m.Item("全部采用 Theirs").Chosen() {
					a.conflictApplyAll(conflict.Theirs)
				}
				if m.Item("全部采用 Both").Chosen() {
					a.conflictApplyAll(conflict.Both)
				}
				m.Separator()
				if m.Item("使用索引 Ours 版本").Chosen() {
					a.conflictUseVersion(conflict.Ours)
				}
				if m.Item("使用索引 Theirs 版本").Chosen() {
					a.conflictUseVersion(conflict.Theirs)
				}
				if a.conflictDoc != nil && len(a.conflictDoc.Hunks) > 0 && a.conflictDoc.Hunks[a.conflictHunkIdx].HasBase {
					if m.Item("采用 Base 版本").Chosen() {
						a.conflictApplyCurrent(conflict.Base)
					}
				}
				if a.conflictOurs == "" || a.conflictTheirs == "" {
					if m.Item("删除文件").Chosen() {
						a.conflictDeleteFile()
					}
				}
				m.Separator()
				if m.Item("重新加载文件").Chosen() {
					a.reloadConflictFile()
				}
				if m.Item("重新解析冲突块").Chosen() {
					a.refreshConflictDoc()
					a.updateConflictPanes()
				}
			})
			if a.conflictEditor != nil && a.conflictEditor.Buffer != nil && a.conflictEditor.Buffer.Modified() {
				ui.Box(c).Size(6, 6).Radius(3).Background(p.Amber).Tooltip("未保存").Label("未保存")
			}
			iconAction(c, p, "conflict-save", icoResolve, "保存并标记解决 (⌘S)", true, func() { a.saveConflictFile() })
		})

		ui.Row(c).Grow(1).FillWidth().Gap(6).Children(func() {
			oursSub, theirsSub := a.conflictSideSubs()
			a.conflictPaneSimple(c, p, "OURS", oursSub, a.conflictOursView, p.Accent)
			a.conflictPaneSimple(c, p, "RESULT", "", a.conflictEditor, p.Green)
			a.conflictPaneSimple(c, p, "THEIRS", theirsSub, a.conflictTheirsView, p.Amber)
		})

	})
}

func (a *App) currentConflictFileIndex() int {
	for i, file := range a.conflictFiles {
		if file == a.conflictSelected {
			return i
		}
	}
	return 0
}
