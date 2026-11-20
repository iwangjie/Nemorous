package main

import (
	"fmt"
	"maps"
	"runtime"
	"slices"
	"strings"

	"github.com/egoist/mygo"
	"github.com/egoist/mygo/nemorouseditor"
	"github.com/egoist/mygo/ui"
	"github.com/iwangjie/nemorous/internal/config"
)

type shortcutAction string

const (
	keySwitcher         shortcutAction = "switcher"
	keySearchEverywhere shortcutAction = "searchEverywhere"
	keyGoToFile         shortcutAction = "goToFile"
	keyRecentFiles      shortcutAction = "recentFiles"
	keyRecentLocations  shortcutAction = "recentLocations"
	keyFindInFiles      shortcutAction = "findInFiles"
	keyReplaceInFiles   shortcutAction = "replaceInFiles"
	keyFind             shortcutAction = "find"
	keyReplace          shortcutAction = "replace"
	keyFindNext         shortcutAction = "findNext"
	keyFindPrevious     shortcutAction = "findPrevious"
	keyGoToLine         shortcutAction = "goToLine"
	keyUndo             shortcutAction = "undo"
	keyRedo             shortcutAction = "redo"
	keyToggleComment    shortcutAction = "toggleComment"
	keyProject          shortcutAction = "project"
	keyCommitWindow     shortcutAction = "commitWindow"
	keyGitWindow        shortcutAction = "gitWindow"
	keyManiWindow       shortcutAction = "maniWindow"
	keyOpenSettings     shortcutAction = "settings"
	keySave             shortcutAction = "save"
	keyCloseTab         shortcutAction = "closeTab"
	keyNextTab          shortcutAction = "nextTab"
	keyPreviousTab      shortcutAction = "previousTab"
	keyJumpToSource     shortcutAction = "jumpToSource"
	keyNextDifference   shortcutAction = "nextDifference"
	keyPrevDifference   shortcutAction = "previousDifference"
	keyNextDiffFile     shortcutAction = "nextDiffFile"
	keyPrevDiffFile     shortcutAction = "previousDiffFile"
	keyDiffIgnore       shortcutAction = "diffIgnore"
	keySelectIn         shortcutAction = "selectIn"
	keyCopyAbsolute     shortcutAction = "copyAbsolute"
	keyCopyReference    shortcutAction = "copyReference"
	keyBack             shortcutAction = "back"
	keyForward          shortcutAction = "forward"
	keyStructure        shortcutAction = "structure"
	keyTodos            shortcutAction = "todos"
	keyActions          shortcutAction = "actions"
	keyBookmark         shortcutAction = "bookmark"
	keyBranches         shortcutAction = "branches"
	keyRefresh          shortcutAction = "refresh"
	keyGitCommit        shortcutAction = "gitCommit"
	keyPush             shortcutAction = "push"
	keyPull             shortcutAction = "pull"
	keyRollback         shortcutAction = "rollback"
	keyFindUsages       shortcutAction = "findUsages"
	keyContextActions   shortcutAction = "contextActions"
	keyFormat           shortcutAction = "format"
	keyZoomIn           shortcutAction = "zoomIn"
	keyZoomOut          shortcutAction = "zoomOut"
	keyZoomReset        shortcutAction = "zoomReset"
)

type shortcutDefinition struct {
	id       shortcutAction
	label    string
	group    string
	defaults []config.KeyStroke
	command  editor.Command
}

func stroke(mods ui.Modifiers, key ui.Key) config.KeyStroke {
	return config.KeyStroke{Modifiers: uint8(mods), Key: uint16(key)}
}

func newShortcutDefinitions() []shortcutDefinition {
	cmd, alt, ctrl, shift := ui.Cmd, ui.Alt, ui.Ctrl, ui.Shift
	const search = "查找"
	const editorGroup = "编辑器"
	const navigation = "导航"
	const git = "Git"
	const tools = "工具窗口"
	const diff = "差异"
	const main = "主菜单"
	definition := func(id shortcutAction, label, group string, keys ...config.KeyStroke) shortcutDefinition {
		return shortcutDefinition{id: id, label: label, group: group, defaults: keys}
	}
	editorAction := func(id shortcutAction, label, group string, command editor.Command, keys ...config.KeyStroke) shortcutDefinition {
		if len(keys) == 0 {
			for _, key := range editor.DefaultBindings() {
				if key.Command == command && key.ThenKey == 0 {
					keys = append(keys, stroke(key.Mods, key.Key))
				}
			}
		}
		return shortcutDefinition{id: id, label: label, group: group, defaults: keys, command: command}
	}

	return []shortcutDefinition{
		definition(keySwitcher, "切换器", navigation, stroke(ctrl, ui.KeyTab), stroke(ctrl|shift, ui.KeyTab)),
		definition(keyGoToFile, "转到文件", navigation, stroke(cmd|shift, ui.KeyO)),
		definition(keyRecentFiles, "最近文件", navigation, stroke(cmd, ui.KeyE)),
		definition(keyRecentLocations, "最近位置", navigation, stroke(cmd|shift, ui.KeyE)),
		definition(keySelectIn, "选择目标", navigation, stroke(alt, ui.KeyF1)),
		definition(keyBack, "后退", navigation, stroke(cmd, ui.KeyBracketLeft), stroke(cmd|alt, ui.KeyLeft)),
		definition(keyForward, "前进", navigation, stroke(cmd, ui.KeyBracketRight), stroke(cmd|alt, ui.KeyRight)),
		definition(keyStructure, "文件结构", navigation, stroke(cmd, ui.KeyF12)),
		definition(keyBookmark, "切换书签", navigation, stroke(cmd|shift, ui.KeyB)),
		definition(keyPreviousTab, "上一个标签页", navigation, stroke(cmd|shift, ui.KeyBracketLeft)),
		definition(keyNextTab, "下一个标签页", navigation, stroke(cmd|shift, ui.KeyBracketRight)),
		editorAction(keyGoToLine, "转到行/列", navigation, editor.GoToLine, stroke(cmd, reviewEditorGoToLineKey())),

		definition(keySearchEverywhere, "全局搜索", search, stroke(shift, ui.KeyUnknown)),
		definition(keyActions, "查找操作", search, stroke(cmd|shift, ui.KeyP), stroke(cmd|shift, ui.KeyA)),
		definition(keyFindInFiles, "在文件中查找", search, stroke(cmd|shift, ui.KeyF)),
		definition(keyReplaceInFiles, "在文件中替换", search, stroke(cmd|shift, ui.KeyR)),
		editorAction(keyFind, "查找", search, editor.Find),
		editorAction(keyReplace, "替换", search, editor.FindReplace, replaceShortcuts()...),
		editorAction(keyFindNext, "下一个匹配", search, editor.FindNext),
		editorAction(keyFindPrevious, "上一个匹配", search, editor.FindPrevious),
		definition(keyFindUsages, "查找用法", search, stroke(alt, ui.KeyF7)),

		definition(keyProject, "项目 (Project)", tools, stroke(cmd, ui.Key1)),
		definition(keyCommitWindow, "提交 (Commit)", tools, stroke(cmd, ui.Key0)),
		definition(keyGitWindow, "Git", tools, stroke(cmd, ui.Key9)),
		definition(keyManiWindow, "Mani", tools, stroke(cmd, ui.Key4)),
		definition(keyTodos, "TODO", tools, stroke(cmd|shift, ui.KeyT)),

		definition(keyJumpToSource, "跳转到源码", diff, stroke(0, ui.KeyF4), stroke(cmd, ui.KeyDown)),
		definition(keyPrevDifference, "上一处差异", diff, stroke(shift, ui.KeyF7)),
		definition(keyNextDifference, "下一处差异", diff, stroke(0, ui.KeyF7)),
		definition(keyPrevDiffFile, "上一个差异文件", diff, stroke(alt, ui.KeyLeft)),
		definition(keyNextDiffFile, "下一个差异文件", diff, stroke(alt, ui.KeyRight)),
		definition(keyDiffIgnore, "差异忽略选项", diff, stroke(cmd|shift, ui.KeyD)),

		definition(keySave, "保存", editorGroup, stroke(cmd, ui.KeyS)),
		editorAction(keyUndo, "撤销", editorGroup, editor.Undo),
		editorAction(keyRedo, "重做", editorGroup, editor.Redo),
		editorAction(keyToggleComment, "切换行注释", editorGroup, editor.ToggleComment),
		definition(keyContextActions, "显示上下文操作", editorGroup, stroke(alt, ui.KeyEnter)),
		definition(keyFormat, "重新格式化代码", editorGroup, stroke(cmd|alt, ui.KeyL)),
		definition(keyCopyAbsolute, "复制绝对路径", editorGroup, stroke(cmd|shift, ui.KeyC)),
		definition(keyCopyReference, "复制引用", editorGroup, stroke(cmd|alt|shift, ui.KeyC)),
		definition(keyCloseTab, "关闭编辑器标签页", editorGroup, stroke(cmd, ui.KeyW)),

		definition(keyBranches, "Git 分支", git, stroke(cmd, ui.KeyB)),
		definition(keyRefresh, "刷新", git, stroke(cmd|alt, ui.KeyY)),
		definition(keyGitCommit, "打开提交", git, stroke(cmd, ui.KeyK)),
		definition(keyPush, "推送", git, stroke(cmd|shift, ui.KeyK)),
		definition(keyPull, "拉取", git, stroke(cmd, ui.KeyT)),
		definition(keyRollback, "回滚改动", git, stroke(cmd|alt, ui.KeyZ)),

		definition(keyOpenSettings, "设置", main, stroke(cmd, ui.KeyComma)),
		definition(keyZoomIn, "放大界面", main, stroke(cmd, ui.KeyEqual), stroke(cmd|shift, ui.KeyEqual)),
		definition(keyZoomOut, "缩小界面", main, stroke(cmd, ui.KeyMinus)),
		definition(keyZoomReset, "重置界面缩放", main, stroke(cmd|shift, ui.Key0)),
	}
}

func replaceShortcuts() []config.KeyStroke {
	keys := []config.KeyStroke{stroke(ui.Cmd, ui.KeyR)}
	for _, binding := range editor.DefaultBindings() {
		if binding.Command == editor.FindReplace && binding.ThenKey == 0 {
			keys = append(keys, stroke(binding.Mods, binding.Key))
		}
	}
	return keys
}

var shortcutDefinitions = newShortcutDefinitions()

var shortcutByID = func() map[shortcutAction]shortcutDefinition {
	m := make(map[shortcutAction]shortcutDefinition, len(shortcutDefinitions))
	for _, definition := range shortcutDefinitions {
		m[definition.id] = definition
	}
	return m
}()

func (a *App) shortcuts(id shortcutAction) []config.KeyStroke {
	if keys, ok := a.cfg.KeyboardShortcuts[string(id)]; ok {
		return keys
	}
	return shortcutByID[id].defaults
}

func (a *App) shortcut(c *ui.Context, id shortcutAction) bool {
	for _, key := range a.shortcuts(id) {
		if key.Key != 0 && c.Shortcut(ui.Modifiers(key.Modifiers), ui.Key(key.Key)) {
			return true
		}
	}
	return false
}

func (a *App) shortcutOn(element ui.Element, id shortcutAction) bool {
	for _, key := range a.shortcuts(id) {
		if key.Key != 0 && element.Shortcut(ui.Modifiers(key.Modifiers), ui.Key(key.Key)) {
			return true
		}
	}
	return false
}

func (a *App) shortcutLabel(label string, id shortcutAction) string {
	if hint := a.shortcutHint(id); hint != "" {
		return label + " (" + hint + ")"
	}
	return label
}

func (a *App) shortcutHint(id shortcutAction) string {
	if hint, ok := a.keymapHints[id]; ok {
		return hint
	}
	if a.keymapHints == nil {
		a.keymapHints = make(map[shortcutAction]string)
	}
	hint := ""
	if keys := a.shortcuts(id); len(keys) > 0 {
		hint = keyStrokeLabel(keys[0])
		if keys[0] == stroke(ui.Shift, ui.KeyUnknown) {
			hint = "双击 Shift"
		}
	}
	a.keymapHints[id] = hint
	return hint
}

func keyStrokeLabel(key config.KeyStroke) string {
	var parts []string
	mods := ui.Modifiers(key.Modifiers)
	mac := ui.Cmd == ui.Super
	for _, mod := range []struct {
		value ui.Modifiers
		mac   string
		other string
	}{{ui.Ctrl, "⌃", "Ctrl"}, {ui.Alt, "⌥", "Alt"}, {ui.Shift, "⇧", "Shift"}, {ui.Super, "⌘", "Super"}} {
		if mods&mod.value != 0 {
			if mac {
				parts = append(parts, mod.mac)
			} else {
				parts = append(parts, mod.other)
			}
		}
	}
	if key.Key == 0 {
		if len(parts) == 1 {
			return parts[0] + " " + parts[0]
		}
		return ""
	}
	k := ui.Key(key.Key)
	var label string
	switch {
	case k >= ui.KeyA && k <= ui.KeyZ:
		label = string(rune('A' + k - ui.KeyA))
	case k >= ui.Key0 && k <= ui.Key9:
		label = string(rune('0' + k - ui.Key0))
	case k >= ui.KeyF1 && k <= ui.KeyF12:
		label = fmt.Sprintf("F%d", 1+k-ui.KeyF1)
	default:
		label = shortcutKeyNames[k]
	}
	parts = append(parts, label)
	if mac {
		return strings.Join(parts, "")
	}
	return strings.Join(parts, "+")
}

var shortcutKeyNames = map[ui.Key]string{
	ui.KeyEnter: "Enter", ui.KeyEscape: "Esc", ui.KeyBackspace: "Backspace", ui.KeyTab: "Tab", ui.KeySpace: "Space",
	ui.KeyDelete: "Delete", ui.KeyInsert: "Insert", ui.KeyHome: "Home", ui.KeyEnd: "End", ui.KeyPageUp: "Page Up",
	ui.KeyPageDown: "Page Down", ui.KeyLeft: "←", ui.KeyRight: "→", ui.KeyUp: "↑", ui.KeyDown: "↓",
	ui.KeyMinus: "-", ui.KeyEqual: "=", ui.KeyComma: ",", ui.KeyPeriod: ".", ui.KeySlash: "/", ui.KeySemicolon: ";",
	ui.KeyQuote: "'", ui.KeyBracketLeft: "[", ui.KeyBracketRight: "]", ui.KeyBackslash: "\\", ui.KeyBackquote: "`",
}

func setShortcutOverride(overrides map[string][]config.KeyStroke, id shortcutAction, keys []config.KeyStroke) {
	if slices.Equal(keys, shortcutByID[id].defaults) {
		delete(overrides, string(id))
		return
	}
	overrides[string(id)] = slices.Clone(keys)
}

func (a *App) applyKeymap(overrides map[string][]config.KeyStroke) error {
	cfg := a.cfg
	cfg.KeyboardShortcuts = maps.Clone(overrides)
	if err := cfg.Save(); err != nil {
		return err
	}
	a.cfg.KeyboardShortcuts = cfg.KeyboardShortcuts
	a.keymapBindings = nil
	a.keymapHints = nil
	a.resetSearchGesture()
	a.syncNativeEditorShortcuts()
	return nil
}

// The Edit menu has native accelerators even when the editor overrides its
// keymap. Keep the menu actions available while removing any old accelerator.
func (a *App) syncNativeEditorShortcuts() {
	if a.keymapNativeEditMenu == nil {
		return
	}
	for _, item := range a.keymapNativeEditMenu.Submenu {
		switch item.Role {
		case mygo.RoleUndo:
			if _, changed := a.cfg.KeyboardShortcuts[string(keyUndo)]; changed {
				item.SetAccelerator("")
			} else {
				item.SetAccelerator("CmdOrCtrl+Z")
			}
		case mygo.RoleRedo:
			if _, changed := a.cfg.KeyboardShortcuts[string(keyRedo)]; changed {
				item.SetAccelerator("")
			} else if runtime.GOOS == "windows" {
				item.SetAccelerator("Ctrl+Y")
			} else {
				item.SetAccelerator("Shift+CmdOrCtrl+Z")
			}
		}
	}
}

// Rebuild the editor overrides only when the keymap changes. The editor
// still handles its own focus, selections and native Edit menu commands.
func (a *App) configuredEditorBindings() []editor.Binding {
	if a.keymapBindings != nil {
		return a.keymapBindings
	}
	var bindings, removed []editor.Binding
	for _, action := range shortcutDefinitions {
		keys := a.shortcuts(action.id)
		if action.command == editor.NoCommand {
			// A reassigned window action still gets a chance when the
			// editor would normally consume its new key.
			for _, key := range keys {
				if key.Key != 0 && !slices.Contains(action.defaults, key) {
					bindings = append(bindings, editor.Binding{Mods: ui.Modifiers(key.Modifiers), Key: ui.Key(key.Key), Command: editor.NoCommand})
				}
			}
			continue
		}
		for _, key := range keys {
			bindings = append(bindings, editor.Binding{Mods: ui.Modifiers(key.Modifiers), Key: ui.Key(key.Key), Command: action.command})
		}
		for _, key := range action.defaults {
			if !slices.Contains(keys, key) {
				removed = append(removed, editor.Binding{Mods: ui.Modifiers(key.Modifiers), Key: ui.Key(key.Key), Command: editor.NoCommand})
			}
		}
	}
	if _, changed := a.cfg.KeyboardShortcuts[string(keyGoToLine)]; changed && ui.Cmd == ui.Super {
		removed = append(removed, editor.Binding{Mods: ui.Ctrl, Key: ui.KeyG, Command: editor.NoCommand})
	}
	a.keymapDiffBindings = append(slices.Clone(bindings), removed...)
	if ui.Cmd == ui.Super {
		bindings = append(bindings, editor.Binding{Mods: ui.Ctrl, Key: ui.KeyG, Command: editor.SelectNext})
	}
	a.keymapBindings = append(bindings, removed...)
	return a.keymapBindings
}
