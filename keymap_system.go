package main

import (
	"context"
	"encoding/xml"
	"os/exec"
	"runtime"
	"strconv"
	"time"

	"github.com/egoist/mygo/ui"
	"github.com/iwangjie/nemorous/internal/config"
)

type plistValue struct {
	XMLName xml.Name
	Text    string       `xml:",chardata"`
	Values  []plistValue `xml:",any"`
}

func (v plistValue) property(key string) plistValue {
	for i := 0; i+1 < len(v.Values); i += 2 {
		if v.Values[i].Text == key {
			return v.Values[i+1]
		}
	}
	return plistValue{}
}

func loadSystemShortcuts() map[config.KeyStroke]string {
	if runtime.GOOS != "darwin" {
		return nil
	}
	ctx, cancel := context.WithTimeout(context.Background(), time.Second)
	defer cancel()
	data, _ := exec.CommandContext(ctx, "/usr/bin/defaults", "export", "com.apple.symbolichotkeys", "-").Output()
	return macSystemShortcuts(data)
}

func macSystemShortcuts(data []byte) map[config.KeyStroke]string {
	var list plistValue
	_ = xml.Unmarshal(data, &list)
	var hotkeys plistValue
	if len(list.Values) > 0 {
		hotkeys = list.Values[0].property("AppleSymbolicHotKeys")
	}

	// The app switcher is reserved separately from symbolic shortcuts.
	result := map[config.KeyStroke]string{
		stroke(ui.Super, ui.KeyTab):           "macOS 应用切换器",
		stroke(ui.Super|ui.Shift, ui.KeyTab):  "macOS 应用切换器",
		stroke(ui.Super|ui.Alt, ui.KeyEscape): "macOS 强制退出应用",
	}
	for _, item := range []struct {
		id, label string
		fallback  config.KeyStroke
	}{
		{"32", "调度中心", stroke(ui.Ctrl, ui.KeyUp)},
		{"33", "应用程序窗口", stroke(ui.Ctrl, ui.KeyDown)},
		{"79", "向左移动一个空间", stroke(ui.Ctrl, ui.KeyLeft)},
		{"81", "向右移动一个空间", stroke(ui.Ctrl, ui.KeyRight)},
		{"60", "选择上一个输入法", config.KeyStroke{}},
		{"61", "选择下一个输入法", config.KeyStroke{}},
		{"64", "Spotlight", stroke(ui.Super, ui.KeySpace)},
		{"65", "访达搜索", config.KeyStroke{}},
		{"118", "切换到桌面 1", config.KeyStroke{}},
		{"119", "切换到桌面 2", config.KeyStroke{}},
	} {
		value := hotkeys.property(item.id)
		if value.property("enabled").XMLName.Local == "false" {
			continue
		}
		key := item.fallback
		if params := value.property("value").property("parameters").Values; len(params) == 3 {
			code, _ := strconv.Atoi(params[1].Text)
			mask, _ := strconv.Atoi(params[2].Text)
			var mods ui.Modifiers
			for bit, mod := range map[int]ui.Modifiers{17: ui.Shift, 18: ui.Ctrl, 19: ui.Alt, 20: ui.Super} {
				if mask&(1<<bit) != 0 {
					mods |= mod
				}
			}
			key = stroke(mods, macShortcutKey(code))
		}
		if key.Key != 0 {
			result[key] = "macOS " + item.label
		}
	}
	return result
}

func macShortcutKey(code int) ui.Key {
	if key := macShortcutKeyCodes[code]; key != 0 {
		return key
	}
	for i, v := range []int{0, 11, 8, 2, 14, 3, 5, 4, 34, 38, 40, 37, 46, 45, 31, 35, 12, 15, 1, 17, 32, 9, 13, 7, 16, 6} {
		if code == v {
			return ui.KeyA + ui.Key(i)
		}
	}
	return ui.KeyUnknown
}

var macShortcutKeyCodes = map[int]ui.Key{
	18: ui.Key1, 19: ui.Key2, 20: ui.Key3, 21: ui.Key4, 23: ui.Key5, 22: ui.Key6, 26: ui.Key7, 28: ui.Key8, 25: ui.Key9, 29: ui.Key0,
	36: ui.KeyEnter, 48: ui.KeyTab, 49: ui.KeySpace, 53: ui.KeyEscape, 123: ui.KeyLeft, 124: ui.KeyRight, 125: ui.KeyDown, 126: ui.KeyUp,
	122: ui.KeyF1, 120: ui.KeyF2, 99: ui.KeyF3, 118: ui.KeyF4, 96: ui.KeyF5, 97: ui.KeyF6, 98: ui.KeyF7, 100: ui.KeyF8, 101: ui.KeyF9, 109: ui.KeyF10, 103: ui.KeyF11, 111: ui.KeyF12,
}
