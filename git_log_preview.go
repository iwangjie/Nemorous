package main

import (
	"context"
	"slices"
	"time"

	"github.com/egoist/mygo/nemorouseditor"
	"github.com/egoist/mygo/ui"
	"github.com/iwangjie/nemorous/internal/gitops"
)

type gitLogPreview struct {
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
	if current := a.gitLogPreview; current != nil && current.detail.Hash == detail.Hash &&
		current.detail.BaseRevision == detail.BaseRevision && current.detail.RepoPath == detail.RepoPath && current.file.Path == file.Path {
		return
	}
	a.clearGitLogPreview()
	ctx, cancel := context.WithTimeout(context.Background(), 8*time.Second)
	view := &gitLogPreview{detail: detail, file: file, cancel: cancel, loading: true}
	a.gitLogPreview = view
	load := func() {
		defer cancel()
		old, next, err := gitops.RevisionFileVersions(ctx, detail.RepoPath, commitDiffBase(detail), detail.Hash, file)
		apply := func() {
			if a.gitLogPreview != view {
				return
			}
			view.loading = false
			if err != nil {
				view.err = err.Error()
				return
			}
			if previous := a.gitCommitDiff; previous != nil && a.gitDiffHash == detail.Hash &&
				a.gitDiffPath == file.Path && a.gitDiffAfter == detail.Hash && a.gitDiffDetail != nil &&
				a.gitDiffDetail.RepoPath == detail.RepoPath && a.gitDiffDetail.BaseRevision == detail.BaseRevision {
				view.diff = a.newGitLogPreviewDiff(previous.Old, previous.New)
			} else {
				left, right := editor.NewBuffer(old), editor.NewBuffer(next)
				lang := editor.LanguageFor(file.Path)
				left.SetLanguage(lang)
				right.SetLanguage(lang)
				view.diff = a.newGitLogPreviewDiff(left, right)
			}
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

func (a *App) newGitLogPreviewDiff(old, next *editor.Buffer) *editor.DiffState {
	return &editor.DiffState{
		Old: old, New: next, LineSelection: true, Font: ui.Font{Size: a.cfg.EditorFontSize},
		Layout: diffLayout(a.cfg.DiffSideBySide), Inline: parseInlineMode(a.cfg.DiffInlineMode),
		Wrap: a.cfg.EditorSoftWrap, RenderFindBar: renderReviewFindBar,
		Bindings: []editor.Binding{{Mods: ui.Cmd, Key: reviewEditorGoToLineKey(), Command: editor.GoToLine}},
	}
}

func (a *App) clearGitLogPreview() {
	if a.gitLogPreview != nil {
		a.gitLogPreview.cancel()
		a.gitLogPreview = nil
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
	ui.Column(c).Fill().Clip().Background(p.Surface).Children(func() {
		view := a.gitLogPreview
		switch {
		case view == nil:
			emptyState(c, p, icoColumns, "选择变更文件查看差异", "")
		case view.loading:
			emptyState(c, p, icoColumns, "正在加载提交差异…", "")
		case view.err != "":
			emptyState(c, p, icoColumns, "无法显示差异", view.err)
		default:
			view.diff.Layout = diffLayout(a.cfg.DiffSideBySide)
			view.diff.Inline = parseInlineMode(a.cfg.DiffInlineMode)
			view.diff.Wrap = a.cfg.EditorSoftWrap
			a.buildDiffToolbar(c, p, view.diff, a.gitLogPreviewOptions())
			ui.Text(c, view.file.Path).Padding(3, 8).FontSize(10.5).TextColor(p.TextMuted).SingleLine()
			editor.DiffView(c, view.diff).Bind(&a.gitLogPreviewHandle).Grow(1).FillWidth()
		}
	})
}
