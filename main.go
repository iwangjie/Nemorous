package main

import (
	"context"
	"fmt"
	"log"
	"path"
	"path/filepath"
	"runtime"
	"strings"
	"time"

	"github.com/egoist/mygo"
	"github.com/egoist/mygo/ui"
	"github.com/iwangjie/nemorous/internal/config"
)

func main() {
	cfg := config.Load()
	app := NewApp(cfg)

	mygo.App.WhenReady(func() {
		mygo.App.SetMenu(mygo.NewMenu([]*mygo.MenuItem{
			{Role: mygo.RoleAppMenu},
			{Role: mygo.RoleEditMenu},
			{Role: mygo.RoleWindowMenu},
		}))
		app.win = mygo.NewWindow(mygo.WindowOptions{
			Title:         "Nemorous",
			Width:         1440,
			Height:        900,
			MinWidth:      1040,
			MinHeight:     660,
			TitleBarStyle: mygo.TitleBarHidden,
			Content:       ui.View(app.BuildUI),
		})
		if cfg.WorkspacePath != "" {
			app.LoadWorkspaceAsync(cfg.WorkspacePath)
		}
		app.startWorkspaceWatcher()
	})

	if err := mygo.App.Run(); err != nil {
		log.Fatal(err)
	}
}

func (a *App) startWorkspaceWatcher() {
	if a.watcherCancel != nil {
		a.watcherCancel()
		a.watcherCancel = nil
	}
	if a.cfg.WorkspacePath == "" {
		return
	}
	ctx, cancel := context.WithCancel(context.Background())
	a.watcherCancel = cancel

	// 1. Recursive filesystem watcher in background goroutine for immediate real-time change detection
	go func() {
		w, err := mygo.WatchFiles(ctx, a.cfg.WorkspacePath, mygo.FileWatchOptions{
			Recursive: true,
			Debounce:  300 * time.Millisecond,
			Ignore: func(p string, isDir bool) bool {
				base := path.Base(p)
				if isDir {
					switch base {
					case ".git", "node_modules", "target", "build", "dist", ".idea", ".gradle", "www", "logs", "objects", "logs_cache":
						return true
					}
				}
				if strings.HasSuffix(base, ".lock") {
					return true
				}
				return false
			},
		})
		if err != nil {
			return
		}
		defer w.Close()
		var lastTrigger time.Time
		for {
			_, err := w.Next(ctx)
			if err != nil {
				return
			}
			if time.Since(lastTrigger) < 400*time.Millisecond {
				continue
			}
			lastTrigger = time.Now()
			a.runUI(func() {
				a.RefreshSilentAsync()
			})
		}
	}()

	// 2. Fallback lightweight real-time polling every 2.5s to catch external git/IDE operations
	go func() {
		ticker := time.NewTicker(8 * time.Second)
		defer ticker.Stop()
		for {
			select {
			case <-ctx.Done():
				return
			case <-ticker.C:
				a.runUI(func() {
					a.RefreshSilentAsync()
				})
			}
		}
	}()
}

// BuildUI constructs the complete immediate-mode UI tree for Nemorous.
func (a *App) BuildUI(c *ui.Context) {
	if a.pendingClipboard != "" {
		c.Services().WriteClipboard(a.pendingClipboard)
		a.pendingClipboard = ""
	}
	p := a.palette()
	t := *ui.LightTheme()
	if p.IsDark {
		t = *ui.DarkTheme()
	}
	t.Background = p.AppBg
	t.Surface = p.Surface2
	t.SurfaceHover = p.SurfaceHover
	t.Border = p.Border
	t.Text = p.TextPrimary
	t.TextMuted = p.TextMuted
	t.Accent = p.Accent
	t.AccentHover = p.AccentHover
	t.AccentText = p.AccentText
	t.Danger = p.Danger
	c.SetTheme(&t)
	a.registerShortcuts(c)

	ui.Column(c).Bind(&a.rootHandle).Fill().Background(p.AppBg).TextColor(p.TextPrimary).Children(func() {
		a.buildTopHeaderBar(c, p)

		if a.showTopMoreMenu {
			a.buildCollapsibleMoreToolbar(c, p)
		}

		// Main Content Stage
		ui.Column(c).Grow(1).FillWidth().Children(func() {
			switch a.activeTab {
			case TabCode:
				a.buildCodeView(c, p)
			case TabOverview:
				a.buildOverviewView(c, p)
			case TabMerge:
				a.buildMergeView(c, p)
			case TabMani:
				a.buildManiView(c, p)
			default:
				a.buildChangesView(c, p)
			}
		})

		a.buildStatusBar(c, p)

		// Modals & Dialogs
		a.buildCommandPaletteModal(c, p)
		a.buildBranchSwitchModal(c, p)
		a.buildSettingsModal(c, p)
		a.buildDiscardConfirmModal(c, p)
		a.buildUndoConfirmModal(c, p)
		a.buildFileHistoryModal(c, p)
		a.buildOpLogModal(c, p)
	})
}

func (a *App) registerShortcuts(c *ui.Context) {
	// Dialogs own the keyboard: global actions must not fire behind them.
	if a.overlayOpen() {
		return
	}
	if c.Shortcut(ui.Cmd, ui.KeyK) || c.Shortcut(ui.Cmd, ui.KeyP) {
		a.showCmdPalette = !a.showCmdPalette
		a.cmdPaletteQuery = ""
		if a.showCmdPalette {
			a.cmdInputHandle.Focus()
		}
	}
	if c.Shortcut(ui.Cmd, ui.KeyComma) {
		a.showSettings = !a.showSettings
	}
	if c.Shortcut(ui.Cmd, ui.KeyB) {
		if a.showBranchModal {
			a.showBranchModal = false
		} else {
			a.openBranchSwitchModal()
		}
	}
	if c.Shortcut(ui.Cmd, ui.KeyR) {
		a.RefreshAsync()
	}
	if c.Shortcut(ui.Cmd, ui.KeyS) {
		if a.activeTab == TabChanges {
			a.saveDiffMicroEdit()
		} else if a.activeTab == TabCode {
			a.saveCodeMicroEdit()
		}
	}
	if a.activeTab == TabChanges {
		if c.Shortcut(ui.Cmd, ui.KeyG) {
			a.generateCommitMessage()
		}
		if c.Shortcut(ui.Cmd|ui.Shift, ui.KeyEnter) {
			a.TriggerCommit(true)
		} else if c.Shortcut(ui.Cmd, ui.KeyEnter) {
			a.TriggerCommit(false)
		}
		if !a.textFocusActive(c) {
			if c.Shortcut(0, ui.KeyDown) {
				a.moveChangeSelection(1)
			}
			if c.Shortcut(0, ui.KeyUp) {
				a.moveChangeSelection(-1)
			}
			if c.Shortcut(0, ui.KeySpace) {
				a.toggleSelectedChange()
			}
			if c.Shortcut(0, ui.KeyS) {
				a.stageSelectedChange(true)
			}
			if c.Shortcut(0, ui.KeyU) {
				a.stageSelectedChange(false)
			}
		}
	}
	if c.Shortcut(ui.Cmd, ui.Key1) {
		a.activeTab = TabChanges
	}
	if c.Shortcut(ui.Cmd, ui.Key2) {
		a.activeTab = TabCode
	}
	if c.Shortcut(ui.Cmd, ui.Key3) {
		a.activeTab = TabOverview
	}
	if c.Shortcut(ui.Cmd, ui.Key4) {
		a.activeTab = TabMerge
	}
	if c.Shortcut(ui.Cmd, ui.Key5) {
		a.activeTab = TabMani
	}
}

func (a *App) buildTopHeaderBar(c *ui.Context, p Palette) {
	wsName := filepath.Base(a.cfg.WorkspacePath)
	if wsName == "" || wsName == "." {
		wsName = "选择工作区"
	}

	bar := c.TitleBar()
	leftClearance := max(bar.Left+12, float32(14))
	if runtime.GOOS == "darwin" && leftClearance < 76 {
		leftClearance = 76
	}

	domBranch, alignedCnt, totalChild := a.DominantBranch()
	_, totalChangedFiles, aheadRepos, _, _ := a.SummaryCounts()
	conflictPlanCnt := 0
	for _, pl := range a.mergePlans {
		if pl.Kind == "conflict" {
			conflictPlanCnt++
		}
	}

	ui.Row(c).Height(46).FillWidth().Padding(0, 14, 0, leftClearance).Background(p.Surface).BorderWidth(0, 0, 1, 0).BorderColor(p.Border).AlignItems(ui.Center).DragWindow().Children(func() {
		// Left: Workspace Pop-up Pill + Unified Branch Switcher Pill
		ui.Row(c).Gap(8).AlignItems(ui.Center).Children(func() {
			wsBtn := ui.Row(c).Padding(5, 10).Gap(6).Radius(6).Background(p.Surface2).Cursor(ui.CursorPointer).AlignItems(ui.Center)
			if wsBtn.Hovered() {
				wsBtn.Background(p.SurfaceHover)
			}
			if wsBtn.Clicked() {
				a.showSettings = true
			}
			wsBtn.Children(func() {
				drawIcon(c, icoFolder, 13, p.TextControl)
				ui.Text(c, wsName).FontSize(12.5).FontWeight(weightSemibold).TextColor(p.TextPrimary).SingleLine()
				ui.Text(c, fmt.Sprintf("%d", len(a.repos))).FontSize(11).TextColor(p.TextMuted).SingleLine()
				drawIcon(c, icoChevronDown, 11, p.TextMuted)
			})

			// Branch Switcher Pill: recent-first hierarchy, no flat branch wall.
			recentTree, staleTree := a.workspaceBranchTrees()
			brBtn := ui.Row(c).Padding(5, 10).Gap(6).Radius(6).Background(p.Surface2).Cursor(ui.CursorPointer).AlignItems(ui.Center)
			if brBtn.Hovered() {
				brBtn.Background(p.SurfaceHover)
			}
			brBtn.PopoverMenuButton(func(m *ui.Menu) {
				renderBranchMenu(m, recentTree, domBranch, a.branchCoverage, func(target string) {
					a.TriggerBatchSwitch(target, false)
				})
				if len(staleTree.children) > 0 {
					m.Submenu("更早的分支", func(sub *ui.Menu) {
						renderBranchMenu(sub, staleTree, domBranch, a.branchCoverage, func(target string) {
							a.TriggerBatchSwitch(target, false)
						})
					})
				}
				if m.Item("切换全部仓库的分支…").Shortcut(ui.Cmd, ui.KeyB).Chosen() {
					a.openBranchSwitchModal()
				}
			})
			brBtn.Children(func() {
				drawIcon(c, icoBranch, 13, p.Accent)
				ui.Text(c, domBranch).FontSize(12.5).FontWeight(weightMedium).TextColor(p.TextPrimary).SingleLine()
				if totalChild > 0 && alignedCnt < totalChild {
					badgeBox(c, fmt.Sprintf("%d/%d", alignedCnt, totalChild), p.Amber, p.AmberSoft, ui.Color{}, 10.5, true)
				}
				drawIcon(c, icoChevronDown, 11, p.TextMuted)
			})

			// Quick Saved Branch Presets in Top Bar (if any exist)
			for idx, preset := range a.cfg.BranchPresets {
				if idx >= 2 {
					break
				}
				prCopy := preset
				miniBtn(c.Key("top-pr-"+prCopy.Name), p, fmt.Sprintf("%s (%d)", prCopy.Name, len(prCopy.RepoBranches)), func() {
					a.ApplyBranchPreset(prCopy, true)
				})
			}
		})

		ui.Spacer(c)

		// Center: Magpie-style Segmented View Switcher
		ui.Row(c).Padding(3).Gap(2).Radius(8).Background(p.Surface3).AlignItems(ui.Center).Children(func() {
			segItem := func(tab NavTab, label string, badge int, badgeCol, badgeBg ui.Color) {
				active := a.activeTab == tab
				bg := ui.Transparent
				fg := p.TextControl
				weight := weightMedium
				if active {
					bg = p.Surface
					if p.IsDark {
						bg = p.Surface2
					}
					fg = p.TextPrimary
					weight = weightSemibold
				}
				kc := c.Key("seg-" + string(tab))
				el := ui.Row(kc).Padding(4, 12).Gap(5).Radius(6).Background(bg).Cursor(ui.CursorPointer).AlignItems(ui.Center)
				if el.Hovered() && !active {
					fg = p.TextPrimary
				}
				if el.Clicked() {
					a.activeTab = tab
				}
				el.Children(func() {
					ui.Text(kc, label).FontSize(12.5).FontWeight(weight).TextColor(fg).SingleLine()
					if badge > 0 {
						badgeBox(kc, fmt.Sprintf("%d", badge), badgeCol, badgeBg, ui.Color{}, 10.5, true)
					}
				})
			}

			segItem(TabChanges, "变更", totalChangedFiles, p.Amber, p.AmberSoft)
			segItem(TabCode, "文件", 0, p.Accent, p.AccentSoft)
			segItem(TabOverview, "仓库", 0, p.Accent, p.AccentSoft)
			segItem(TabMerge, "合并", conflictPlanCnt, p.Danger, p.DangerSoft)
			if a.maniCfg != nil && len(a.maniCfg.Tasks) > 0 {
				segItem(TabMani, "脚本", 0, p.Accent, p.AccentSoft)
			}
		})

		ui.Spacer(c)

		// Right: High-Frequency Daily Controls + Collapsible "More"
		ui.Row(c).Gap(6).AlignItems(ui.Center).Children(func() {
			iconPill(c, p, icoPull, "拉取", false, func() {
				a.TriggerBatchPull()
			})

			pushLabel := "推送"
			if aheadRepos > 0 {
				pushLabel = fmt.Sprintf("推送 (%d)", aheadRepos)
			}
			iconPill(c, p, icoPush, pushLabel, aheadRepos > 0, func() {
				a.TriggerPushAllAhead()
			})

			iconPill(c, p, icoSearch, "搜索 ⌘K", false, func() {
				a.showCmdPalette = true
				a.cmdPaletteQuery = ""
				a.cmdInputHandle.Focus()
			})

			// Collapsible "More" button for less-frequent workspace actions
			ghostBtn(c, p, icoMore, "", a.showTopMoreMenu, func() {
				a.showTopMoreMenu = !a.showTopMoreMenu
			})

			themeIcon := icoMoon
			if p.IsDark {
				themeIcon = icoSun
			}
			ghostBtn(c, p, themeIcon, "", false, func() {
				a.toggleTheme()
			})

			ghostBtn(c, p, icoSettings, "", a.showSettings, func() {
				a.showSettings = true
			})
		})
	})
}

// buildCollapsibleMoreToolbar renders low-frequency batch operations only when expanded.
func (a *App) buildCollapsibleMoreToolbar(c *ui.Context, p Palette) {
	ui.Row(c).Padding(7, 16).Gap(8).Background(p.RailBg).BorderWidth(0, 0, 1, 0).BorderColor(p.Border).AlignItems(ui.Center).Children(func() {
		ui.Text(c, "批量操作：").FontSize(11.5).TextColor(p.TextMuted)
		iconPill(c, p, icoRefresh, "刷新状态 (⌘R)", false, func() {
			a.RefreshAsync()
			a.showTopMoreMenu = false
		})
		miniBtn(c, p, "获取远端分支 (Fetch)", func() {
			a.TriggerBatchFetch()
			a.showTopMoreMenu = false
		})
		miniBtn(c, p, "全部切回默认主分支", func() {
			a.TriggerSwitchToMain()
			a.showTopMoreMenu = false
		})
		if a.maniCfg != nil && len(a.maniCfg.Tasks) > 0 {
			miniBtn(c, p, "执行批量脚本 (mani.yaml)", func() {
				a.activeTab = TabMani
				a.showTopMoreMenu = false
			})
		}

		ui.Spacer(c)

		// Repo filter pills
		ui.Text(c, "只看单个仓库：").FontSize(11.5).TextColor(p.TextMuted)
		miniBtn(c, p, "全部", func() {
			a.repoFilterID = ""
		})
		for _, st := range a.statuses {
			stCopy := st
			active := a.repoFilterID == st.Repo.ID
			pill(c.Key("more-flt-"+st.Repo.ID), p, st.Repo.Name, active, func() {
				if a.repoFilterID == stCopy.Repo.ID {
					a.repoFilterID = ""
				} else {
					a.repoFilterID = stCopy.Repo.ID
				}
			})
		}
	})
}

func (a *App) buildStatusBar(c *ui.Context, p Palette) {
	statusMsg := a.busyLabel
	msgColor := p.Accent
	if statusMsg == "" && a.toastMessage != "" && time.Now().Before(a.toastExpires) {
		statusMsg = a.toastMessage
		switch a.toastKind {
		case "err":
			msgColor = p.Danger
		case "warn":
			msgColor = p.Amber
		default:
			msgColor = p.Green
		}
	}
	if statusMsg == "" {
		statusMsg = fmt.Sprintf("实时同步中 · %s", a.lastRefresh.Format("15:04:05"))
		msgColor = p.TextMuted
	}

	ui.Row(c).Height(25).FillWidth().Padding(0, 14).Background(p.RailBg).BorderWidth(1, 0, 0, 0).BorderColor(p.Border).AlignItems(ui.Center).Children(func() {
		ui.Row(c).Gap(8).AlignItems(ui.Center).Children(func() {
			ui.Text(c, displayPath(a.cfg.WorkspacePath)).FontSize(11).TextColor(p.TextMuted)
			if a.repoFilterID != "" {
				badgeBox(c, "当前筛选: "+a.repoFilterID, p.Accent, p.AccentSoft, ui.Color{}, 10, true)
				miniBtn(c, p, "显示全部仓库", func() {
					a.repoFilterID = ""
				})
			}
		})

		ui.Spacer(c)
		ui.Text(c, statusMsg).FontSize(11).FontWeight(weightMedium).TextColor(msgColor)
		ui.Spacer(c)

		ui.Row(c).Gap(8).AlignItems(ui.Center).Children(func() {
			if len(a.lastBatchResults) > 0 {
				miniBtn(c, p, fmt.Sprintf("操作记录 (%d)", len(a.lastBatchResults)), func() {
					a.showOpLog = true
				})
			}
			hint := "⌘1-4 切换视图 · ⌘K 搜索 · ⌘B 切换分支 · ⌘G 生成说明 · ⌘Enter 提交 · ⌘S 保存"
			if a.activeTab == TabChanges {
				hint = "↑↓ 选择 · 空格勾选 · S/U 暂存 · ⌘G 说明 · ⌘Enter 提交"
			}
			ui.Text(c, hint).FontSize(10.5).TextColor(p.TextMuted)
		})
	})
}
