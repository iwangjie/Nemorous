package main

import (
	"context"
	"fmt"
	"path/filepath"
	"sort"
	"time"

	"github.com/egoist/mygo/ui"
	"github.com/egoist/mygo/ui/editor"
	"github.com/iwangjie/nemorous/internal/lsp"
)

func lspRangeForOffsets(tab *codeTab, start, end int) lsp.Range {
	text := tab.state.Buffer.String()
	_, sl, sc := documentPosition(text, start)
	_, el, ec := documentPosition(text, end)
	lineText := func(line int) string {
		if line < 0 || line >= tab.state.Buffer.LineCount() {
			return ""
		}
		return tab.state.Buffer.Line(line)
	}
	return lsp.Range{
		Start: lsp.Position{Line: sl, Character: lsp.ByteToUTF16(lineText(sl), sc)},
		End:   lsp.Position{Line: el, Character: lsp.ByteToUTF16(lineText(el), ec)},
	}
}

func (a *App) openLSPReferences() {
	tab := a.activeCodeTab()
	if !a.canUseLSP(tab) {
		a.showToast("warn", "LSP 未就绪")
		return
	}
	client := a.lspClients[tab.lspKey]
	text := tab.state.Buffer.String()
	lineText, line, col := documentPosition(text, tab.state.Cursor())
	uri := a.lspURI(tab)
	a.showLSPReferences = true
	a.lspReferencesLoading = true
	a.lspReferencesErr = ""
	a.lspReferences = nil
	a.lspReferencesTitle = tab.file.RelPath
	go func() {
		ctx, cancel := context.WithTimeout(context.Background(), 12*time.Second)
		defer cancel()
		locs, err := client.References(ctx, uri, lineText, line, col)
		a.runUI(func() {
			a.lspReferencesLoading = false
			if err != nil {
				a.lspReferencesErr = err.Error()
				return
			}
			if len(locs) > 500 {
				locs = locs[:500]
			}
			a.lspReferences = locs
		})
	}()
}

func (a *App) openLSPCodeActions() {
	tab := a.activeCodeTab()
	if !a.canUseLSP(tab) {
		a.showToast("warn", "LSP 未就绪")
		return
	}
	client := a.lspClients[tab.lspKey]
	offset := tab.state.Cursor()
	uri := a.lspURI(tab)
	rng := lspRangeForOffsets(tab, offset, offset)
	var diags []lsp.DiagnosticContext
	for _, d := range tab.state.Diagnostics() {
		if d.Start > offset || d.End < offset {
			continue
		}
		diags = append(diags, lsp.DiagnosticContext{
			Range:    lspRangeForOffsets(tab, d.Start, d.End),
			Severity: int(d.Severity) + 1,
			Message:  d.Message,
			Source:   d.Source,
		})
	}
	a.showLSPCodeActions = true
	a.lspCodeActionsLoading = true
	a.lspCodeActionsErr = ""
	a.lspCodeActions = nil
	a.lspCodeActionTab = tab
	go func() {
		ctx, cancel := context.WithTimeout(context.Background(), 12*time.Second)
		defer cancel()
		actions, err := client.CodeActions(ctx, uri, rng, diags)
		a.runUI(func() {
			a.lspCodeActionsLoading = false
			if err != nil {
				a.lspCodeActionsErr = err.Error()
				return
			}
			if len(actions) > 100 {
				actions = actions[:100]
			}
			a.lspCodeActions = actions
		})
	}()
}

func (a *App) applyLSPCodeAction(action lsp.CodeAction) {
	tab := a.lspCodeActionTab
	if tab == nil || !a.canUseLSP(tab) {
		a.showToast("warn", "LSP 未就绪")
		return
	}
	client := a.lspClients[tab.lspKey]
	a.showLSPCodeActions = false
	var applyErr error
	if action.Edit != nil {
		if n, err := a.applyLSPRenameChanges(action.Edit.EditsByURI()); err != nil {
			applyErr = err
		} else if n == 0 {
			applyErr = fmt.Errorf("没有可写入的工作区文件")
		}
	}
	if applyErr != nil {
		a.showToast("err", applyErr.Error())
		return
	}
	if action.Command != nil && action.Command.Command != "" {
		cmd := action.Command
		go func() {
			ctx, cancel := context.WithTimeout(context.Background(), 20*time.Second)
			defer cancel()
			err := client.ExecuteCommand(ctx, cmd.Command, cmd.Arguments)
			a.runUI(func() {
				if err != nil {
					a.showToast("warn", "命令执行失败："+err.Error())
					return
				}
				a.showToast("ok", "已应用："+action.Title)
				a.RefreshAsync()
			})
		}()
	} else {
		a.showToast("ok", "已应用："+action.Title)
	}
}

func (a *App) triggerLSPFormat() {
	tab := a.activeCodeTab()
	if tab == nil {
		a.showToast("warn", "先打开一个代码文件")
		return
	}
	if !a.canUseLSP(tab) {
		a.formatActiveFile()
		return
	}
	client := a.lspClients[tab.lspKey]
	uri := a.lspURI(tab)
	beforeVersion := tab.state.Buffer.Version()
	before := tab.state.Buffer.String()
	go func() {
		ctx, cancel := context.WithTimeout(context.Background(), 20*time.Second)
		defer cancel()
		edits, err := client.Formatting(ctx, uri, 4, true)
		a.runUI(func() {
			if err != nil {
				a.showToast("err", "LSP 格式化失败："+err.Error())
				return
			}
			if len(edits) == 0 {
				a.showToast("info", "LSP 没有返回格式化修改")
				return
			}
			next := lsp.ApplyTextEdits(before, edits)
			if next == before || tab.state.Buffer.Version() != beforeVersion {
				return
			}
			tab.state.Buffer.Set(next)
			tab.lspVersion = 0
			a.showToast("ok", "已用 LSP 格式化（未保存）")
		})
	}()
}

func (a *App) refreshLSPProblems() {
	var problems []lspProblem
	for _, tab := range a.codeTabs {
		if tab == nil || tab.state == nil {
			continue
		}
		for _, d := range tab.state.Diagnostics() {
			pos := tab.state.Buffer.Pos(d.Start)
			problems = append(problems, lspProblem{
				RepoID: tab.file.RepoID, RepoName: tab.file.RepoName, RelPath: tab.file.RelPath,
				Line: pos.Line + 1, Message: d.Message, Severity: int(d.Severity),
			})
		}
	}
	if len(problems) > 500 {
		problems = problems[:500]
	}
	sort.Slice(problems, func(i, j int) bool {
		if problems[i].RepoName != problems[j].RepoName {
			return problems[i].RepoName < problems[j].RepoName
		}
		if problems[i].RelPath != problems[j].RelPath {
			return problems[i].RelPath < problems[j].RelPath
		}
		return problems[i].Line < problems[j].Line
	})
	a.lspProblems = problems
}

func (a *App) refreshInlayHints(tab *codeTab) {
	if !a.canUseLSP(tab) || tab.state == nil {
		return
	}
	client := a.lspClients[tab.lspKey]
	lastLine := tab.state.Buffer.LineCount() - 1
	lastText := ""
	if lastLine >= 0 {
		lastText = tab.state.Buffer.Line(lastLine)
	}
	rng := lsp.Range{Start: lsp.Position{}, End: lsp.Position{Line: max(lastLine, 0), Character: lsp.ByteToUTF16(lastText, len(lastText))}}
	version := tab.state.Buffer.Version()
	uri := a.lspURI(tab)
	go func() {
		ctx, cancel := context.WithTimeout(context.Background(), 8*time.Second)
		defer cancel()
		hints, err := client.InlayHints(ctx, uri, rng)
		if err != nil {
			return
		}
		a.runUI(func() {
			if tab.state.Buffer.Version() != version {
				return
			}
			if len(hints) > 500 {
				hints = hints[:500]
			}
			out := make([]editor.Inlay, 0, len(hints))
			for _, h := range hints {
				label := lsp.InlayText(h.Label)
				if label == "" {
					continue
				}
				line := h.Position.Line
				lineText := ""
				if line >= 0 && line < tab.state.Buffer.LineCount() {
					lineText = tab.state.Buffer.Line(line)
				}
				col := lsp.UTF16ToByte(lineText, h.Position.Character)
				offset := tab.state.Buffer.Offset(editor.Pos{Line: line, Col: col})
				out = append(out, editor.Inlay{Offset: offset, Text: label})
			}
			tab.state.SetInlays(out)
		})
	}()
}

func (a *App) openLSPProblems() {
	a.refreshLSPProblems()
	a.showLSPProblems = true
}

func (a *App) buildLSPReferencesModal(c *ui.Context, p Palette) {
	ui.DialogBase(c, &a.showLSPReferences, func(backdrop, panel ui.Element) {
		backdrop.Background(ui.RGBA(20, 22, 30, 0.40))
		panel.Width(680).Height(560).Padding(14).Gap(8).Radius(12).Background(p.Surface).Border(1, p.Border)
		modalHeader(c, p, icoSearch, "引用", a.lspReferencesTitle, nil, func() { a.showLSPReferences = false })
		if a.lspReferencesLoading {
			ui.Row(c).Gap(8).AlignItems(ui.Center).Children(func() {
				ui.Spinner(c).Size(13, 13).Label("查找中").TextColor(p.TextMuted)
			})
		}
		if a.lspReferencesErr != "" {
			ui.Text(c, a.lspReferencesErr).FontSize(10).TextColor(p.Danger)
		}
		ui.Scroll(c).Grow(1).FillWidth().Children(func() {
			ui.Column(c).FillWidth().Gap(1).Children(func() {
				for i, loc := range a.lspReferences {
					loc := loc
					path, err := lsp.PathFromURI(loc.URI)
					if err != nil {
						continue
					}
					kc := c.Key(fmt.Sprintf("lsp-ref-%d", i))
					row := ui.Row(kc).FillWidth().Padding(5, 4).Gap(6).Radius(4).Cursor(ui.CursorPointer).AlignItems(ui.Center)
					if row.Hovered() {
						row.Background(p.SurfaceHover)
					}
					if row.Clicked() {
						a.showLSPReferences = false
						a.navigateLSPLocation(loc)
					}
					row.Children(func() {
						ui.Text(kc, filepath.Base(path)).FontSize(10.5).FontWeight(weightMedium).TextColor(p.TextPrimary).Width(180).MaxLines(1)
						ui.Text(kc, fmt.Sprintf("%d:%d", loc.Range.Start.Line+1, loc.Range.Start.Character+1)).Font("monospace").FontSize(9).TextColor(p.TextMuted).Width(70)
						ui.Text(kc, path).Font("monospace").FontSize(9).TextColor(p.TextMuted).Grow(1).MaxLines(1)
					})
				}
			})
		})
		if !a.lspReferencesLoading && len(a.lspReferences) == 0 && a.lspReferencesErr == "" {
			emptyState(c, p, icoSearch, "没有引用", "光标所在符号未被引用")
		}
	})
}

func (a *App) buildLSPCodeActionsModal(c *ui.Context, p Palette) {
	ui.DialogBase(c, &a.showLSPCodeActions, func(backdrop, panel ui.Element) {
		backdrop.Background(ui.RGBA(20, 22, 30, 0.40))
		panel.Width(620).Height(500).Padding(14).Gap(8).Radius(12).Background(p.Surface).Border(1, p.Border)
		modalHeader(c, p, icoSparkles, "快速修复", "", nil, func() { a.showLSPCodeActions = false })
		if a.lspCodeActionsLoading {
			ui.Row(c).Gap(8).AlignItems(ui.Center).Children(func() {
				ui.Spinner(c).Size(13, 13).Label("请求 Code Action").TextColor(p.TextMuted)
			})
		}
		if a.lspCodeActionsErr != "" {
			ui.Text(c, a.lspCodeActionsErr).FontSize(10).TextColor(p.Danger)
		}
		ui.Scroll(c).Grow(1).FillWidth().Children(func() {
			ui.Column(c).FillWidth().Gap(2).Children(func() {
				for i, action := range a.lspCodeActions {
					action := action
					kc := c.Key(fmt.Sprintf("lsp-action-%d", i))
					row := ui.Row(kc).FillWidth().Padding(7, 5).Gap(8).Radius(4).Cursor(ui.CursorPointer).AlignItems(ui.Center)
					if row.Hovered() {
						row.Background(p.SurfaceHover)
					}
					if row.Clicked() {
						a.applyLSPCodeAction(action)
					}
					row.Children(func() {
						drawIcon(kc, icoSparkles, 12, p.Accent)
						ui.Text(kc, action.Title).FontSize(11).TextColor(p.TextPrimary).Grow(1)
						if action.Kind != "" {
							ui.Text(kc, action.Kind).FontSize(9).TextColor(p.TextMuted)
						}
					})
				}
			})
		})
		if !a.lspCodeActionsLoading && len(a.lspCodeActions) == 0 && a.lspCodeActionsErr == "" {
			emptyState(c, p, icoSparkles, "没有可用的快速修复", "把光标放在诊断或符号上再试")
		}
	})
}

func (a *App) buildLSPProblemsModal(c *ui.Context, p Palette) {
	ui.DialogBase(c, &a.showLSPProblems, func(backdrop, panel ui.Element) {
		backdrop.Background(ui.RGBA(20, 22, 30, 0.40))
		panel.Width(760).Height(560).Padding(14).Gap(8).Radius(12).Background(p.Surface).Border(1, p.Border)
		modalHeader(c, p, icoAlert, "Problems", fmt.Sprintf("%d 个诊断", len(a.lspProblems)), nil, func() { a.showLSPProblems = false })
		ui.Scroll(c).Grow(1).FillWidth().Children(func() {
			ui.Column(c).FillWidth().Gap(1).Children(func() {
				for i, problem := range a.lspProblems {
					problem := problem
					kc := c.Key(fmt.Sprintf("lsp-problem-%d", i))
					row := ui.Row(kc).FillWidth().Padding(5, 4).Gap(6).Radius(4).Cursor(ui.CursorPointer).AlignItems(ui.Center)
					if row.Hovered() {
						row.Background(p.SurfaceHover)
					}
					if row.Clicked() {
						a.showLSPProblems = false
						a.openLSPProblem(problem)
					}
					row.Children(func() {
						fg, bg := p.Danger, p.DangerSoft
						if problem.Severity >= 2 {
							fg, bg = p.Amber, p.AmberSoft
						}
						badgeBox(kc, fmt.Sprintf("%d", problem.Severity+1), fg, bg, ui.Color{}, 9, true)
						ui.Text(kc, problem.RepoName+"/"+problem.RelPath+":"+fmt.Sprint(problem.Line)).FontSize(9.5).TextColor(p.TextMuted).Width(240).MaxLines(1)
						ui.Text(kc, problem.Message).FontSize(10.5).TextColor(p.TextPrimary).Grow(1).MaxLines(2)
					})
				}
			})
		})
		if len(a.lspProblems) == 0 {
			emptyState(c, p, icoCheck, "没有诊断", "LSP 没有报告问题")
		}
	})
}

func (a *App) openLSPProblem(problem lspProblem) {
	for _, tab := range a.codeTabs {
		if tab.file.RepoID == problem.RepoID && tab.file.RelPath == problem.RelPath {
			a.selectCodeFile(tab.file)
			a.activeTab = TabCode
			if tab.state != nil && problem.Line > 0 {
				tab.state.GoTo(editor.Pos{Line: problem.Line - 1, Col: 0})
			}
			return
		}
	}
}
