package main

import (
	"slices"

	"github.com/egoist/mygo/nemorouseditor"
	"github.com/egoist/mygo/ui"
	"github.com/iwangjie/nemorous/internal/gitops"
)

func reviewChangeRange(h editor.DiffHunk) gitops.ChangeRange {
	return gitops.ChangeRange{OldStart: h.OldStart, OldEnd: h.OldEnd, NewStart: h.NewStart, NewEnd: h.NewEnd}
}

func commitRangesIntersect(a, b gitops.ChangeRange) bool {
	return a == b || a.OldStart < b.OldEnd && b.OldStart < a.OldEnd ||
		a.NewStart < b.NewEnd && b.NewStart < a.NewEnd
}

func commitHunkIncluded(selection gitops.PartialCommitFile, hunk gitops.ChangeRange) bool {
	return slices.ContainsFunc(selection.Included, func(other gitops.ChangeRange) bool { return commitRangesIntersect(hunk, other) })
}

func (a *App) canChooseCommitHunks() bool {
	file := a.selectedChangeFile
	return file != nil && !a.diffLoading && a.diffReadError == "" && !a.diffStagedOnly &&
		!file.Staged && !file.Untracked && !file.Conflicted && file.Kind == "M"
}

func (a *App) buildCommitHunkCheckbox(c *ui.Context, bar ui.Element, acts *editor.DiffHunkActions) {
	if !a.canChooseCommitHunks() {
		return
	}
	file := a.selectedChangeFile
	key := file.Key()
	hunk := reviewChangeRange(acts.Hunk)
	partial, hasPartial := a.partialCommitFiles[key]
	checked := a.checkedChanges[key]
	if hasPartial {
		checked = commitHunkIncluded(partial, hunk)
	}
	bar.Children(func() {
		p := a.palette()
		check := ui.ButtonBase(c).Size(14, 14).Radius(3).Center().KeepFocus().Cursor(ui.CursorPointer).
			Label("纳入提交").Tooltip("纳入提交")
		if checked {
			check.Background(p.Accent)
			check.Children(func() { drawIcon(c, icoCheck, 10, p.AccentText) })
		} else {
			check.Background(p.Surface).Border(1, p.BorderStrong)
		}
		if check.Clicked() {
			a.toggleCommitHunk(*file, hunk)
		}
	})
}

func (a *App) toggleCommitHunk(file gitops.ChangedFile, hunk gitops.ChangeRange) {
	if a.workingDiffDirty() {
		a.showToast("info", "请先保存文件，再选择提交块")
		return
	}
	diff := a.diffState
	oldText, newText := reviewBufferText(diff.Old), reviewBufferText(diff.New)
	exact := &editor.DiffState{Old: editor.NewBuffer(oldText), New: editor.NewBuffer(newText)}
	hunks := exact.Hunks()
	var clicked []gitops.ChangeRange
	for _, h := range hunks {
		part := reviewChangeRange(h)
		if commitRangesIntersect(hunk, part) {
			clicked = append(clicked, part)
		}
	}
	if len(clicked) == 0 {
		return
	}
	key := file.Key()
	old := gitops.ReviewTextVersion(oldText)
	next := gitops.ReviewTextVersion(newText)
	selection, partial := a.partialCommitFiles[key]
	if !partial || selection.Old != old || selection.New != next {
		selection = gitops.PartialCommitFile{File: file, Old: old, New: next}
		if a.checkedChanges[key] {
			for _, h := range hunks {
				selection.Included = append(selection.Included, reviewChangeRange(h))
			}
		}
	}
	included := slices.Clone(selection.Included)
	if commitHunkIncluded(selection, hunk) {
		included = slices.DeleteFunc(included, func(other gitops.ChangeRange) bool { return slices.Contains(clicked, other) })
	} else {
		included = append(included, clicked...)
	}
	selection.Included = included
	switch {
	case len(included) == 0:
		a.setChangeChecked(key, false)
	case len(included) == len(hunks):
		a.setChangeChecked(key, true)
	default:
		if a.partialCommitFiles == nil {
			a.partialCommitFiles = make(map[string]gitops.PartialCommitFile)
		}
		a.partialCommitFiles[key] = selection
		a.checkedChanges[key] = true
	}
}

func (a *App) setChangeChecked(key string, checked bool) {
	a.checkedChanges[key] = checked
	delete(a.partialCommitFiles, key)
}
