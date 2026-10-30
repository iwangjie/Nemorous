package main

import (
	"math"
	"testing"

	"github.com/egoist/mygo/ui"
)

func relativeLuminance(c ui.Color) float64 {
	channel := func(v uint8) float64 {
		x := float64(v) / 255
		if x <= 0.04045 {
			return x / 12.92
		}
		return math.Pow((x+0.055)/1.055, 2.4)
	}
	return 0.2126*channel(c.R) + 0.7152*channel(c.G) + 0.0722*channel(c.B)
}

func contrastRatio(a, b ui.Color) float64 {
	x, y := relativeLuminance(a), relativeLuminance(b)
	if x < y {
		x, y = y, x
	}
	return (x + 0.05) / (y + 0.05)
}

func TestThemeChoices(t *testing.T) {
	if got := len(themeChoices); got != 4 {
		t.Fatalf("expected four themes, got %d", got)
	}
	if normalizeTheme("unknown") != "light" {
		t.Fatal("unknown theme should fall back to light")
	}
	if nativeThemeSource("darcula") != "dark" || nativeThemeSource("system") != "system" {
		t.Fatal("native appearance sources do not match the theme")
	}
}

func TestThemeContrast(t *testing.T) {
	themes := []struct {
		name string
		p    Palette
	}{
		{"light", lightPalette()},
		{"dark", darkPalette()},
		{"darcula", darculaPalette()},
	}
	for _, theme := range themes {
		p := theme.p
		checks := []struct {
			name string
			fg   ui.Color
			bg   ui.Color
			min  float64
		}{
			{"primary/background", p.TextPrimary, p.AppBg, 4.5},
			{"primary/surface", p.TextPrimary, p.Surface, 4.5},
			{"primary/rail", p.TextPrimary, p.RailBg, 4.5},
			{"primary/control", p.TextPrimary, p.Surface2, 4.5},
			{"secondary/surface", p.TextSecondary, p.Surface, 4.5},
			{"muted/surface", p.TextMuted, p.Surface, 3.0},
			{"control/rail", p.TextControl, p.RailBg, 4.5},
			{"accent-soft-text/soft", p.AccentSoftText, p.AccentSoft, 4.5},
			{"accent-text/accent", p.AccentText, p.Accent, 4.5},
			{"primary/active-row", p.TextPrimary, p.ActiveRowBg, 4.5},
			{"green/surface", p.Green, p.Surface, 3.0},
			{"amber/surface", p.Amber, p.Surface, 3.0},
			{"danger/surface", p.Danger, p.Surface, 3.0},
			{"blue/surface", p.Blue, p.Surface, 3.0},
			{"purple/surface", p.Purple, p.Surface, 3.0},
			{"green/soft", p.Green, p.GreenSoft, 3.0},
			{"amber/soft", p.Amber, p.AmberSoft, 3.0},
			{"danger/soft", p.Danger, p.DangerSoft, 3.0},
			{"blue/soft", p.Blue, p.BlueSoft, 3.0},
			{"purple/soft", p.Purple, p.PurpleSoft, 3.0},
		}
		for _, check := range checks {
			if ratio := contrastRatio(check.fg, check.bg); ratio < check.min {
				t.Errorf("%s: %s contrast %.2f, want >= %.1f", theme.name, check.name, ratio, check.min)
			}
		}
	}
}

func TestRepoBadgeContrast(t *testing.T) {
	for _, dark := range []bool{false, true} {
		for i := 0; i < 7; i++ {
			badge := repoBadgeColors(i, dark)
			if ratio := contrastRatio(badge.Text, badge.Bg); ratio < 3.0 {
				t.Errorf("repo badge %d (dark=%v) contrast %.2f, want >= 3.0", i, dark, ratio)
			}
		}
	}
}
