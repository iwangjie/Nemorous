package main

import (
	"context"
	"slices"
	"time"

	"github.com/egoist/mygo/nemorouseditor"
	"github.com/egoist/mygo/ui"
	"github.com/iwangjie/nemorous/internal/gitops"
)

type revisionPreview struct {
	detail   gitops.CommitDetail
	file     gitops.CommitFile
	cancel   context.CancelFunc
	diff     *editor.DiffState
	loading  bool
	err      string
	edge     int
	boundary diffNavigationBoundary
}

func (a *App) loadGitLogPreview(detail gitops.CommitDetail, file gitops.CommitFile) {
	a.loadRevisionPreview(&a.gitLogPreview, detail, file)
}

func (a *App) loadRevisionPreview(slot **revisionPreview, detail gitops.CommitDetail, file gitops.CommitFile) {
	base := commitDiffBase(detail)
	if current := *slot; current != nil && current.detail.Hash == detail.Hash &&
		commitDiffBase(current.detail) == base && current.detail.RepoPath == detail.RepoPath && current.file == file {
		return
	}
	a.clearRevisionPreview(slot)
	ctx, cancel := context.WithTimeout(context.Background(), 8*time.Second)
	view := &revisionPreview{detail: detail, file: file, cancel: cancel, loading: true}
	*slot = view
	if previous := a.gitCommitDiff; previous != nil && a.gitDiffAfter == detail.Hash &&
		a.gitDiffBefore == base && a.gitDiffPath == file.Path && a.gitDiffOldPath == file.OldPath &&
		a.gitDiffDetail != nil && a.gitDiffDetail.RepoPath == detail.RepoPath {
		cancel()
		view.loading = false
		view.diff = a.newRevisionPreviewDiff(previous.Old, previous.New)
		return
	}
	load := func() {
		defer cancel()
		old, next, err := gitops.RevisionFileVersions(ctx, detail.RepoPath, base, detail.Hash, file)
		apply := func() {
			if *slot != view {
				return
			}
			view.loading = false
			if err != nil {
				view.err = err.Error()
				return
			}
			left, right := editor.NewBuffer(old), editor.NewBuffer(next)
			lang := editor.LanguageFor(file.Path)
			left.SetLanguage(lang)
			right.SetLanguage(lang)
			view.diff = a.newRevisionPreviewDiff(left, right)
		}
		if a.win == nil {
			apply()
		} else {
			a.runUI(apply)
		}
	}
	if a.win == nil {
		load()
	} else {
		go load()
	}
}

func (a *App) newRevisionPreviewDiff(old, next *editor.Buffer) *editor.DiffState {
	return &editor.DiffState{
		Old: old, New: next, LineSelection: true, Font: ui.Font{Size: a.cfg.EditorFontSize},
		Layout: diffLayout(a.cfg.DiffSideBySide), Inline: parseInlineMode(a.cfg.DiffInlineMode),
		Wrap: a.cfg.EditorSoftWrap, RenderFindBar: renderReviewFindBar,
		Bindings: []editor.Binding{{Mods: ui.Cmd, Key: reviewEditorGoToLineKey(), Command: editor.GoToLine}},
	}
}

func (a *App) clearGitLogPreview() {
	a.clearRevisionPreview(&a.gitLogPreview)
}

func (a *App) clearRevisionPreview(slot **revisionPreview) {
	if view := *slot; view != nil {
		view.cancel()
		if view.diff != nil && a.diffIgnoreOwner == view.diff {
			a.diffIgnoreOwner = nil
			a.showDiffIgnoreOptions = false
			a.diffIgnoreFocus = false
		}
		*slot = nil
	}
}

func (a *App) gitLogPreviewOptions() diffToolbarOptions {
	view := a.gitLogPreview
	files := view.detail.Files
	index := slices.IndexFunc(files, func(file gitops.CommitFile) bool { return file.Path == view.file.Path })
	return diffToolbarOptions{
		prefix: "git-log-preview", boundary: &view.boundary, jumpEdge: &view.edge, loading: view.loading,
		canPrevFile: index > 0, canNextFile: index >= 0 && index+1 < len(files),
		moveFile: func(delta int) bool {
			if i := index + delta; index >= 0 && i >= 0 && i < len(files) {
				a.selectGitChangedFile(view.detail, files[i])
				return true
			}
			return false
		},
		source: func() {
			a.openDiffSourceAtCaret(committedFile(view.detail, view.file.Path), view.diff)
		},
		sourceEnabled: view.diff != nil && view.file.Status != "D",
	}
}

func (a *App) buildGitLogPreview(c *ui.Context, p Palette) {
	if a.gitLogPreview == nil {
		ui.Column(c).Fill().Clip().Background(p.Surface).Children(func() {
			emptyState(c, p, icoColumns, "选择变更文件查看差异", "")
		})
		return
	}
	a.buildRevisionPreview(c, p, a.gitLogPreview, &a.gitLogPreviewHandle, a.gitLogPreviewOptions(), a.gitLogPreview.file.Path)
}

func (a *App) buildRevisionPreview(c *ui.Context, p Palette, view *revisionPreview, handle *ui.Handle, opts diffToolbarOptions, title string) {
	ui.Column(c).Fill().Clip().Background(p.Surface).Children(func() {
		switch {
		case view.loading:
			emptyState(c, p, icoColumns, "正在加载提交差异…", "")
		case view.err != "":
			emptyState(c, p, icoColumns, "无法显示差异", view.err)
		default:
			view.diff.Layout = diffLayout(a.cfg.DiffSideBySide)
			view.diff.Inline = parseInlineMode(a.cfg.DiffInlineMode)
			view.diff.Wrap = a.cfg.EditorSoftWrap
			a.buildDiffToolbar(c, p, view.diff, opts)
			ui.Text(c, title).FillWidth().Padding(3, 8).FontSize(10.5).TextColor(p.TextMuted).SingleLine().Tooltip(title)
			a.bindDiffKeymap(view.diff)
			editor.DiffView(c, view.diff).Bind(handle).Grow(1).FillWidth()
		}
	})
}
