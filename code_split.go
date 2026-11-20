package main

import (
	"slices"

	"github.com/egoist/mygo/nemorouseditor"
	"github.com/egoist/mygo/ui"
)

type codeEditorSplit struct {
	left, right           *codeTab
	leftTabs, rightTabs   []*codeTab
	activeRight           bool
	leftPct               float32
	leftGroup, rightGroup ui.Handle
	sharedView            *editor.State
	sharedHandle          ui.Handle
}

func (a *App) splitCodeEditorRight(index int) {
	a.activateCodeTab(index)
	tab := a.activeCodeTab()
	if tab == nil || a.codeEditorSplit != nil {
		return
	}
	a.codeEditorSplit = &codeEditorSplit{left: tab, right: tab, leftTabs: slices.Clone(a.codeTabs), rightTabs: []*codeTab{tab}, activeRight: true, leftPct: 50}
	a.activeTab = TabCode
	a.codeEditorSplit.sharedHandle.Focus()
}

func (a *App) selectCodeTabInGroup(index int) {
	a.codeTabIdx = index
	tab := a.codeTabs[index]
	if s := a.codeEditorSplit; s != nil {
		previous := s.right
		group := &s.rightTabs
		if s.activeRight {
			s.right = tab
		} else {
			previous = s.left
			s.left = tab
			group = &s.leftTabs
		}
		if !slices.Contains(*group, tab) {
			*group = append(*group, tab)
		}
		if tab != previous {
			s.sharedView = nil
		}
	}
	a.followOpenedCodeFile(tab)
}

func (a *App) syncCodeSplitFocus(c *ui.Context) {
	s := a.codeEditorSplit
	if s == nil || a.editorTab != "file" {
		return
	}
	var tab *codeTab
	switch {
	case s.rightGroup.FocusWithin(c):
		s.activeRight, tab = true, s.right
	case s.leftGroup.FocusWithin(c):
		s.activeRight, tab = false, s.left
	default:
		return
	}
	if tab != a.activeCodeTab() {
		for i, candidate := range a.codeTabs {
			if candidate == tab {
				a.rememberCodeLocation()
				a.codeTabIdx = i
				a.followOpenedCodeFile(tab)
				break
			}
		}
	}
}

func (a *App) activeCodeViewHandle(tab *codeTab) *ui.Handle {
	if s := a.codeEditorSplit; s != nil && s.activeRight && s.left == tab && s.right == tab {
		return &s.sharedHandle
	}
	return &tab.viewHandle
}

func (a *App) unsplitCodeEditor() {
	if s := a.codeEditorSplit; s != nil {
		tab := a.activeCodeTab()
		if tab != nil && s.activeRight && s.left == s.right && s.sharedView != nil {
			tab.state.GoTo(s.sharedView.CursorPos())
			tab.state.SetScrollY(s.sharedView.ScrollY())
		}
		a.codeEditorSplit = nil
		if tab != nil {
			tab.viewHandle.Focus()
		}
	}
}

func (a *App) buildSplitCodeEditor(c *ui.Context, p Palette) {
	s := a.codeEditorSplit
	pane := func(kc *ui.Context, tab *codeTab, tabs []*codeTab, right bool, group *ui.Handle) {
		state, handle := tab.state, &tab.viewHandle
		if right == s.activeRight && tab.revision == "" {
			a.prepareLSP(tab)
			a.syncLSP(tab)
		}
		if right && s.right == s.left {
			if s.sharedView == nil {
				s.sharedView = &editor.State{Buffer: state.Buffer, ReadOnly: state.ReadOnly,
					Bindings: state.Bindings, RenderFindBar: renderReviewFindBar}
				s.sharedView.GoTo(state.CursorPos())
				s.sharedView.SetScrollY(state.ScrollY())
			}
			state, handle = s.sharedView, &s.sharedHandle
			state.Font, state.SoftWrap, state.DiffBase = tab.state.Font, tab.state.SoftWrap, tab.state.DiffBase
		}
		a.bindCodeKeymap(state)
		if state == tab.state {
			a.syncEditorAids(kc, tab, p)
		} else {
			state.ContextMenu = func(m *ui.Menu) { a.gitFileContextMenu(m, tab.file) }
		}
		ui.Column(kc).Bind(group).Fill().Clip().Background(p.Surface).Children(func() {
			ui.ScrollHorizontal(kc).Height(32).FillWidth().Background(p.RailBg).Children(func() {
				for _, item := range tabs {
					tc := kc.Key(item.file.AbsPath + "\x00" + item.revision)
					row := ui.Row(tc).Height(28).Padding(0, 7).Gap(5).AlignItems(ui.Center).Cursor(ui.CursorPointer).Radius(4).Focusable()
					if item == tab {
						row.Background(p.Surface)
					} else if row.Hovered() {
						row.Background(p.SurfaceHover)
					}
					if row.Clicked() {
						row.Focus()
						s.activeRight = right
						if i := slices.Index(a.codeTabs, item); i >= 0 {
							a.activateCodeTab(i)
							a.activeCodeViewHandle(item).Focus()
						}
					}
					row.PopoverMenu(func(m *ui.Menu) {
						if m.Item("取消所有拆分").Chosen() {
							a.unsplitCodeEditor()
						}
					})
					row.Children(func() {
						drawIcon(tc, icoFile, 12, p.TextMuted)
						ui.Text(tc, codeTabTitle(item)).MaxWidth(180).SingleLine().FontSize(11.5)
						if item.dirty() {
							ui.Box(tc).Size(6, 6).Radius(3).Background(p.Amber)
						}
					})
				}
			})
			editor.View(kc, state).Bind(handle).Grow(1).FillWidth()
		})
	}
	ui.SplitPercent(c.Key("code-editor-split"), &s.leftPct,
		func() { pane(c.Key("code-editor-left"), s.left, s.leftTabs, false, &s.leftGroup) },
		func() { pane(c.Key("code-editor-right"), s.right, s.rightTabs, true, &s.rightGroup) },
	).Grow(1).Fill()
}
