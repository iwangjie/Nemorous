package main

import (
	"path/filepath"

	"github.com/egoist/mygo/ui"
	"github.com/iwangjie/nemorous/internal/config"
)

type reviewSwitchItem struct {
	name, path string
	icon       *ui.SVG
	open       func()
}

type reviewSwitcher struct {
	files, tools []reviewSwitchItem
	index        int
	tool         bool
	held         config.KeyStroke
	mods         ui.Modifiers
}

func (a *App) observeReviewInput(ev ui.InputEvent) {
	a.observeSearchGesture(ev)
	a.observeSwitcherTab(ev)
}

func (a *App) observeSwitcherTab(ev ui.InputEvent) {
	if ev.Kind != ui.InputKeyDown && ev.Kind != ui.InputKeyUp {
		return
	}
	key := ev.Key
	if ev.Code == ui.CodeTab {
		key = ui.KeyTab
	}
	down := ev.Kind == ui.InputKeyDown
	for _, binding := range a.shortcuts(keySwitcher) {
		if binding.Key != uint16(key) || binding.Modifiers != uint8(ev.Mods) {
			continue
		}
		if s := a.reviewSwitcher; s != nil {
			// AppKit can handle a Tab down for focus traversal. Use its
			// release only when the down event did not reach us.
			if down || key == ui.KeyTab && s.held != binding {
				delta := 1
				if ev.Mods&ui.Shift != 0 && s.mods&ui.Shift == 0 {
					delta = -1
				}
				s.move(delta)
			}
			if down {
				s.held = binding
			} else {
				s.held = config.KeyStroke{}
			}
		} else if !down && key == ui.KeyTab && !a.overlayOpen() && !ev.Repeat {
			a.startReviewSwitcher(binding, false)
		}
		return
	}
	// Shift can be released before Tab. Clear the recorded down event
	// even if the modifiers on release are different.
	if s := a.reviewSwitcher; s != nil && !down && s.held.Key == uint16(key) {
		s.held = config.KeyStroke{}
	}
}

func (a *App) startReviewSwitcher(key config.KeyStroke, down bool) {
	s := a.newReviewSwitcher()
	s.mods = ui.Modifiers(key.Modifiers)
	if s.mods&^ui.Shift != 0 {
		s.mods &^= ui.Shift
	}
	if ui.Modifiers(key.Modifiers)&ui.Shift != 0 && s.mods&ui.Shift == 0 && len(s.files) > 1 {
		s.index = len(s.files) - 1
	}
	if down {
		s.held = key
	}
	a.reviewSwitcher = s
}

func (s *reviewSwitcher) move(delta int) {
	n := len(s.items())
	s.index = (s.index + delta + n) % n
}

func (a *App) newReviewSwitcher() *reviewSwitcher {
	switcher := &reviewSwitcher{}
	seen := make(map[string]bool)
	addTab := func(tab *codeTab) {
		key := tab.file.AbsPath + "\x00" + tab.revision
		if seen[key] {
			return
		}
		seen[key] = true
		switcher.files = append(switcher.files, reviewSwitchItem{codeTabTitle(tab), filepath.Dir(tab.file.RelPath), icoFile, func() {
			for i, candidate := range a.codeTabs {
				if candidate == tab {
					a.activeTab = TabCode
					a.activateCodeTab(i)
					a.activeCodeViewHandle(tab).Focus()
					return
				}
			}
		}})
	}
	if a.editorTab == "file" {
		if tab := a.activeCodeTab(); tab != nil {
			addTab(tab)
		}
	}
	for _, loc := range a.recentCode {
		key := loc.file.AbsPath + "\x00"
		if !seen[key] && a.relativeWorkspacePath(loc.file.AbsPath) != "" {
			seen[key] = true
			switcher.files = append(switcher.files, reviewSwitchItem{filepath.Base(loc.file.RelPath), filepath.Dir(loc.file.RelPath), icoFile, func() {
				a.openReviewLocation(loc)
			}})
		}
	}
	for _, tab := range a.codeTabs {
		addTab(tab)
	}
	switcher.tools = []reviewSwitchItem{
		{name: "Commit", icon: icoCommit, open: a.focusCommitToolWindow},
		{name: "Project", icon: icoFolder, open: a.focusProjectToolWindow},
		{name: "Git", icon: icoBranch, open: func() {
			a.showRunPanel, a.showTerminalPanel, a.showLogPanel = false, false, false
			a.showGitPanel = true
		}},
	}
	if a.findResults != nil {
		switcher.tools = append(switcher.tools, reviewSwitchItem{name: "查找", icon: icoSearch, open: a.showFindResults})
	}
	switcher.tool = len(switcher.files) == 0
	if a.editorTab == "file" && len(switcher.files) > 1 {
		switcher.index = 1
	}
	return switcher
}

func (a *App) finishReviewSwitcher(open bool) {
	switcher := a.reviewSwitcher
	a.reviewSwitcher = nil
	if open && switcher != nil {
		items := switcher.items()
		items[switcher.index].open()
	}
}

func (s *reviewSwitcher) items() []reviewSwitchItem {
	if s.tool {
		return s.tools
	}
	return s.files
}

func (a *App) registerSwitcher(c *ui.Context) bool {
	s := a.reviewSwitcher
	if s == nil {
		if !a.overlayOpen() {
			for _, key := range a.shortcuts(keySwitcher) {
				if c.Shortcut(ui.Modifiers(key.Modifiers), ui.Key(key.Key)) {
					a.startReviewSwitcher(key, true)
					return true
				}
			}
		}
		return false
	}
	if !c.WindowActive() || c.Shortcut(0, ui.KeyEscape) || s.mods != 0 && c.Shortcut(s.mods, ui.KeyEscape) {
		a.finishReviewSwitcher(false)
		return true
	}
	if s.mods != 0 && c.Modifiers()&s.mods != s.mods {
		a.finishReviewSwitcher(true)
	}
	return true
}

func (a *App) handleSwitcherInput(ev ui.InputEvent) bool {
	s := a.reviewSwitcher
	if s == nil || ev.Kind != ui.InputKeyDown {
		return false
	}
	if ev.Key == ui.KeyEscape && (ev.Mods == 0 || ev.Mods == s.mods) {
		a.finishReviewSwitcher(false)
		return true
	}
	if ev.Mods != s.mods {
		return false
	}
	switch ev.Key {
	case ui.KeyEnter:
		a.finishReviewSwitcher(true)
	case ui.KeyLeft, ui.KeyRight:
		if len(s.files) > 0 {
			s.tool, s.index = !s.tool, 0
		}
	case ui.KeyDown:
		s.move(1)
	case ui.KeyUp:
		s.move(-1)
	default:
		return false
	}
	return true
}

func (a *App) buildReviewSwitcher(c *ui.Context, p Palette) {
	s := a.reviewSwitcher
	if s == nil {
		return
	}
	open := true
	ui.DialogBase(c.Key("review-switcher"), &open, func(backdrop, panel ui.Element) {
		panel.Focusable().AutoFocus().FocusRing(false).ObserveInput(a.observeSwitcherTab).HandleInput(a.handleSwitcherInput).
			Width(580).Padding(12).Gap(8).Radius(8).Background(p.Surface).Border(1, p.Border)
		ui.Text(c, "Switcher").FontSize(13).FontWeight(weightSemibold).TextColor(p.TextPrimary)
		ui.Row(c).Gap(10).FillWidth().Children(func() {
			for _, group := range []struct {
				name string
				list []reviewSwitchItem
				tool bool
			}{{"工具窗口", s.tools, true}, {"文件", s.files, false}} {
				ui.Column(c.Key(group.name)).Grow(1).Gap(4).Children(func() {
					ui.Text(c, group.name).TextColor(p.TextMuted).FontSize(11)
					ui.Scroll(c).MaxHeight(330).Children(func() {
						for i, item := range group.list {
							kc := c.Key(i)
							row := ui.Row(kc).Height(26).Padding(0, 6).Gap(6).AlignItems(ui.Center).Cursor(ui.CursorPointer)
							if s.tool == group.tool && s.index == i {
								row.Background(p.ActiveRowBg).ScrollIntoView()
							} else if row.Hovered() {
								row.Background(p.SurfaceHover)
							}
							if row.Clicked() {
								s.tool, s.index = group.tool, i
								a.finishReviewSwitcher(true)
							}
							row.Children(func() {
								drawIcon(kc, item.icon, 13, p.TextMuted)
								ui.Text(kc, item.name).SingleLine().Grow(1).FontSize(11.5)
								if item.path != "." {
									ui.Text(kc, item.path).SingleLine().TextColor(p.TextMuted).FontSize(10.5)
								}
							})
						}
					})
				})
			}
		})
	})
	if !open && a.reviewSwitcher != nil {
		a.finishReviewSwitcher(false)
	}
}
