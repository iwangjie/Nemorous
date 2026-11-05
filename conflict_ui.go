package main

import (
	"context"
	"fmt"
	"math"
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
	a.conflictShowList = true
	a.conflictRepoID = repoID
	a.conflictRepoPath = path
	a.conflictRepoName = st.Repo.Name
	a.conflictManual = manual
	if !sameRepo {
		a.conflictFiles = nil
		a.conflictTotal = 0
		a.conflictSelected = ""
		a.conflictEditor = nil
		a.conflictBaseView = nil
		a.conflictOursView = nil
		a.conflictTheirsView = nil
		a.conflictDoc = nil
		a.conflictHunkIdx = 0
		a.conflictDrafts = make(map[string]*editor.State)
		a.conflictHunkCount = make(map[string]int)
		a.conflictKinds = make(map[string]conflict.Kind)
		a.conflictBinaryFiles = make(map[string]bool)
		a.conflictNoMarkerFiles = make(map[string]bool)
		a.conflictHunkTotals = make(map[string]int)
		a.conflictResolved = nil
		a.conflictResolvedOpen = true
		a.conflictBasePresent = false
		a.conflictOursPresent = false
		a.conflictTheirsPresent = false
		a.conflictBeforeAuto = ""
		a.conflictBase, a.conflictOurs, a.conflictTheirs = "", "", ""
	}
	a.conflictOursBranch, a.conflictTheirsBranch = "", ""
	if target := a.mergeTargetByID(repoID); target != nil {
		a.conflictOursBranch = target.TargetBranch
		a.conflictTheirsBranch = target.SourceBranch
	}
	a.conflictBinary = false
	a.conflictConfirmAbort = false
	a.conflictConfirmClose = false
	if !sameRepo {
		a.conflictScrollInit = false
	}
	a.conflictOpLabel = "继续操作"
	a.conflictErr = ""
	a.conflictStatus = ""
	a.conflictNoMarkers = false
	a.conflictUndoRaw = ""
	a.conflictUndoExpect = ""
	a.loadConflictFiles()
	a.refreshConflictOpLabel()
}

func (a *App) loadConflictFiles() {
	if a.conflictRepoPath == "" {
		return
	}
	path := a.conflictRepoPath
	a.conflictLoadSeq++
	seq := a.conflictLoadSeq
	a.conflictLoading = true
	go func() {
		ctx, cancel := context.WithTimeout(context.Background(), 20*time.Second)
		defer cancel()
		entries, err := conflict.UnmergedEntries(ctx, path)
		a.runUI(func() {
			if seq != a.conflictLoadSeq || a.conflictRepoPath != path {
				return
			}
			a.conflictLoading = false
			if err != nil {
				a.conflictErr = err.Error()
				return
			}
			files := make([]string, len(entries))
			kinds := make(map[string]conflict.Kind, len(entries))
			for i, entry := range entries {
				files[i] = entry.Path
				kinds[entry.Path] = entry.Kind
			}
			a.conflictFiles = files
			// A file that is unmerged again must leave the resolved section.
			if len(a.conflictResolved) > 0 {
				kept := a.conflictResolved[:0]
				for _, rec := range a.conflictResolved {
					if !slices.Contains(files, rec.Path) {
						kept = append(kept, rec)
					}
				}
				a.conflictResolved = kept
			}
			if n := len(files) + len(a.conflictResolved); n > a.conflictTotal {
				a.conflictTotal = n
			}
			a.conflictKinds = kinds
			a.conflictHunkCount = make(map[string]int)
			a.conflictBinaryFiles = make(map[string]bool)
			a.conflictNoMarkerFiles = make(map[string]bool)
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
		for _, entry := range entries {
			rel := entry.Path
			content, err := conflict.ReadFile(path, rel)
			if err != nil {
				continue
			}
			doc, err := conflict.Parse(content)
			count := 0
			if err == nil {
				count = len(doc.Hunks)
			}
			noMarkers := err == conflict.ErrNoMarkers
			binary := isBinaryContent(content)
			a.runUI(func() {
				if seq != a.conflictLoadSeq || a.conflictRepoPath != path {
					return
				}
				a.conflictHunkCount[rel] = count
				if _, ok := a.conflictHunkTotals[rel]; !ok {
					a.conflictHunkTotals[rel] = count
				}
				if noMarkers {
					a.conflictNoMarkerFiles[rel] = true
				} else {
					delete(a.conflictNoMarkerFiles, rel)
				}
				if binary {
					a.conflictBinaryFiles[rel] = true
				} else {
					delete(a.conflictBinaryFiles, rel)
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

func (a *App) openConflictFile(rel string) {
	a.conflictShowList = false
	a.selectConflictFile(rel)
}

func (a *App) selectConflictFile(rel string) {
	if a.conflictSelected == rel && a.conflictEditor != nil {
		return
	}
	a.stashConflictDraft()
	path := a.conflictRepoPath
	a.conflictSelectSeq++
	seq := a.conflictSelectSeq
	a.conflictSelected = rel
	a.conflictHunkIdx = 0
	a.conflictErr = ""
	a.conflictStatus = ""
	a.conflictUndoRaw = ""
	a.conflictUndoExpect = ""
	a.conflictBeforeAuto = ""
	reuse := a.conflictDrafts[rel]

	go func() {
		ctx, cancel := context.WithTimeout(context.Background(), 20*time.Second)
		defer cancel()
		baseStage, oursStage, theirsStage := conflict.Stages(ctx, path, rel)
		base, ours, theirs := baseStage.Text, oursStage.Text, theirsStage.Text
		raw := ""
		fellBack := false
		if data, err := conflict.ReadFile(path, rel); err == nil {
			raw = data
		} else {
			if oursStage.Present {
				raw = ours
			} else {
				raw = theirs
			}
			fellBack = true
		}
		doc, perr := conflict.Parse(raw)
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
		display := raw
		var ranges []conflict.LineRange
		if doc != nil {
			display, ranges = doc.OursPreview()
		}
		a.runUI(func() {
			// The file may have been resolved (or another one selected)
			// while the index read was in flight; never resurrect it. A
			// duplicate selection of the same path must not undo an accept
			// either, so the generation must still be current.
			if seq != a.conflictSelectSeq || a.conflictSelected != rel {
				return
			}
			a.conflictBase, a.conflictOurs, a.conflictTheirs = base, ours, theirs
			a.conflictBasePresent = baseStage.Present
			a.conflictOursPresent = oursStage.Present
			a.conflictTheirsPresent = theirsStage.Present
			a.conflictKeepEmpty = oursStage.Present
			a.conflictRaw = raw
			a.conflictBeforeRaw = ""
			a.conflictBinary = isBinaryContent(base) || isBinaryContent(ours) || isBinaryContent(theirs) || isBinaryContent(raw)
			a.conflictDoc = doc
			a.conflictRanges = ranges
			a.conflictNoMarkers = doc == nil
			a.conflictBinaryFiles[rel] = a.conflictBinary
			a.conflictScrollInit = false
			if perr != nil && perr != conflict.ErrNoMarkers {
				a.conflictErr = perr.Error()
			}
			if fellBack {
				a.conflictSay("warn", "工作区不可读，显示索引版本")
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
				buf := editor.NewBuffer(display)
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

// recordResolvedConflict moves a file into the list's resolved section and
// keeps its original hunk count for the row.
func (a *App) recordResolvedConflict(rel string) {
	for _, rec := range a.conflictResolved {
		if rec.Path == rel {
			return
		}
	}
	hunks := a.conflictHunkTotals[rel]
	if hunks == 0 {
		hunks = a.conflictHunkCount[rel]
	}
	a.conflictResolved = append(a.conflictResolved, resolvedConflictFile{
		Path:   rel,
		Kind:   a.conflictKind(rel),
		Hunks:  hunks,
		Binary: a.conflictBinaryFiles[rel],
	})
	if n := len(a.conflictResolved); n > a.conflictTotal {
		a.conflictTotal = n
	}
}

// conflictSay writes the latest file-level result into the modal header.
// Unlike a toast it cannot be hidden behind the dialog, and it never shifts
// the editor layout.
func (a *App) conflictKind(path string) conflict.Kind {
	if k, ok := a.conflictKinds[path]; ok {
		return k
	}
	return conflict.KindUnknown
}

func (a *App) conflictSay(kind, msg string) {
	a.conflictStatusKind = kind
	a.conflictStatus = msg
}

func (a *App) updateConflictPanes() {
	idx := a.conflictHunkIdx
	if a.conflictDoc == nil || len(a.conflictDoc.Hunks) == 0 {
		for _, view := range []*editor.State{a.conflictBaseView, a.conflictOursView, a.conflictTheirsView, a.conflictEditor} {
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
		bgs := make([]editor.LineBackground, 0, len(a.conflictRanges))
		for i, r := range a.conflictRanges {
			if r.Count <= 0 {
				continue
			}
			color := ui.RGBA(220, 90, 70, 0.10)
			if i == idx {
				color = ui.RGBA(220, 90, 70, 0.18)
			}
			bgs = append(bgs, editor.LineBackground{First: r.Start, Last: r.Start + r.Count - 1, Color: color})
		}
		a.conflictEditor.LineBackgrounds = bgs
		if idx < len(a.conflictRanges) {
			a.conflictEditor.ScrollTo(a.conflictRanges[idx].Start)
		}
	}
	a.conflictScrollInit = false
}

// syncConflictScroll mirrors the pane the user just scrolled into the other
// two. ScrollTo calls made while navigating hunks reset the baseline, so they
// never get overwritten by this follow-up.
func (a *App) syncConflictScroll() {
	views := [3]*editor.State{a.conflictOursView, a.conflictEditor, a.conflictTheirsView}
	for _, view := range views {
		if view == nil {
			return
		}
	}
	var ys [3]float64
	for i, view := range views {
		ys[i] = view.ScrollY()
	}
	if !a.conflictScrollInit {
		a.conflictScrollLast = ys
		a.conflictScrollInit = true
		return
	}
	lead := -1
	for i := range views {
		if math.Abs(ys[i]-a.conflictScrollLast[i]) > 0.5 {
			lead = i
			break
		}
	}
	if lead < 0 {
		return
	}
	for i, view := range views {
		if i != lead {
			view.SetScrollY(ys[lead])
		}
	}
	for i, view := range views {
		a.conflictScrollLast[i] = view.ScrollY()
	}
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

func conflictLineCount(text string) int {
	if text == "" {
		return 0
	}
	n := strings.Count(text, "\n")
	if !strings.HasSuffix(text, "\n") {
		n++
	}
	return n
}

// rememberConflictEdit keeps the last pre-resolution state so an accidental
// one-sided accept can be undone and the block resolved with BOTH instead.
func (a *App) rememberConflictEdit() {
	if a.conflictRaw == "" || a.conflictEditor == nil {
		a.conflictUndoRaw = ""
		a.conflictUndoExpect = ""
		return
	}
	a.conflictUndoRaw = a.conflictRaw
	a.conflictUndoIdx = a.conflictHunkIdx
}

func (a *App) conflictUndoLast() {
	if a.conflictUndoRaw == "" || a.conflictEditor == nil {
		return
	}
	if a.conflictUndoExpect != "" && editorText(a.conflictEditor.Buffer) != a.conflictUndoExpect {
		a.conflictSay("warn", "结果已手动编辑，无法撤销上一步")
		return
	}
	doc, err := conflict.Parse(a.conflictUndoRaw)
	if err != nil {
		return
	}
	display, ranges := doc.OursPreview()
	a.conflictEditor.Buffer.Set(display)
	a.conflictRaw = a.conflictUndoRaw
	a.conflictDoc = doc
	a.conflictRanges = ranges
	a.conflictHunkIdx = min(a.conflictUndoIdx, len(doc.Hunks)-1)
	a.conflictHunkCount[a.conflictSelected] = len(doc.Hunks)
	a.conflictUndoRaw = ""
	a.conflictUndoExpect = ""
	a.conflictSay("ok", "已撤销上一步")
	a.updateConflictPanes()
}

func (a *App) conflictApplyHunk(index int, strategy conflict.Strategy) {
	if a.conflictEditor == nil || a.conflictDoc == nil || index < 0 || index >= len(a.conflictDoc.Hunks) {
		return
	}
	if index >= len(a.conflictRanges) {
		a.conflictErr = "冲突块位置已变化，请重新加载文件"
		return
	}
	h := a.conflictDoc.Hunks[index]
	r := a.conflictRanges[index]
	replacement, err := conflict.HunkText(h, strategy)
	if err != nil {
		a.conflictErr = err.Error()
		return
	}
	content := editorText(a.conflictEditor.Buffer)
	lines := strings.SplitAfter(content, "\n")
	if r.Start < 0 || r.Count < 0 || r.Start+r.Count > len(lines) {
		a.conflictErr = "冲突块位置已变化，请重新加载文件"
		return
	}
	if actual := strings.Join(lines[r.Start:r.Start+r.Count], ""); actual != h.Ours {
		a.conflictErr = "结果已被手动编辑，请重新加载后再选择冲突块"
		return
	}
	resolved, err := conflict.ApplyRange(content, r.Start, r.Count, replacement)
	if err != nil {
		a.conflictErr = err.Error()
		return
	}
	a.rememberConflictEdit()
	a.conflictEditor.Buffer.Set(resolved)
	a.conflictUndoExpect = editorText(a.conflictEditor.Buffer)
	a.conflictErr = ""
	a.conflictStatus = ""
	if a.conflictRaw != "" {
		if nextRaw, err := conflict.ApplyTo(a.conflictRaw, h, strategy); err == nil {
			a.conflictRaw = nextRaw
		} else {
			a.conflictRaw = ""
		}
	}
	a.conflictBeforeAuto = ""
	a.conflictBeforeRaw = ""
	a.conflictDoc.Hunks = append(a.conflictDoc.Hunks[:index], a.conflictDoc.Hunks[index+1:]...)
	a.conflictRanges = append(a.conflictRanges[:index], a.conflictRanges[index+1:]...)
	delta := conflictLineCount(replacement) - r.Count
	for i := index; i < len(a.conflictRanges); i++ {
		a.conflictRanges[i].Start += delta
	}
	if len(a.conflictDoc.Hunks) == 0 {
		a.conflictDoc = nil
		a.conflictRanges = nil
		a.conflictRaw = ""
		a.conflictHunkIdx = 0
		a.conflictHunkCount[a.conflictSelected] = 0
	} else {
		if a.conflictHunkIdx >= len(a.conflictDoc.Hunks) {
			a.conflictHunkIdx = len(a.conflictDoc.Hunks) - 1
		}
		a.conflictHunkCount[a.conflictSelected] = len(a.conflictDoc.Hunks)
	}
	a.updateConflictPanes()
}

func (a *App) conflictApplyAll(strategy conflict.Strategy) {
	if a.conflictEditor == nil {
		return
	}
	raw := a.conflictRaw
	if raw == "" {
		raw = editorText(a.conflictEditor.Buffer)
	}
	doc, err := conflict.Parse(raw)
	if err != nil {
		a.conflictErr = "没有可解决的冲突标记"
		return
	}
	resolved, err := doc.Resolve(strategy)
	if err != nil {
		a.conflictErr = err.Error()
		return
	}
	a.rememberConflictEdit()
	a.conflictEditor.Buffer.Set(resolved)
	a.conflictUndoExpect = editorText(a.conflictEditor.Buffer)
	a.conflictRaw = resolved
	a.conflictDoc = nil
	a.conflictRanges = nil
	a.conflictBeforeAuto = ""
	a.conflictBeforeRaw = ""
	a.conflictHunkIdx = 0
	a.conflictHunkCount[a.conflictSelected] = 0
	a.conflictSay("ok", "整文件采用 "+string(strategy))
	a.updateConflictPanes()
}

func (a *App) conflictUseVersion(side conflict.Strategy) {
	if a.conflictEditor == nil {
		return
	}
	content := a.conflictOurs
	present := a.conflictOursPresent
	if side == conflict.Theirs {
		content, present = a.conflictTheirs, a.conflictTheirsPresent
	} else if side == conflict.Base {
		content, present = a.conflictBase, a.conflictBasePresent
	} else if side == conflict.Both {
		content, present = a.conflictOurs+a.conflictTheirs, a.conflictOursPresent || a.conflictTheirsPresent
	}
	a.conflictKeepEmpty = present && content == ""
	a.rememberConflictEdit()
	a.conflictEditor.Buffer.Set(content)
	a.conflictUndoExpect = editorText(a.conflictEditor.Buffer)
	a.conflictRaw = ""
	a.conflictDoc = nil
	a.conflictRanges = nil
	a.conflictBeforeAuto = ""
	a.conflictBeforeRaw = ""
	a.conflictHunkIdx = 0
	a.conflictHunkCount[a.conflictSelected] = 0
	a.conflictSay("ok", "整文件替换为索引 "+string(side))
	a.updateConflictPanes()
}

func (a *App) conflictAutoResolve() {
	if a.conflictEditor == nil || a.conflictDoc == nil {
		return
	}
	raw := a.conflictRaw
	if raw == "" {
		a.conflictSay("warn", "结果已手动编辑，先重新加载")
		return
	}
	doc, err := conflict.Parse(raw)
	if err != nil {
		a.conflictSay("warn", "没有可自动解决的冲突")
		return
	}
	resolvedRaw, resolved, remaining := doc.SmartResolve()
	if resolved == 0 {
		a.conflictSay("warn", "没有可自动解决的冲突块")
		return
	}
	newDoc, perr := conflict.Parse(resolvedRaw)
	if perr == conflict.ErrNoMarkers {
		newDoc = nil
	} else if perr != nil {
		a.conflictErr = perr.Error()
		return
	}
	display := resolvedRaw
	var ranges []conflict.LineRange
	if newDoc != nil {
		display, ranges = newDoc.OursPreview()
	}
	a.conflictUndoRaw = ""
	a.conflictUndoExpect = ""
	a.conflictBeforeAuto = editorText(a.conflictEditor.Buffer)
	a.conflictBeforeRaw = a.conflictRaw
	a.conflictEditor.Buffer.Set(display)
	a.conflictRaw = resolvedRaw
	a.conflictDoc = newDoc
	a.conflictRanges = ranges
	a.conflictHunkIdx = 0
	if newDoc == nil {
		a.conflictHunkCount[a.conflictSelected] = 0
	} else {
		a.conflictHunkCount[a.conflictSelected] = len(newDoc.Hunks)
	}
	a.conflictSay("ok", fmt.Sprintf("自动解决 %d · 剩余 %d", resolved, remaining))
	a.updateConflictPanes()
}

func (a *App) conflictUndoAuto() {
	if a.conflictEditor == nil || a.conflictBeforeAuto == "" {
		return
	}
	a.conflictEditor.Buffer.Set(a.conflictBeforeAuto)
	a.conflictRaw = a.conflictBeforeRaw
	a.conflictBeforeAuto = ""
	a.conflictBeforeRaw = ""
	a.conflictUndoRaw = ""
	a.conflictUndoExpect = ""
	if a.conflictRaw != "" {
		if doc, err := conflict.Parse(a.conflictRaw); err == nil {
			a.conflictDoc = doc
			_, a.conflictRanges = doc.OursPreview()
			a.conflictHunkCount[a.conflictSelected] = len(doc.Hunks)
		} else {
			a.conflictDoc = nil
			a.conflictRanges = nil
			a.conflictHunkCount[a.conflictSelected] = 0
		}
	} else {
		a.conflictDoc = nil
		a.conflictRanges = nil
		a.conflictHunkCount[a.conflictSelected] = 0
	}
	a.conflictHunkIdx = 0
	a.conflictSay("ok", "已撤销自动合并")
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
	if a.conflictDoc != nil && len(a.conflictDoc.Hunks) > 0 {
		a.conflictErr = fmt.Sprintf("还有 %d 个冲突块未选择，请用左侧箭头或「全部采用」处理", len(a.conflictDoc.Hunks))
		return
	}
	if _, err := conflict.Parse(content); err == nil {
		a.conflictErr = "文件仍有冲突标记，请先解决"
		return
	} else if err != conflict.ErrNoMarkers {
		a.conflictErr = err.Error()
		return
	}
	path := a.conflictRepoPath
	rel := a.conflictSelected
	// An empty result is a real empty file when the chosen side exists; only
	// choosing a missing/deleted side means the resolution is a deletion.
	removeEmpty := content == "" && !a.conflictKeepEmpty
	go func() {
		ctx, cancel := context.WithTimeout(context.Background(), 20*time.Second)
		defer cancel()
		var err error
		if removeEmpty {
			err = conflict.RemoveAndStage(ctx, path, rel)
		} else {
			err = conflict.WriteAndStage(ctx, path, rel, content)
		}
		a.runUI(func() {
			if err != nil {
				a.conflictErr = err.Error()
				return
			}
			a.recordResolvedConflict(rel)
			delete(a.conflictDrafts, rel)
			delete(a.conflictHunkCount, rel)
			a.conflictUndoRaw = ""
			a.conflictUndoExpect = ""
			a.conflictEditor = nil
			a.conflictShowList = true
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
		stage := 2
		if side == conflict.Theirs {
			stage = 3
		} else if side == conflict.Base {
			stage = 1
		}
		if content != "" {
			err = conflict.WriteAndStage(ctx, path, rel, content)
		} else if size, sizeErr := conflict.StageSize(ctx, path, rel, stage); sizeErr != nil {
			// The selected side is absent, i.e. a delete/modify conflict.
			err = conflict.RemoveAndStage(ctx, path, rel)
		} else if size == 0 {
			// The side exists but is an empty file; keep it empty.
			err = conflict.WriteAndStage(ctx, path, rel, "")
		} else {
			// The index still has bytes: retry the read rather than deleting.
			base, ours, theirs := conflict.Versions(ctx, path, rel)
			retried := theirs
			if stage == 1 {
				retried = base
			} else if stage == 2 {
				retried = ours
			}
			if retried == "" {
				err = fmt.Errorf("无法读取索引阶段 %d，请重试", stage)
			} else {
				err = conflict.WriteAndStage(ctx, path, rel, retried)
			}
		}
		a.runUI(func() {
			if err != nil {
				a.conflictErr = err.Error()
				return
			}
			delete(a.conflictDrafts, rel)
			delete(a.conflictHunkCount, rel)
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
			a.recordResolvedConflict(rel)
			delete(a.conflictDrafts, rel)
			delete(a.conflictHunkCount, rel)
			a.conflictEditor = nil
			a.conflictShowList = true
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
		ours, theirs := "目标分支（目标版本）", "合入分支（来源版本）"
		if a.conflictOursBranch != "" {
			ours = "目标分支（" + a.conflictOursBranch + "）"
		}
		if a.conflictTheirsBranch != "" {
			theirs = "合入分支（" + a.conflictTheirsBranch + "）"
		}
		return ours, theirs
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

func (a *App) conflictPaneSimple(c *ui.Context, p Palette, title, sub string, view *editor.State, tone ui.Color, present, deleted bool) {
	ui.Column(c).Grow(1).FillHeight().Background(p.Surface).Border(1, p.BorderSubtle).Radius(8).Children(func() {
		ui.Row(c).Height(30).Padding(0, 9).Gap(5).AlignItems(ui.Center).BorderWidth(0, 0, 1, 0).BorderColor(p.BorderSubtle).Children(func() {
			ui.Box(c).Size(6, 6).Radius(3).Background(tone).Shrink(0)
			ui.Text(c, title).FontSize(10.5).FontWeight(weightBold).TextColor(p.TextPrimary)
			if sub != "" {
				ui.Text(c, "· "+sub).FontSize(9.5).TextColor(p.TextMuted).MaxLines(1)
			}
			if deleted {
				ui.Text(c, "· 已删除").FontSize(9.5).FontWeight(weightSemibold).TextColor(p.Danger).MaxLines(1)
			} else if present && view != nil && view.Buffer != nil && view.Buffer.Len() == 0 {
				ui.Text(c, "· 空文件").FontSize(9.5).TextColor(p.TextMuted).MaxLines(1)
			}
			ui.Spacer(c)
		})
		if deleted {
			if view == nil || view.Buffer == nil || view.Buffer.LineCount() == 0 {
				ui.Column(c).Grow(1).Center().Children(func() {
					ui.Text(c, "已删除（删除前无内容）").FontSize(10.5).TextColor(p.Danger)
				})
				return
			}
			// Show what the delete removed, all in red, so a delete conflict
			// is a diff instead of three panes saying "no version".
			view.LineBackgrounds = []editor.LineBackground{{First: 0, Last: view.Buffer.LineCount() - 1, Color: ui.RGBA(239, 68, 68, 0.16)}}
			editor.View(c, view).Grow(1).FillWidth()
			return
		}
		if !present || view == nil || view.Buffer == nil {
			ui.Column(c).Grow(1).Center().Children(func() {
				ui.Text(c, "无此版本").FontSize(10.5).TextColor(p.TextMuted)
			})
			return
		}
		if view.Buffer.Len() == 0 {
			ui.Column(c).Grow(1).Center().Children(func() {
				ui.Text(c, "空文件").FontSize(10.5).TextColor(p.TextMuted)
			})
			return
		}
		editor.View(c, view).Grow(1).FillWidth()
	})
}

// conflictArrowRail puts IDEA's accept arrow in the gutter beside the
// change, so it reads as "copy this side into RESULT" rather than a toolbar
// command far from the code.
func (a *App) conflictArrowRail(c *ui.Context, p Palette, side conflict.Strategy) {
	view := a.conflictOursView
	tone, bg, tip, icon := p.AccentSoftText, p.AccentSoft, "采用 OURS（只保留本侧；两侧都要用 BOTH）", icoAcceptRight
	if side == conflict.Theirs {
		view = a.conflictTheirsView
		tone, bg, tip, icon = p.Amber, p.AmberSoft, "采用 THEIRS（只保留本侧；两侧都要用 BOTH）", icoAcceptLeft
	}
	if view == nil || a.conflictDoc == nil {
		return
	}
	width := float32(22)
	if side == conflict.Ours {
		width = 56 // accept OURS plus BOTH, next to the block
	}
	ui.Column(c).Width(width).FillHeight().Clip().Children(func() {
		for i, h := range a.conflictDoc.Hunks {
			start, count := h.OursStart, h.OursCount
			if side == conflict.Theirs {
				start, count = h.TheirsStart, h.TheirsCount
			}
			if count <= 0 {
				continue
			}
			idx := i
			top := float32(30 + view.LineTop(start) - view.ScrollY())
			ui.Row(c).Absolute().Left(2).Top(top).Height(20).Gap(2).Children(func() {
				sideName := "OURS"
				if side == conflict.Theirs {
					sideName = "THEIRS"
				}
				btn := ui.Box(c).Key(fmt.Sprintf("conflict-rail-%s-%d", side, idx)).
					Size(18, 20).Radius(4).Center().Background(p.Surface).Border(1, p.Border).
					Cursor(ui.CursorPointer).Tooltip(tip).
					Label(fmt.Sprintf("冲突块 %d 采用 %s", idx+1, sideName))
				if btn.Hovered() {
					btn.Background(bg).BorderColor(p.BorderSubtle)
				}
				if btn.Clicked() {
					a.conflictHunkIdx = idx
					a.conflictApplyHunk(idx, side)
				}
				btn.Children(func() { drawIcon(c, icon, 12, tone) })

				if side == conflict.Ours {
					both := ui.Box(c).Key(fmt.Sprintf("conflict-rail-both-%d", idx)).
						Width(32).Height(20).Radius(4).Center().Background(p.Surface).
						Border(1, p.Border).Cursor(ui.CursorPointer).
						Tooltip("采用两者（OURS + THEIRS）").
						Label(fmt.Sprintf("冲突块 %d 采用 BOTH", idx+1))
					if both.Hovered() {
						both.Background(p.SurfaceHover).BorderColor(p.BorderSubtle)
					}
					if both.Clicked() {
						a.conflictHunkIdx = idx
						a.conflictApplyHunk(idx, conflict.Both)
					}
					both.Children(func() {
						ui.Text(c, "BOTH").FontSize(8).FontWeight(weightSemibold).TextColor(p.TextSecondary).PassThrough()
					})
				}
			})
		}
	})
}

func (a *App) conflictResultPane(c *ui.Context, p Palette) {
	ui.Column(c).Grow(1).FillHeight().Background(p.Surface).Border(1, p.BorderSubtle).Radius(8).Children(func() {
		ui.Row(c).Height(30).Padding(0, 9).Gap(5).AlignItems(ui.Center).BorderWidth(0, 0, 1, 0).BorderColor(p.BorderSubtle).Children(func() {
			ui.Box(c).Size(6, 6).Radius(3).Background(p.Green).Shrink(0)
			ui.Text(c, "RESULT").FontSize(10.5).FontWeight(weightBold).TextColor(p.TextPrimary)
			if a.conflictDoc != nil && len(a.conflictDoc.Hunks) > 0 {
				badgeBox(c, fmt.Sprintf("%d/%d", a.conflictHunkIdx+1, len(a.conflictDoc.Hunks)), p.Danger, p.DangerSoft, p.Danger, 9.5, true)
				toolButton(c, p, "conflict-result-prev", icoChevronUp, "上一个冲突块 (⌥←)", false, true, func() { a.conflictNav(-1) })
				toolButton(c, p, "conflict-result-next", icoChevronDown, "下一个冲突块 (⌥→)", false, true, func() { a.conflictNav(1) })
			} else if a.conflictNoMarkers {
				label := "无冲突标记"
				if a.conflictKind(a.conflictSelected).DeletesFile() {
					label = "待选择"
				}
				badgeBox(c, label, p.Amber, p.AmberSoft, p.Amber, 9.5, true)
			} else {
				badgeBox(c, "已解决", p.Green, p.GreenSoft, p.Green, 9.5, true)
			}
			ui.Spacer(c)
			if a.conflictDoc != nil && len(a.conflictDoc.Hunks) > 0 {
				accept := func(label string, fg, bg ui.Color, strategy conflict.Strategy, tip string) {
					btn := ui.ButtonBase(c).Key("conflict-accept-"+string(strategy)).
						Height(21).Padding(0, 6).Radius(4).Background(bg).
						AlignItems(ui.Center).Cursor(ui.CursorPointer).Tooltip(tip)
					if btn.Hovered() {
						btn.Background(p.SurfaceHover)
					}
					if btn.Clicked() {
						a.conflictApplyHunk(a.conflictHunkIdx, strategy)
					}
					btn.Children(func() {
						ui.Text(c, label).FontSize(9.5).FontWeight(weightSemibold).TextColor(fg).SingleLine().PassThrough()
					})
				}
				accept("← OURS", p.AccentSoftText, p.AccentSoft, conflict.Ours, "采用 OURS（目标版本）")
				accept("BOTH", p.TextSecondary, p.Surface2, conflict.Both, "采用两者")
				accept("THEIRS →", p.Amber, p.AmberSoft, conflict.Theirs, "采用 THEIRS（合入版本）")
			}
		})
		if a.conflictEditor == nil || a.conflictEditor.Buffer == nil || a.conflictEditor.Buffer.Len() == 0 {
			label := "无此版本"
			if a.conflictOursPresent {
				label = "空文件"
			} else if a.conflictKind(a.conflictSelected).DeletesFile() {
				label = "我方已删除（结果为空）"
			}
			ui.Column(c).Grow(1).Center().Children(func() {
				ui.Text(c, label).FontSize(10.5).TextColor(p.TextMuted)
			})
			return
		}
		editor.View(c, a.conflictEditor).Grow(1).FillWidth()
	})
}

func (a *App) buildConflictModal(c *ui.Context, p Palette) {
	a.syncConflictScroll()
	ui.DialogBase(c, &a.showConflict, func(backdrop, panel ui.Element) {
		backdrop.Background(ui.RGBA(15, 17, 24, 0.50))
		width, height := float32(1400), float32(860)
		if a.win != nil {
			if w, h := a.win.ContentSize(); w > 0 && h > 0 {
				width = min(width, float32(w)-24)
				height = min(height, float32(h)-24)
			}
		}
		panel.Width(width).Height(height).Padding(14).Gap(8).Radius(12).Background(p.AppBg).Border(1, p.Border)

		context := a.conflictRepoName
		if !a.conflictShowList && a.conflictSelected != "" {
			context += " · " + filepath.ToSlash(a.conflictSelected)
		}
		modalHeader(c, p, icoMerge, "冲突解决", context, func() {
			if a.conflictManual {
				badgeBox(c, "手动合并", p.Amber, p.AmberSoft, p.Amber, 9, true)
			}
			if !a.conflictShowList && len(a.conflictFiles) > 0 {
				toolButton(c, p, "conflict-back-list", icoArrowLeft, "返回冲突文件列表", false, true, func() {
					a.conflictShowList = true
				})
			}
			if len(a.conflictFiles) > 0 && !a.conflictShowList {
				if a.conflictStatus != "" {
					col := p.TextMuted
					switch a.conflictStatusKind {
					case "ok":
						col = p.Green
					case "warn":
						col = p.Amber
					case "err":
						col = p.Danger
					}
					ui.Text(c, a.conflictStatus).FontSize(9.5).TextColor(col).MaxLines(1)
				}
				ui.Box(c).Width(1).Height(16).Background(p.BorderSubtle)
				toolButton(c, p, "conflict-auto", icoSparkles, "自动合并不重叠修改", false, true, func() { a.conflictAutoResolve() })
				if a.conflictUndoRaw != "" {
					toolButton(c, p, "conflict-undo-last", icoRestore, "撤销上一步冲突选择", false, true, func() { a.conflictUndoLast() })
				}
				if a.conflictBeforeAuto != "" {
					toolButton(c, p, "conflict-undo", icoRestore, "撤销自动合并", false, true, func() { a.conflictUndoAuto() })
				}
				menuIconAction(c, p, "conflict-more", icoMore, "更多操作", false, func(m *ui.Menu) {
					if m.Item("所有冲突块 → Ours").Chosen() {
						a.conflictApplyAll(conflict.Ours)
					}
					if m.Item("所有冲突块 → Theirs").Chosen() {
						a.conflictApplyAll(conflict.Theirs)
					}
					if m.Item("所有冲突块 → 两者").Chosen() {
						a.conflictApplyAll(conflict.Both)
					}
					m.Separator()
					if m.Item("整个文件替换为索引 Ours").Chosen() {
						a.conflictUseVersion(conflict.Ours)
					}
					if m.Item("整个文件替换为索引 Theirs").Chosen() {
						a.conflictUseVersion(conflict.Theirs)
					}
					if a.conflictDoc != nil && len(a.conflictDoc.Hunks) > 0 && a.conflictDoc.Hunks[a.conflictHunkIdx].HasBase {
						if m.Item("当前冲突块采用 Base").Chosen() {
							a.conflictApplyCurrent(conflict.Base)
						}
					}
					if a.conflictOurs == "" || a.conflictTheirs == "" {
						if m.Item("删除文件").Chosen() {
							a.conflictDeleteFile()
						}
					}
					m.Separator()
					if a.conflictDoc != nil && len(a.conflictDoc.Hunks) > 0 {
						if m.Item("将当前结果标记为已解决").Chosen() {
							a.conflictDoc = nil
							a.conflictRanges = nil
							a.conflictRaw = ""
							a.conflictHunkIdx = 0
							a.conflictHunkCount[a.conflictSelected] = 0
							a.conflictSay("ok", "已标记为已解决，保存后暂存")
							a.updateConflictPanes()
						}
					}
					if m.Item("重新加载文件").Chosen() {
						a.reloadConflictFile()
					}
				})
				if a.conflictEditor != nil && a.conflictEditor.Buffer != nil && a.conflictEditor.Buffer.Modified() {
					ui.Box(c).Size(6, 6).Radius(3).Background(p.Amber).Tooltip("未保存").Label("未保存")
				}
				iconAction(c, p, "conflict-save", icoResolve, "保存并标记解决 (⌘S)", true, func() { a.saveConflictFile() })
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

		a.conflictWorkArea(c, p)

	})
}

func conflictKindColor(k conflict.Kind, p Palette) ui.Color {
	switch k {
	case conflict.KindBothAdded, conflict.KindAddedByUs, conflict.KindAddedByThem:
		return p.Green
	case conflict.KindDeletedByUs, conflict.KindDeletedByThem, conflict.KindBothDeleted:
		return p.Danger
	case conflict.KindBothModified:
		return p.Amber
	default:
		return p.TextMuted
	}
}

func sideStatusColor(status string, p Palette) ui.Color {
	switch status {
	case "删除":
		return p.Danger
	case "新增":
		return p.Green
	case "修改":
		return p.TextSecondary
	default:
		return p.TextMuted
	}
}

func (a *App) resolvedConflictRecord(rel string) (resolvedConflictFile, bool) {
	for _, rec := range a.conflictResolved {
		if rec.Path == rel {
			return rec, true
		}
	}
	return resolvedConflictFile{}, false
}

// conflictSimpleResolve stages only files whose every hunk is trivial
// (identical sides, or one side untouched). Anything ambiguous stays for
// manual review, matching IDEA's "resolve all simple conflicts".
func (a *App) conflictSimpleResolve() {
	if a.conflictSimpleBusy || len(a.conflictFiles) == 0 {
		return
	}
	path := a.conflictRepoPath
	files := slices.Clone(a.conflictFiles)
	kinds := make(map[string]conflict.Kind, len(files))
	for _, rel := range files {
		kinds[rel] = a.conflictKind(rel)
	}
	a.conflictSimpleBusy = true
	go func() {
		ctx, cancel := context.WithTimeout(context.Background(), 60*time.Second)
		defer cancel()
		var records []resolvedConflictFile
		for _, rel := range files {
			content, err := conflict.ReadFile(path, rel)
			if err != nil || isBinaryContent(content) {
				continue
			}
			doc, err := conflict.Parse(content)
			if err != nil || len(doc.Hunks) == 0 {
				continue
			}
			raw, resolved, remaining := doc.SmartResolve()
			if resolved == 0 || remaining > 0 {
				continue
			}
			if err := conflict.WriteAndStage(ctx, path, rel, raw); err != nil {
				continue
			}
			records = append(records, resolvedConflictFile{
				Path:  rel,
				Kind:  kinds[rel],
				Hunks: len(doc.Hunks),
			})
		}
		a.runUI(func() {
			a.conflictSimpleBusy = false
			if a.conflictRepoPath != path {
				return
			}
			for _, rec := range records {
				found := false
				for _, existing := range a.conflictResolved {
					if existing.Path == rec.Path {
						found = true
						break
					}
				}
				if !found {
					a.conflictResolved = append(a.conflictResolved, rec)
				}
			}
			if len(records) == 0 {
				a.showToast("info", "没有可自动解决的简单冲突")
			} else {
				a.showToast("ok", fmt.Sprintf("已自动解决 %d 个简单冲突", len(records)))
			}
			a.conflictShowList = true
			a.loadConflictFiles()
			a.RefreshAsync()
		})
	}()
}

func (a *App) conflictListPage(c *ui.Context, p Palette) {
	remaining := len(a.conflictFiles)
	done := len(a.conflictResolved)
	total := remaining + done
	if total == 0 {
		total = a.conflictTotal
	}
	oursHead, theirsHead := "我方更改", "对方更改"
	if a.conflictOursBranch != "" {
		oursHead = "我方 (" + a.conflictOursBranch + ")"
	}
	if a.conflictTheirsBranch != "" {
		theirsHead = "对方 (" + a.conflictTheirsBranch + ")"
	}
	ui.Column(c).Grow(1).FillHeight().Background(p.Surface).Border(1, p.BorderSubtle).Radius(8).Children(func() {
		ui.Row(c).Height(48).Padding(0, 12).Gap(7).AlignItems(ui.Center).BorderWidth(0, 0, 1, 0).BorderColor(p.BorderSubtle).Children(func() {
			if remaining == 0 && done > 0 {
				drawIcon(c, icoResolve, 16, p.Green)
			} else {
				drawIcon(c, icoAlert, 16, p.Amber)
			}
			if remaining == 0 && done > 0 {
				ui.Text(c, "已解决").FontSize(11.5).FontWeight(weightSemibold).TextColor(p.TextPrimary)
				badgeBox(c, fmt.Sprintf("%d", done), p.Green, p.GreenSoft, p.Green, 8.5, true)
			} else {
				ui.Text(c, "冲突文件").FontSize(11.5).FontWeight(weightSemibold).TextColor(p.TextPrimary)
				badgeBox(c, fmt.Sprintf("%d", remaining), p.Amber, p.AmberSoft, p.Amber, 8.5, true)
			}
			ui.Spacer(c)
			if remaining > 0 {
				if a.conflictSimpleBusy {
					ui.Text(c, "正在处理…").FontSize(9.5).TextColor(p.TextMuted)
				} else {
					miniBtn(c, p, "解决简单冲突", func() { a.conflictSimpleResolve() })
				}
				ui.Text(c, fmt.Sprintf("已完成 %d / %d", done, total)).FontSize(9.5).TextColor(p.TextMuted)
				pillDisabled(c, p, fmt.Sprintf("完成 · 还有 %d 个未解决", remaining))
			} else {
				ui.Text(c, "所有冲突已解决").FontSize(9.5).TextColor(p.Green)
				if a.conflictManual {
					miniBtn(c, p, "完成（本地）", func() { a.finishManualMerge(a.conflictRepoID, false) })
					pill(c, p, "完成并推送", true, func() { a.finishManualMerge(a.conflictRepoID, true) })
				} else {
					pill(c, p, a.conflictOperationLabel(), true, func() { a.continueAfterConflict() })
				}
			}
		})
		if a.conflictOursBranch != "" && a.conflictTheirsBranch != "" {
			ui.Row(c).Height(24).Padding(0, 12).AlignItems(ui.Center).BorderWidth(0, 0, 1, 0).BorderColor(p.BorderSubtle).Children(func() {
				ui.Text(c, fmt.Sprintf("将 %s 合并到 %s", a.conflictTheirsBranch, a.conflictOursBranch)).FontSize(9.5).TextColor(p.TextMuted).MaxLines(1)
			})
		}
		ui.Row(c).Height(28).Padding(0, 12).Gap(8).AlignItems(ui.Center).Background(p.Surface2).BorderWidth(0, 0, 1, 0).BorderColor(p.BorderSubtle).Children(func() {
			ui.Text(c, "文件").FontSize(9).FontWeight(weightSemibold).TextColor(p.TextMuted).Width(320).Shrink(0)
			ui.Text(c, "冲突").FontSize(9).FontWeight(weightSemibold).TextColor(p.TextMuted).Width(58).Shrink(0)
			ui.Text(c, oursHead).FontSize(9).FontWeight(weightSemibold).TextColor(p.TextMuted).Width(104).Shrink(0).MaxLines(1).Tooltip(a.conflictOursBranch)
			ui.Text(c, theirsHead).FontSize(9).FontWeight(weightSemibold).TextColor(p.TextMuted).Width(104).Shrink(0).MaxLines(1).Tooltip(a.conflictTheirsBranch)
			ui.Text(c, "路径").FontSize(9).FontWeight(weightSemibold).TextColor(p.TextMuted).Grow(1)
			ui.Box(c).Width(62).Shrink(0)
		})
		if a.conflictLoading && remaining == 0 && done == 0 {
			ui.Column(c).Grow(1).Center().Children(func() {
				ui.Text(c, "正在读取冲突…").FontSize(10.5).TextColor(p.TextMuted)
			})
			return
		}
		ui.Scroll(c).Grow(1).FillWidth().Children(func() {
			ui.Column(c).FillWidth().Padding(6, 0).Children(func() {
				if remaining > 0 {
					ui.Row(c).Height(30).Padding(0, 8).AlignItems(ui.Center).Children(func() {
						ui.Text(c, "未解决").FontSize(10).FontWeight(weightSemibold).TextColor(p.TextPrimary)
						badgeBox(c, fmt.Sprintf("%d 文件", remaining), p.Amber, p.AmberSoft, p.Amber, 8.5, true)
					})
					for _, rel := range a.conflictFiles {
						a.conflictListRow(c, p, rel, false)
					}
				}
				if done > 0 {
					section := ui.ButtonBase(c).Key("conflict-resolved-section").FillWidth().Height(32).
						Padding(0, 8).Gap(6).AlignItems(ui.Center).Cursor(ui.CursorPointer)
					if section.Hovered() {
						section.Background(p.SurfaceHover)
					}
					if section.Clicked() {
						a.conflictResolvedOpen = !a.conflictResolvedOpen
					}
					section.Children(func() {
						drawIcon(c, icoResolve, 13, p.Green)
						ui.Text(c, "已解决").FontSize(10).FontWeight(weightSemibold).TextColor(p.TextPrimary)
						badgeBox(c, fmt.Sprintf("%d 文件", done), p.Green, p.GreenSoft, p.Green, 8.5, true)
						ui.Spacer(c)
						ui.Text(c, map[bool]string{true: "收起", false: "展开"}[a.conflictResolvedOpen]).FontSize(9).TextColor(p.TextMuted)
					})
					if a.conflictResolvedOpen {
						for _, rec := range a.conflictResolved {
							a.conflictListRow(c, p, rec.Path, true)
						}
					}
				}
			})
		})
	})
}

func (a *App) conflictListRow(c *ui.Context, p Palette, rel string, resolved bool) {
	rec, _ := a.resolvedConflictRecord(rel)
	kind := a.conflictKind(rel)
	hunks := a.conflictHunkCount[rel]
	binary := a.conflictBinaryFiles[rel]
	noMarkers := a.conflictNoMarkerFiles[rel]
	if resolved {
		kind = rec.Kind
		hunks = rec.Hunks
		binary = rec.Binary
		noMarkers = kind.DeletesFile() || hunks == 0
	}
	var row ui.Element
	if resolved {
		row = ui.Row(c).FillWidth().MinHeight(38).Padding(4, 8).Gap(8).AlignItems(ui.Center).
			BorderWidth(0, 0, 1, 0).BorderColor(p.BorderSubtle)
	} else {
		row = ui.ButtonBase(c).Key("conflict-list-"+rel).FillWidth().MinHeight(38).Padding(4, 8).Gap(8).
			AlignItems(ui.Center).Cursor(ui.CursorPointer).
			BorderWidth(0, 0, 1, 0).BorderColor(p.BorderSubtle)
		if row.Hovered() {
			row.Background(p.SurfaceHover)
		}
		if row.Clicked() {
			a.openConflictFile(rel)
		}
	}
	row.Tooltip(kind.Label() + "：" + kind.Detail())
	row.Children(func() {
		drawIcon(c, func() *ui.SVG {
			if resolved {
				return icoResolve
			}
			return icoFile
		}(), 14, func() ui.Color {
			if resolved {
				return p.Green
			}
			return conflictKindColor(kind, p)
		}())
		ui.Text(c, filepath.Base(rel)).FontSize(10.5).FontWeight(weightSemibold).TextColor(p.TextPrimary).Width(292).Shrink(0).MaxLines(1)
		count := ""
		switch {
		case binary:
			count = "二进制"
		case hunks > 0 && resolved:
			count = fmt.Sprintf("%d/%d", hunks, hunks)
		case hunks > 0:
			count = fmt.Sprintf("%d 块", hunks)
		case noMarkers:
			count = "—"
		}
		ui.Text(c, count).FontSize(9).TextColor(p.TextMuted).Width(58).Shrink(0)
		ui.Text(c, kind.OursStatus()).FontSize(9.5).TextColor(sideStatusColor(kind.OursStatus(), p)).Width(104).Shrink(0)
		ui.Text(c, kind.TheirsStatus()).FontSize(9.5).TextColor(sideStatusColor(kind.TheirsStatus(), p)).Width(104).Shrink(0)
		ui.Text(c, filepath.ToSlash(rel)).Font("monospace").FontSize(8.5).TextColor(p.TextMuted).Grow(1).MaxLines(1)
		if resolved {
			ui.Text(c, "已解决").FontSize(9).TextColor(p.Green).Width(62).Shrink(0)
		} else {
			ui.Row(c).Height(24).Padding(0, 10).Radius(5).Background(p.AccentSoft).AlignItems(ui.Center).PassThrough().Children(func() {
				ui.Text(c, "解决").FontSize(9.5).FontWeight(weightSemibold).TextColor(p.AccentSoftText).PassThrough()
			})
		}
	})
}

func (a *App) conflictNoMarkersBanner(c *ui.Context, p Palette) {
	if !a.conflictNoMarkers {
		return
	}
	kind := a.conflictKind(a.conflictSelected)
	ui.Row(c).FillWidth().Padding(8, 8).Gap(8).AlignItems(ui.Center).Radius(6).Background(p.AmberSoft).Children(func() {
		drawIcon(c, icoAlert, 14, p.Amber)
		ui.Column(c).Grow(1).Gap(1).Children(func() {
			ui.Text(c, kind.Detail()).FontSize(10.5).TextColor(p.TextPrimary).MaxLines(2)
			hint := "没有冲突标记；请选择要保留的版本，或确认删除。"
			if kind.DeletesFile() && a.conflictBasePresent {
				hint = "没有冲突标记；红色为删除前的原始内容，可保留空文件或确认删除。"
			}
			ui.Text(c, hint).FontSize(9.5).TextColor(p.TextMuted).MaxLines(1)
		})
		if a.conflictOursPresent {
			miniBtn(c, p, "保留 OURS", func() { a.conflictUseVersion(conflict.Ours) })
		}
		if a.conflictTheirsPresent {
			miniBtn(c, p, "保留 THEIRS", func() { a.conflictUseVersion(conflict.Theirs) })
		}
		if !kind.DeletesFile() && a.conflictEditor != nil {
			miniBtn(c, p, "保留当前内容", func() { a.saveConflictFile() })
		}
		if kind.DeletesFile() || !a.conflictOursPresent || !a.conflictTheirsPresent {
			dangerPill(c, p, "确认删除", func() { a.conflictDeleteFile() })
		}
	})
}

func (a *App) conflictWorkArea(c *ui.Context, p Palette) {
	ui.Column(c).Grow(1).FillHeight().Children(func() {
		if a.conflictShowList && (len(a.conflictFiles) > 0 || len(a.conflictResolved) > 0) {
			a.conflictListPage(c, p)
			return
		}
		if a.conflictSelected == "" {
			ui.Column(c).Grow(1).FillHeight().Center().Gap(10).Children(func() {
				if a.conflictLoading {
					ui.Text(c, "正在读取冲突…").FontSize(11.5).TextColor(p.TextMuted)
					return
				}
				if len(a.conflictFiles) == 0 {
					drawIcon(c, icoResolve, 34, p.Green)
					ui.Text(c, "所有冲突已解决").FontSize(16).FontWeight(weightBold).TextColor(p.TextPrimary)
					ui.Text(c, "文件已暂存").FontSize(10.5).TextColor(p.TextMuted)
					if a.conflictManual {
						ui.Row(c).Gap(8).Children(func() {
							pill(c, p, "完成（本地）", false, func() { a.finishManualMerge(a.conflictRepoID, false) })
							pill(c, p, "完成并推送", true, func() { a.finishManualMerge(a.conflictRepoID, true) })
						})
					} else {
						pill(c, p, a.conflictOperationLabel(), true, func() { a.continueAfterConflict() })
					}
				} else {
					ui.Text(c, "选择要解决的冲突文件。").FontSize(11.5).TextColor(p.TextMuted)
				}
			})
			return
		}
		if a.conflictBinary {
			ui.Column(c).Grow(1).FillHeight().Center().Gap(10).Children(func() {
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
		a.conflictNoMarkersBanner(c, p)
		ui.Row(c).Grow(1).FillWidth().Gap(6).Children(func() {
			oursSub, theirsSub := a.conflictSideSubs()
			kind := a.conflictKind(a.conflictSelected)
			deletes := kind.DeletesFile()
			oursDeleted := deletes && !a.conflictOursPresent
			theirsDeleted := deletes && !a.conflictTheirsPresent
			oursView, theirsView := a.conflictOursView, a.conflictTheirsView
			if oursDeleted {
				oursView = a.conflictBaseView
			}
			if theirsDeleted {
				theirsView = a.conflictBaseView
			}
			a.conflictPaneSimple(c, p, "OURS", oursSub, oursView, p.Accent, a.conflictOursPresent, oursDeleted)
			a.conflictArrowRail(c, p, conflict.Ours)
			a.conflictResultPane(c, p)
			a.conflictArrowRail(c, p, conflict.Theirs)
			a.conflictPaneSimple(c, p, "THEIRS", theirsSub, theirsView, p.Amber, a.conflictTheirsPresent, theirsDeleted)
		})
		a.conflictActionBar(c, p)
	})
}

// conflictActionBar gives each hunk large, predictable targets at the
// bottom, like IDEA's merge dialog; the gutter arrows remain for precision.
func (a *App) conflictActionBar(c *ui.Context, p Palette) {
	if a.conflictDoc == nil || len(a.conflictDoc.Hunks) == 0 {
		return
	}
	idx := a.conflictHunkIdx
	if idx < 0 || idx >= len(a.conflictDoc.Hunks) {
		idx = 0
	}
	ui.Row(c).FillWidth().Height(34).Gap(7).AlignItems(ui.Center).Padding(0, 4).
		BorderWidth(1, 0, 0, 0).BorderColor(p.BorderSubtle).Children(func() {
		ui.Text(c, fmt.Sprintf("冲突块 %d / %d", idx+1, len(a.conflictDoc.Hunks))).FontSize(9.5).TextColor(p.TextMuted)
		ui.Box(c).Size(6, 6).Radius(3).Background(p.Accent)
		ui.Text(c, "我方").FontSize(9).TextColor(p.TextMuted)
		ui.Box(c).Size(6, 6).Radius(3).Background(p.Amber)
		ui.Text(c, "对方").FontSize(9).TextColor(p.TextMuted)
		ui.Spacer(c)
		miniBtn(c, p, "接受左侧 (⌥1)", func() { a.conflictApplyHunk(idx, conflict.Ours) })
		miniBtn(c, p, "接受右侧 (⌥2)", func() { a.conflictApplyHunk(idx, conflict.Theirs) })
		miniBtn(c, p, "接受两者 (⌥3)", func() { a.conflictApplyHunk(idx, conflict.Both) })
	})
}
