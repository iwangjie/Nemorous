package main

import (
	"context"
	"fmt"
	"os"
	"os/exec"
	"path/filepath"
	"regexp"
	"strings"
	"time"

	"github.com/egoist/mygo/ui"
	"github.com/egoist/mygo/ui/editor"
	"github.com/iwangjie/nemorous/internal/config"
	"github.com/iwangjie/nemorous/internal/gitops"
	runpkg "github.com/iwangjie/nemorous/internal/run"
)

// structureItem is one symbol in the Structure view.
type structureItem struct {
	Name  string
	Kind  string
	Line  int // 1-based
	Depth int
}

// todoItem is one TODO/FIXME hit.
type todoItem struct {
	RepoID   string
	RepoName string
	RepoPath string
	RelPath  string
	AbsPath  string
	Line     int
	Text     string
}

func (a *App) openStructure() {
	tab := a.activeCodeTab()
	if tab == nil {
		a.showToast("warn", "先打开一个代码文件")
		return
	}
	a.structureItems = parseStructure(tab.file.RelPath, tab.state.Buffer.String())
	a.showStructure = true
}

var (
	goStructure   = regexp.MustCompile(`^(\s*)(func|type)\s+(?:\([^)]*\)\s*)?([A-Za-z_]\w*)`)
	jsStructure   = regexp.MustCompile(`^(\s*)(?:export\s+)?(?:async\s+)?(function|class|interface|enum|const|let|var)\s+([A-Za-z_$][\w$]*)`)
	pyStructure   = regexp.MustCompile(`^(\s*)(?:async\s+)?(def|class)\s+([A-Za-z_]\w*)`)
	javaStructure = regexp.MustCompile(`^(\s*)(?:(?:public|protected|private|static|final|abstract|synchronized|native|default)\s+)*(class|interface|enum|record|[A-Za-z_][\w<>\[\], ?]*)\s+([A-Za-z_]\w*)\s*[({;]`)
	rsStructure   = regexp.MustCompile(`^(\s*)(?:pub\s+)?(fn|struct|enum|trait|impl|mod)\s+([A-Za-z_]\w*)`)
)

// parseStructure extracts a lightweight outline without a language server.
func parseStructure(path, content string) []structureItem {
	ext := strings.ToLower(filepath.Ext(path))
	var re *regexp.Regexp
	switch ext {
	case ".go":
		re = goStructure
	case ".js", ".jsx", ".ts", ".tsx":
		re = jsStructure
	case ".py":
		re = pyStructure
	case ".java", ".kt", ".kts", ".cs", ".scala":
		re = javaStructure
	case ".rs":
		re = rsStructure
	default:
		return nil
	}
	var out []structureItem
	for i, line := range strings.Split(content, "\n") {
		if len(out) >= 800 {
			break
		}
		m := re.FindStringSubmatch(line)
		if m == nil {
			continue
		}
		name := m[3]
		kind := m[2]
		if kind == "const" || kind == "let" || kind == "var" {
			if !strings.Contains(line, "function") && !strings.Contains(line, "=>") {
				continue
			}
		}
		indent := 0
		for _, r := range m[1] {
			if r == '\t' {
				indent += 4
			} else {
				indent++
			}
		}
		out = append(out, structureItem{Name: name, Kind: kind, Line: i + 1, Depth: indent / 2})
	}
	return out
}

func (a *App) buildStructureModal(c *ui.Context, p Palette) {
	ui.DialogBase(c, &a.showStructure, func(backdrop, panel ui.Element) {
		backdrop.Background(ui.RGBA(20, 22, 30, 0.36))
		panel.Width(480).Height(440).Padding(14).Gap(6).Radius(12).Background(p.Surface).Border(1, p.Border)
		context := ""
		if tab := a.activeCodeTab(); tab != nil {
			context = filepath.Base(tab.file.RelPath)
		}
		modalHeader(c, p, icoCode, "结构", context, nil, func() { a.showStructure = false })
		if len(a.structureItems) == 0 {
			emptyState(c, p, icoCode, "没有可识别的结构", "")
			return
		}
		ui.Scroll(c).Grow(1).Children(func() {
			for _, item := range a.structureItems {
				item := item
				kc := c.Key(fmt.Sprintf("struct-%d-%s", item.Line, item.Name))
				row := ui.Row(kc).Padding(3, 6).Gap(6).Radius(3).Cursor(ui.CursorPointer).AlignItems(ui.Center)
				if row.Hovered() {
					row.Background(p.SurfaceHover)
				}
				if row.Clicked() {
					if tab := a.activeCodeTab(); tab != nil {
						line := item.Line - 1
						if line < 0 {
							line = 0
						}
						tab.state.GoTo(editor.Pos{Line: line})
						tab.state.ScrollTo(line)
					}
					a.showStructure = false
				}
				row.Children(func() {
					for i := 0; i < item.Depth && i < 8; i++ {
						ui.Box(kc).Width(12).Shrink(0)
					}
					ui.Text(kc, item.Kind).Font("monospace").FontSize(8.5).TextColor(p.TextMuted).Width(32)
					ui.Text(kc, item.Name).Font("monospace").FontSize(11).TextColor(p.TextPrimary)
					ui.Spacer(kc)
					ui.Text(kc, fmt.Sprint(item.Line)).FontSize(9.5).TextColor(p.TextMuted)
				})
			}
		})
	})
}

func (a *App) openTodos() {
	a.showTodos = true
	a.todosLoading = false
	a.todoErr = ""
	if strings.TrimSpace(a.todoQuery) == "" {
		a.todoQuery = `TODO|FIXME|HACK|XXX`
	}
	a.refreshTodos()
}

func (a *App) refreshTodos() {
	pattern := strings.TrimSpace(a.todoQuery)
	if pattern == "" {
		return
	}
	if _, err := regexp.Compile(pattern); err != nil {
		a.todoErr = "正则表达式无效：" + err.Error()
		return
	}
	a.todosLoading = true
	a.todoErr = ""
	files := append([]gitops.WorkspaceFile(nil), a.workspaceFiles...)
	root := a.cfg.WorkspacePath
	go func() {
		ctx, cancel := context.WithTimeout(context.Background(), 30*time.Second)
		defer cancel()
		out, err := searchWorkspace(ctx, root, files, pattern, globalSearchOptions{Regex: true})
		a.runUI(func() {
			a.todosLoading = false
			if err != nil {
				a.todoErr = err.Error()
				return
			}
			a.todos = a.todos[:0]
			for _, hit := range out.Hits {
				a.todos = append(a.todos, todoItem{
					RepoID: hit.RepoID, RepoName: hit.RepoName, RepoPath: hit.RepoPath,
					RelPath: hit.RelPath, AbsPath: hit.AbsPath, Line: hit.Line, Text: strings.TrimSpace(hit.Snippet),
				})
			}
		})
	}()
}

func (a *App) openCodeAtLine(repoID, repoName, repoPath, relPath, absPath string, line int) {
	a.selectCodeFile(gitops.WorkspaceFile{
		RepoID: repoID, RepoName: repoName, RepoPath: repoPath, RelPath: relPath, AbsPath: absPath,
	})
	a.activeTab = TabCode
	if tab := a.activeCodeTab(); tab != nil && tab.state != nil {
		line--
		if line < 0 {
			line = 0
		}
		tab.state.GoTo(editor.Pos{Line: line})
		tab.state.ScrollTo(line)
	}
}

func (a *App) buildTodosModal(c *ui.Context, p Palette) {
	ui.DialogBase(c, &a.showTodos, func(backdrop, panel ui.Element) {
		backdrop.Background(ui.RGBA(20, 22, 30, 0.36))
		panel.Width(740).Height(600).Padding(14).Gap(6).Radius(12).Background(p.Surface).Border(1, p.Border)
		modalHeader(c, p, icoSearch, "TODO", fmt.Sprint(len(a.todos)), func() {
			if a.todosLoading {
				ui.Spinner(c).Size(14, 14).Label("扫描中").TextColor(p.TextMuted)
			} else {
				toolButton(c, p, "todo-scan", icoRefresh, "重新扫描", false, true, func() { a.refreshTodos() })
			}
		}, func() { a.showTodos = false })
		ui.Row(c).FillWidth().Gap(6).AlignItems(ui.Center).Children(func() {
			ui.TextInput(c.Key("todo-q"), &a.todoQuery).Grow(1).Placeholder("TODO|FIXME|HACK|XXX")
		})
		if a.todoErr != "" {
			ui.Text(c, a.todoErr).FontSize(10).TextColor(p.Danger)
		}
		if len(a.todos) == 0 && !a.todosLoading {
			emptyState(c, p, icoSearch, "没有匹配项", "")
			return
		}
		ui.Scroll(c).Grow(1).Children(func() {
			for i, todo := range a.todos {
				todo := todo
				kc := c.Key(fmt.Sprintf("todo-%d", i))
				row := ui.Row(kc).Padding(4, 6).Gap(7).Radius(3).Cursor(ui.CursorPointer).AlignItems(ui.Center)
				if row.Hovered() {
					row.Background(p.SurfaceHover)
				}
				if row.Clicked() {
					a.showTodos = false
					a.openCodeAtLine(todo.RepoID, todo.RepoName, todo.RepoPath, todo.RelPath, todo.AbsPath, todo.Line)
				}
				row.Children(func() {
					ui.Text(kc, todo.RepoName).FontSize(9).TextColor(p.TextMuted).Width(80).MaxLines(1)
					ui.Text(kc, fmt.Sprintf("%s:%d", todo.RelPath, todo.Line)).Font("monospace").FontSize(9.5).TextColor(p.TextSecondary).Width(250).MaxLines(1)
					ui.Text(kc, todo.Text).FontSize(10.5).TextColor(p.TextPrimary).Grow(1).MaxLines(1)
				})
			}
		})
	})
}

func (a *App) openBookmarks() {
	a.showBookmarks = true
}

func (a *App) toggleBookmarkAtCursor() {
	tab := a.activeCodeTab()
	if tab == nil || tab.state == nil {
		a.showToast("warn", "先打开一个代码文件")
		return
	}
	line := tab.state.CursorPos().Line + 1
	added := a.cfg.ToggleBookmark(tab.file.RepoID, tab.file.RelPath, line)
	_ = a.cfg.Save()
	if added {
		a.showToast("ok", fmt.Sprintf("已添加书签 %s:%d", tab.file.RelPath, line))
	} else {
		a.showToast("ok", fmt.Sprintf("已移除书签 %s:%d", tab.file.RelPath, line))
	}
}

func (a *App) buildBookmarksModal(c *ui.Context, p Palette) {
	ui.DialogBase(c, &a.showBookmarks, func(backdrop, panel ui.Element) {
		backdrop.Background(ui.RGBA(20, 22, 30, 0.36))
		panel.Width(680).Height(460).Padding(14).Gap(6).Radius(12).Background(p.Surface).Border(1, p.Border)
		modalHeader(c, p, icoBookmark, "书签", fmt.Sprint(len(a.cfg.Bookmarks)), func() {
			iconAction(c, p, "bm-add", icoPlus, "记录当前行", true, func() { a.toggleBookmarkAtCursor() })
		}, func() { a.showBookmarks = false })
		ui.TextInput(c.Key("bm-q"), &a.bookmarkQuery).Placeholder("搜索路径…")
		query := strings.ToLower(strings.TrimSpace(a.bookmarkQuery))
		ui.Scroll(c).Grow(1).Children(func() {
			shown := 0
			for _, bm := range a.cfg.Bookmarks {
				bm := bm
				hay := strings.ToLower(bm.RepoID + "/" + bm.RelPath)
				if query != "" && !strings.Contains(hay, query) {
					continue
				}
				shown++
				kc := c.Key(fmt.Sprintf("bm-%s-%s-%d", bm.RepoID, bm.RelPath, bm.Line))
				row := ui.Row(kc).Padding(4, 6).Gap(7).Radius(3).Cursor(ui.CursorPointer).AlignItems(ui.Center)
				if row.Hovered() {
					row.Background(p.SurfaceHover)
				}
				if row.Clicked() {
					if wf, ok := a.workspaceFileByKey(bm.RepoID, bm.RelPath); ok {
						a.showBookmarks = false
						a.openCodeAtLine(bm.RepoID, wf.RepoName, wf.RepoPath, bm.RelPath, wf.AbsPath, bm.Line)
					}
				}
				row.Children(func() {
					drawIcon(kc, icoBookmark, 11, p.Accent)
					ui.Text(kc, bm.RepoID+"/"+bm.RelPath).FontSize(10.5).TextColor(p.TextPrimary).Grow(1).MaxLines(1)
					ui.Text(kc, fmt.Sprintf(":%d", bm.Line)).FontSize(9.5).TextColor(p.TextMuted)
					toolButton(kc, p, "bm-del-"+bm.RepoID+bm.RelPath+fmt.Sprint(bm.Line), icoTrash, "删除书签", false, true, func() {
						a.cfg.ToggleBookmark(bm.RepoID, bm.RelPath, bm.Line)
						_ = a.cfg.Save()
					})
				})
			}
			if shown == 0 {
				emptyState(c, p, icoBookmark, "暂无书签", "⌘⇧B 记录当前行")
			}
		})
	})
}

func (a *App) workspaceFileByKey(repoID, relPath string) (gitops.WorkspaceFile, bool) {
	for _, f := range a.workspaceFiles {
		if f.RepoID == repoID && f.RelPath == relPath {
			return f, true
		}
	}
	for _, st := range a.statuses {
		if st.Repo.ID != repoID {
			continue
		}
		return gitops.WorkspaceFile{
			RepoID: repoID, RepoName: st.Repo.Name, RepoPath: st.Repo.AbsPath,
			RelPath: relPath, AbsPath: filepath.Join(st.Repo.AbsPath, filepath.FromSlash(relPath)),
		}, true
	}
	return gitops.WorkspaceFile{}, false
}

// syncEditorAids installs gutter marks (bookmarks and breakpoints) and the
// editor's context menu before a tab is drawn.
func (a *App) syncEditorAids(tab *codeTab, p Palette) {
	if tab == nil || tab.state == nil {
		return
	}
	key := tab.file.RepoID + "::" + tab.file.RelPath
	var marks []editor.GutterMark
	for _, line := range a.cfg.LinesFor(tab.file.RepoID, tab.file.RelPath) {
		if line > 0 {
			marks = append(marks, editor.GutterMark{Line: line - 1, Color: p.Accent})
		}
	}
	for _, line := range a.breakpoints[key] {
		if line > 0 {
			marks = append(marks, editor.GutterMark{Line: line - 1, Color: p.Danger})
		}
	}
	tab.state.GutterMarks = marks
	tab.state.OnGutterClick = func(line int) {
		if a.debugModeActive() {
			a.toggleBreakpoint(tab, line+1)
			return
		}
		a.toggleBookmarkLine(tab, line+1)
	}
	tab.state.ContextMenu = func(m *ui.Menu) {
		if m.Item("切换书签").Chosen() {
			a.toggleBookmarkAtCursor()
		}
		if m.Item("文件结构…").Chosen() {
			a.openStructure()
		}
		if m.Item("本地历史…").Chosen() {
			a.openLocalHistory(tab.file.RepoID, tab.file.RepoPath, tab.file.RelPath)
		}
		if m.Item("格式化文件").Chosen() {
			a.formatActiveFile()
		}
	}
}

func (a *App) toggleBookmarkLine(tab *codeTab, line int) {
	if tab == nil {
		return
	}
	added := a.cfg.ToggleBookmark(tab.file.RepoID, tab.file.RelPath, line)
	_ = a.cfg.Save()
	if added {
		a.showToast("ok", fmt.Sprintf("已添加书签 %s:%d", tab.file.RelPath, line))
	} else {
		a.showToast("ok", fmt.Sprintf("已移除书签 %s:%d", tab.file.RelPath, line))
	}
}

func (a *App) toggleBreakpoint(tab *codeTab, line int) {
	if tab == nil {
		return
	}
	a.toggleBreakpointKey(tab.file.RepoID+"::"+tab.file.RelPath, tab.file.RepoPath, tab.file.RelPath, line)
}

func (a *App) toggleBreakpointKey(key, repoPath, relPath string, line int) {
	lines := a.breakpoints[key]
	for i, l := range lines {
		if l == line {
			a.breakpoints[key] = append(lines[:i], lines[i+1:]...)
			a.cfg.Breakpoints = a.breakpoints
			_ = a.cfg.Save()
			a.showToast("ok", fmt.Sprintf("已移除断点 %s:%d", relPath, line))
			a.syncBreakpoints()
			return
		}
	}
	a.breakpoints[key] = append(lines, line)
	a.cfg.Breakpoints = a.breakpoints
	_ = a.cfg.Save()
	a.showToast("ok", fmt.Sprintf("已添加断点 %s:%d", relPath, line))
	a.syncBreakpoints()
}

func (a *App) debugModeActive() bool {
	if a.debugClient != nil || a.debugStopped {
		return true
	}
	if cfg := a.selectedRunConfig(); cfg != nil && cfg.Kind == "debug" {
		return true
	}
	return false
}

func (a *App) syncBreakpoints() {
	client := a.debugClient
	if client == nil {
		return
	}
	snapshot := make(map[string][]int, len(a.breakpoints))
	for key, lines := range a.breakpoints {
		snapshot[key] = append([]int(nil), lines...)
	}
	files := make(map[string]gitops.WorkspaceFile)
	for key := range snapshot {
		parts := strings.SplitN(key, "::", 2)
		if len(parts) != 2 {
			continue
		}
		if wf, ok := a.workspaceFileByKey(parts[0], parts[1]); ok {
			files[key] = wf
		}
	}
	go func() {
		ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
		defer cancel()
		for key, lines := range snapshot {
			if wf, ok := files[key]; ok {
				_, _ = client.SetBreakpoints(ctx, wf.AbsPath, lines)
			}
		}
	}()
}

func (a *App) formatActiveFile() {
	tab := a.activeCodeTab()
	if tab == nil {
		a.showToast("warn", "先打开一个代码文件")
		return
	}
	if er := gitops.SaveMicroEdit(tab.file.RepoPath, tab.file.RelPath, tab.state.Buffer.String()); er != nil {
		a.showToast("err", "保存失败: "+er.Error())
		return
	}
	tab.state.Buffer.MarkSaved()
	changed, err := a.formatFile(tab.file.RepoPath, tab.file.RelPath)
	if err != nil {
		a.showToast("err", "格式化失败: "+err.Error())
		return
	}
	if !changed {
		a.showToast("info", "没有可用的格式化器，或文件已是格式化状态")
		return
	}
	a.reloadCodeTab(tab)
	a.recordLocalHistory(tab.file.RepoID, tab.file.RepoPath, tab.file.RelPath, "格式化")
	a.showToast("ok", "已格式化 "+tab.file.RelPath)
}

// formatFile runs the configured formatter, or the default for the file's
// language, and reports whether the file changed. A missing formatter is
// not an error.
func (a *App) formatFile(repoPath, relPath string) (bool, error) {
	abs := filepath.Join(repoPath, filepath.FromSlash(relPath))
	args, ok := formatterCommand(a.cfg, abs, relPath)
	if !ok {
		return false, nil
	}
	before, _ := os.ReadFile(abs)
	ctx, cancel := context.WithTimeout(context.Background(), 20*time.Second)
	defer cancel()
	cmd := exec.CommandContext(ctx, args[0], args[1:]...)
	cmd.Dir = repoPath
	out, err := cmd.CombinedOutput()
	if err != nil {
		msg := strings.TrimSpace(string(out))
		if msg == "" {
			msg = err.Error()
		}
		return false, fmt.Errorf("%s", msg)
	}
	after, err := os.ReadFile(abs)
	if err != nil {
		return false, err
	}
	return string(before) != string(after), nil
}

func formatterCommand(cfg config.Config, absPath, relPath string) ([]string, bool) {
	if strings.TrimSpace(cfg.FormatCommand) != "" {
		parts := runpkg.Lines(cfg.FormatCommand)
		if len(parts) == 0 {
			return nil, false
		}
		replaced := false
		for i, part := range parts {
			if strings.Contains(part, "{}") {
				parts[i] = strings.ReplaceAll(part, "{}", absPath)
				replaced = true
			}
		}
		if !replaced {
			parts = append(parts, absPath)
		}
		if _, err := exec.LookPath(parts[0]); err != nil {
			return nil, false
		}
		return parts, true
	}
	ext := strings.ToLower(filepath.Ext(relPath))
	type candidate struct {
		cmd  string
		args []string
	}
	var candidates []candidate
	switch ext {
	case ".go":
		candidates = []candidate{{"gofmt", []string{"-w", absPath}}}
	case ".js", ".jsx", ".ts", ".tsx", ".json", ".css", ".scss", ".md", ".html", ".yaml", ".yml":
		candidates = []candidate{{"prettier", []string{"--write", absPath}}}
	case ".py":
		candidates = []candidate{{"ruff", []string{"format", absPath}}, {"black", []string{absPath}}}
	case ".rs":
		candidates = []candidate{{"rustfmt", []string{absPath}}}
	case ".java":
		candidates = []candidate{{"google-java-format", []string{"-i", absPath}}}
	}
	for _, c := range candidates {
		if _, err := exec.LookPath(c.cmd); err == nil {
			return append([]string{c.cmd}, c.args...), true
		}
	}
	return nil, false
}

func (a *App) reloadCodeTab(tab *codeTab) {
	if tab == nil {
		return
	}
	cur, _, err := gitops.ReadRepoFile(context.Background(), tab.file.RepoPath, tab.file.RelPath)
	if err != nil {
		a.showToast("err", "重新读取文件失败: "+err.Error())
		return
	}
	tab.state.Buffer.Set(cur)
	tab.state.Buffer.MarkSaved()
}

// recordLocalHistory snapshots a file after a save or commit.
func (a *App) recordLocalHistory(repoID, repoPath, relPath, reason string) {
	store := a.historyStore()
	if store == nil {
		return
	}
	_ = store.RecordFile(repoID, repoPath, relPath, reason)
}

func (a *App) recordLocalHistoryContent(repoID, repoPath, relPath, content, reason string) {
	store := a.historyStore()
	if store == nil {
		return
	}
	_ = store.Record(repoID, relPath, content, reason)
}
