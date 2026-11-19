package main

import (
	"bytes"
	"path/filepath"
	"testing"

	"github.com/egoist/mygo/ui"
	"github.com/iwangjie/nemorous/internal/config"
)

func TestAppIconChoicesAndAssets(t *testing.T) {
	if got := len(appIconChoices); got != 5 {
		t.Fatalf("expected 5 app icon choices, got %d", got)
	}
	if defaultAppIcon != "aperture" {
		t.Fatalf("expected default app icon to be aperture, got %q", defaultAppIcon)
	}
	if normalizeAppIcon("") != "aperture" || normalizeAppIcon("unknown") != "aperture" {
		t.Fatal("empty or unknown app icon should normalize to aperture")
	}
	if appIconLabel("aperture") != "焦距" {
		t.Fatalf("expected aperture label 焦距, got %q", appIconLabel("aperture"))
	}

	for _, ch := range appIconChoices {
		lightPNG := appIconPNG(ch.ID, false)
		darkPNG := appIconPNG(ch.ID, true)
		if len(lightPNG) == 0 {
			t.Fatalf("missing light PNG for %s", ch.ID)
		}
		if len(darkPNG) == 0 {
			t.Fatalf("missing dark PNG for %s", ch.ID)
		}
		if bytes.Equal(lightPNG, darkPNG) {
			t.Fatalf("light and dark PNGs for %s should differ", ch.ID)
		}
		if img := appIconPreviewImage(ch.ID, false); img == nil {
			t.Fatalf("failed to decode light preview image for %s", ch.ID)
		}
		if img := appIconPreviewImage(ch.ID, true); img == nil {
			t.Fatalf("failed to decode dark preview image for %s", ch.ID)
		}
	}
}

func TestSettingsAppIconPicker(t *testing.T) {
	cfgPath := filepath.Join(t.TempDir(), "config.json")
	t.Setenv("NEMOROUS_CONFIG", cfgPath)

	cfg := config.Default()
	if cfg.AppIcon != "aperture" {
		t.Fatalf("expected config.Default().AppIcon == aperture, got %q", cfg.AppIcon)
	}

	app := NewApp(cfg)
	app.showSettings = true

	tester := ui.NewTester(app.BuildUI, 1280, 860)
	tester.Frame()

	for _, ch := range appIconChoices {
		if !tester.HasText(ch.Label) {
			t.Fatalf("expected app icon option %q in Settings", ch.Label)
		}
	}

	tester.Click("锐切")
	tester.Frame()
	if app.cfg.AppIcon != "slash" {
		t.Fatalf("expected clicking 锐切 to set AppIcon=slash, got %q", app.cfg.AppIcon)
	}

	// Verify persistence
	reloaded := config.Load()
	if reloaded.AppIcon != "slash" {
		t.Fatalf("expected persisted AppIcon=slash, got %q", reloaded.AppIcon)
	}

	// Switch OS appearance and verify UI re-renders cleanly
	app.osDark = true
	tester.Frame()
	tester.Click("环轨")
	tester.Frame()
	if app.cfg.AppIcon != "orbit" {
		t.Fatalf("expected clicking 环轨 to set AppIcon=orbit, got %q", app.cfg.AppIcon)
	}
}
