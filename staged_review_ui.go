package main

import (
	"context"
	"path/filepath"

	"github.com/egoist/mygo/nemorouseditor"
	"github.com/egoist/mygo/ui"
	"github.com/iwangjie/nemorous/internal/gitops"
)

type stagedReviewView struct {
	file        gitops.ChangedFile
	entry       string
	state       *editor.ThreeWayState
	loading     bool
	saving      bool
	closeOnSave bool
	err         string
}

func (a *App) openStagedReview(file gitops.ChangedFile) {
	if active := a.stagedReview; active != nil {
		if active.file.Key() == file.Key() || active.dirty() || active.saving {
			a.editorTab, a.gitDiffOpen = "staged", false
			return
		}
	}
	view := &stagedReviewView{file: file, loading: true}
	a.stagedReview = view
	a.editorTab, a.gitDiffOpen = "staged", false
	load := func() {
		versions, err := gitops.ReadStagedReview(context.Background(), file)
		a.runUI(func() {
			if a.stagedReview != view {
				return
			}
			view.loading = false
			if err != nil {
				view.err = err.Error()
				return
			}
			view.entry = versions.Entry
			view.state = &editor.ThreeWayState{}
			view.state.RenderAccept = func(c *ui.Context, control ui.Element, fromLeft bool, apply func()) {
				if view.entry == "" || view.saving {
					return
				}
				control.Children(func() {
					label, icon := "暂存此变更块", icoArrowLeft
					if fromLeft {
						label, icon = "取消暂存此变更块", icoChevronRight
					}
					button := ui.ButtonBase(c).Size(17, 17).Center().KeepFocus().Label(label).Tooltip(label)
					button.Children(func() { drawIcon(c, icon, 11, a.palette().TextControl) })
					if button.Clicked() {
						apply()
						a.saveStagedReview(view)
					}
				})
			}
			for i, content := range []string{versions.Head, versions.Index, versions.Worktree} {
				buffer := editor.NewBuffer(content)
				buffer.SetLanguage(editor.LanguageFor(file.Path))
				view.state.Versions[i] = &editor.State{
					Buffer: buffer, ReadOnly: i != 1 || versions.Entry == "",
					Font: ui.Font{Size: a.cfg.EditorFontSize}, NoScrollPastEnd: true,
					RenderFindBar: renderReviewFindBar,
					Bindings:      []editor.Binding{{Mods: ui.Cmd, Key: reviewEditorGoToLineKey(), Command: editor.GoToLine}},
				}
			}
		})
	}
	if a.win == nil {
		load()
	} else {
		go load()
	}
}

func (v *stagedReviewView) title() string { return filepath.Base(v.file.Path) }

func (v *stagedReviewView) dirty() bool {
	return v.state != nil && v.state.Versions[1].Buffer.Modified()
}

func (a *App) closeStagedReview(view *stagedReviewView) {
	if view.dirty() || view.saving {
		view.closeOnSave = true
		a.saveStagedReview(view)
	} else {
		a.removeStagedReview()
	}
}

func (a *App) removeStagedReview() {
	a.stagedReview = nil
	if a.editorTab == "staged" {
		a.focusDefaultEditor()
	}
}

func (a *App) saveStagedReview(view *stagedReviewView) {
	if view.saving || !view.dirty() || view.entry == "" {
		return
	}
	buffer := view.state.Versions[1].Buffer
	entry, content, version := view.entry, reviewBufferText(buffer), buffer.Version()
	view.saving = true
	view.err = ""
	go func() {
		next, err := gitops.SaveStagedReview(context.Background(), view.file.RepoPath, view.file.Path, content, entry)
		a.runUI(func() {
			if a.stagedReview != view {
				return
			}
			view.saving = false
			if err != nil {
				view.closeOnSave = false
				view.err = err.Error()
				return
			}
			view.entry = next
			if buffer.Version() == version {
				buffer.MarkSaved()
			}
			if view.closeOnSave {
				a.closeStagedReview(view)
			}
			a.RefreshAsync()
		})
	}()
}

func (a *App) buildStagedReview(c *ui.Context, p Palette) {
	view := a.stagedReview
	ui.Column(c).Fill().Clip().Background(p.Surface).Children(func() {
		ui.Row(c).Height(36).FillWidth().Gap(8).Padding(0, 10).Background(p.RailBg).
			BorderWidth(0, 0, 1, 0).BorderColor(p.Border).AlignItems(ui.Center).Children(func() {
			ui.Text(c, view.file.Path).FontSize(12).FontWeight(weightSemibold).TextColor(p.TextPrimary).SingleLine().Grow(1)
			if view.dirty() {
				miniBtn(c.Key("save-index"), p, "保存暂存版本 (⌘S)", func() { a.saveStagedReview(view) })
			}
		})
		if view.err != "" {
			ui.Text(c, view.err).FontSize(11).TextColor(p.Danger)
		}
		if view.loading {
			ui.Spinner(c).Size(14, 14).Label("读取文件版本")
		}
		if view.state != nil {
			editor.ThreeWayView(c.Key("staged-three-way"), view.state, [3]string{"HEAD", "暂存版本", "本地版本"}).Grow(1).FillWidth()
		}
	})
}
