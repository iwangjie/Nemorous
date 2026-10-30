package main

import (
	"fmt"
	"strings"

	"github.com/egoist/mygo/ui"
)

func (a *App) checkedChangeKeys() []string {
	var keys []string
	for _, st := range a.statuses {
		for _, f := range st.Files {
			if a.checkedChanges[f.Key()] {
				keys = append(keys, f.Key())
			}
		}
	}
	return keys
}

func (a *App) openChangeListModal() {
	a.showChangeListModal = true
	a.changeListNameInput = ""
	a.changeListRenameFrom = ""
	a.changeListRenameInput = ""
	a.changeListErr = ""
}

func (a *App) createChangeList(assignChecked bool) {
	name := strings.TrimSpace(a.changeListNameInput)
	if name == "" {
		a.changeListErr = "请输入变更列表名称"
		return
	}
	var keys []string
	if assignChecked {
		keys = a.checkedChangeKeys()
		if len(keys) == 0 {
			a.changeListErr = "没有勾选的文件"
			return
		}
	}
	if assignChecked {
		a.cfg.AssignToChangeList(name, keys)
	} else {
		a.cfg.SaveChangeList(name, nil)
	}
	_ = a.cfg.Save()
	a.changeListFilter = name
	a.showChangeListModal = false
	a.showToast("ok", fmt.Sprintf("已创建变更列表「%s」", name))
}

func (a *App) buildChangeListModal(c *ui.Context, p Palette) {
	ui.DialogBase(c, &a.showChangeListModal, func(backdrop, panel ui.Element) {
		backdrop.Background(ui.RGBA(20, 22, 30, 0.36))
		panel.Width(480).Height(360).Padding(14).Gap(8).Radius(12).Background(p.Surface).Border(1, p.Border)
		modalHeader(c, p, icoList, "变更列表", "", nil, func() { a.showChangeListModal = false })
		ui.Row(c).FillWidth().Gap(6).AlignItems(ui.Center).Children(func() {
			ui.TextInput(c.Key("cl-name"), &a.changeListNameInput).Grow(1).Placeholder("新列表名称")
			checked := len(a.checkedChangeKeys())
			tip := "新建并加入勾选文件"
			if checked == 0 {
				tip = "新建变更列表"
			}
			iconAction(c, p, "cl-create", icoPlus, tip, true, func() { a.createChangeList(checked > 0) })
		})
		if a.changeListErr != "" {
			ui.Text(c, a.changeListErr).FontSize(10).TextColor(p.Danger)
		}
		ui.Scroll(c).Grow(1).Children(func() {
			var keys []string
			for _, st := range a.statuses {
				for _, f := range st.Files {
					keys = append(keys, f.Key())
				}
			}
			all := make(map[string]bool, len(keys))
			for _, k := range keys {
				all[k] = true
			}
			rows := append([]string{"__all__", "__default__"}, a.cfg.ChangeListNames()...)
			for _, name := range rows {
				name := name
				label := name
				count := 0
				icon := icoList
				switch name {
				case "__all__":
					label = "全部变更"
					count = len(keys)
				case "__default__":
					label = "默认列表"
					icon = icoFolder
					count = a.cfg.ChangeListCount("", all)
				default:
					count = a.cfg.ChangeListCount(name, all)
				}
				kc := c.Key("cl-" + name)
				row := ui.Row(kc).FillWidth().Padding(6, 8).Gap(7).AlignItems(ui.Center).Radius(4).Cursor(ui.CursorPointer)
				if a.changeListFilter == name {
					row.Background(p.AccentSoft)
				}
				if row.Hovered() && a.changeListFilter != name {
					row.Background(p.SurfaceHover)
				}
				if row.Clicked() {
					a.changeListFilter = name
				}
				row.Children(func() {
					drawIcon(kc, icon, 12, p.TextMuted)
					ui.Text(kc, label).FontSize(11.5).TextColor(p.TextPrimary).Grow(1)
					ui.Text(kc, fmt.Sprint(count)).FontSize(10).TextColor(p.TextMuted)
					if name != "__all__" && name != "__default__" {
						if a.changeListRenameFrom == name {
							ui.TextInput(kc, &a.changeListRenameInput).Width(160).Placeholder("新名称")
							toolButton(kc, p, "cl-save-"+name, icoCheck, "保存名称", false, true, func() {
								if a.cfg.RenameChangeList(name, a.changeListRenameInput) {
									_ = a.cfg.Save()
									if a.changeListFilter == name {
										a.changeListFilter = strings.TrimSpace(a.changeListRenameInput)
									}
									a.changeListRenameFrom = ""
								} else {
									a.changeListErr = "名称无效或已存在"
								}
							})
							toolButton(kc, p, "cl-cancel-"+name, icoClose, "取消", false, true, func() { a.changeListRenameFrom = "" })
						} else {
							toolButton(kc, p, "cl-rename-"+name, icoEdit, "重命名", false, true, func() {
								a.changeListRenameFrom = name
								a.changeListRenameInput = name
							})
							toolButton(kc, p, "cl-delete-"+name, icoTrash, "删除", false, true, func() {
								a.cfg.DeleteChangeList(name)
								_ = a.cfg.Save()
								if a.changeListFilter == name {
									a.changeListFilter = "__all__"
								}
							})
						}
					}
				})
			}
		})
	})
}
