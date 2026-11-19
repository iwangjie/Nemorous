package main

import (
	"embed"
	"os/exec"
	"runtime"
	"strings"
	"sync"

	"github.com/egoist/mygo"
	"github.com/egoist/mygo/ui"
)

//go:embed resources/appicons/*.png
var appIconFS embed.FS

// appIconChoice is one selectable application icon variant in Settings.
type appIconChoice struct {
	ID       string
	Label    string
	Subtitle string
}

const defaultAppIcon = "aperture"

var appIconChoices = []appIconChoice{
	{ID: "aperture", Label: "焦距", Subtitle: "Agent 精密核"},
	{ID: "slash", Label: "锐切", Subtitle: "极速去冗"},
	{ID: "prism", Label: "棱构", Subtitle: "轻盈架构"},
	{ID: "orbit", Label: "环轨", Subtitle: "静谧流态"},
	{ID: "aero", Label: "飞矢", Subtitle: "原生疾速"},
}

func normalizeAppIcon(id string) string {
	for _, ch := range appIconChoices {
		if id == ch.ID {
			return id
		}
	}
	return defaultAppIcon
}

func appIconLabel(id string) string {
	for _, ch := range appIconChoices {
		if id == ch.ID {
			return ch.Label
		}
	}
	return "焦距"
}

func appIconAssetKey(id string, dark bool) string {
	id = normalizeAppIcon(id)
	mode := "light"
	if dark {
		mode = "dark"
	}
	return id + "_" + mode
}

func appIconPNG(id string, dark bool) []byte {
	key := appIconAssetKey(id, dark)
	data, err := appIconFS.ReadFile("resources/appicons/" + key + ".png")
	if err != nil {
		return nil
	}
	return data
}

var (
	appIconImgMu    sync.Mutex
	appIconImgCache = make(map[string]ui.ImageSource)
)

func appIconPreviewImage(id string, dark bool) ui.ImageSource {
	key := appIconAssetKey(id, dark)
	appIconImgMu.Lock()
	if img, ok := appIconImgCache[key]; ok {
		appIconImgMu.Unlock()
		return img
	}
	appIconImgMu.Unlock()

	data := appIconPNG(id, dark)
	if len(data) == 0 {
		return nil
	}
	img, err := ui.DecodeImage(data)
	if err != nil {
		return nil
	}
	appIconImgMu.Lock()
	appIconImgCache[key] = img
	appIconImgMu.Unlock()
	return img
}

// detectOSSystemDark returns the OS-level dark appearance, independent of any
// per-app light/dark theme override.
func detectOSSystemDark(fallback bool) bool {
	if runtime.GOOS == "darwin" {
		out, err := exec.Command("defaults", "read", "-g", "AppleInterfaceStyle").Output()
		if err != nil {
			// macOS omits AppleInterfaceStyle when in Light Mode.
			return false
		}
		return strings.EqualFold(strings.TrimSpace(string(out)), "dark")
	}
	return fallback
}

func (a *App) setAppIcon(id string) {
	id = normalizeAppIcon(id)
	a.cfg.AppIcon = id
	_ = a.cfg.Save()
	a.applyAppIcon()
	if a.win != nil {
		a.win.Invalidate()
	}
}

// refreshOSDarkAndAppIcon queries the OS dark/light appearance off the UI
// thread and updates the Dock/window icon to match.
func (a *App) refreshOSDarkAndAppIcon() {
	if !mygo.App.IsReady() {
		return
	}
	fallback := mygo.Theme.IsDark()
	if runtime.GOOS != "darwin" {
		a.osDark = fallback
		a.applyAppIcon()
		return
	}
	go func() {
		dark := detectOSSystemDark(fallback)
		a.runUI(func() {
			changed := a.osDark != dark
			a.osDark = dark
			a.applyAppIcon()
			if changed && a.win != nil {
				a.win.Invalidate()
			}
		})
	}()
}

// applyAppIcon updates the native Dock icon (macOS) and window icon
// (Windows/Linux) using the selected icon style and current OS appearance.
func (a *App) applyAppIcon() {
	if !mygo.App.IsReady() {
		return
	}
	key := appIconAssetKey(a.cfg.AppIcon, a.osDark)
	if key == a.appliedIconKey {
		return
	}
	png := appIconPNG(a.cfg.AppIcon, a.osDark)
	if len(png) == 0 {
		return
	}
	a.appliedIconKey = key
	_ = mygo.App.Dock.SetIcon(png)
	if a.win != nil {
		_ = a.win.SetIcon(png)
	}
}
