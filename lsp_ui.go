package main

import (
	"context"
	"fmt"
	"os"
	"path/filepath"
	"strings"
	"time"

	"github.com/egoist/mygo/ui"
	"github.com/egoist/mygo/ui/editor"
	"github.com/iwangjie/nemorous/internal/gitops"
	"github.com/iwangjie/nemorous/internal/lsp"
)

func (a *App) lspURI(tab *codeTab) string {
	abs := tab.file.AbsPath
	if abs == "" {
		abs = filepath.Join(tab.file.RepoPath, tab.file.RelPath)
	}
	return lsp.FileURI(abs)
}

func (a *App) lspRepoKey(tab *codeTab, languageID string) string {
	return tab.file.RepoID + "::" + languageID
}

// installLSPHooks wires editor gestures to the language server. The hooks are
// inert while the optional setting is off.
func (a *App) installLSPHooks(tab *codeTab) {
	if tab == nil || tab.state == nil || tab.lspHooksInstalled {
		return
	}
	tab.lspHooksInstalled = true
	st := tab.state
	st.OnCompletion = func(req editor.CompletionRequest) {
		a.lspCompletion(tab, req)
	}
	st.OnHover = func(req editor.HoverRequest) {
		a.lspHover(tab, req)
	}
	st.OnDefinition = func(offset int) {
		a.lspDefinition(tab, offset)
	}
	st.ContextMenu = func(m *ui.Menu) {
		if !a.canUseLSP(tab) {
			return
		}
		m.Separator()
		if m.Item("查找引用 (⌥F7)").Chosen() {
			a.openLSPReferences()
		}
		if m.Item("快速修复 (⌥↩)").Chosen() {
			a.openLSPCodeActions()
		}
		if m.Item("格式化 (⌘⌥L)").Chosen() {
			a.triggerLSPFormat()
		}
		if m.Item("Problems").Chosen() {
			a.openLSPProblems()
		}
	}
}

// prepareLSP starts a server for the tab's language when enabled and a server
// is installed. Failures are remembered per repository/language.
func (a *App) prepareLSP(tab *codeTab) {
	if tab == nil || tab.state == nil {
		return
	}
	a.installLSPHooks(tab)
	if !a.cfg.LSPEnabled {
		return
	}
	if tab.lspKey != "" {
		a.openLSPDocument(tab)
		return
	}
	spec, available := lsp.Detect(tab.file.RelPath, a.cfg.LSPServers)
	if !available {
		return
	}
	key := a.lspRepoKey(tab, spec.LanguageID)
	tab.lspKey = key
	if a.lspClients == nil {
		a.lspClients = make(map[string]*lsp.Client)
	}
	if a.lspStarting == nil {
		a.lspStarting = make(map[string]bool)
	}
	if a.lspFailed == nil {
		a.lspFailed = make(map[string]string)
	}
	if _, ok := a.lspClients[key]; ok {
		a.openLSPDocument(tab)
		return
	}
	if a.lspStarting[key] {
		return
	}
	if failed, ok := a.lspFailed[key]; ok {
		_ = failed
		return
	}
	a.lspStarting[key] = true
	root := tab.file.RepoPath
	langID := spec.LanguageID
	command := spec.Command
	gen := a.lspGen
	go func() {
		ctx, cancel := context.WithTimeout(context.Background(), 20*time.Second)
		defer cancel()
		client, err := lsp.Start(ctx, root, command, langID, func(uri string, ds []lsp.Diagnostic) {
			a.runUI(func() { a.applyLSPDiagnostics(uri, ds) })
		})
		a.runUI(func() {
			if gen != a.lspGen {
				if client != nil {
					go client.Close()
				}
				return
			}
			delete(a.lspStarting, key)
			if err != nil {
				a.lspFailed[key] = err.Error()
				a.showToast("warn", "LSP 启动失败："+err.Error())
				return
			}
			a.lspClients[key] = client
			for _, t := range a.codeTabs {
				if t.lspKey == key {
					a.openLSPDocument(t)
				}
			}
		})
	}()
}

func (a *App) openLSPDocument(tab *codeTab) {
	if !a.cfg.LSPEnabled || tab == nil || tab.state == nil || tab.lspKey == "" {
		return
	}
	client := a.lspClients[tab.lspKey]
	if client == nil || tab.lspOpened {
		return
	}
	text := tab.state.Buffer.String()
	version := tab.state.Buffer.Version()
	if client.DidOpen(a.lspURI(tab), text) == nil {
		tab.lspVersion = version
		tab.lspOpened = true
		a.refreshInlayHints(tab)
	}
}

// syncLSP sends full-document changes when the buffer changes and a server is
// already running. It is called from the code pane's frame.
func (a *App) syncLSP(tab *codeTab) {
	if !a.cfg.LSPEnabled || tab == nil || tab.state == nil || tab.lspKey == "" {
		return
	}
	client := a.lspClients[tab.lspKey]
	if client == nil {
		return
	}
	version := tab.state.Buffer.Version()
	if version == tab.lspVersion {
		return
	}
	if err := client.DidChange(a.lspURI(tab), tab.state.Buffer.String()); err == nil {
		tab.lspVersion = version
	}
}

func (a *App) notifyLSPDidSave(tab *codeTab) {
	if !a.cfg.LSPEnabled || tab == nil || tab.state == nil || tab.lspKey == "" {
		return
	}
	if client := a.lspClients[tab.lspKey]; client != nil {
		_ = client.DidSave(a.lspURI(tab), tab.state.Buffer.String())
		a.refreshInlayHints(tab)
	}
}

func (a *App) canUseLSP(tab *codeTab) bool {
	return a.cfg.LSPEnabled && tab != nil && tab.lspKey != "" && a.lspClients[tab.lspKey] != nil
}

func documentPosition(text string, offset int) (lineText string, line, col int) {
	offset = max(0, min(offset, len(text)))
	line = strings.Count(text[:offset], "\n")
	start := strings.LastIndex(text[:offset], "\n") + 1
	end := strings.Index(text[offset:], "\n")
	if end < 0 {
		end = len(text)
	} else {
		end += offset
	}
	return text[start:end], line, offset - start
}

func offsetAtLine(text string, line, col int) int {
	lines := strings.Split(text, "\n")
	if line < 0 {
		return 0
	}
	if line >= len(lines) {
		return len(text)
	}
	off := 0
	for i := 0; i < line; i++ {
		off += len(lines[i]) + 1
	}
	lineText := lines[line]
	return off + max(0, min(col, len(lineText)))
}

func (a *App) lspCompletion(tab *codeTab, req editor.CompletionRequest) {
	if !a.canUseLSP(tab) {
		return
	}
	client := a.lspClients[tab.lspKey]
	text := tab.state.Buffer.String()
	lineText, line, col := documentPosition(text, req.Offset)
	uri := a.lspURI(tab)
	go func() {
		ctx, cancel := context.WithTimeout(context.Background(), 6*time.Second)
		defer cancel()
		items, err := client.Completion(ctx, uri, lineText, line, col)
		if err != nil {
			return
		}
		a.runUI(func() {
			if tab.state.Buffer.Version() != req.Version {
				return
			}
			out := make([]editor.Completion, 0, len(items))
			for _, item := range items {
				insert := item.InsertText
				if item.TextEdit != nil && item.TextEdit.NewText != "" {
					insert = item.TextEdit.NewText
				}
				out = append(out, editor.Completion{
					Label:         item.Label,
					Detail:        item.Detail,
					Kind:          mapCompletionKind(item.Kind),
					Insert:        insert,
					Documentation: lsp.MessageText(item.Documentation),
					FilterText:    item.FilterText,
				})
			}
			tab.state.ShowCompletions(req.Offset-len(req.Prefix), out)
		})
	}()
}

func mapCompletionKind(kind int) editor.CompletionKind {
	switch kind {
	case 2:
		return editor.CompletionMethod
	case 3:
		return editor.CompletionFunction
	case 6:
		return editor.CompletionVariable
	case 10:
		return editor.CompletionField
	case 7, 8, 22, 25:
		return editor.CompletionType
	case 9:
		return editor.CompletionModule
	case 14:
		return editor.CompletionKeyword
	case 15:
		return editor.CompletionSnippet
	case 21:
		return editor.CompletionConstant
	default:
		return editor.CompletionText
	}
}

func (a *App) lspHover(tab *codeTab, req editor.HoverRequest) {
	if !a.canUseLSP(tab) {
		return
	}
	client := a.lspClients[tab.lspKey]
	text := tab.state.Buffer.String()
	lineText, line, col := documentPosition(text, req.Offset)
	start := req.Offset
	for start > 0 && isIdentByte(text[start-1]) {
		start--
	}
	end := req.Offset
	for end < len(text) && isIdentByte(text[end]) {
		end++
	}
	uri := a.lspURI(tab)
	go func() {
		ctx, cancel := context.WithTimeout(context.Background(), 6*time.Second)
		defer cancel()
		hover, err := client.Hover(ctx, uri, lineText, line, col)
		if err != nil || hover == nil {
			return
		}
		msg := lsp.MessageText(hover.Contents)
		if strings.TrimSpace(msg) == "" {
			return
		}
		a.runUI(func() {
			if tab.state.Buffer.Version() != req.Version {
				return
			}
			tab.state.ShowHover(start, end, msg, nil)
		})
	}()
}

func isIdentByte(b byte) bool {
	return b == '_' || b >= '0' && b <= '9' || b >= 'a' && b <= 'z' || b >= 'A' && b <= 'Z' || b >= 0x80
}

func (a *App) lspDefinition(tab *codeTab, offset int) {
	if !a.canUseLSP(tab) {
		return
	}
	client := a.lspClients[tab.lspKey]
	text := tab.state.Buffer.String()
	lineText, line, col := documentPosition(text, offset)
	uri := a.lspURI(tab)
	go func() {
		ctx, cancel := context.WithTimeout(context.Background(), 8*time.Second)
		defer cancel()
		locs, err := client.Definition(ctx, uri, lineText, line, col)
		if err != nil || len(locs) == 0 {
			a.runUI(func() { a.showToast("info", "LSP 未找到定义") })
			return
		}
		a.runUI(func() { a.navigateLSPLocation(locs[0]) })
	}()
}

func (a *App) navigateLSPLocation(loc lsp.Location) {
	path, err := lsp.PathFromURI(loc.URI)
	if err != nil {
		a.showToast("warn", err.Error())
		return
	}
	var wf *gitops.WorkspaceFile
	for i := range a.workspaceFiles {
		f := a.workspaceFiles[i]
		candidate := f.AbsPath
		if candidate == "" {
			candidate = filepath.Join(f.RepoPath, f.RelPath)
		}
		if filepath.Clean(candidate) == filepath.Clean(path) {
			copy := f
			wf = &copy
			break
		}
	}
	if wf == nil {
		for _, repo := range a.repos {
			rel, relErr := filepath.Rel(repo.AbsPath, path)
			if relErr != nil || rel == ".." || strings.HasPrefix(rel, ".."+string(filepath.Separator)) {
				continue
			}
			wf = &gitops.WorkspaceFile{
				RepoID: repo.ID, RepoName: repo.Name, RepoPath: repo.AbsPath,
				RelPath: filepath.ToSlash(rel), AbsPath: path,
				Monogram: repo.Monogram, ColorIdx: repo.ColorIndex,
			}
			break
		}
	}
	if wf == nil {
		a.showToast("warn", "定义位于工作区之外："+path)
		return
	}
	a.selectCodeFile(*wf)
	a.activeTab = TabCode
	if tab := a.activeCodeTab(); tab != nil && tab.state != nil {
		lineText := ""
		if loc.Range.Start.Line >= 0 && loc.Range.Start.Line < tab.state.Buffer.LineCount() {
			lineText = tab.state.Buffer.Line(loc.Range.Start.Line)
		}
		tab.state.GoTo(editor.Pos{Line: loc.Range.Start.Line, Col: lsp.UTF16ToByte(lineText, loc.Range.Start.Character)})
	}
}

func (a *App) applyLSPDiagnostics(uri string, ds []lsp.Diagnostic) {
	for _, tab := range a.codeTabs {
		if a.lspURI(tab) != uri || tab.state == nil {
			continue
		}
		out := make([]editor.Diagnostic, 0, len(ds))
		errs, warns := 0, 0
		msg := ""
		for _, d := range ds {
			sev := editor.SeverityInfo
			switch d.Severity {
			case 1:
				sev = editor.SeverityError
				errs++
			case 2:
				sev = editor.SeverityWarning
				warns++
			case 4:
				sev = editor.SeverityHint
			}
			if msg == "" {
				msg = d.Message
			}
			start := max(0, min(d.Start, tab.state.Buffer.Len()))
			end := max(start, min(d.End, tab.state.Buffer.Len()))
			out = append(out, editor.Diagnostic{Start: start, End: end, Severity: sev, Message: d.Message, Source: d.Source})
		}
		tab.state.SetDiagnostics(tab.state.Buffer.Version(), out)
		tab.lspDiagErrors, tab.lspDiagWarns, tab.lspDiagMsg = errs, warns, msg
	}
	a.refreshLSPProblems()
}

// requestLSPRename asks the server for a rename; the returned edits must be
// applied on the UI thread with applyLSPRenameChanges.
func (a *App) requestLSPRename(tab *codeTab, newName string) (map[string][]lsp.TextEdit, error) {
	if !a.canUseLSP(tab) {
		return nil, fmt.Errorf("LSP 未就绪")
	}
	newName = strings.TrimSpace(newName)
	if newName == "" {
		return nil, fmt.Errorf("新名称不能为空")
	}
	client := a.lspClients[tab.lspKey]
	text := tab.state.Buffer.String()
	offset := tab.state.Cursor()
	lineText, line, col := documentPosition(text, offset)
	ctx, cancel := context.WithTimeout(context.Background(), 15*time.Second)
	defer cancel()
	changes, err := client.Rename(ctx, a.lspURI(tab), lineText, line, col, newName)
	if err != nil {
		return nil, err
	}
	if len(changes) == 0 {
		return nil, fmt.Errorf("LSP 没有返回重命名修改")
	}
	return changes, nil
}

// applyLSPRenameChanges writes rename edits to files inside the workspace.
func (a *App) applyLSPRenameChanges(changes map[string][]lsp.TextEdit) (int, error) {
	ws := filepath.Clean(a.cfg.WorkspacePath)
	updated := 0
	for uri, edits := range changes {
		path, err := lsp.PathFromURI(uri)
		if err != nil {
			continue
		}
		rel, err := filepath.Rel(ws, path)
		if err != nil || rel == ".." || strings.HasPrefix(rel, ".."+string(filepath.Separator)) {
			continue
		}
		var content string
		var open *codeTab
		for _, t := range a.codeTabs {
			abs := t.file.AbsPath
			if abs == "" {
				abs = filepath.Join(t.file.RepoPath, t.file.RelPath)
			}
			if filepath.Clean(abs) == filepath.Clean(path) {
				open = t
				content = t.state.Buffer.String()
				break
			}
		}
		if open == nil {
			data, readErr := os.ReadFile(path)
			if readErr != nil {
				continue
			}
			content = string(data)
		}
		next := lsp.ApplyTextEdits(content, edits)
		info, statErr := os.Stat(path)
		mode := os.FileMode(0o644)
		if statErr == nil {
			mode = info.Mode()
		}
		if err := os.WriteFile(path, []byte(next), mode); err != nil {
			continue
		}
		if open != nil {
			open.state.Buffer.Reset(next)
			open.lspVersion = 0
		}
		updated++
	}
	if updated == 0 {
		return 0, fmt.Errorf("没有可写入的工作区文件")
	}
	a.RefreshAsync()
	return updated, nil
}

func (a *App) closeLSPClients() {
	a.lspGen++
	for key, client := range a.lspClients {
		go client.Close()
		delete(a.lspClients, key)
	}
	a.lspStarting = make(map[string]bool)
	a.lspFailed = make(map[string]string)
	for _, tab := range a.codeTabs {
		tab.lspKey = ""
		tab.lspVersion = 0
		tab.lspOpened = false
	}
}

// buildLSPRenameRow draws the inline rename input.
func (a *App) buildLSPRenameRow(c *ui.Context, p Palette, tab *codeTab) {
	ui.Row(c).Padding(6, 14).Gap(8).Background(p.RailBg).BorderWidth(0, 0, 1, 0).BorderColor(p.Border).AlignItems(ui.Center).Children(func() {
		ui.Text(c, "LSP 重命名").FontSize(10.5).TextColor(p.TextMuted)
		ui.TextInput(c.Key("lsp-rename"), &a.lspRenameName).Placeholder("新名称").Width(220)
		pill(c, p, "应用", true, func() {
			if a.lspRenameBusy {
				return
			}
			a.lspRenameBusy = true
			a.lspRenameErr = ""
			name := a.lspRenameName
			go func() {
				changes, err := a.requestLSPRename(tab, name)
				a.runUI(func() {
					if err == nil {
						var n int
						n, err = a.applyLSPRenameChanges(changes)
						if err == nil {
							a.lspRenameOpen = false
							a.showToast("ok", fmt.Sprintf("已重命名 %d 个文件", n))
						}
					}
					a.lspRenameBusy = false
					if err != nil {
						a.lspRenameErr = err.Error()
					}
				})
			}()
		})
		miniBtn(c, p, "取消", func() { a.lspRenameOpen = false })
		if a.lspRenameBusy {
			ui.Spinner(c).Size(12, 12).Label("应用中").TextColor(p.TextMuted)
		}
		if a.lspRenameErr != "" {
			ui.Text(c, a.lspRenameErr).FontSize(9.5).TextColor(p.Danger).MaxLines(2).Grow(1)
		}
	})
}
