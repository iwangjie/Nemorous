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
	wsPath := a.cfg.WorkspacePath

	// FSEvents (macOS) / ReadDirectoryChangesW (Windows) / inotify (Linux)
	// deliver coarse hints in one native stream. HintsOnly keeps no snapshot and
	// stats nothing, so idle cost is near zero; git status runs only on a hint.
	go func() {
		for ctx.Err() == nil {
			w, err := mygo.WatchFiles(ctx, wsPath, mygo.FileWatchOptions{
				Recursive: true,
				HintsOnly: true,
				Debounce:  300 * time.Millisecond,
				MaxDelay:  1500 * time.Millisecond,
				Ignore: func(p string, isDir bool) bool {
					if !isDir {
						return false
					}
					base := path.Base(p)
					slash := filepath.ToSlash(p)
					switch base {
					case "node_modules", "target", "dist", "build", ".idea", ".gradle", "www", "logs_cache":
						return true
					case "objects", "lfs":
						return strings.Contains(slash, "/.git/") || strings.HasPrefix(slash, ".git/")
					}
					return false
				},
			})
			if err != nil {
				select {
				case <-ctx.Done():
					return
				case <-time.After(time.Second):
					continue
				}
			}
			for {
				events, err := w.Next(ctx)
				if err != nil {
					w.Close()
					break
				}
				ids, listFiles, workspaceLevel := a.classifyWatchHints(events)
				switch {
				case workspaceLevel:
					a.runUI(func() { a.RefreshWorkspaceAsync() })
				case len(ids) > 0:
					a.runUI(func() { a.RefreshReposAsync(ids, listFiles) })
				}
			}
			if ctx.Err() != nil {
				return
			}
		}
	}()

	// A five-minute safety net for filesystems that lose FSEvents; events do the work.
	go func() {
		ticker := time.NewTicker(5 * time.Minute)
		defer ticker.Stop()
		for {
			select {
			case <-ctx.Done():
				return
			case <-ticker.C:
				a.runUI(func() {
					if time.Since(a.lastRefresh) > 5*time.Minute {
						a.RefreshWorkspaceAsync()
					}
				})
			}
		}
	}()
}

// classifyWatchHints maps coarse directory hints to repositories. Workspace-level
// or unknown paths need a full rediscovery; hints inside .git need status only.
func (a *App) classifyWatchHints(events []mygo.FileEvent) (repoIDs map[string]bool, listFiles bool, workspaceLevel bool) {
	repos := a.reposSnapshot()
	repoIDs = make(map[string]bool)
	for _, ev := range events {
		rel := strings.TrimPrefix(filepath.ToSlash(ev.Path), "./")
		if rel == "" || rel == "." {
			return nil, false, true
		}
		matched := ""
		matchedID := ""
		for _, repo := range repos {
			repoRel := strings.TrimPrefix(filepath.ToSlash(repo.RelPath), "./")
			if repoRel == "." || repoRel == "" {
				continue
			}
			if rel == repoRel || strings.HasPrefix(rel, repoRel+"/") {
				if len(repoRel) > len(matched) {
					matched = repoRel
					matchedID = repo.ID
				}
			}
		}
		if matchedID == "" {
			return nil, false, true
		}
		repoIDs[matchedID] = true
		if !strings.Contains(rel, "/.git") && !strings.HasPrefix(rel, ".git") {
			listFiles = true
		}
	}
	return repoIDs, listFiles, false
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

		// Main Content Stage: left tool rail + the active view.
		ui.Row(c).Grow(1).FillWidth().Children(func() {
			a.buildVerticalNav(c, p)
			ui.Column(c).Grow(1).FillHeight().Children(func() {
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
		})

		a.buildStatusBar(c, p)
		a.buildNotificationLayer(c, p)

		// Modals & Dialogs
		a.buildCommandPaletteModal(c, p)
		a.buildBranchSwitchModal(c, p)
		a.buildSettingsModal(c, p)
		a.buildDiscardConfirmModal(c, p)
		a.buildCodeTabCloseModal(c, p)
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

// navDisplayMenu toggles the vertical rail between icons and icons + labels.
func (a *App) navDisplayMenu(m *ui.Menu) {
	if m.Item("仅图标").Radio(!a.cfg.NavShowLabels).Chosen() {
		a.cfg.NavShowLabels = false
		_ = a.cfg.Save()
	}
	if m.Item("图标 + 文字").Radio(a.cfg.NavShowLabels).Chosen() {
		a.cfg.NavShowLabels = true
		_ = a.cfg.Save()
	}
}

// buildVerticalNav renders the primary views as an IDEA-style tool window rail
// on the left. Icons only by default; right-click switches to icons + labels.
func (a *App) buildVerticalNav(c *ui.Context, p Palette) {
	labels := a.cfg.NavShowLabels
	width := float32(52)
	itemH := float32(32)
	if labels {
		width = 72
		itemH = 52
	}
	nav := ui.Column(c).Width(width).FillHeight().Padding(6).Gap(4).Background(p.RailBg).BorderWidth(0, 0, 0, 1).BorderColor(p.Border)
	nav.PopoverMenu(a.navDisplayMenu)
	nav.Children(func() {
		item := func(tab NavTab, icon *ui.SVG, label string) {
			active := a.activeTab == tab
			bg := ui.Transparent
			fg := p.TextControl
			iconCol := fg
			if active {
				bg = p.Accent
				fg = p.AccentText
				iconCol = p.AccentText
			}
			kc := c.Key("nav-" + string(tab))
			row := ui.Column(kc).Height(itemH).FillWidth().Radius(6).Background(bg).Cursor(ui.CursorPointer).AlignItems(ui.Center).Justify(ui.Center).Tooltip(label).Label(label)
			row.PopoverMenu(a.navDisplayMenu)
			if labels {
				row.Gap(2)
			}
			if row.Hovered() && !active {
				row.Background(p.SurfaceHover)
			}
			if row.Clicked() {
				a.activeTab = tab
			}
			row.Children(func() {
				drawIcon(kc, icon, 15, iconCol)
				if labels {
					ui.Text(kc, label).FontSize(10.5).FontWeight(weightMedium).TextColor(fg).SingleLine()
				}
			})
		}
		item(TabChanges, icoCommit, "变更")
		item(TabCode, icoFile, "文件")
		item(TabOverview, icoGrid, "仓库")
		item(TabMerge, icoMerge, "合并")
		if a.maniCfg != nil && len(a.maniCfg.Tasks) > 0 {
			item(TabMani, icoTerminal, "脚本")
		}
	})
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
	_, _, aheadRepos, _, _ := a.SummaryCounts()

	ui.Row(c).Height(46).FillWidth().Padding(0, 14, 0, leftClearance).Background(p.Surface).BorderWidth(0, 0, 1, 0).BorderColor(p.Border).AlignItems(ui.Center).DragWindow().Children(func() {
		// Left: workspace and branch context.
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

			// Branch Switcher Pill: opens the structured branch picker popover.
			brBtn := ui.Row(c).Padding(5, 10).Gap(6).Radius(6).Background(p.Surface2).Cursor(ui.CursorPointer).AlignItems(ui.Center)
			if brBtn.Hovered() || a.showBranchPicker {
				brBtn.Background(p.SurfaceHover)
			}
			brBtn.Children(func() {
				drawIcon(c, icoBranch, 13, p.Accent)
				ui.Text(c, domBranch).FontSize(12.5).FontWeight(weightMedium).TextColor(p.TextPrimary).SingleLine()
				if totalChild > 0 && alignedCnt < totalChild {
					badgeBox(c, fmt.Sprintf("%d/%d", alignedCnt, totalChild), p.Amber, p.AmberSoft, ui.Color{}, 10.5, true)
				}
				drawIcon(c, icoChevronDown, 11, p.TextMuted)
			})
			if brBtn.Clicked() {
				a.showBranchPicker = !a.showBranchPicker
				if a.showBranchPicker {
					a.branchPickerQuery = ""
					a.branchPickerRepoID = ""
				}
			}
			a.buildBranchPicker(c, p, brBtn, domBranch)

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
	})
}

func (a *App) buildStatusBar(c *ui.Context, p Palette) {
	ui.Row(c).Height(25).FillWidth().Padding(0, 14).Background(p.RailBg).BorderWidth(1, 0, 0, 0).BorderColor(p.Border).AlignItems(ui.Center).Children(func() {
		ui.Row(c).Gap(8).AlignItems(ui.Center).Children(func() {
			ui.Text(c, displayPath(a.cfg.WorkspacePath)).FontSize(11).TextColor(p.TextMuted)
		})

		ui.Spacer(c)

		ui.Row(c).Gap(8).AlignItems(ui.Center).Children(func() {
			if len(a.lastBatchResults) > 0 {
				miniBtn(c, p, fmt.Sprintf("操作记录 (%d)", len(a.lastBatchResults)), func() {
					a.showOpLog = true
				})
			}
			hint := "⌘1-4 切换视图 · ⌘K 搜索 · ⌘B 切换分支 · ⌘Enter 提交 · ⌘S 保存"
			if a.activeTab == TabChanges {
				hint = "↑↓ 选择 · 空格勾选 · S/U 暂存 · ⌘Enter 提交"
			}
			ui.Text(c, hint).FontSize(10.5).TextColor(p.TextMuted)
		})
	})
}

// buildNotificationLayer floats the command/result card above the status bar.
func (a *App) buildNotificationLayer(c *ui.Context, p Palette) {
	ui.Box(c).Absolute().Right(14).Bottom(33).Width(340).Children(func() {
		a.buildOperationNotification(c, p)
	})
}

// buildOperationNotification shows a running command's indeterminate progress,
// then its result; batch results open the operation log when clicked.
func (a *App) buildOperationNotification(c *ui.Context, p Palette) {
	now := time.Now()
	running := a.busyLabel != "" && !a.isRefreshing
	if running {
		c.After(80 * time.Millisecond)
	} else {
		if a.notify == nil {
			return
		}
		if now.After(a.notify.Expires) {
			a.notify = nil
			return
		}
		c.After(time.Until(a.notify.Expires) + 20*time.Millisecond)
	}

	kind := "info"
	message := a.busyLabel
	clickable := false
	if !running {
		kind = a.notify.Kind
		message = a.notify.Message
		clickable = a.notify.Clickable
	}

	icon, iconCol := icoRefresh, p.Accent
	switch kind {
	case "ok":
		icon, iconCol = icoCheck, p.Green
	case "warn":
		icon, iconCol = icoAlert, p.Amber
	case "err":
		icon, iconCol = icoAlert, p.Danger
	}

	shadow := ui.RGBA(0, 0, 0, 0.16)
	if p.IsDark {
		shadow = ui.RGBA(0, 0, 0, 0.42)
	}

	closed := false
	card := ui.Column(c).Width(340).Padding(10, 12).Gap(6).Radius(8).Background(p.Surface).Border(1, p.Border).Shadow(0, 6, 18, 0, shadow)
	if clickable {
		card.Cursor(ui.CursorPointer)
	}
	card.Children(func() {
		ui.Row(c).Gap(8).AlignItems(ui.Center).Children(func() {
			drawIcon(c, icon, 14, iconCol)
			ui.Text(c, message).FontSize(11.5).FontWeight(weightMedium).TextColor(p.TextPrimary).Grow(1).MaxLines(2)
			if !running {
				xb := ui.Box(c).Size(16, 16).Radius(3).Center().Cursor(ui.CursorPointer)
				if xb.Hovered() {
					xb.Background(p.SurfaceHover)
				}
				if xb.Clicked() {
					closed = true
					a.notify = nil
				}
				xb.Children(func() {
					drawIcon(c, icoClose, 10, p.TextMuted)
				})
			}
		})
		if running {
			ui.Progress(c, -1).Height(3).FillWidth()
		} else if clickable {
			ui.Text(c, "点击查看详情").FontSize(10.5).TextColor(p.Accent)
		}
	})
	if !closed && clickable && card.Clicked() {
		a.showOpLog = true
		a.notify = nil
	}
}
