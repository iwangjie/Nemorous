package main

import (
	"context"
	"fmt"
	"strings"
	"time"

	"github.com/egoist/mygo/ui"
	"github.com/egoist/mygo/ui/editor"
	"github.com/iwangjie/nemorous/internal/gitops"
)

// wireCodeBlame attaches the editor callbacks that reveal blame on a line
// click, a gutter click, and the editor's context menu.
func (a *App) wireCodeBlame(tab *codeTab) {
	if tab == nil || tab.state == nil {
		return
	}
	tab.state.OnGutterClick = func(line int) {
		if time.Since(tab.blameHiddenAt) < 200*time.Millisecond {
			return
		}
		tab.blameEnabled = true
		if !tab.blameAll {
			tab.blameLine = line
		}
		if snap := tab.state.Buffer.Snapshot(); line >= 0 && line < snap.LineCount() {
			off := snap.LineStart(line)
			tab.state.Select(off, off)
		}
		a.ensureCodeBlame(tab)
		a.invalidateUI()
	}
	tab.state.OnInput = func(ev ui.InputEvent) bool {
		if ev.Kind == ui.InputPointerUp && ev.Button == 0 {
			if time.Since(tab.blameHiddenAt) < 200*time.Millisecond {
				return false
			}
			tab.blameEnabled = true
			if !tab.blameAll {
				tab.blameShownKey = ""
			}
			a.ensureCodeBlame(tab)
			a.invalidateUI()
		}
		return false
	}
	tab.state.ContextMenu = func(m *ui.Menu) {
		line := tab.state.CursorPos().Line
		if tab.blameEnabled && tab.blameLoaded {
			if entry, ok := tab.blameLines[line+1]; ok && strings.Trim(entry.Hash, "0") != "" {
				if m.Item("在 Git 树中查看该提交").Chosen() {
					a.openBlameCommit(tab, entry)
				}
			}
		}
		if m.Item("显示整文件 Git Blame").Chosen() {
			tab.blameEnabled = true
			tab.blameAll = true
			tab.blameShownKey = ""
			a.ensureCodeBlame(tab)
			a.invalidateUI()
		}
		if m.Item("只显示该行 Git Blame").Chosen() {
			tab.blameEnabled = true
			tab.blameAll = false
			tab.blameLine = line
			tab.blameShownKey = ""
			a.ensureCodeBlame(tab)
			a.invalidateUI()
		}
		if tab.blameEnabled {
			if m.Item("隐藏 Git Blame").Chosen() {
				tab.blameEnabled = false
				tab.blameAll = false
				tab.blameShownKey = ""
				tab.blameHiddenAt = time.Now()
				tab.state.SetInlays(nil)
				a.invalidateUI()
			}
		}
	}
}

func (a *App) invalidateUI() {
	if a.win != nil {
		a.win.Invalidate()
	}
}

// syncCodeBlame keeps the inline blame in step with the caret. Blame is
// fetched once per tab and then looked up by line.
func (a *App) syncCodeBlame(tab *codeTab) {
	if tab == nil || tab.state == nil || tab.state.Buffer == nil || !tab.blameEnabled {
		return
	}
	line := tab.state.CursorPos().Line
	if line < 0 {
		return
	}
	if !tab.blameAll {
		tab.blameLine = line
	}
	if !tab.blameLoaded {
		a.ensureCodeBlame(tab)
		return
	}
	a.applyCodeBlame(tab)
}

func (a *App) ensureCodeBlame(tab *codeTab) {
	if tab == nil || tab.blameLoaded || tab.blameLoading {
		return
	}
	tab.blameLoading = true
	tab.blameErr = ""
	tab.blameSeq++
	seq := tab.blameSeq
	file := tab.file
	go func() {
		lines, err := gitops.FileBlame(context.Background(), file.RepoPath, file.RelPath)
		a.runUI(func() {
			if !a.hasCodeTab(tab) || seq != tab.blameSeq {
				return
			}
			tab.blameLoading = false
			if err != nil {
				tab.blameErr = err.Error()
				a.showToast("err", "Git blame 失败: "+err.Error())
				return
			}
			tab.blameLines = make(map[int]gitops.BlameLine, len(lines))
			for _, line := range lines {
				tab.blameLines[line.Line] = line
			}
			tab.blameLoaded = true
			a.applyCodeBlame(tab)
			a.invalidateUI()
		})
	}()
}

// blameInlayText fits a single-line annotation into the columns left after
// the code on its line. The editor wraps long lines, so the annotation is
// truncated rather than allowed to wrap onto a second row.
func blameInlayText(label string, line string, width float32, fontSize float32) string {
	if label == "" {
		return ""
	}
	if width <= 0 || fontSize <= 0 {
		return "  " + truncateRunes(label, 72)
	}
	charW := fontSize * 0.65
	cols := int((width - charW*7 - 24) / charW)
	if cols <= 0 {
		return ""
	}
	lineCols := 0
	for _, r := range line {
		if r == '\t' {
			lineCols += 4 - lineCols%4
		} else {
			lineCols++
		}
	}
	budget := cols - lineCols - 3
	if budget < 2 {
		return ""
	}
	return "  " + truncateRunes(label, budget-2)
}

func truncateRunes(s string, max int) string {
	if max <= 0 {
		return ""
	}
	runes := []rune(s)
	if len(runes) <= max {
		return s
	}
	if max == 1 {
		return "…"
	}
	return string(runes[:max-1]) + "…"
}

// blameRecencyColor tints blame metadata by commit age: recent work stands
// out, while older history stays quiet.
func blameRecencyColor(unix int64, now time.Time, p Palette) ui.Color {
	if unix <= 0 {
		return p.TextMuted
	}
	age := now.Sub(time.Unix(unix, 0))
	switch {
	case age <= 7*24*time.Hour:
		return p.Accent
	case age <= 30*24*time.Hour:
		return p.Amber
	default:
		return p.TextMuted
	}
}

// openBlameCommit jumps from a blame annotation to the commit in the Git tree.
// Uncommitted lines and zero hashes have no commit to reveal.
func (a *App) openBlameCommit(tab *codeTab, entry gitops.BlameLine) {
	if tab == nil || tab.file.RepoID == "" || entry.Hash == "" || strings.Trim(entry.Hash, "0") == "" {
		return
	}
	short := entry.Hash
	if len(short) > 8 {
		short = short[:8]
	}
	a.openGitCommit(gitops.CommitEntry{
		RepoID:     tab.file.RepoID,
		RepoName:   tab.file.RepoName,
		RepoPath:   tab.file.RepoPath,
		Hash:       entry.Hash,
		ShortHash:  short,
		Subject:    entry.Summary,
		Author:     entry.Author,
		AuthorUnix: entry.AuthorUnix,
	})
}

// buildCodeBlameGutter draws the whole-file blame as a left annotation
// column, aligned with the editor's visible lines, as IDEA's Annotate view.
func (a *App) buildCodeBlameGutter(c *ui.Context, p Palette, tab *codeTab) {
	if tab == nil || tab.state == nil || tab.state.Buffer == nil || !tab.blameAll {
		return
	}
	width := float32(176)
	box := ui.Box(c.Key("blame-gutter")).Width(width).FillHeight().Clip().Background(p.RailBg).
		BorderWidth(0, 0, 1, 0).BorderColor(p.Border)
	box.Children(func() {
		if !tab.blameLoaded {
			text := "加载 Git Blame…"
			if tab.blameErr != "" {
				text = "Git Blame 不可用"
			}
			ui.Row(c.Key("blame-gutter-status")).Padding(8, 6).Children(func() {
				ui.Text(c.Key("blame-gutter-status-text"), text).FontSize(9.5).TextColor(p.TextMuted).MaxLines(2)
			})
			return
		}
		snap := tab.state.Buffer.Snapshot()
		first, last := tab.state.VisibleLines()
		first -= 1
		last += 2
		if first < 0 {
			first = 0
		}
		if last >= snap.LineCount() {
			last = snap.LineCount() - 1
		}
		scroll := float32(tab.state.ScrollY())
		now := time.Now()
		for line := first; line <= last; line++ {
			top := float32(tab.state.LineTop(line)) - scroll
			next := top + float32(tab.state.LineTop(line+1)-tab.state.LineTop(line))
			if next-top < 10 {
				continue
			}
			entry, ok := tab.blameLines[line+1]
			if !ok {
				continue
			}
			date := ""
			if entry.AuthorUnix > 0 {
				date = time.Unix(entry.AuthorUnix, 0).Format("2006/1/2")
			}
			kc := c.Key(fmt.Sprintf("blame-gutter-%d", line))
			recency := blameRecencyColor(entry.AuthorUnix, now, p)
			clickable := strings.Trim(entry.Hash, "0") != ""
			row := ui.Row(kc).Absolute().Left(0).Top(top).Width(width).Height(next-top).
				Padding(2, 6).Gap(6).AlignItems(ui.Start)
			if clickable {
				row.Cursor(ui.CursorPointer)
				if row.Hovered() {
					row.Background(p.SurfaceHover)
				}
				short := entry.Hash
				if len(short) > 8 {
					short = short[:8]
				}
				tip := strings.TrimSpace(entry.Summary)
				if tip == "" {
					tip = "提交 " + short
				}
				row.Tooltip(fmt.Sprintf("%s\n%s · %s\n点击在提交图中查看", tip, date, short))
				if row.Clicked() {
					a.openBlameCommit(tab, entry)
				}
			}
			row.Children(func() {
				ui.Text(kc, date).Font("SF Mono, Menlo, monospace").FontSize(9.5).TextColor(recency).Width(66).MaxLines(1).Shrink(0)
				ui.Text(kc, entry.Author).FontSize(9.5).TextColor(recency).MaxLines(1).Grow(1)
			})
		}
	})
}

func (a *App) applyCodeBlame(tab *codeTab) {
	if tab == nil || tab.state == nil || tab.state.Buffer == nil {
		return
	}
	if !tab.blameEnabled || !tab.blameLoaded {
		return
	}
	snap := tab.state.Buffer.Snapshot()
	if tab.blameAll {
		// The whole-file annotation is drawn in the left blame gutter, not
		// as inlays that would wrap inside a soft-wrapped line.
		key := fmt.Sprintf("gutter|%d|%s", len(tab.blameLines), tab.blameErr)
		if key != tab.blameShownKey {
			tab.blameShownKey = key
			tab.state.SetInlays(nil)
		}
		return
	}
	key := fmt.Sprintf("%d|%d|%d|%s", tab.blameLine, snap.LineCount(), int(tab.editorWidth), tab.blameErr)
	if key == tab.blameShownKey {
		return
	}
	tab.blameShownKey = key
	if tab.blameErr != "" {
		return
	}
	if tab.blameLine < 0 || tab.blameLine >= snap.LineCount() {
		tab.state.SetInlays(nil)
		return
	}
	entry, ok := tab.blameLines[tab.blameLine+1]
	if !ok {
		tab.state.SetInlays(nil)
		return
	}
	label := gitops.BlameDisplay(entry)
	tab.state.SetInlays([]editor.Inlay{{
		Offset: snap.LineEnd(tab.blameLine),
		Text:   blameInlayText(label, snap.Line(tab.blameLine), tab.editorWidth, tab.state.Font.Size),
	}})
}

func (a *App) hasCodeTab(target *codeTab) bool {
	for _, tab := range a.codeTabs {
		if tab == target {
			return true
		}
	}
	return false
}
