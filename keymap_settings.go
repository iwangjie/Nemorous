package main

import (
	"maps"
	"slices"
	"strings"

	"github.com/egoist/mygo/nemorouseditor"
	"github.com/egoist/mygo/ui"
	"github.com/iwangjie/nemorous/internal/config"
)

type keymapSettings struct {
	original, draft map[string][]config.KeyStroke
	query           string
	selected        shortcutAction
	capture         *keymapCapture
	system          map[config.KeyStroke]string
	err             string
}

type keymapCapture struct {
	id  shortcutAction
	key config.KeyStroke
	tap modifierTap
}

func (a *App) openKeymapSettings() {
	original := maps.Clone(a.cfg.KeyboardShortcuts)
	state := &keymapSettings{original: original, draft: make(map[string][]config.KeyStroke), selected: keySwitcher}
	maps.Copy(state.draft, original)
	a.keymap = state
	if a.win != nil && ui.Cmd == ui.Super {
		go func() {
			system := loadSystemShortcuts()
			a.runUI(func() {
				if a.keymap == state {
					state.system = system
				}
			})
		}()
	}
}

func (s *keymapSettings) shortcuts(id shortcutAction) []config.KeyStroke {
	if keys, ok := s.draft[string(id)]; ok {
		return keys
	}
	return shortcutByID[id].defaults
}

func (s *keymapSettings) set(id shortcutAction, keys []config.KeyStroke) {
	setShortcutOverride(s.draft, id, keys)
	s.err = ""
}

func (s *keymapSettings) conflicts(id shortcutAction, key config.KeyStroke) []shortcutDefinition {
	var conflicts []shortcutDefinition
	for _, item := range shortcutDefinitions {
		if item.id != id && slices.Contains(s.shortcuts(item.id), key) {
			conflicts = append(conflicts, item)
		}
	}
	return conflicts
}

func (s *keymapSettings) add(id shortcutAction, key config.KeyStroke, conflicts []shortcutDefinition) {
	for _, item := range conflicts {
		keys := slices.DeleteFunc(slices.Clone(s.shortcuts(item.id)), func(existing config.KeyStroke) bool { return existing == key })
		s.set(item.id, keys)
	}
	keys := s.shortcuts(id)
	if !slices.Contains(keys, key) {
		s.set(id, append(slices.Clone(keys), key))
	}
	s.capture = nil
}

func (capture *keymapCapture) input(ev ui.InputEvent) bool {
	mod := modifierKey(ev.Code)
	if capture.id == keySearchEverywhere && capture.tap.input(ev, mod) {
		capture.key = stroke(mod, ui.KeyUnknown)
	}
	switch ev.Kind {
	case ui.InputKeyDown:
		if ev.Key != ui.KeyUnknown && !ev.Repeat {
			capture.key = stroke(ev.Mods, ev.Key)
		}
		return true
	case ui.InputCommand, ui.InputText:
		return true
	}
	return false
}

func (capture *keymapCapture) validation() string {
	key := ui.Key(capture.key.Key)
	switch {
	case capture.key == (config.KeyStroke{}):
		return "按下要设置的快捷键"
	case key >= ui.KeyA && key <= ui.KeyZ || key >= ui.Key0 && key <= ui.Key9:
		if ui.Modifiers(capture.key.Modifiers)&^ui.Shift == 0 {
			return "字母和数字需要搭配 Ctrl、Option 或 Command"
		}
	case key != 0 && keyStrokeLabel(stroke(0, key)) == "":
		return "无法识别这个按键"
	}
	return ""
}

func (s *keymapSettings) systemConflict(id shortcutAction, key config.KeyStroke) string {
	if key.Key != 0 && s.system[key] != "" {
		return s.system[key]
	}
	if id != keySwitcher {
		return ""
	}
	mods := ui.Modifiers(key.Modifiers)
	if mods&^ui.Shift != 0 {
		mods &^= ui.Shift
	}
	for _, arrow := range []ui.Key{ui.KeyLeft, ui.KeyRight, ui.KeyUp, ui.KeyDown} {
		if label := s.system[stroke(mods, arrow)]; label != "" {
			return "切换器中的方向键与 " + label + " 冲突"
		}
	}
	return ""
}

func (a *App) buildKeymapSettings(c *ui.Context, p Palette) {
	s := a.keymap
	if s == nil {
		return
	}
	open := true
	ui.DialogBase(c.Key("keymap-settings"), &open, func(backdrop, panel ui.Element) {
		backdrop.Background(ui.RGBA(20, 22, 30, 0.36))
		width, height := float32(610), float32(530)
		if a.win != nil {
			w, h := a.win.ContentSize()
			width = min(width, max(280, float32(w)-24))
			height = min(height, max(280, float32(h)-24))
		}
		panel.Width(width).Height(height).Padding(14).Gap(10).Radius(8).Background(p.Surface).Border(1, p.Border)
		modalHeader(c, p, icoSettings, "键盘映射", "", nil, func() { a.keymap = nil })
		if s.capture != nil {
			a.buildShortcutCapture(c, p, s, s.capture)
			return
		}
		ui.SearchField(c.Key("keymap-search"), &s.query).Placeholder("搜索动作或快捷键").AutoFocus().FillWidth()
		query := strings.ToLower(strings.TrimSpace(s.query))
		ui.Scroll(c.Key("keymap-actions")).Grow(1).FillWidth().Children(func() {
			group := ""
			for _, item := range shortcutDefinitions {
				keys := s.shortcuts(item.id)
				var labels []string
				warning := ""
				for _, key := range keys {
					labels = append(labels, keyStrokeLabel(key))
					if conflict := s.systemConflict(item.id, key); conflict != "" {
						warning = conflict
					}
				}
				hint := strings.Join(labels, "、")
				if !strings.Contains(strings.ToLower(item.group+" "+item.label+" "+string(item.id)+" "+hint), query) {
					continue
				}
				if group != item.group {
					group = item.group
					ui.Text(c.Key(group), group).FillWidth().Padding(6, 5).FontSize(11).TextColor(p.TextMuted)
				}
				kc := c.Key(string(item.id))
				row := ui.Row(kc).Height(27).Padding(0, 5).Gap(8).AlignItems(ui.Center).Focusable()
				if row.Clicked() || row.RightClicked() || row.Focused() {
					s.selected = item.id
				}
				if s.selected == item.id {
					row.Background(p.ActiveRowBg)
				}
				if row.DoubleClicked() || row.Shortcut(ui.Alt, ui.KeyEnter) {
					s.capture = &keymapCapture{id: item.id}
				}
				row.ContextMenu(func(m *ui.Menu) {
					if m.Item("添加键盘快捷键").Chosen() {
						s.capture = &keymapCapture{id: item.id}
					}
					for _, key := range keys {
						if m.Item("移除 " + keyStrokeLabel(key)).Chosen() {
							s.set(item.id, slices.DeleteFunc(slices.Clone(keys), func(old config.KeyStroke) bool { return old == key }))
						}
					}
					if _, changed := s.draft[string(item.id)]; changed && m.Item("恢复默认快捷键").Chosen() {
						s.set(item.id, item.defaults)
					}
				})
				row.Children(func() {
					ui.Text(kc, item.label).SingleLine().Grow(1).FontSize(12)
					if warning != "" {
						w := ui.Box(kc).Size(17, 17).Center().Tooltip(warning).Label(warning)
						w.Children(func() { drawIcon(kc, icoAlert, 13, p.Accent) })
					}
					ui.Text(kc, hint).SingleLine().FontSize(11.5).TextColor(p.TextMuted)
				})
			}
		})
		if s.err != "" {
			ui.Text(c, s.err).FontSize(11).TextColor(p.Danger)
		}
		ui.Row(c).Gap(8).AlignItems(ui.Center).Children(func() {
			miniBtn(c.Key("keymap-add"), p, "添加键盘快捷键", func() { s.capture = &keymapCapture{id: s.selected} })
			miniBtn(c.Key("keymap-reset"), p, "恢复默认快捷键", func() {
				s.set(s.selected, shortcutByID[s.selected].defaults)
			})
			ui.Spacer(c)
			miniBtn(c.Key("keymap-cancel"), p, "取消", func() { a.keymap = nil })
			pill(c.Key("keymap-apply"), p, "确定", true, func() {
				if !maps.EqualFunc(s.original, s.draft, slices.Equal[[]config.KeyStroke]) {
					if err := a.applyKeymap(s.draft); err != nil {
						s.err = "保存键盘映射失败：" + err.Error()
						return
					}
				}
				a.keymap = nil
			})
		})
	})
	if !open {
		a.keymap = nil
	}
}

func (a *App) buildShortcutCapture(c *ui.Context, p Palette, s *keymapSettings, capture *keymapCapture) {
	ui.Text(c, shortcutByID[capture.id].label).FontSize(13).FontWeight(weightSemibold)
	ui.Row(c.Key("keymap-capture")).Height(38).Padding(0, 8).AlignItems(ui.Center).Border(1, p.Accent).
		Label("快捷键录入").Focusable().AutoFocus().HandleInput(capture.input).Children(func() {
		label := keyStrokeLabel(capture.key)
		if label == "" {
			label = "按下要设置的快捷键"
		}
		ui.Text(c, label).FontSize(13)
	})
	validation := capture.validation()
	var conflicts []shortcutDefinition
	if validation == "" {
		conflicts = s.conflicts(capture.id, capture.key)
		for _, conflict := range conflicts {
			ui.Text(c.Key(string(conflict.id)), "已分配给「"+conflict.label+"」").FontSize(12).TextColor(p.Accent)
		}
		if warning := s.systemConflict(capture.id, capture.key); warning != "" {
			ui.Text(c, warning).FontSize(12).TextColor(p.Accent)
		}
	} else if capture.key != (config.KeyStroke{}) {
		ui.Text(c, validation).FontSize(12).TextColor(p.Danger)
	}
	ui.Spacer(c)
	ui.Row(c).Justify(ui.End).Gap(8).Children(func() {
		miniBtn(c.Key("capture-cancel"), p, "取消", func() { s.capture = nil })
		label := "确定"
		if len(conflicts) > 0 {
			label = "移除冲突并添加"
		}
		if validation == "" {
			pill(c.Key("capture-confirm"), p, label, true, func() { s.add(capture.id, capture.key, conflicts) })
		}
	})
}

func (a *App) bindCodeKeymap(state *editor.State) {
	state.Bindings = a.configuredEditorBindings()
}

func (a *App) bindDiffKeymap(state *editor.DiffState) {
	_ = a.configuredEditorBindings()
	state.Bindings = a.keymapDiffBindings
}
