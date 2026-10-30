package main

import (
	"context"
	"fmt"
	"strings"
	"time"

	"github.com/egoist/mygo"
	"github.com/egoist/mygo/ui"
)

const (
	updateStartupDelay   = 20 * time.Second
	updateIdleGrace      = 30 * time.Second
	updateRequestTimeout = 45 * time.Second
	updateMinMinutes     = 5
	updateMaxMinutes     = 24 * 60
)

// updateVersion is the version compiled into this build by mygo build.
func updateVersion() string {
	if v := strings.TrimSpace(mygo.App.Version()); v != "" {
		return v
	}
	return "dev"
}

// updateIsSkipped reports whether the user asked to skip this version.
func (a *App) updateIsSkipped(version string) bool {
	return version != "" && a.cfg.SkippedUpdateVersion == version
}

// startUpdateLoop checks for signed releases on a timer. The timer is
// restarted when the user changes the interval in settings.
func (a *App) startUpdateLoop() {
	a.stopUpdateLoop()
	if !a.cfg.AutoUpdate || !mygo.Updater.Enabled() {
		return
	}
	interval := time.Duration(a.cfg.UpdateCheckMinutes) * time.Minute
	if interval < updateMinMinutes*time.Minute {
		interval = updateMinMinutes * time.Minute
	}
	ctx, cancel := context.WithCancel(context.Background())
	a.updateLoopCancel = cancel
	go func() {
		initial := time.NewTimer(updateStartupDelay)
		select {
		case <-ctx.Done():
			initial.Stop()
			return
		case <-initial.C:
		}
		a.runUI(func() { a.checkForUpdates(false) })
		timer := time.NewTimer(interval)
		defer timer.Stop()
		for {
			select {
			case <-ctx.Done():
				return
			case <-timer.C:
				a.runUI(func() { a.checkForUpdates(false) })
				timer.Reset(interval)
			}
		}
	}()
}

func (a *App) stopUpdateLoop() {
	if a.updateLoopCancel != nil {
		a.updateLoopCancel()
		a.updateLoopCancel = nil
	}
	a.cancelIdleUpdate()
}

func (a *App) restartUpdateLoop() {
	a.stopUpdateLoop()
	a.startUpdateLoop()
}

func (a *App) configureUpdateSource() {
	mirror := strings.TrimSpace(a.cfg.UpdateMirror)
	if mirror == "" {
		mygo.Updater.SetSource(nil)
		return
	}
	mygo.Updater.SetSource(mirrorUpdateSource{mirror: mirror})
}

// shutdownUpdates stops timers and in-flight update requests when the window
// closes. A completed installer is left to the next launch.
func (a *App) shutdownUpdates() {
	a.stopUpdateLoop()
	if a.updateCheckCancel != nil {
		a.updateCheckCancel()
		a.updateCheckCancel = nil
	}
	if a.updateInstallCancel != nil {
		a.updateInstallCancel()
		a.updateInstallCancel = nil
	}
}

// checkForUpdates asks the signed GitHub release feed for a newer build.
// Automatic checks stay quiet on errors; manual checks explain the result.
func (a *App) checkForUpdates(manual bool) {
	if a.updateChecking || a.updateInstalling {
		if manual {
			a.showToast("info", "正在检查或安装更新")
		}
		return
	}
	if !manual && !a.cfg.AutoUpdate {
		return
	}
	if !mygo.Updater.Enabled() {
		if manual {
			a.showToast("info", "开发构建或包管理器安装不启用自动更新")
		}
		return
	}
	if !manual && a.updatePending != nil {
		if !a.updateIsSkipped(a.updatePending.Version) && !a.updateShowDialog {
			if a.win == nil || a.win.IsFocused() {
				a.offerUpdatePrompt(false)
			}
		}
		return
	}
	a.configureUpdateSource()
	a.updateChecking = true
	a.updateLastErr = ""
	if a.updateCheckCancel != nil {
		a.updateCheckCancel()
	}
	ctx, cancel := context.WithTimeout(context.Background(), updateRequestTimeout)
	a.updateCheckCancel = cancel
	go func() {
		up, err := mygo.Updater.Check(ctx)
		a.runUI(func() {
			a.updateChecking = false
			a.updateCheckCancel = nil
			a.updateLastCheck = time.Now()
			if err != nil {
				a.updateLastErr = err.Error()
				if manual {
					a.showToast("err", "检查更新失败: "+err.Error())
				}
				return
			}
			a.updateLastErr = ""
			if up == nil {
				if manual {
					a.showToast("ok", "已是最新版本 v"+updateVersion())
				}
				return
			}
			a.updatePending = up
			if a.updateIsSkipped(up.Version) && !manual {
				return
			}
			if a.win != nil && !a.win.IsFocused() {
				a.scheduleIdleUpdate()
				return
			}
			a.offerUpdatePrompt(manual)
		})
	}()
}

// offerUpdatePrompt opens the update dialog, unless this version was already
// offered in this session and the user chose "later". force is used by a
// manual check and when an idle install was deferred by unsaved work.
func (a *App) offerUpdatePrompt(force bool) {
	up := a.updatePending
	if up == nil || a.updateInstalling {
		return
	}
	if a.updateIsSkipped(up.Version) && !force {
		return
	}
	if !force && a.updatePromptedFor == up.Version {
		return
	}
	a.updatePromptedFor = up.Version
	a.updateShowDialog = true
	a.updateInstallErr = ""
	a.showSettings = false
}

func (a *App) handleAppForeground() {
	a.cancelIdleUpdate()
	if a.updatePending == nil {
		return
	}
	if a.updateIsSkipped(a.updatePending.Version) {
		return
	}
	if a.updateDeferred {
		a.updateDeferred = false
		a.offerUpdatePrompt(true)
		return
	}
	a.offerUpdatePrompt(false)
}

func (a *App) handleAppBackground() {
	if a.updatePending != nil && !a.updateIsSkipped(a.updatePending.Version) {
		a.scheduleIdleUpdate()
	}
	if a.cfg.AutoUpdate && !a.updateChecking && (a.updateLastCheck.IsZero() || time.Since(a.updateLastCheck) >= 2*time.Minute) {
		a.checkForUpdates(false)
	}
}

func (a *App) scheduleIdleUpdate() {
	if !a.cfg.AutoUpdate || !a.cfg.AutoUpdateIdle || a.updatePending == nil || a.updateInstalling {
		return
	}
	if a.updateIsSkipped(a.updatePending.Version) {
		return
	}
	if a.updateIdleTimer != nil {
		return
	}
	a.updateIdleTimer = time.AfterFunc(updateIdleGrace, func() {
		a.runUI(func() {
			a.updateIdleTimer = nil
			a.tryIdleUpdate()
		})
	})
}

func (a *App) cancelIdleUpdate() {
	if a.updateIdleTimer != nil {
		a.updateIdleTimer.Stop()
		a.updateIdleTimer = nil
	}
}

func (a *App) tryIdleUpdate() {
	if a.updatePending == nil || a.updateInstalling || !a.cfg.AutoUpdateIdle {
		return
	}
	if a.win != nil && a.win.IsFocused() {
		return
	}
	if a.hasUnsavedEdits() || a.hasActiveWork() {
		a.updateDeferred = true
		return
	}
	a.installUpdate(true)
}

// hasUnsavedEdits guards restarting the app: an automatic restart must never
// discard code tabs or a manually edited working diff.
func (a *App) hasUnsavedEdits() bool {
	for _, tab := range a.codeTabs {
		if tab != nil && tab.dirty() {
			return true
		}
	}
	return a.diffModified || (a.diffState != nil && a.diffState.New != nil && a.diffState.New.Modified())
}

func (a *App) hasActiveWork() bool {
	return a.busyLabel != "" || a.mergeRunning || a.maniTaskRunning
}

// installUpdate downloads and installs the pending signed release. The
// updater replaces the app bundle; Relaunch starts the new build. auto is
// true when this came from the idle path, where errors are kept for the next
// foreground prompt.
func (a *App) installUpdate(auto bool) {
	up := a.updatePending
	if up == nil || a.updateInstalling {
		return
	}
	if a.hasUnsavedEdits() {
		if auto {
			a.updateDeferred = true
			return
		}
		a.updateInstallErr = "有未保存的文件，请先保存后再更新"
		a.updateShowDialog = true
		return
	}
	if a.hasActiveWork() {
		if auto {
			a.updateDeferred = true
			return
		}
		a.updateInstallErr = "当前有任务在运行，完成后再更新"
		a.updateShowDialog = true
		return
	}
	if a.updateInstallCancel != nil {
		a.updateInstallCancel()
	}
	ctx, cancel := context.WithCancel(context.Background())
	a.updateInstallCancel = cancel
	a.updateInstalling = true
	a.updateProgress = 0
	a.updateInstallErr = ""
	a.showSettings = false
	if !auto {
		a.updateShowDialog = true
	}
	lastProgress := time.Now().Add(-time.Second)
	go func() {
		err := up.Install(ctx, func(downloaded, total int64) {
			if time.Since(lastProgress) < 120*time.Millisecond && (total <= 0 || downloaded < total) {
				return
			}
			lastProgress = time.Now()
			progress := 0.0
			if total > 0 {
				progress = float64(downloaded) / float64(total)
				if progress > 1 {
					progress = 1
				}
			}
			a.runUI(func() { a.updateProgress = progress })
		})
		a.runUI(func() {
			a.updateInstalling = false
			a.updateInstallCancel = nil
			if err != nil {
				a.updateInstallErr = err.Error()
				a.updateShowDialog = true
				a.updateDeferred = true
				a.showToast("err", "更新安装失败: "+err.Error())
				return
			}
			a.updateShowDialog = false
			a.showToast("ok", "新版本已安装，正在重启...")
			time.AfterFunc(900*time.Millisecond, func() {
				a.runUI(func() { mygo.App.Relaunch() })
			})
		})
	}()
}

// buildUpdateSettings renders the auto-update section inside Settings.
func (a *App) buildUpdateSettings(c *ui.Context, p Palette) {
	ui.Column(c).FillWidth().Padding(14).Gap(10).Radius(8).Background(p.RailBg).Border(1, p.BorderSubtle).Children(func() {
		ui.Row(c).Justify(ui.SpaceBetween).AlignItems(ui.Center).Children(func() {
			ui.Text(c, "自动更新").FontSize(12.5).FontWeight(weightSemibold).TextColor(p.TextPrimary)
			ui.Text(c, "当前 v"+updateVersion()).FontSize(10.5).TextColor(p.TextMuted)
		})

		ui.Row(c).Gap(8).AlignItems(ui.Center).Children(func() {
			if ui.Checkbox(c, &a.cfg.AutoUpdate, "前台定时检查更新").Changed() {
				_ = a.cfg.Save()
				a.restartUpdateLoop()
			}
			ui.Text(c, "每").FontSize(11.5).TextColor(p.TextSecondary)
			if ui.NumberInput(c.Key("update-interval"), &a.updateMinutesDraft, updateMinMinutes, updateMaxMinutes, 5).Width(72).Changed() {
				minutes := int(a.updateMinutesDraft)
				if minutes < updateMinMinutes {
					minutes = updateMinMinutes
				}
				if minutes > updateMaxMinutes {
					minutes = updateMaxMinutes
				}
				a.updateMinutesDraft = float64(minutes)
				if a.cfg.UpdateCheckMinutes != minutes {
					a.cfg.UpdateCheckMinutes = minutes
					_ = a.cfg.Save()
					a.restartUpdateLoop()
				}
			}
			ui.Text(c, "分钟").FontSize(11.5).TextColor(p.TextSecondary)
		})

		ui.Row(c).Gap(8).AlignItems(ui.Center).Children(func() {
			if ui.Checkbox(c, &a.cfg.AutoUpdateIdle, "后台自动安装并重启").Changed() {
				_ = a.cfg.Save()
				if !a.cfg.AutoUpdateIdle {
					a.cancelIdleUpdate()
				} else if a.updatePending != nil && a.win != nil && !a.win.IsFocused() {
					a.scheduleIdleUpdate()
				}
			}
			ui.Text(c, "切到后台 30 秒后执行；有未保存改动或任务运行时会延后").FontSize(10.5).TextColor(p.TextMuted)
		})

		ui.Row(c).Gap(8).AlignItems(ui.Center).Children(func() {
			ui.Text(c, "下载镜像").FontSize(11.5).TextColor(p.TextSecondary).Width(58).Shrink(0)
			if ui.TextInput(c.Key("update-mirror"), &a.cfg.UpdateMirror).Grow(1).Changed() {
				a.cfg.UpdateMirror = strings.TrimSpace(a.cfg.UpdateMirror)
				_ = a.cfg.Save()
				a.configureUpdateSource()
			}
			ui.Text(c, "留空则直连 GitHub").FontSize(10).TextColor(p.TextMuted).Shrink(0)
		})
		ui.Text(c, "镜像只加速下载；更新包仍由内置公钥验签。").FontSize(10).TextColor(p.TextMuted)

		ui.Row(c).Gap(8).AlignItems(ui.Center).Children(func() {
			if a.updatePending != nil && !a.updateIsSkipped(a.updatePending.Version) {
				pill(c, p, "查看 v"+a.updatePending.Version, true, func() {
					a.offerUpdatePrompt(true)
				})
			} else {
				miniBtn(c, p, "立即检查", func() {
					a.showSettings = false
					a.checkForUpdates(true)
				})
			}
			status := "尚未检查"
			switch {
			case a.updateChecking:
				status = "正在检查..."
			case !a.updateLastCheck.IsZero():
				status = "上次检查 " + a.updateLastCheck.Format("15:04")
			}
			if !mygo.Updater.Enabled() {
				status = "开发构建或包管理器安装不启用自动更新"
			}
			if a.updateLastErr != "" {
				status += " · " + truncateRunes(a.updateLastErr, 42)
			}
			ui.Text(c, status).FontSize(10.5).TextColor(p.TextMuted).Grow(1).MaxLines(1).SingleLine()
		})
	})
}

// buildUpdateModal shows the foreground update prompt and install progress.
func (a *App) buildUpdateModal(c *ui.Context, p Palette) {
	open := a.updateShowDialog
	ui.DialogBase(c, &open, func(backdrop, panel ui.Element) {
		if !open || a.updatePending == nil {
			a.updateShowDialog = false
			return
		}
		up := a.updatePending
		backdrop.Background(ui.RGBA(20, 22, 30, 0.36))
		panel.Width(560).Padding(18).Gap(12).Radius(12).Background(p.Surface).Border(1, p.Border)
		ui.Row(c).Justify(ui.SpaceBetween).AlignItems(ui.Center).Children(func() {
			ui.Text(c, "软件更新").FontSize(15).FontWeight(weightBold).TextColor(p.TextPrimary)
			ui.Text(c, "当前 v"+updateVersion()).FontSize(10.5).TextColor(p.TextMuted)
		})
		if a.updateInstalling {
			ui.Text(c, "正在下载并安装 v"+up.Version+"…").FontSize(12.5).TextColor(p.TextPrimary)
			progress := a.updateProgress
			if progress <= 0 {
				progress = 0
			}
			ui.Progress(c, progress).Height(6).FillWidth()
			ui.Text(c, fmt.Sprintf("%.0f%% · 安装完成后自动重启", progress*100)).FontSize(10.5).TextColor(p.TextMuted)
			ui.Row(c).Justify(ui.End).Children(func() {
				miniBtn(c, p, "隐藏", func() { a.updateShowDialog = false })
			})
			return
		}
		ui.Text(c, "发现新版本 v"+up.Version).FontSize(13).FontWeight(weightSemibold).TextColor(p.TextPrimary)
		if !up.Date.IsZero() {
			ui.Text(c, "发布于 "+up.Date.Format("2006-01-02")).FontSize(10.5).TextColor(p.TextMuted)
		}
		notes := strings.TrimSpace(up.Notes)
		if notes == "" {
			notes = "此版本没有附加更新说明。"
		}
		ui.Scroll(c).MaxHeight(190).Children(func() {
			ui.Text(c, notes).FontSize(11.5).TextColor(p.TextSecondary).MaxLines(20)
		})
		if a.updateInstallErr != "" {
			ui.Text(c, a.updateInstallErr).FontSize(11).TextColor(p.Danger).MaxLines(3)
		}
		ui.Row(c).Justify(ui.End).Gap(8).AlignItems(ui.Center).Children(func() {
			miniBtn(c, p, "稍后", func() { a.updateShowDialog = false })
			miniBtn(c, p, "跳过此版本", func() {
				a.cfg.SkippedUpdateVersion = up.Version
				_ = a.cfg.Save()
				a.updatePending = nil
				a.updateShowDialog = false
			})
			pill(c, p, "立即更新", true, func() { a.installUpdate(false) })
		})
	})
	a.updateShowDialog = open
}
