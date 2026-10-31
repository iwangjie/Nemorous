package main

import (
	"context"
	"fmt"
	"math"
	"path/filepath"
	"strings"
	"time"

	"github.com/egoist/mygo"
	"github.com/egoist/mygo/ui"
	"github.com/egoist/mygo/ui/editor"
	"github.com/egoist/mygo/ui/markdown"
	"github.com/iwangjie/nemorous/internal/config"
	"github.com/iwangjie/nemorous/internal/mani"
	"github.com/iwangjie/nemorous/internal/plugins"
)

// initPluginManager opens the per-user plugin store and starts the
// enabled plugins. Called once the window exists, so plugin renders can
// schedule a frame.
func (a *App) initPluginManager() {
	root := filepath.Join(filepath.Dir(config.ConfigPath()), "plugins")
	mgr, err := plugins.NewManager(plugins.Options{
		Root:                root,
		AppVersion:          updateVersion(),
		AutoInstallDefaults: true,
		Invalidate: func() {
			a.runUI(func() {
				if a.win != nil {
					a.win.Invalidate()
				}
			})
		},
		Notify: func(kind, message string) {
			a.runUI(func() { a.showToast(kind, message) })
		},
		ActiveFile: func() *plugins.ActiveFile {
			return a.pluginActive.Load()
		},
		OpenURL: func(raw string) error {
			if !strings.HasPrefix(raw, "http://") && !strings.HasPrefix(raw, "https://") {
				return fmt.Errorf("only http and https URLs are allowed")
			}
			return openPath(raw)
		},
		OpenTerminal: func(command []string, dir, title string) error {
			a.runUI(func() { a.openPluginTerminal(command, dir, title) })
			return nil
		},
		CloseTerminal: func() error {
			a.runUI(func() { a.closePluginTerminal() })
			return nil
		},
		FileWritten: func(path string) {
			a.runUI(func() {
				for _, tab := range a.codeTabs {
					if tab.file.AbsPath == path {
						a.reloadCodeTab(tab)
					}
				}
				a.RefreshAsync()
			})
		},
	})
	if err != nil {
		a.logRuntimeDetail("err", "插件引擎初始化失败", err.Error())
		return
	}
	a.pluginMgr = mgr
	mgr.SetWorkspace(a.cfg.WorkspacePath, nil)
	mgr.Load()
}

func (a *App) closePluginManager() {
	if a.pluginMgr != nil {
		a.pluginMgr.Close()
	}
}

// syncPluginsWorkspace publishes the discovered repositories to plugins.
// Plugin panels are re-opened (which lets them rescan) after a change.
func (a *App) syncPluginsWorkspace(repos []mani.DiscoveredRepo) {
	if a.pluginMgr == nil {
		return
	}
	rs := make([]plugins.Repo, 0, len(repos))
	for _, r := range repos {
		rs = append(rs, plugins.Repo{ID: r.ID, Name: r.Name, Path: r.AbsPath})
	}
	a.pluginMgr.SetWorkspace(a.cfg.WorkspacePath, rs)
	if a.pluginPanel != "" {
		if _, ok := a.pluginMgr.Panel(a.pluginPanel); !ok {
			a.pluginPanel = ""
			return
		}
		a.pluginMgr.Dispatch(a.pluginPanel, "open", nil)
	}
}

func (a *App) pluginPanels() []plugins.PanelInfo {
	if a.pluginMgr == nil {
		return nil
	}
	return a.pluginMgr.Panels()
}

func (a *App) pluginCommands() []plugins.CommandInfo {
	if a.pluginMgr == nil {
		return nil
	}
	return a.pluginMgr.Commands()
}

func (a *App) pluginPanelRef(pluginID string) string {
	for _, panel := range a.pluginPanels() {
		if panel.PluginID == pluginID {
			return panel.Ref
		}
	}
	return ""
}

func (a *App) openPluginPanel(ref string) {
	if a.pluginMgr == nil {
		return
	}
	if _, ok := a.pluginMgr.Panel(ref); !ok {
		return
	}
	info, _ := a.pluginMgr.Panel(ref)
	if a.pluginPanelPct > 50 && (info.PluginID == "markdown" || info.PluginID == "csv" || info.PluginID == "gitlab") {
		a.pluginPanelPct = 50
	}
	a.pluginPanel = ref
	a.showManiPanel = false
	a.pluginActiveKey = ""
	a.updatePluginActiveFile()
	a.pluginMgr.Dispatch(ref, "open", nil)
}

func (a *App) togglePluginPanel(ref string) {
	if a.pluginPanel == ref {
		a.pluginPanel = ""
		return
	}
	a.openPluginPanel(ref)
}

func (a *App) runPluginCommand(cmd plugins.CommandInfo) {
	if a.pluginMgr == nil {
		return
	}
	if ref := a.pluginPanelRef(cmd.PluginID); ref != "" {
		a.openPluginPanel(ref)
	}
	a.pluginMgr.DispatchCommand(cmd.PluginID, cmd.Action)
	if a.win != nil {
		a.win.Invalidate()
	}
}

// updatePluginActiveFile publishes a cheap snapshot of the file in the code
// editor. It only rebuilds when the active file changes; Markdown preview
// reads its own draft while the user types.
func (a *App) updatePluginActiveFile() bool {
	tab := a.activeCodeTab()
	if tab == nil || tab.file.RelPath == "" {
		if a.pluginActiveKey != "" {
			a.pluginActive.Store(nil)
			a.pluginActiveKey = ""
			return true
		}
		return false
	}
	key := tab.file.RepoID + "\x00" + tab.file.RelPath
	if key == a.pluginActiveKey {
		return false
	}
	content := ""
	if tab.state != nil && tab.state.Buffer != nil {
		content = editorText(tab.state.Buffer)
	}
	if len(content) > 512<<10 {
		content = content[:512<<10]
	}
	a.pluginActive.Store(&plugins.ActiveFile{
		RepoID: tab.file.RepoID, RepoName: tab.file.RepoName,
		Path: tab.file.AbsPath, RelPath: tab.file.RelPath,
		Name: filepath.Base(tab.file.RelPath), Content: content,
	})
	a.pluginActiveKey = key
	return true
}

// editorPluginForFile finds an installed editor contribution for a file.
func (a *App) editorPluginForFile(path string) (plugins.EditorInfo, bool) {
	if a.pluginMgr == nil {
		return plugins.EditorInfo{}, false
	}
	return a.pluginMgr.EditorForFile(path)
}

func (a *App) pluginEditorTextKey(repoID, relPath string) string {
	return repoID + "\x00" + relPath
}

func (a *App) pluginEditorInTextMode(repoID, relPath string) bool {
	return a.pluginEditorText[a.pluginEditorTextKey(repoID, relPath)]
}

func (a *App) setPluginEditorTextMode(repoID, relPath string, text bool) {
	if a.pluginEditorText == nil {
		a.pluginEditorText = make(map[string]bool)
	}
	key := a.pluginEditorTextKey(repoID, relPath)
	if text {
		a.pluginEditorText[key] = true
	} else {
		delete(a.pluginEditorText, key)
	}
	a.pluginActiveKey = ""
}

// syncPluginEditor follows the active editor tab and asks the plugin to load
// the new file before the next render.
func (a *App) syncPluginEditor(info plugins.EditorInfo) {
	if a.pluginMgr == nil {
		return
	}
	if a.pluginEditorRef != info.Ref {
		a.pluginEditorRef = info.Ref
		a.pluginActiveKey = ""
		a.updatePluginActiveFile()
		a.pluginMgr.Dispatch(info.Ref, "open", nil)
		return
	}
	if a.updatePluginActiveFile() {
		a.pluginMgr.Dispatch(info.Ref, "activeFileChanged", nil)
	}
}

// buildPluginEditorView draws the plugin-owned editor surface in the main
// editor area, falling back to plain text on demand or on plugin failure.
func (a *App) buildPluginEditorView(c *ui.Context, p Palette, ref string) {
	snap := a.pluginMgr.Snapshot(ref)
	if snap == nil || snap.Root == nil {
		ui.Column(c).Grow(1).Center().Gap(6).Children(func() {
			ui.Spinner(c).Size(16, 16).Label("正在启动编辑器插件").TextColor(p.TextMuted)
		})
		return
	}
	if snap.Err != "" {
		if tab := a.activeCodeTab(); tab != nil {
			tab := tab
			ui.Column(c).Grow(1).Center().Gap(8).Padding(20).Children(func() {
				drawIcon(c, icoAlert, 24, p.Amber)
				ui.Text(c, "编辑器插件加载失败").FontSize(12.5).FontWeight(weightSemibold).TextColor(p.TextPrimary)
				ui.Text(c, snap.Err).FontSize(10.5).TextColor(p.TextMuted).MaxLines(4)
				pill(c, p, "以文本打开", false, func() {
					a.setPluginEditorTextMode(tab.file.RepoID, tab.file.RelPath, true)
					a.pluginEditorRef = ""
				})
			})
		}
		return
	}
	ui.Column(c).Grow(1).FillHeight().Children(func() {
		a.renderPluginNode(c, p, ref, snap.Root, "editor")
	})
}

// activePluginEditorRef is the editor plugin that should receive Save.
func (a *App) activePluginEditorRef() string {
	tab := a.activeCodeTab()
	if tab == nil || a.pluginEditorInTextMode(tab.file.RepoID, tab.file.RelPath) {
		return ""
	}
	if info, ok := a.editorPluginForFile(tab.file.AbsPath); ok {
		return info.Ref
	}
	return ""
}

func (a *App) pluginIcon(panel plugins.PanelInfo) *ui.SVG {
	if panel.Icon == "maven" {
		return icoMaven
	}
	if len(panel.IconData) > 0 {
		key := panel.PluginID + "@" + panel.Version
		if icon, ok := a.pluginIconCache[key]; ok {
			return icon
		}
		if icon, err := ui.ParseSVG(panel.IconData); err == nil {
			a.pluginIconCache[key] = icon
			return icon
		}
	}
	return icoPlugin
}

func (a *App) buildPluginPanel(c *ui.Context, p Palette) {
	if a.pluginMgr == nil {
		return
	}
	if a.updatePluginActiveFile() && a.pluginPanel != "" {
		a.pluginMgr.Dispatch(a.pluginPanel, "activeFileChanged", nil)
	}
	ref := a.pluginPanel
	info, ok := a.pluginMgr.Panel(ref)
	if !ok {
		a.pluginPanel = ""
		return
	}
	snap := a.pluginMgr.Snapshot(ref)
	ui.Column(c).Grow(1).FillHeight().Background(p.RailBg).BorderWidth(0, 0, 0, 1).BorderColor(p.Border).Children(func() {
		ui.Row(c).Height(40).Padding(0, 10).Gap(6).Justify(ui.SpaceBetween).AlignItems(ui.Center).
			BorderWidth(0, 0, 1, 0).BorderColor(p.Border).Children(func() {
			drawIcon(c, a.pluginIcon(info), 14, p.Accent)
			ui.Text(c, info.Title).FontSize(12.5).FontWeight(weightBold).TextColor(p.TextPrimary)
			version := info.PluginName
			if info.Version != "" {
				version = "v" + info.Version
			}
			ui.Text(c, version).FontSize(10).TextColor(p.TextMuted).MaxLines(1).Grow(1)
			toolButton(c, p, "plugin-refresh-"+ref, icoRefresh, "刷新插件", false, true, func() {
				a.pluginMgr.Dispatch(ref, "refresh", nil)
			})
			toolButton(c, p, "plugin-close-"+ref, icoClose, "关闭", false, true, func() {
				a.pluginPanel = ""
			})
		})
		if snap == nil || snap.Root == nil {
			ui.Column(c).Grow(1).Center().Children(func() {
				ui.Text(c, "正在启动插件…").FontSize(11.5).TextColor(p.TextMuted)
			})
			return
		}
		ui.Column(c).Grow(1).FillHeight().Children(func() {
			a.renderPluginNode(c, p, ref, snap.Root, "root")
		})
	})
}

func pluginToneColor(tone string, p Palette) ui.Color {
	switch tone {
	case "accent":
		return p.Accent
	case "ok":
		return p.Green
	case "warn":
		return p.Amber
	case "err":
		return p.Danger
	case "muted":
		return p.TextMuted
	default:
		return p.TextPrimary
	}
}

func pluginToneBg(tone string, p Palette) ui.Color {
	switch tone {
	case "ok":
		return p.GreenSoft
	case "warn":
		return p.AmberSoft
	case "err":
		return p.DangerSoft
	case "accent":
		return p.AccentSoft
	default:
		return p.Surface2
	}
}

func applyPluginBox(e ui.Element, n *plugins.Node) {
	if n.Grow {
		e.Grow(1)
	}
	if n.Width > 0 {
		e.Width(float32(n.Width))
	}
	if n.WidthPercent > 0 {
		e.WidthPercent(float32(n.WidthPercent))
	}
	if n.Height > 0 {
		e.Height(float32(n.Height))
	}
	if n.HeightPercent > 0 {
		e.HeightPercent(float32(n.HeightPercent))
	}
	if n.MaxHeight > 0 {
		e.MaxHeight(float32(n.MaxHeight))
	}
	if n.Gap > 0 {
		e.Gap(float32(n.Gap))
	}
	if n.Padding > 0 {
		e.Padding(float32(n.Padding))
	}
}

func (a *App) pluginButton(c *ui.Context, p Palette, ref string, n *plugins.Node, key string) {
	fg := p.TextPrimary
	bg := p.Surface2
	border := p.Border
	if n.Active || n.Primary {
		bg = p.Accent
		fg = p.AccentText
		border = p.Accent
	}
	if n.Disabled {
		bg = p.RailBg
		fg = p.TextMuted
		border = p.BorderSubtle
	}
	size := float32(11.5)
	padV, padH := float32(4), float32(9)
	radius := float32(5)
	if n.Small {
		size = 10.5
		padV, padH = 2, 7
		radius = 4
	}
	el := ui.Row(c.Key(key)).Padding(padV, padH).Radius(radius).Background(bg).Border(1, border).AlignItems(ui.Center).Shrink(0)
	el.Tooltip(n.Text).Label(n.Text)
	if !n.Disabled {
		el.Cursor(ui.CursorPointer)
		if el.Hovered() {
			if n.Active || n.Primary {
				el.Background(p.AccentHover)
			} else {
				el.Background(p.SurfaceHover)
			}
		}
		if el.Clicked() && n.Action != "" {
			a.pluginMgr.Dispatch(ref, n.Action, n.Payload)
		}
	}
	el.Children(func() {
		ui.Text(c, n.Text).FontSize(size).FontWeight(weightMedium).TextColor(fg).SingleLine()
	})
}

func (a *App) pluginMarkdownOptions(base string) *markdown.Options {
	if base != "" && a.pluginMgr != nil && !a.pluginMgr.PathAllowed(base) {
		base = ""
	}
	if base == "" {
		return &a.mdOpts
	}
	if a.pluginMarkdown == nil {
		a.pluginMarkdown = make(map[string]*markdown.Options)
	}
	if opts := a.pluginMarkdown[base]; opts != nil {
		return opts
	}
	load := markdown.FileImages(base)
	opts := &markdown.Options{}
	opts.LoadImage = func(raw string) (ui.ImageSource, error) {
		p := strings.TrimPrefix(raw, "file:")
		if !filepath.IsAbs(p) {
			p = filepath.Join(base, filepath.FromSlash(p))
		}
		p = filepath.Clean(p)
		if a.pluginMgr != nil && !a.pluginMgr.PathAllowed(p) {
			return nil, fmt.Errorf("image is outside the workspace")
		}
		return load(raw)
	}
	a.pluginMarkdown[base] = opts
	return opts
}

func (a *App) renderPluginNode(c *ui.Context, p Palette, ref string, n *plugins.Node, path string) {
	if n == nil {
		return
	}
	switch n.Type {
	case "column":
		children := func() {
			for i, child := range n.Children {
				a.renderPluginNode(c, p, ref, child, fmt.Sprintf("%s.%d", path, i))
			}
		}
		if n.Scroll {
			scroll := ui.Scroll(c)
			if n.MaxHeight > 0 {
				scroll.MaxHeight(float32(n.MaxHeight))
			} else {
				scroll.Grow(1)
			}
			scroll.Children(func() {
				inner := ui.Column(c).Gap(float32(n.Gap)).Padding(float32(n.Padding))
				inner.Children(children)
			})
			return
		}
		el := ui.Column(c)
		applyPluginBox(el, n)
		el.Children(children)
	case "card":
		el := ui.Column(c.Key(path)).Radius(8).Background(p.Surface2).Border(1, p.BorderSubtle)
		applyPluginBox(el, n)
		el.Children(func() {
			for i, child := range n.Children {
				a.renderPluginNode(c, p, ref, child, fmt.Sprintf("%s.%d", path, i))
			}
		})
	case "row":
		el := ui.Row(c).AlignItems(ui.Center).Wrap()
		applyPluginBox(el, n)
		if n.Scroll {
			el = ui.ScrollHorizontal(c).AlignItems(ui.Center)
			applyPluginBox(el, n)
		}
		el.Children(func() {
			for i, child := range n.Children {
				a.renderPluginNode(c, p, ref, child, fmt.Sprintf("%s.%d", path, i))
			}
		})
	case "text":
		size := float32(12)
		if n.Size > 0 {
			size = float32(n.Size)
		}
		t := ui.Text(c, n.Text).FontSize(size)
		if n.Mono {
			t.Font("monospace")
		}
		if n.Bold {
			t.FontWeight(weightSemibold)
		}
		t.TextColor(pluginToneColor(n.Tone, p))
		if n.Muted {
			t.TextColor(p.TextMuted)
		}
		if n.Grow {
			t.Grow(1)
		}
		if n.Width > 0 {
			t.Width(float32(n.Width))
		}
	case "button":
		a.pluginButton(c, p, ref, n, path)
	case "input":
		if n.Key == "" {
			return
		}
		ptr := a.pluginMgr.Input(ref, n.Key, n.Value)
		el := ui.TextInput(c.Key(path), ptr).Placeholder(n.Placeholder)
		if n.Password {
			el.Password()
		}
		if n.Grow {
			el.Grow(1)
		}
		if n.Width > 0 {
			el.Width(float32(n.Width))
		}
		if n.AutoFocus {
			el.AutoFocus()
		}
		if el.Changed() {
			action := n.Action
			if action == "" {
				action = "input"
			}
			a.pluginMgr.Dispatch(ref, action, map[string]any{"key": n.Key, "value": *ptr})
		}
	case "checkbox":
		row := ui.Row(c.Key(path)).Gap(6).AlignItems(ui.Center).Cursor(ui.CursorPointer)
		box := ui.Box(c.Key(path+".box")).Size(14, 14).Radius(3).Border(1, p.Border).Center()
		if n.Checked {
			box.Background(p.Accent)
			box.Border(1, p.Accent)
		} else {
			box.Background(p.Surface)
		}
		box.Children(func() {
			if n.Checked {
				drawIcon(c, icoCheck, 9, p.AccentText)
			}
		})
		label := ui.Text(c, n.Text).FontSize(11).TextColor(p.TextSecondary)
		if n.Grow {
			label.Grow(1)
		}
		if row.Clicked() && n.Action != "" {
			payload := map[string]any{"key": n.Key, "checked": !n.Checked}
			if n.Payload != nil {
				if m, ok := n.Payload.(map[string]any); ok {
					for k, v := range m {
						payload[k] = v
					}
				}
			}
			a.pluginMgr.Dispatch(ref, n.Action, payload)
		}
	case "list":
		scroll := ui.Scroll(c)
		if n.MaxHeight > 0 {
			scroll.MaxHeight(float32(n.MaxHeight))
		} else {
			scroll.Grow(1)
		}
		scroll.Children(func() {
			for i, item := range n.Items {
				key := fmt.Sprintf("%s.i%d", path, i)
				row := ui.Row(c.Key(key)).Height(22).Padding(0, float32(8+item.Depth*12)).Gap(6).AlignItems(ui.Center).Radius(3)
				if item.Selected {
					row.Background(p.AccentSoft)
				}
				if item.Action != "" {
					row.Cursor(ui.CursorPointer)
					if row.Hovered() && !item.Selected {
						row.Background(p.SurfaceHover)
					}
					if row.Clicked() {
						a.pluginMgr.Dispatch(ref, item.Action, item.Payload)
					}
				}
				row.Children(func() {
					t := ui.Text(c, item.Text).FontSize(10.5).TextColor(pluginToneColor(item.Tone, p)).SingleLine().Grow(1)
					if item.Mono {
						t.Font("monospace")
					}
					if item.Bold {
						t.FontWeight(weightSemibold)
					}
					if item.Detail != "" {
						ui.Text(c, item.Detail).FontSize(9.5).TextColor(p.TextMuted).SingleLine().Shrink(0)
					}
				})
			}
		})
	case "code":
		scroll := ui.ScrollBoth(c)
		if n.MaxHeight > 0 {
			scroll.MaxHeight(float32(n.MaxHeight))
		} else {
			scroll.Grow(1)
		}
		scroll.Background(p.Surface).Radius(5).Padding(6)
		key := ref + "::" + path
		st := a.pluginScroll[key]
		if st == nil {
			st = &ui.ScrollState{}
			a.pluginScroll[key] = st
		}
		scroll.TrackScroll(st)
		if n.Follow && a.pluginScrollText[key] != n.Text {
			a.pluginScrollText[key] = n.Text
			st.Y = math.MaxFloat32
		}
		scroll.Children(func() {
			line := ui.Text(c, n.Text).Font("monospace").FontSize(10).TextColor(p.TextSecondary)
			if !n.SoftWrap {
				line.NoWrap()
			}
		})
	case "codeeditor":
		if n.Key == "" {
			return
		}
		ptr := a.pluginMgr.Input(ref, n.Key, n.Value)
		if a.pluginCodeEditors == nil {
			a.pluginCodeEditors = make(map[string]*pluginCodeEditor)
		}
		key := ref + "::" + path
		rec := a.pluginCodeEditors[key]
		if rec == nil {
			buf := editor.NewBuffer(n.Value)
			if lang := editor.LanguageNamed(n.Language); lang != nil {
				buf.SetLanguage(lang)
			} else if lang := editor.LanguageFor(n.Language); lang != nil {
				buf.SetLanguage(lang)
			}
			rec = &pluginCodeEditor{
				state: &editor.State{
					Buffer:      buf,
					ReadOnly:    n.Disabled,
					Font:        ui.Font{Size: a.cfg.EditorFontSize},
					SoftWrap:    n.SoftWrap,
					Placeholder: n.Placeholder,
					Label:       n.Text,
				},
				version: buf.Version(),
			}
			a.pluginCodeEditors[key] = rec
		}
		view := editor.View(c, rec.state)
		applyPluginBox(view, n)
		if rec.state.Buffer.Version() != rec.version {
			rec.version = rec.state.Buffer.Version()
			*ptr = editorText(rec.state.Buffer)
			action := n.Action
			if action == "" {
				action = "input"
			}
			a.pluginMgr.Dispatch(ref, action, map[string]any{"key": n.Key, "value": *ptr})
		}
	case "textarea":
		if n.Key == "" {
			return
		}
		ptr := a.pluginMgr.Input(ref, n.Key, n.Value)
		el := ui.TextArea(c.Key(path), ptr)
		applyPluginBox(el, n)
		if n.AutoFocus {
			el.AutoFocus()
		}
		if el.Changed() {
			action := n.Action
			if action == "" {
				action = "input"
			}
			a.pluginMgr.Dispatch(ref, action, map[string]any{"key": n.Key, "value": *ptr})
		}
	case "markdown":
		scroll := ui.Scroll(c)
		if n.MaxHeight > 0 {
			scroll.MaxHeight(float32(n.MaxHeight))
		} else {
			scroll.Grow(1)
		}
		scroll.Padding(12)
		applyPluginBox(scroll, n)
		scroll.Children(func() {
			markdown.View(c, n.Text, a.pluginMarkdownOptions(n.BasePath)).FillWidth()
		})
	case "table":
		if a.pluginTables == nil {
			a.pluginTables = make(map[string]*ui.ListState)
		}
		st := a.pluginTables[ref+"::"+path]
		if st == nil {
			st = &ui.ListState{}
			a.pluginTables[ref+"::"+path] = st
		}
		cols := make([]ui.TableColumn, 0, len(n.Headers))
		for i, header := range n.Headers {
			align := ui.Start
			if i > 0 {
				align = ui.Start
			}
			cols = append(cols, ui.TableColumn{Title: header, Align: align})
		}
		if len(cols) == 0 {
			return
		}
		rows := n.Rows
		table := ui.Table(c, st, cols, len(rows), func(row, col int) {
			if row < 0 || row >= len(rows) || col < 0 || col >= len(rows[row]) {
				return
			}
			ui.Text(c, rows[row][col]).FontSize(10.5).SingleLine().Tooltip(rows[row][col])
		})
		applyPluginBox(table, n)
	case "bar":
		ui.Row(c).FillWidth().Gap(8).AlignItems(ui.Center).Children(func() {
			label := n.Text
			if label == "" {
				label = n.Detail
			}
			ui.Text(c, label).FontSize(10.5).TextColor(pluginToneColor(n.Tone, p)).Width(92).Shrink(0)
			ui.Progress(c, n.Fraction).Grow(1).Height(6)
			if n.Text != "" && n.Detail != "" {
				ui.Text(c, n.Detail).FontSize(9.5).TextColor(p.TextMuted).Shrink(0)
			}
		})
	case "badge":
		fg := pluginToneColor(n.Tone, p)
		bg := pluginToneBg(n.Tone, p)
		badgeBox(c, n.Text, fg, bg, p.BorderSubtle, 9.5, true)
	case "divider":
		if n.Vertical {
			ui.Box(c).Width(1).FillHeight().Background(p.Border)
		} else {
			ui.Box(c).Height(1).FillWidth().Background(p.Border)
		}
	case "spacer":
		ui.Spacer(c)
	case "spinner":
		ui.Row(c).Gap(6).AlignItems(ui.Center).Children(func() {
			ui.Text(c, "◌").FontSize(12).TextColor(p.Accent)
			ui.Text(c, n.Text).FontSize(10.5).TextColor(p.TextMuted)
		})
	}
}

// ---------- marketplace ----------

func (a *App) openPluginMarket() {
	if a.pluginMgr == nil {
		a.showToast("err", "插件引擎不可用（无法读写插件目录）")
		return
	}
	a.showPluginMarket = true
	a.pluginMarketTab = "market"
	a.pluginMarketQuery = ""
	a.pluginMarketErr = ""
	a.pluginMarketConfirm = ""
	a.pluginMarketDraft = a.pluginMgr.RegistryURL()
	if time.Since(a.pluginMgr.RegistryUpdatedAt()) > 10*time.Minute && !a.pluginRegistryLoading {
		a.refreshPluginRegistry()
	}
}

func (a *App) refreshPluginRegistry() {
	if a.pluginMgr == nil || a.pluginRegistryLoading {
		return
	}
	a.pluginRegistryLoading = true
	mgr := a.pluginMgr
	go func() {
		ctx, cancel := context.WithTimeout(context.Background(), 30*time.Second)
		defer cancel()
		err := mgr.RefreshRegistry(ctx)
		a.runUI(func() {
			a.pluginRegistryLoading = false
			if err != nil {
				a.pluginMarketErr = "远程市场刷新失败，正在使用内置索引：" + err.Error()
			} else {
				a.pluginMarketErr = ""
			}
			if a.win != nil {
				a.win.Invalidate()
			}
		})
	}()
}

func (a *App) installPluginEntry(entry plugins.RegistryEntry) {
	if a.pluginMgr == nil || a.pluginMarketBusy[entry.ID] {
		return
	}
	mgr := a.pluginMgr
	a.pluginMarketBusy[entry.ID] = true
	a.pluginMarketErr = ""
	go func() {
		ctx, cancel := context.WithTimeout(context.Background(), 5*time.Minute)
		defer cancel()
		var manifest *plugins.Manifest
		var err error
		if entry.Builtin {
			manifest, err = mgr.InstallBuiltin(ctx, entry.ID)
		} else {
			manifest, err = mgr.InstallRegistry(ctx, entry)
		}
		a.runUI(func() {
			delete(a.pluginMarketBusy, entry.ID)
			if err != nil {
				a.pluginMarketErr = "安装失败：" + err.Error()
				a.showToast("err", "插件安装失败："+err.Error())
				return
			}
			action := "已安装"
			if entry.Installed {
				action = "已更新"
			}
			a.showToast("ok", action+"插件 "+manifest.Name+" "+manifest.Version)
			if a.pluginMgr != nil {
				a.syncPluginPanelsAfterInstall()
			}
			if a.win != nil {
				a.win.Invalidate()
			}
		})
	}()
}

func (a *App) uninstallPluginEntry(entry plugins.RegistryEntry) {
	if a.pluginMgr == nil || a.pluginMarketBusy[entry.ID] {
		return
	}
	mgr := a.pluginMgr
	a.pluginMarketBusy[entry.ID] = true
	go func() {
		err := mgr.Uninstall(entry.ID)
		a.runUI(func() {
			delete(a.pluginMarketBusy, entry.ID)
			if err != nil {
				a.pluginMarketErr = "卸载失败：" + err.Error()
				a.showToast("err", "卸载失败："+err.Error())
				return
			}
			if strings.HasPrefix(a.pluginPanel, entry.ID+":") {
				a.pluginPanel = ""
			}
			a.showToast("ok", "已卸载插件 "+entry.Name)
			if a.win != nil {
				a.win.Invalidate()
			}
		})
	}()
}

func (a *App) setPluginEnabled(id string, enabled bool) {
	if a.pluginMgr == nil || a.pluginMarketBusy[id] {
		return
	}
	mgr := a.pluginMgr
	a.pluginMarketBusy[id] = true
	go func() {
		err := mgr.SetEnabled(id, enabled)
		a.runUI(func() {
			delete(a.pluginMarketBusy, id)
			if err != nil {
				a.pluginMarketErr = "切换插件状态失败：" + err.Error()
				a.showToast("err", "插件状态切换失败")
				return
			}
			if !enabled && strings.HasPrefix(a.pluginPanel, id+":") {
				a.pluginPanel = ""
			}
			a.syncPluginPanelsAfterInstall()
			if a.win != nil {
				a.win.Invalidate()
			}
		})
	}()
}

func (a *App) syncPluginPanelsAfterInstall() {
	if a.pluginMgr == nil || a.pluginPanel == "" {
		return
	}
	if _, ok := a.pluginMgr.Panel(a.pluginPanel); !ok {
		a.pluginPanel = ""
	}
}

func (a *App) pickPluginFile() {
	go func() {
		paths, err := mygo.Dialog.Open(mygo.OpenDialogOptions{
			Parent:  a.win,
			Title:   "安装 Nemorous 插件",
			Filters: []mygo.FileFilter{{Name: "Nemorous 插件包", Extensions: []string{"zip"}}},
		})
		if err != nil || len(paths) == 0 {
			return
		}
		a.runUI(func() { a.installPluginFile(paths[0]) })
	}()
}

func (a *App) installPluginFile(path string) {
	if a.pluginMgr == nil || a.pluginMarketBusy["__file"] {
		return
	}
	mgr := a.pluginMgr
	a.pluginMarketBusy["__file"] = true
	a.pluginMarketErr = ""
	go func() {
		ctx, cancel := context.WithTimeout(context.Background(), 2*time.Minute)
		defer cancel()
		manifest, err := mgr.InstallFile(ctx, path)
		a.runUI(func() {
			delete(a.pluginMarketBusy, "__file")
			if err != nil {
				a.pluginMarketErr = "安装失败：" + err.Error()
				a.showToast("err", "插件安装失败："+err.Error())
			} else {
				a.showToast("ok", "已安装插件 "+manifest.Name+" "+manifest.Version)
				a.syncPluginPanelsAfterInstall()
			}
			if a.win != nil {
				a.win.Invalidate()
			}
		})
	}()
}

func (a *App) savePluginRegistryURL() {
	if a.pluginMgr == nil {
		return
	}
	url := strings.TrimSpace(a.pluginMarketDraft)
	if url == plugins.DefaultRegistryURL {
		url = ""
	}
	if err := a.pluginMgr.SetRegistryURL(url); err != nil {
		a.showToast("err", "保存插件市场地址失败")
		return
	}
	a.showToast("ok", "插件市场地址已保存")
	a.refreshPluginRegistry()
}

func (a *App) buildPluginMarketModal(c *ui.Context, p Palette) {
	ui.DialogBase(c, &a.showPluginMarket, func(backdrop, panel ui.Element) {
		backdrop.Background(ui.RGBA(20, 22, 30, 0.40))
		if a.pluginMgr == nil {
			panel.Width(480).Padding(18).Gap(10).Radius(12).Background(p.Surface).Border(1, p.Border)
			ui.Text(c, "插件引擎不可用").FontSize(14).FontWeight(weightBold).TextColor(p.TextPrimary)
			ui.Text(c, "无法创建插件目录。请检查配置目录权限。").FontSize(11.5).TextColor(p.TextMuted)
			pill(c, p, "关闭", false, func() { a.showPluginMarket = false })
			return
		}
		panelHeight := float32(700)
		if a.win != nil {
			if _, h := a.win.ContentSize(); h > 0 {
				panelHeight = min(panelHeight, float32(h)-48)
				if panelHeight < 520 {
					panelHeight = 520
				}
			}
		}
		panel.Width(780).Height(panelHeight).Padding(18).Gap(12).Radius(12).Background(p.Surface).Border(1, p.Border)

		installed := len(a.pluginMgr.Installed())
		ui.Row(c).Justify(ui.SpaceBetween).AlignItems(ui.Center).Children(func() {
			ui.Row(c).Gap(8).AlignItems(ui.Center).Children(func() {
				drawIcon(c, icoPlugin, 16, p.Accent)
				ui.Text(c, "插件市场").FontSize(15).FontWeight(weightBold).TextColor(p.TextPrimary)
				ui.Text(c, fmt.Sprintf("已安装 %d", installed)).FontSize(10.5).TextColor(p.TextMuted)
			})
			miniBtn(c, p, "关闭", func() { a.showPluginMarket = false })
		})

		ui.Row(c).Gap(6).AlignItems(ui.Center).Children(func() {
			drawIcon(c, icoSearch, 14, p.TextMuted)
			ui.TextInput(c.Key("market-q"), &a.pluginMarketQuery).Grow(1).Placeholder("搜索插件名称、描述或标签…")
			pill(c, p, "刷新市场", false, func() { a.refreshPluginRegistry() })
			pill(c, p, "从文件安装…", false, func() { a.pickPluginFile() })
		})

		ui.Row(c).Gap(6).AlignItems(ui.Center).Children(func() {
			pill(c, p, "市场", a.pluginMarketTab != "installed", func() { a.pluginMarketTab = "market" })
			pill(c, p, "已安装", a.pluginMarketTab == "installed", func() { a.pluginMarketTab = "installed" })
			ui.Spacer(c)
			if a.pluginRegistryLoading {
				ui.Text(c, "正在刷新市场…").FontSize(10).TextColor(p.AccentSoftText)
			}
			ui.Text(c, "内置索引 + 远程").FontSize(9.5).TextColor(p.TextMuted)
		})

		if a.pluginMarketErr != "" {
			ui.Column(c).FillWidth().Padding(6, 8).Radius(5).Background(p.DangerSoft).Children(func() {
				ui.Text(c, a.pluginMarketErr).FontSize(10).TextColor(p.Danger)
			})
		}

		if a.pluginMarketBusy["__file"] {
			ui.Text(c, "正在安装插件包…").FontSize(10.5).TextColor(p.AccentSoftText)
		}

		query := strings.ToLower(strings.TrimSpace(a.pluginMarketQuery))
		entries := a.pluginMgr.Catalog()
		shown := 0
		ui.Scroll(c).Grow(1).FillWidth().Children(func() {
			ui.Column(c).FillWidth().Gap(8).Children(func() {
				for _, entry := range entries {
					entry := entry
					if a.pluginMarketTab == "installed" && !entry.Installed {
						continue
					}
					if a.pluginMarketTab == "market" && entry.Installed && !entry.UpdateAvailable {
						// Keep installed plugins visible in the market tab,
						// as an IDE does, so update/disable are reachable.
					}
					if query != "" {
						hay := strings.ToLower(entry.Name + " " + entry.Description + " " + strings.Join(entry.Tags, " ") + " " + entry.ID)
						if !strings.Contains(hay, query) {
							continue
						}
					}
					shown++
					a.buildPluginCard(c, p, entry)
				}
				if shown == 0 {
					ui.Column(c).Padding(20).Center().Children(func() {
						ui.Text(c, "没有找到插件").FontSize(12).TextColor(p.TextMuted)
					})
				}
			})
		})

		ui.Row(c).Gap(6).AlignItems(ui.Center).Children(func() {
			ui.Text(c, "市场地址：").FontSize(10).TextColor(p.TextMuted).Shrink(0)
			ui.TextInput(c.Key("market-url"), &a.pluginMarketDraft).Grow(1).Placeholder(plugins.DefaultRegistryURL)
			miniBtn(c, p, "保存", func() { a.savePluginRegistryURL() })
			miniBtn(c, p, "恢复官方", func() {
				a.pluginMarketDraft = plugins.DefaultRegistryURL
				a.savePluginRegistryURL()
			})
		})
	})
}

func (a *App) buildPluginCard(c *ui.Context, p Palette, entry plugins.RegistryEntry) {
	busy := a.pluginMarketBusy[entry.ID]
	ui.Column(c).FillWidth().Padding(12).Gap(6).Radius(8).Background(p.RailBg).Border(1, p.BorderSubtle).Children(func() {
		ui.Row(c).Gap(8).AlignItems(ui.Center).Children(func() {
			ui.Text(c, entry.Name).FontSize(13).FontWeight(weightSemibold).TextColor(p.TextPrimary)
			badgeBox(c, "v"+entry.Version, p.TextMuted, p.Surface2, p.Border, 9.5, false)
			if entry.Official {
				badgeBox(c, "官方", p.AccentSoftText, p.AccentSoft, p.Accent, 9, true)
			}
			if entry.Builtin {
				badgeBox(c, "内置", p.TextMuted, p.Surface2, p.Border, 9, false)
			}
			if entry.Installed {
				badgeBox(c, "已安装", p.Green, p.GreenSoft, p.Green, 9, true)
				if entry.UpdateAvailable {
					badgeBox(c, "可更新", p.Amber, p.AmberSoft, p.Amber, 9, true)
				}
			}
			if !entry.Compatible && !entry.Installed {
				badgeBox(c, "不兼容", p.Danger, p.DangerSoft, p.Danger, 9, true)
			}
			ui.Spacer(c)
			if busy {
				ui.Text(c, "处理中…").FontSize(10).TextColor(p.AccentSoftText)
			} else if entry.Installed {
				if ref := a.pluginPanelRef(entry.ID); ref != "" {
					miniBtn(c, p, "打开", func() {
						a.showPluginMarket = false
						a.openPluginPanel(ref)
					})
				}
				if entry.Enabled {
					miniBtn(c, p, "禁用", func() { a.setPluginEnabled(entry.ID, false) })
				} else {
					miniBtn(c, p, "启用", func() { a.setPluginEnabled(entry.ID, true) })
				}
				if entry.UpdateAvailable && entry.Compatible {
					pill(c, p, "更新", true, func() { a.installPluginEntry(entry) })
				}
				if a.pluginMarketConfirm == entry.ID {
					pill(c, p, "确认卸载", true, func() {
						a.pluginMarketConfirm = ""
						a.uninstallPluginEntry(entry)
					})
					miniBtn(c, p, "取消", func() { a.pluginMarketConfirm = "" })
				} else {
					miniBtn(c, p, "卸载", func() { a.pluginMarketConfirm = entry.ID })
				}
			} else if entry.Compatible {
				pill(c, p, "安装", true, func() { a.installPluginEntry(entry) })
			} else {
				pillDisabled(c, p, "安装")
			}
		})
		if entry.Description != "" {
			ui.Text(c, entry.Description).FontSize(11).TextColor(p.TextSecondary)
		}
		if !entry.Compatible && entry.IncompatibleWhy != "" && !entry.Installed {
			ui.Text(c, entry.IncompatibleWhy).FontSize(10).TextColor(p.Danger)
		}
		meta := make([]string, 0, 4)
		if entry.Author != "" {
			meta = append(meta, entry.Author)
		}
		if len(entry.Tags) > 0 {
			meta = append(meta, strings.Join(entry.Tags, " · "))
		}
		if len(entry.Permissions) > 0 {
			meta = append(meta, "权限: "+strings.Join(entry.Permissions, ", "))
		}
		if entry.Installed && entry.InstalledVersion != "" {
			meta = append(meta, "本地 v"+entry.InstalledVersion)
		}
		if len(meta) > 0 {
			ui.Text(c, strings.Join(meta, "    ")).FontSize(9.5).TextColor(p.TextMuted).MaxLines(1)
		}
	})
}

func (a *App) buildPluginSettingsSection(c *ui.Context, p Palette) {
	ui.Column(c).Padding(12).Gap(8).Radius(8).Background(p.RailBg).Border(1, p.BorderSubtle).Children(func() {
		ui.Row(c).Justify(ui.SpaceBetween).AlignItems(ui.Center).Children(func() {
			ui.Text(c, "插件").FontSize(12.5).FontWeight(weightSemibold).TextColor(p.TextPrimary)
			if a.pluginMgr != nil {
				ui.Text(c, fmt.Sprintf("%d 个已安装", len(a.pluginMgr.Installed()))).FontSize(10.5).TextColor(p.TextMuted)
			}
		})
		if a.pluginMgr == nil {
			ui.Text(c, "插件引擎未初始化。").FontSize(11).TextColor(p.TextMuted)
			return
		}
		ui.Row(c).Gap(6).AlignItems(ui.Center).Children(func() {
			pill(c, p, "打开插件市场", true, func() { a.openPluginMarket() })
			pill(c, p, "从文件安装…", false, func() { a.pickPluginFile() })
			ui.Spacer(c)
			if a.pluginMgr.RegistryError() != "" {
				ui.Text(c, "远程市场离线（使用内置索引）").FontSize(10).TextColor(p.Amber)
			}
		})
		ui.Text(c, "官方 Maven 插件提供模块树、生命周期目标、Profiles、依赖树与 Effective POM；插件在沙箱中运行，仅能使用清单声明的权限。").FontSize(10.5).TextColor(p.TextMuted)
	})
}
