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
	cfg.Theme = normalizeTheme(cfg.Theme)
	mygo.Theme.SetSource(nativeThemeSource(cfg.Theme))
	app := NewApp(cfg)

	mygo.App.WhenReady(func() {
		app.systemDark = mygo.Theme.IsDark()
		mygo.Theme.OnUpdated(func() {
			app.systemDark = mygo.Theme.IsDark()
			if app.win != nil {
				app.win.Invalidate()
			}
		})
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
		app.win.OnClosed(func() {
			app.closeTerminal()
			app.shutdownUpdates()
			app.closePluginManager()
		})
		app.win.OnFocus(func() { app.runUI(func() { app.handleAppForeground() }) })
		app.win.OnBlur(func() { app.runUI(func() { app.handleAppBackground() }) })
		app.initPluginManager()
		if cfg.WorkspacePath != "" {
			app.LoadWorkspaceAsync(cfg.WorkspacePath)
		}
		app.startWorkspaceWatcher()
		app.startUpdateLoop()
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
	if a.cfg.Zoom >= 0.5 && a.cfg.Zoom <= 3.0 {
		c.SetZoom(a.cfg.Zoom)
	}
	a.registerShortcuts(c)

	ui.Column(c).Bind(&a.rootHandle).Fill().Background(p.AppBg).TextColor(p.TextPrimary).Children(func() {
		a.buildTopHeaderBar(c, p)

		// IDEA-style layout: left tree | editor above the bottom Git tool window.
		ui.Row(c).Grow(1).FillWidth().Children(func() {
			a.buildVerticalNav(c, p)

			ui.Column(c).Grow(1).FillHeight().Children(func() {
				editorArea := func() {
					center := func() {
						editor := func() {
							if a.gitDiffOpen {
								a.buildGitCommitDiffPane(c, p)
								return
							}
							if a.activeTab == TabCode {
								a.buildCodeRightPane(c, p)
							} else {
								a.buildDiffRightPane(c, p)
							}
						}
						ui.Column(c).FillHeight().Children(editor)
					}
					if a.activeTab == TabCode {
						ui.Split(c, &a.codeSplit,
							func() { a.buildCodeLeftPane(c, p) },
							center,
						).Fill()
					} else {
						ui.Split(c, &a.changesSplit,
							func() { a.buildChangesLeftPane(c, p) },
							center,
						).Fill()
					}
				}
				workArea := func() {
					if a.pluginPanel != "" && a.pluginMgr != nil {
						ui.SplitPercent(c, &a.pluginPanelPct,
							func() { ui.Column(c).FillHeight().Children(editorArea) },
							func() { a.buildPluginPanel(c, p) },
						).Fill()
					} else if a.maniAvailable() && a.showManiPanel {
						ui.SplitPercent(c, &a.maniContentPct,
							func() { ui.Column(c).FillHeight().Children(editorArea) },
							func() { a.buildManiPanel(c, p) },
						).Fill()
					} else {
						ui.Column(c).FillHeight().Children(editorArea)
					}
				}
				if a.showTerminalPanel {
					ui.SplitVerticalPercent(c, &a.terminalEditorPct,
						func() { ui.Column(c).FillHeight().Children(workArea) },
						func() { a.buildTerminalPanel(c, p) },
					).Fill()
				} else if a.showLogPanel {
					ui.SplitVerticalPercent(c, &a.logEditorPct,
						func() { ui.Column(c).FillHeight().Children(workArea) },
						func() { a.buildLogPanel(c, p) },
					).Fill()
				} else if a.showGitPanel {
					ui.SplitVerticalPercent(c, &a.gitEditorPct,
						func() { ui.Column(c).FillHeight().Children(workArea) },
						func() { a.buildGitView(c, p) },
					).Fill()
				} else {
					workArea()
				}
			})

			if a.maniAvailable() || len(a.pluginPanels()) > 0 {
				a.buildRightToolRail(c, p)
			}
		})

		a.buildStatusBar(c, p)
		a.buildNotificationLayer(c, p)

		// Modals & Dialogs
		a.buildCommandPaletteModal(c, p)
		a.buildGlobalSearchModal(c, p)
		a.buildBranchSwitchModal(c, p)
		a.buildSettingsModal(c, p)
		a.buildUpdateModal(c, p)
		a.buildDiscardConfirmModal(c, p)
		a.buildCodeTabCloseModal(c, p)
		a.buildCodeTabsCloseModal(c, p)
		a.buildUndoConfirmModal(c, p)
		a.buildFileHistoryModal(c, p)
		a.buildOpLogModal(c, p)
		a.buildManiResultModal(c, p)
		a.buildPluginMarketModal(c, p)
	})
}

func (a *App) registerShortcuts(c *ui.Context) {
	// Global window zoom shortcuts (Cmd +, Cmd -, Cmd 0)
	if c.Shortcut(ui.Cmd, ui.KeyEqual) || c.Shortcut(ui.Cmd|ui.Shift, ui.KeyEqual) {
		a.ZoomIn()
	}
	if c.Shortcut(ui.Cmd, ui.KeyMinus) {
		a.ZoomOut()
	}
	if c.Shortcut(ui.Cmd, ui.Key0) {
		a.ZoomReset()
	}

	// Dialogs own the keyboard: global actions must not fire behind them.
	if a.overlayOpen() {
		return
	}
	if c.Shortcut(ui.Cmd|ui.Shift, ui.KeyF) {
		a.openGlobalSearch()
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
	// The embedded terminal needs plain Space/S/U/arrows as shell input;
	// keep the changes pane's single-key shortcuts out of its way.
	terminalFocused := a.showTerminalPanel
	if a.activeTab == TabChanges {
		if c.Shortcut(ui.Cmd|ui.Shift, ui.KeyEnter) {
			a.TriggerCommit(true)
		} else if c.Shortcut(ui.Cmd, ui.KeyEnter) {
			a.TriggerCommit(false)
		}
		if !terminalFocused && !a.textFocusActive(c) {
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
		a.gitDiffOpen = false
	}
	if c.Shortcut(ui.Cmd, ui.Key2) {
		a.activeTab = TabCode
		a.gitDiffOpen = false
	}
	if c.Shortcut(ui.Cmd, ui.Key3) {
		a.showLogPanel = false
		a.showTerminalPanel = false
		a.showGitPanel = !a.showGitPanel
	}
	if c.Shortcut(ui.Cmd, ui.Key4) && a.maniAvailable() {
		a.showManiPanel = !a.showManiPanel
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
	width := float32(40)
	itemH := float32(32)
	padding := float32(4)
	if labels {
		width = 72
		itemH = 52
		padding = 6
	}
	nav := ui.Column(c).Width(width).FillHeight().Padding(padding).Gap(4).Background(p.RailBg).BorderWidth(0, 0, 0, 1).BorderColor(p.Border)
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
				a.gitDiffOpen = false
			}
			row.Children(func() {
				drawIcon(kc, icon, 15, iconCol)
				if labels {
					ui.Text(kc, label).FontSize(10.5).FontWeight(weightMedium).TextColor(fg).SingleLine()
				}
			})
		}
		item(TabChanges, icoCommit, "提交")
		item(TabCode, icoFile, "文件")
		// Push the bottom tool window and app utilities to the lower edge.
		ui.Spacer(c)
		ui.Box(c).Height(1).FillWidth().Background(p.Border)
		// Git is a bottom tool window, grouped with the utility actions.
		gitActive := a.showGitPanel
		gitBg := ui.Transparent
		gitFg := p.TextControl
		if gitActive {
			gitBg = p.Accent
			gitFg = p.AccentText
		}
		gitKc := c.Key("nav-git")
		gitRow := ui.Column(gitKc).Height(itemH).FillWidth().Radius(6).Background(gitBg).Cursor(ui.CursorPointer).AlignItems(ui.Center).Justify(ui.Center).Tooltip("Git").Label("Git")
		gitRow.PopoverMenu(a.navDisplayMenu)
		if labels {
			gitRow.Gap(2)
		}
		if gitRow.Hovered() && !gitActive {
			gitRow.Background(p.SurfaceHover)
			gitFg = p.TextPrimary
		}
		if gitRow.Clicked() {
			if a.showGitPanel {
				a.showGitPanel = false
			} else {
				a.showLogPanel = false
				a.showTerminalPanel = false
				a.showGitPanel = true
				a.expandGitForMerge()
			}
		}
		gitRow.Children(func() {
			drawIcon(gitKc, icoBranch, 15, gitFg)
			if labels {
				ui.Text(gitKc, "Git").FontSize(10.5).FontWeight(weightMedium).TextColor(gitFg).SingleLine()
			}
		})

		toolRow := func(key string, icon *ui.SVG, label string, active bool, onClick func()) {
			bg := ui.Transparent
			fg := p.TextControl
			if active {
				bg = p.Accent
				fg = p.AccentText
			}
			kc := c.Key(key)
			row := ui.Column(kc).Height(itemH).FillWidth().Radius(6).Background(bg).Cursor(ui.CursorPointer).
				AlignItems(ui.Center).Justify(ui.Center).Tooltip(label).Label(label)
			row.PopoverMenu(a.navDisplayMenu)
			if labels {
				row.Gap(2)
			}
			if row.Hovered() && !active {
				row.Background(p.SurfaceHover)
				fg = p.TextPrimary
			}
			if row.Clicked() {
				onClick()
			}
			row.Children(func() {
				drawIcon(kc, icon, 15, fg)
				if labels {
					ui.Text(kc, label).FontSize(10.5).FontWeight(weightMedium).TextColor(fg).SingleLine()
				}
			})
		}
		toolRow("nav-logs", icoLog, "日志", a.showLogPanel, func() {
			if a.showLogPanel {
				a.showLogPanel = false
			} else {
				a.showGitPanel = false
				a.showTerminalPanel = false
				a.showLogPanel = true
			}
		})
		toolRow("nav-terminal", icoTerminal, "终端", a.showTerminalPanel, func() {
			a.toggleTerminalPanel()
		})

		utility := func(key string, icon *ui.SVG, tooltip string, active bool, onClick func()) {
			bg := ui.Transparent
			fg := p.TextControl
			if active {
				bg = p.Surface2
				fg = p.TextPrimary
			}
			kc := c.Key(key)
			btn := ui.Row(kc).Height(32).FillWidth().Radius(6).Background(bg).Cursor(ui.CursorPointer).AlignItems(ui.Center).Justify(ui.Center).Tooltip(tooltip).Label(tooltip)
			if btn.Hovered() {
				btn.Background(p.SurfaceHover)
				fg = p.TextPrimary
			}
			if btn.Clicked() {
				onClick()
			}
			btn.Children(func() {
				drawIcon(kc, icon, 15, fg)
			})
		}
		themeIcon := icoSun
		switch a.cfg.Theme {
		case "dark", "darcula":
			themeIcon = icoMoon
		case "system":
			themeIcon = icoMonitor
		}
		themeKc := c.Key("nav-theme")
		themeBtn := ui.Row(themeKc).Height(32).FillWidth().Radius(6).Background(ui.Transparent).Cursor(ui.CursorPointer).AlignItems(ui.Center).Justify(ui.Center).Tooltip("外观主题：" + themeLabel(a.cfg.Theme)).Label("外观主题")
		themeBtn.PopoverMenuButton(func(m *ui.Menu) {
			for _, choice := range themeChoices {
				ch := choice
				if m.Item(ch.Label).Radio(a.cfg.Theme == ch.ID).Chosen() {
					a.setTheme(ch.ID)
				}
			}
		})
		if themeBtn.Hovered() {
			themeBtn.Background(p.SurfaceHover)
		}
		themeBtn.Children(func() {
			drawIcon(themeKc, themeIcon, 15, p.TextControl)
		})
		utility("nav-settings", icoSettings, "设置 (⌘,)", a.showSettings, func() {
			a.showSettings = true
		})
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

	ui.Row(c).Height(46).FillWidth().Padding(0, 14, 0, leftClearance).Background(p.RailBg).BorderWidth(0, 0, 1, 0).BorderColor(p.Border).AlignItems(ui.Center).DragWindow().Children(func() {
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

			pullTip := "拉取全部仓库更新"
			iconAction(c, p, "top-pull", icoPull, pullTip, false, func() {
				a.TriggerBatchPull()
			})
			pushTip := "推送待推送仓库"
			if aheadRepos > 0 {
				pushTip = fmt.Sprintf("推送 %d 个待推送仓库", aheadRepos)
			}
			iconAction(c, p, "top-push", icoPush, pushTip, aheadRepos > 0, func() {
				a.TriggerPushAllAhead()
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
		switch {
		case a.updateInstalling:
			pill(c, p, "更新中", false, func() { a.updateShowDialog = true })
		case a.updatePending != nil && !a.updateIsSkipped(a.updatePending.Version):
			up := a.updatePending
			pill(c, p, "新版本 v"+up.Version, true, func() {
				a.updateShowDialog = true
				a.updatePromptedFor = up.Version
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
			hint := "⌘1 提交 · ⌘2 文件 · ⌘3 Git · ⌘4 Mani · ⇧⌘F 全局搜索 · ⌘B 分支 · ⌘Enter 提交"
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
	card := ui.Column(c).Width(340).Padding(16).Gap(8).Radius(8).Background(p.Surface).Border(1, p.Border).Shadow(0, 6, 18, 0, shadow)
	if clickable && !running {
		card.MinHeight(96).Justify(ui.SpaceBetween)
	} else if running {
		card.MinHeight(68)
	} else {
		card.MinHeight(60)
	}
	if clickable {
		card.Cursor(ui.CursorPointer)
	}
	card.Children(func() {
		ui.Row(c).Gap(8).AlignItems(ui.Start).Children(func() {
			ui.Row(c).Gap(8).AlignItems(ui.Center).Grow(1).Children(func() {
				drawIcon(c, icon, 14, iconCol)
				textSize := float32(12)
				if running {
					textSize = 11.5
				}
				ui.Text(c, message).FontSize(textSize).FontWeight(weightMedium).TextColor(p.TextPrimary).Grow(1).MaxLines(2)
			})
			if !running || a.commandCancel != nil {
				xb := ui.Box(c).Size(18, 18).Radius(4).Center().Cursor(ui.CursorPointer)
				tip := "关闭"
				if running {
					tip = "停止当前操作"
				}
				xb.Tooltip(tip)
				if xb.Hovered() {
					xb.Background(p.SurfaceHover)
				}
				if xb.Clicked() {
					closed = true
					if running {
						a.cancelCommand()
					} else {
						a.notify = nil
					}
				}
				xb.Children(func() {
					col := p.TextMuted
					if xb.Hovered() {
						col = p.TextPrimary
						if running {
							col = p.Danger
						}
					}
					drawIcon(c, icoClose, 10, col)
				})
			}
		})
		if running {
			ui.Progress(c, -1).Height(3).FillWidth()
		} else if clickable {
			ui.Row(c).Gap(2).AlignItems(ui.Center).Cursor(ui.CursorPointer).Children(func() {
				ui.Text(c, "查看详情").FontSize(11.5).FontWeight(weightMedium).TextColor(p.AccentSoftText)
				drawIcon(c, icoChevronRight, 11, p.Accent)
			})
		}
	})
	if !closed && clickable && card.Clicked() {
		a.showOpLog = true
		a.notify = nil
	}
}
