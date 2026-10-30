package main

import (
	"fmt"
	"strings"
	"time"

	"github.com/egoist/mygo/ui"
)

const maxAppLogEntries = 400

// appLogLevel is a stable label rendered as the colored badge in the log panel.
type appLogLevel string

const (
	logRun    appLogLevel = "RUN"
	logOK     appLogLevel = "OK"
	logSkip   appLogLevel = "SKIP"
	logFail   appLogLevel = "FAIL"
	logInfo   appLogLevel = "INFO"
	logWarn   appLogLevel = "WARN"
	logError  appLogLevel = "ERROR"
	logCancel appLogLevel = "CANCEL"
)

type appLogEntry struct {
	At      time.Time
	Level   appLogLevel
	Message string
	Detail  string
}

func (a *App) appendLog(dst *[]appLogEntry, level appLogLevel, message, detail string) {
	if strings.TrimSpace(message) == "" {
		return
	}
	entry := appLogEntry{At: time.Now(), Level: level, Message: message, Detail: detail}
	a.logMu.Lock()
	defer a.logMu.Unlock()
	*dst = append(*dst, entry)
	if len(*dst) > maxAppLogEntries {
		*dst = append([]appLogEntry(nil), (*dst)[len(*dst)-maxAppLogEntries:]...)
	}
}

func (a *App) logCommand(level string, message, detail string) {
	lvl := appLogLevel(strings.ToUpper(strings.TrimSpace(level)))
	switch lvl {
	case logRun, logOK, logSkip, logFail, logCancel:
	default:
		lvl = logInfo
	}
	a.appendLog(&a.commandLog, lvl, message, detail)
}

func (a *App) logRuntime(kind, message string) {
	a.logRuntimeDetail(kind, message, "")
}

func (a *App) logRuntimeDetail(kind, message, detail string) {
	lvl := logInfo
	switch strings.ToLower(strings.TrimSpace(kind)) {
	case "err", "error", "fail", "failed":
		lvl = logError
	case "warn", "warning", "skip":
		lvl = logWarn
	}
	// Status refreshes can repeat the same repository error every few seconds.
	a.logMu.Lock()
	if n := len(a.runtimeLog); n > 0 {
		last := a.runtimeLog[n-1]
		if last.Level == lvl && last.Message == message && last.Detail == detail && time.Since(last.At) < 10*time.Second {
			a.logMu.Unlock()
			return
		}
	}
	a.logMu.Unlock()
	a.appendLog(&a.runtimeLog, lvl, message, detail)
}

func (a *App) logEntries(tab string) []appLogEntry {
	a.logMu.Lock()
	defer a.logMu.Unlock()
	src := a.commandLog
	if tab == "runtime" {
		src = a.runtimeLog
	}
	return append([]appLogEntry(nil), src...)
}

func (a *App) logCount(tab string) int {
	a.logMu.Lock()
	defer a.logMu.Unlock()
	if tab == "runtime" {
		return len(a.runtimeLog)
	}
	return len(a.commandLog)
}

func (a *App) clearLog(tab string) {
	a.logMu.Lock()
	defer a.logMu.Unlock()
	if tab == "runtime" {
		a.runtimeLog = nil
		return
	}
	a.commandLog = nil
}

// buildLogPanel is the bottom tool window for command and runtime logs.
func (a *App) buildLogPanel(c *ui.Context, p Palette) {
	ui.Column(c).Fill().Background(p.Surface).Children(func() {
		ui.Row(c).Height(38).Padding(0, 10).Gap(4).AlignItems(ui.Center).Background(p.RailBg).
			BorderWidth(0, 0, 1, 0).BorderColor(p.Border).Children(func() {
			logTab := func(id, label string) {
				active := a.logTab == id
				bg, fg := ui.Transparent, p.TextSecondary
				if active {
					bg, fg = p.AccentSoft, p.AccentSoftText
				}
				kc := c.Key("log-tab-" + id)
				el := ui.Row(kc).Padding(4, 9).Gap(5).Radius(5).Background(bg).Cursor(ui.CursorPointer).AlignItems(ui.Center)
				if el.Hovered() && !active {
					el.Background(p.SurfaceHover)
					fg = p.TextPrimary
				}
				if el.Clicked() {
					a.logTab = id
				}
				el.Children(func() {
					ui.Text(kc, label).FontSize(11.5).FontWeight(weightMedium).TextColor(fg)
					ui.Text(kc, fmt.Sprintf("%d", a.logCount(id))).FontSize(9.5).TextColor(p.TextMuted)
				})
			}
			logTab("command", "命令执行")
			logTab("runtime", "软件运行")
			ui.Spacer(c)
			ui.Text(c, "本次会话").FontSize(10).TextColor(p.TextMuted)
			miniBtn(c.Key("log-clear"), p, "清空", func() {
				a.clearLog(a.logTab)
			})
			closeKc := c.Key("log-close")
			closeBtn := ui.Column(closeKc).Size(24, 24).Radius(4).Center().Cursor(ui.CursorPointer).
				Tooltip("隐藏日志面板").Label("隐藏日志面板")
			if closeBtn.Hovered() {
				closeBtn.Background(p.SurfaceHover)
			}
			if closeBtn.Clicked() {
				a.showLogPanel = false
			}
			closeBtn.Children(func() {
				drawIcon(closeKc, icoClose, 11, p.TextMuted)
			})
		})

		entries := a.logEntries(a.logTab)
		if len(entries) == 0 {
			ui.Column(c).Grow(1).Center().Gap(4).Children(func() {
				if a.logTab == "runtime" {
					ui.Text(c, "暂无软件运行日志").FontSize(12).FontWeight(weightSemibold).TextColor(p.TextSecondary)
					ui.Text(c, "启动、刷新异常和操作结果会记录在这里").FontSize(10.5).TextColor(p.TextMuted)
				} else {
					ui.Text(c, "暂无命令执行日志").FontSize(12).FontWeight(weightSemibold).TextColor(p.TextSecondary)
					ui.Text(c, "拉取、推送、提交、合并等命令的执行结果会记录在这里").FontSize(10.5).TextColor(p.TextMuted)
				}
			})
			return
		}

		ui.Scroll(c).Grow(1).Children(func() {
			for i := len(entries) - 1; i >= 0; i-- {
				entry := entries[i]
				kc := c.Key(fmt.Sprintf("log-row-%s-%d", a.logTab, i))
				row := ui.Row(kc).Padding(5, 10).Gap(8).AlignItems(ui.Center).BorderWidth(0, 0, 1, 0).BorderColor(p.Border)
				if row.Hovered() {
					row.Background(p.SurfaceHover)
				}
				row.Children(func() {
					ui.Text(kc, entry.At.Format("15:04:05")).Font("SF Mono, Menlo, monospace").FontSize(10).
						TextColor(p.TextMuted).Width(64).Shrink(0)
					fg, bg := logBadgeColors(entry.Level, p)
					badgeBox(kc, string(entry.Level), fg, bg, ui.Color{}, 9, true)
					ui.Text(kc, entry.Message).FontSize(11.5).TextColor(p.TextPrimary).Grow(1).MaxLines(2)
					if entry.Detail != "" {
						ui.Text(kc, entry.Detail).Font("SF Mono, Menlo, monospace").FontSize(9.5).
							TextColor(p.TextMuted).MaxWidth(360).MaxLines(1).Shrink(0)
					}
				})
			}
		})
	})
}

func logBadgeColors(level appLogLevel, p Palette) (fg, bg ui.Color) {
	switch level {
	case logRun:
		return p.AccentSoftText, p.AccentSoft
	case logOK:
		return p.Green, p.GreenSoft
	case logSkip, logWarn:
		return p.Amber, p.AmberSoft
	case logFail, logError, logCancel:
		return p.Danger, p.DangerSoft
	default:
		return p.Blue, p.BlueSoft
	}
}
