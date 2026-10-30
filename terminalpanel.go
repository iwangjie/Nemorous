package main

import (
	"fmt"
	"os"
	"strings"

	"github.com/egoist/mygo/plugins/terminal"
	"github.com/egoist/mygo/ui"
)

// toggleTerminalPanel opens the embedded terminal as a bottom tool window
// instead of launching an external terminal application.
func (a *App) toggleTerminalPanel() {
	if a.showTerminalPanel {
		a.showTerminalPanel = false
		return
	}
	a.showGitPanel = false
	a.showLogPanel = false
	a.showRunPanel = false
	a.showTerminalPanel = true
	if a.terminal != nil && a.terminalExited {
		a.restartTerminal()
		return
	}
	a.ensureTerminal()
}

func (a *App) terminalDir() (string, error) {
	if root := strings.TrimSpace(a.cfg.WorkspacePath); root != "" {
		return root, nil
	}
	home, err := os.UserHomeDir()
	if err != nil || strings.TrimSpace(home) == "" {
		return "", fmt.Errorf("无法确定终端工作目录")
	}
	return home, nil
}

// ensureTerminal lazily starts one shell for the embedded terminal. The shell
// lives across hide/show, as a tool-window terminal does.
func (a *App) ensureTerminal() {
	if a.terminal != nil {
		return
	}
	root, err := a.terminalDir()
	if err != nil {
		a.terminalErr = err.Error()
		a.logRuntimeDetail("err", "终端启动失败", a.terminalErr)
		return
	}
	a.terminalGen++
	gen := a.terminalGen
	a.terminalErr = ""
	a.terminalExited = false
	a.terminalExitCode = 0
	a.terminalTitle = ""

	term, err := terminal.New(terminal.Options{
		Dir:  root,
		Font: terminal.Font{Size: a.cfg.EditorFontSize},
		OnTitle: func(title string) {
			a.runUI(func() {
				if gen != a.terminalGen {
					return
				}
				a.terminalTitle = title
			})
		},
		OnExit: func(code int) {
			a.runUI(func() {
				if gen != a.terminalGen {
					return
				}
				a.terminalExited = true
				a.terminalExitCode = code
				a.logRuntimeDetail("warn", "终端进程已退出", fmt.Sprintf("exit code %d", code))
			})
		},
	})
	if err != nil {
		a.terminalErr = err.Error()
		a.logRuntimeDetail("err", "终端启动失败", err.Error())
		return
	}
	a.terminal = term
	a.logRuntimeDetail("info", "终端已启动", root)
}

func (a *App) restartTerminal() {
	a.closeTerminal()
	a.ensureTerminal()
}

func (a *App) closeTerminal() {
	a.terminalGen++
	if a.terminal != nil {
		_ = a.terminal.Close()
		a.terminal = nil
	}
	a.terminalExited = false
	a.terminalExitCode = 0
	a.terminalTitle = ""
}

func (a *App) buildTerminalPanel(c *ui.Context, p Palette) {
	ui.Column(c).Fill().Background(p.Surface).Children(func() {
		ui.Row(c).Height(38).Padding(0, 10).Gap(6).AlignItems(ui.Center).Background(p.RailBg).
			BorderWidth(0, 0, 1, 0).BorderColor(p.Border).Children(func() {
			drawIcon(c, icoTerminal, 14, p.TextMuted)
			ui.Text(c, "终端").FontSize(12).FontWeight(weightSemibold).TextColor(p.TextPrimary)
			if a.terminalExited {
				badgeBox(c, fmt.Sprintf("已退出 %d", a.terminalExitCode), p.Amber, p.AmberSoft, ui.Color{}, 9.5, true)
			}
			title := strings.TrimSpace(a.terminalTitle)
			if title == "" {
				title = "内置终端"
			}
			ui.Text(c, title).FontSize(10.5).TextColor(p.TextMuted).MaxLines(1).Grow(1)
			if a.terminalExited {
				miniBtn(c, p, "重启", func() { a.restartTerminal() })
			}
			closeKc := c.Key("term-close")
			closeBtn := ui.Column(closeKc).Size(24, 24).Radius(4).Center().Cursor(ui.CursorPointer).
				Tooltip("隐藏终端（进程继续运行）").Label("隐藏终端")
			if closeBtn.Hovered() {
				closeBtn.Background(p.SurfaceHover)
			}
			if closeBtn.Clicked() {
				a.showTerminalPanel = false
			}
			closeBtn.Children(func() {
				drawIcon(closeKc, icoClose, 11, p.TextMuted)
			})
		})

		if a.terminalErr != "" {
			ui.Column(c).Grow(1).Center().Gap(8).Padding(20).Children(func() {
				ui.Text(c, "终端启动失败").FontSize(12.5).FontWeight(weightSemibold).TextColor(p.Danger)
				ui.Text(c, a.terminalErr).FontSize(11).TextColor(p.TextSecondary).MaxLines(3)
				miniBtn(c, p, "重试", func() { a.ensureTerminal() })
			})
			return
		}
		if a.terminal == nil {
			ui.Column(c).Grow(1).Center().Children(func() {
				ui.Text(c, "正在启动终端…").FontSize(11.5).TextColor(p.TextMuted)
			})
			return
		}
		terminal.View(c, a.terminal).Fill().AutoFocus()
	})
}
