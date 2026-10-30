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
		panel.Width(520).Height(460).Padding(16).Gap(10).Radius(12).Background(p.Surface).Border(1, p.Border)
		ui.Row(c).Justify(ui.SpaceBetween).AlignItems(ui.Center).Children(func() {
			ui.Row(c).Gap(8).AlignItems(ui.Center).Children(func() {
				drawIcon(c, icoFolder, 15, p.Accent)
				ui.Text(c, "变更列表").FontSize(14).FontWeight(weightBold).TextColor(p.TextPrimary)
			})
			miniBtn(c, p, "关闭", func() { a.showChangeListModal = false })
		})
		ui.Row(c).Gap(6).AlignItems(ui.Center).Children(func() {
			ui.TextInput(c.Key("cl-name"), &a.changeListNameInput).Grow(1).Placeholder("新列表名称，如：登录需求")
			pill(c, p, "创建", false, func() { a.createChangeList(false) })
			pill(c, p, "创建并入勾选", true, func() { a.createChangeList(true) })
		})
		if a.changeListErr != "" {
			ui.Text(c, a.changeListErr).FontSize(10.5).TextColor(p.Danger)
		}
		ui.Text(c, "把未提交文件分组，便于按需求/任务分别审阅；未归组的文件属于默认列表。").FontSize(10.5).TextColor(p.TextMuted)
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
				switch name {
				case "__all__":
					label = "全部变更"
					count = len(keys)
				case "__default__":
					label = "默认列表"
					count = a.cfg.ChangeListCount("", all)
				default:
					count = a.cfg.ChangeListCount(name, all)
				}
				kc := c.Key("cl-" + name)
				row := ui.Row(kc).FillWidth().Padding(6, 8).Gap(8).AlignItems(ui.Center).Radius(5).Cursor(ui.CursorPointer)
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
					ui.Text(kc, label).FontSize(11.5).TextColor(p.TextPrimary).Grow(1)
					ui.Text(kc, fmt.Sprintf("%d 个文件", count)).FontSize(10).TextColor(p.TextMuted)
					if name != "__all__" && name != "__default__" {
						if a.changeListRenameFrom == name {
							ui.TextInput(kc, &a.changeListRenameInput).Width(180).Placeholder("新名称")
							miniBtn(kc, p, "保存", func() {
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
							miniBtn(kc, p, "取消", func() { a.changeListRenameFrom = "" })
						} else {
							miniBtn(kc, p, "重命名", func() {
								a.changeListRenameFrom = name
								a.changeListRenameInput = name
							})
							miniBtn(kc, p, "删除", func() {
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
