use std::collections::BTreeMap;

use nsh_core::keymap::Keystroke;
use nsh_core::platform::Os;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::config::Config;

pub(crate) const DOUBLE_SHIFT: &str = "shift shift";
pub(crate) const LEGACY: &str = "keyboard_shortcuts";

#[derive(Clone, Copy)]
pub(crate) struct Definition {
    pub id: &'static str,
    pub label: &'static str,
    pub group: &'static str,
    defaults: &'static [&'static str],
}

const fn action(
    id: &'static str,
    label: &'static str,
    group: &'static str,
    defaults: &'static [&'static str],
) -> Definition {
    Definition {
        id,
        label,
        group,
        defaults,
    }
}

pub(crate) const ACTIONS: &[Definition] = &[
    action("save", "保存", "文件", &["mod+s"]),
    action("undo", "撤销", "编辑", &["mod+z"]),
    action("redo", "重做", "编辑", &["mod+shift+z"]),
    action("closeTab", "关闭标签", "文件", &["mod+w"]),
    action("goToLine", "转到行列", "导航", &["mod+l"]),
    action("back", "后退", "导航", &["mod+["]),
    action("forward", "前进", "导航", &["mod+]"]),
    action("findInFiles", "在文件中查找", "查找", &["mod+shift+f"]),
    action("replaceInFiles", "在文件中替换", "查找", &["mod+shift+r"]),
    action("structure", "文件结构", "编辑器", &["mod+f12"]),
    action("todos", "TODO", "编辑器", &["mod+shift+t"]),
    action("format", "重新格式化代码", "编辑", &["mod+alt+l"]),
    action("findReferences", "查找引用", "编辑器", &["alt+f7"]),
    action("quickFixes", "快速修复", "编辑器", &["alt+enter"]),
    action("bookmark", "切换书签", "编辑", &["mod+shift+b"]),
    action("find", "查找", "查找", &["mod+f"]),
    action("replace", "替换", "查找", &["mod+r", "mod+alt+f"]),
    action("findNext", "下一个匹配", "查找", &["mod+g"]),
    action("findPrevious", "上一个匹配", "查找", &["mod+shift+g"]),
    action("goToFile", "转到文件", "导航", &["mod+shift+o"]),
    action("recentLocations", "最近位置", "导航", &["mod+shift+e"]),
    action("refresh", "刷新工作区", "文件", &["mod+alt+y"]),
    action("copyAbsolute", "复制绝对路径", "编辑器", &["mod+shift+c"]),
    action("copyReference", "复制引用", "编辑器", &["mod+alt+shift+c"]),
    action("selectIn", "在文件树中定位", "编辑器", &["alt+f1"]),
    action("jumpToSource", "跳转到源码", "编辑器", &["f4", "cmd+down"]),
    action("nextDifference", "下一处差异", "差异", &["f7"]),
    action("previousDifference", "上一处差异", "差异", &["shift+f7"]),
    action("nextDiffFile", "下一个差异文件", "差异", &["alt+right"]),
    action("previousDiffFile", "上一个差异文件", "差异", &["alt+left"]),
    action("rollback", "回滚改动", "Git", &["mod+alt+z"]),
    action("gitCommit", "打开提交", "Git", &["mod+k"]),
    action("pull", "拉取", "Git", &["mod+t"]),
    action("push", "推送", "Git", &["mod+shift+k"]),
    action("branchMatrix", "分支矩阵", "Git", &["mod+b"]),
    action("commitWindow", "提交 (Commit)", "窗口", &["mod+0"]),
    action("project", "项目 (Project)", "窗口", &["mod+1"]),
    action("gitWindow", "Git", "窗口", &["mod+9"]),
    action("maniWindow", "Mani", "窗口", &["mod+4"]),
    action("terminalWindow", "终端", "窗口", &["alt+f12"]),
    action("breakpoint", "切换断点", "运行", &["mod+f8"]),
    action(
        "actions",
        "查找操作",
        "工具窗口",
        &["mod+shift+p", "mod+shift+a"],
    ),
    action(
        "switcher",
        "切换器",
        "窗口",
        &["ctrl+tab", "ctrl+shift+tab"],
    ),
    action("recentFiles", "最近文件", "文件", &["mod+e"]),
    action("searchEverywhere", "随处搜索", "查找", &["shift shift"]),
    action("previousTab", "上一个标签", "导航", &["mod+shift+["]),
    action("nextTab", "下一个标签", "导航", &["mod+shift+]"]),
    action("nextOccurrence", "选择下一处相同文字", "编辑", &["ctrl+g"]),
    action("copy", "复制", "编辑", &["mod+c"]),
    action("compareOrDiff", "打开差异或比较文件", "差异", &["mod+d"]),
    action("settings", "设置", "窗口", &["mod+,"]),
    action("zoomIn", "放大界面", "窗口", &["mod+="]),
    action("zoomOut", "缩小界面", "窗口", &["mod+-"]),
    action("zoomReset", "重置界面缩放", "窗口", &[]),
];

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Keymap {
    entries: BTreeMap<&'static str, Vec<String>>,
}

impl Keymap {
    pub fn from_config(cfg: &Config, apple: bool, os: Os) -> Self {
        Self::from_values(&cfg.keymap_overrides, cfg.extra.get(LEGACY), apple, os)
    }

    pub(crate) fn from_values(
        overrides: &BTreeMap<String, String>,
        legacy: Option<&Value>,
        apple: bool,
        os: Os,
    ) -> Self {
        let legacy = legacy.and_then(Value::as_object);
        let mut claims = BTreeMap::<String, (bool, &'static str)>::new();
        let mut entries = BTreeMap::new();
        for action in ACTIONS {
            let custom = overrides
                .get(action.id)
                .map(|value| {
                    if value.trim().is_empty() {
                        Vec::new()
                    } else {
                        vec![value.clone()]
                    }
                })
                .or_else(|| {
                    legacy
                        .and_then(|legacy| legacy.get(action.id))
                        .and_then(|value| {
                            serde_json::from_value::<Vec<LegacyStroke>>(value.clone()).ok()
                        })
                        .map(|strokes| strokes.into_iter().filter_map(LegacyStroke::name).collect())
                });
            let keys = custom
                .as_deref()
                .map(|keys| keys.iter().map(String::as_str).collect::<Vec<_>>())
                .unwrap_or_else(|| action.defaults.to_vec());
            let mut resolved = Vec::new();
            for key in keys {
                if action.id == "nextOccurrence" && !apple && custom.is_none() {
                    continue;
                }
                let Ok(key) = normalize(key, apple, os) else {
                    continue;
                };
                if resolved.contains(&key) {
                    continue;
                }
                let priority = custom.is_some();
                if claims
                    .get(&key)
                    .is_none_or(|(previous, _)| priority && !previous)
                {
                    claims.insert(key.clone(), (priority, action.id));
                }
                resolved.push(key);
            }
            entries.insert(action.id, resolved);
        }
        for (id, keys) in &mut entries {
            keys.retain(|key| claims.get(key).is_some_and(|(_, owner)| owner == id));
        }
        Self { entries }
    }

    pub fn keys(&self, id: &str) -> &[String] {
        self.entries.get(id).map(Vec::as_slice).unwrap_or_default()
    }
}

#[derive(Clone, Copy, Serialize, Deserialize)]
struct LegacyStroke {
    modifiers: u8,
    key: u16,
}

const NAMED: [&str; 40] = [
    "",
    "enter",
    "escape",
    "backspace",
    "tab",
    "space",
    "delete",
    "insert",
    "home",
    "end",
    "pageup",
    "pagedown",
    "left",
    "right",
    "up",
    "down",
    "f1",
    "f2",
    "f3",
    "f4",
    "f5",
    "f6",
    "f7",
    "f8",
    "f9",
    "f10",
    "f11",
    "f12",
    "-",
    "=",
    ",",
    ".",
    "/",
    ";",
    "'",
    "[",
    "]",
    "\\",
    "`",
    "",
];

impl LegacyStroke {
    fn name(self) -> Option<String> {
        if self.key == 0 && self.modifiers == 1 {
            return Some(DOUBLE_SHIFT.to_string());
        }
        if self.modifiers & !15 != 0 {
            return None;
        }
        let key = match self.key {
            40..=49 => char::from(b'0' + (self.key - 40) as u8).to_string(),
            50..=75 => char::from(b'a' + (self.key - 50) as u8).to_string(),
            number => NAMED
                .get(number as usize)
                .filter(|value| !value.is_empty())?
                .to_string(),
        };
        let mut name = String::new();
        for (bit, text) in [(1, "shift+"), (2, "ctrl+"), (4, "alt+"), (8, "cmd+")] {
            if self.modifiers & bit != 0 {
                name.push_str(text);
            }
        }
        name.push_str(&key);
        Some(name)
    }

    fn from_name(name: &str) -> Option<Self> {
        if name == DOUBLE_SHIFT {
            return Some(Self {
                modifiers: 1,
                key: 0,
            });
        }
        let parts = name.split('+').collect::<Vec<_>>();
        let (key, modifiers) = parts.split_last()?;
        let mut mask = 0;
        for modifier in modifiers {
            mask |= match *modifier {
                "shift" => 1,
                "ctrl" => 2,
                "alt" => 4,
                "cmd" => 8,
                _ => return None,
            };
        }
        let key = match key.as_bytes() {
            [byte @ b'0'..=b'9'] => u16::from(byte - b'0') + 40,
            [byte @ b'a'..=b'z'] => u16::from(byte - b'a') + 50,
            _ => NAMED.iter().position(|name| name == key)? as u16,
        };
        Some(Self {
            modifiers: mask,
            key,
        })
    }
}

fn normalize(text: &str, apple: bool, os: Os) -> Result<String, String> {
    let text = text.trim();
    if text.eq_ignore_ascii_case(DOUBLE_SHIFT) {
        return Ok(DOUBLE_SHIFT.into());
    }
    let key = Keystroke::parse_with(text, apple).map_err(|err| err.to_string())?;
    if key.reserved(os).is_some() {
        return Err("此快捷键由系统保留".to_string());
    }
    Ok(key.to_string())
}

pub(crate) fn set_binding(
    cfg: &mut Config,
    id: &str,
    input: &str,
    apple: bool,
    os: Os,
) -> Result<(), String> {
    let action = ACTIONS
        .iter()
        .find(|action| action.id == id)
        .ok_or_else(|| "找不到该操作".to_string())?;
    let keymap = Keymap::from_config(cfg, apple, os);
    let binding = if input.trim().is_empty() {
        None
    } else {
        let key = normalize(input, apple, os)?;
        if key == DOUBLE_SHIFT && id != "searchEverywhere" {
            return Err("双击 Shift 只用于随处搜索".into());
        }
        if let Some(conflict) = ACTIONS
            .iter()
            .find(|other| other.id != action.id && keymap.keys(other.id).contains(&key))
        {
            return Err(format!("已被「{}」使用", conflict.label));
        }
        Some(key)
    };

    let mut legacy = cfg
        .extra
        .get(LEGACY)
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    cfg.keymap_overrides.remove(id);
    if let Some(key) = binding {
        if let Some(stroke) = LegacyStroke::from_name(&key) {
            legacy.insert(
                id.to_string(),
                serde_json::to_value([stroke]).map_err(|err| err.to_string())?,
            );
        } else {
            legacy.remove(id);
            cfg.keymap_overrides.insert(id.to_string(), key);
        }
    } else {
        legacy.insert(id.to_string(), Value::Array(Vec::new()));
    }
    cfg.extra.insert(LEGACY.into(), Value::Object(legacy));
    Ok(())
}

pub(crate) fn reset(cfg: &mut Config, id: Option<&str>) {
    let mut legacy = cfg
        .extra
        .get(LEGACY)
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_else(Map::new);
    for action in ACTIONS
        .iter()
        .filter(|action| id.is_none_or(|id| action.id == id))
    {
        cfg.keymap_overrides.remove(action.id);
        legacy.remove(action.id);
    }
    if legacy.is_empty() {
        cfg.extra.remove(LEGACY);
    } else {
        cfg.extra.insert(LEGACY.into(), Value::Object(legacy));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn workspace_binding_takes_precedence_over_a_later_editor_binding() {
        use nsh_core::keymap::{KeyBinding, Keymap as SurfaceKeymap, When};

        let shortcut = Keystroke::parse_with("cmd+[", true).unwrap();
        let workspace = KeyBinding::new(1, shortcut, When::NotEditing, false).with_priority(1);
        let editor = KeyBinding::new(2, shortcut, When::Always, false).focus_keys(true);
        let table = SurfaceKeymap::new(vec![workspace, editor]);
        assert_eq!(table.find(shortcut, false, false).fired(), Some(1));
        assert_eq!(table.find(shortcut, true, false).fired(), Some(2));

        let updated = KeyBinding::new(3, shortcut, When::Always, false).focus_keys(true);
        let table = SurfaceKeymap::new(vec![editor, updated]);
        assert_eq!(table.find(shortcut, false, false).fired(), Some(3));
    }

    #[test]
    fn resolves_numeric_cocoa_bindings_rejects_alias_conflicts_and_preserves_unknowns() {
        let mut cfg: Config = serde_json::from_value(serde_json::json!({
            "keyboard_shortcuts": {
                "goToLine": [{"modifiers":9,"key":61}],
                "unknown": [{"modifiers":8,"key":40}]
            }
        }))
        .unwrap();
        let os = Os::MacOs;
        assert_eq!(
            Keymap::from_config(&cfg, true, os).keys("goToLine"),
            &["shift+cmd+l"]
        );
        assert!(set_binding(&mut cfg, "recentFiles", "mod+s", true, os).is_err());
        assert!(set_binding(&mut cfg, "recentFiles", "mod+q", true, os).is_err());
        set_binding(&mut cfg, "recentFiles", "mod+alt+e", true, os).unwrap();
        assert_eq!(cfg.extra[LEGACY]["recentFiles"][0]["key"], 54);
        assert_eq!(cfg.extra[LEGACY]["recentFiles"][0]["modifiers"], 12);
        reset(&mut cfg, None);
        assert_eq!(
            Keymap::from_config(&cfg, true, os).keys("goToLine"),
            &["cmd+l"]
        );
        assert!(cfg.extra[LEGACY]["unknown"].is_array());
    }
}
