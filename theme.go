package main

import (
	"github.com/egoist/mygo/ui"
)

const (
	weightMedium   = 500
	weightSemibold = 600
	weightBold     = 700
)

// Palette implements the Magpie color system:
// "one quiet surface, real logos, controls that read as pop-up buttons."
type Palette struct {
	IsDark       bool
	AppBg        ui.Color // --bg: quiet canvas
	RailBg       ui.Color // --card-2: secondary quiet surface
	Surface      ui.Color // --card: elevated primary card / content surface
	Surface2     ui.Color // --pill: control / secondary surface
	Surface3     ui.Color // --seg-track: segmented control track
	SurfaceHover ui.Color // --pill-hover
	ActiveRowBg  ui.Color // --sel: selected row highlight

	TextPrimary   ui.Color // --fg
	TextSecondary ui.Color // --fg-2
	TextControl   ui.Color // --ctl-fg
	TextMuted     ui.Color // --muted

	Border       ui.Color // --line
	BorderSubtle ui.Color // --line-2
	BorderStrong ui.Color // emphasized border

	Accent       ui.Color // --accent (indigo)
	AccentHover  ui.Color
	AccentSoft   ui.Color // --accent-soft
	AccentBorder ui.Color
	AccentText   ui.Color // --accent-fg

	Green       ui.Color // --green
	GreenSoft   ui.Color // --green-soft
	GreenBorder ui.Color

	Amber       ui.Color // --amber
	AmberSoft   ui.Color // --amber-soft
	AmberBorder ui.Color

	Danger       ui.Color // --red
	DangerSoft   ui.Color // --red-soft
	DangerBorder ui.Color

	Blue       ui.Color
	BlueSoft   ui.Color
	BlueBorder ui.Color

	Purple       ui.Color
	PurpleSoft   ui.Color
	PurpleBorder ui.Color
}

// RepoBadgeStyle defines the subtle monogram badge colors for a repository.
type RepoBadgeStyle struct {
	Bg     ui.Color
	Text   ui.Color
	Border ui.Color
}

func lightPalette() Palette {
	return Palette{
		IsDark:       false,
		AppBg:        ui.Hex("#F4F4F6"),
		RailBg:       ui.Hex("#F9F9FB"),
		Surface:      ui.Hex("#FFFFFF"),
		Surface2:     ui.Hex("#F1F1F4"),
		Surface3:     ui.Hex("#E6E6EB"),
		SurfaceHover: ui.Hex("#E9E9EE"),
		ActiveRowBg:  ui.Hex("#ECEBFB"),

		TextPrimary:   ui.Hex("#1C1C21"),
		TextSecondary: ui.Hex("#4B4B53"),
		TextControl:   ui.Hex("#63636B"),
		TextMuted:     ui.Hex("#85858D"),

		Border:       ui.Hex("#E3E3E8"),
		BorderSubtle: ui.Hex("#ECECF0"),
		BorderStrong: ui.Hex("#D4D4DC"),

		Accent:       ui.Hex("#4F46E5"),
		AccentHover:  ui.Hex("#4338CA"),
		AccentSoft:   ui.Hex("#EEEDFD"),
		AccentBorder: ui.Hex("#C7D2FE"),
		AccentText:   ui.Hex("#FFFFFF"),

		Green:       ui.Hex("#189A58"),
		GreenSoft:   ui.Hex("#E4F6EC"),
		GreenBorder: ui.Hex("#A7F3D0"),

		Amber:       ui.Hex("#B45309"),
		AmberSoft:   ui.Hex("#FBF0DF"),
		AmberBorder: ui.Hex("#FDE68A"),

		Danger:       ui.Hex("#D64545"),
		DangerSoft:   ui.Hex("#FBE9E9"),
		DangerBorder: ui.Hex("#FECACA"),

		Blue:       ui.Hex("#0284C7"),
		BlueSoft:   ui.Hex("#E0F2FE"),
		BlueBorder: ui.Hex("#BAE6FD"),

		Purple:       ui.Hex("#7C3AED"),
		PurpleSoft:   ui.Hex("#F3E8FF"),
		PurpleBorder: ui.Hex("#DDD6FE"),
	}
}

func darkPalette() Palette {
	return Palette{
		IsDark:       true,
		AppBg:        ui.Hex("#2B2B2B"),
		RailBg:       ui.Hex("#3D3F41"),
		Surface:      ui.Hex("#2B2B2B"),
		Surface2:     ui.Hex("#3D3F41"),
		Surface3:     ui.Hex("#323232"),
		SurfaceHover: ui.Hex("#4C5052"),
		ActiveRowBg:  ui.Hex("#0D293E"),

		TextPrimary:   ui.Hex("#BCBEC4"),
		TextSecondary: ui.Hex("#A9B7C6"),
		TextControl:   ui.Hex("#9DA5AF"),
		TextMuted:     ui.Hex("#737980"),

		Border:       ui.Hex("#323232"),
		BorderSubtle: ui.Hex("#2C2C2C"),
		BorderStrong: ui.Hex("#4C5052"),

		Accent:       ui.Hex("#4A88C7"),
		AccentHover:  ui.Hex("#5C9BD9"),
		AccentSoft:   ui.Hex("#1E3A5F"),
		AccentBorder: ui.Hex("#2F5C86"),
		AccentText:   ui.Hex("#FFFFFF"),

		Green:       ui.Hex("#6AAB73"),
		GreenSoft:   ui.Hex("#294436"),
		GreenBorder: ui.Hex("#3B5F45"),

		Amber:       ui.Hex("#D6A45A"),
		AmberSoft:   ui.Hex("#3A3024"),
		AmberBorder: ui.Hex("#5A4A32"),

		Danger:       ui.Hex("#F07178"),
		DangerSoft:   ui.Hex("#3A2428"),
		DangerBorder: ui.Hex("#6A3A3D"),

		Blue:       ui.Hex("#4A88C7"),
		BlueSoft:   ui.Hex("#1D2F42"),
		BlueBorder: ui.Hex("#31516F"),

		Purple:       ui.Hex("#B3A0E8"),
		PurpleSoft:   ui.Hex("#2E2A3D"),
		PurpleBorder: ui.Hex("#4A4160"),
	}
}

// repoBadgeColors uses Magpie's qualitative series (--c1 to --c7).
func repoBadgeColors(idx int, isDark bool) RepoBadgeStyle {
	if isDark {
		styles := []RepoBadgeStyle{
			{Bg: ui.Hex("#1E3A5F"), Text: ui.Hex("#7CB7E8")},
			{Bg: ui.Hex("#183A3A"), Text: ui.Hex("#6FC7C0")},
			{Bg: ui.Hex("#2A3D2F"), Text: ui.Hex("#8CCB8C")},
			{Bg: ui.Hex("#3A3024"), Text: ui.Hex("#D6A45A")},
			{Bg: ui.Hex("#3A2730"), Text: ui.Hex("#E8A2B8")},
			{Bg: ui.Hex("#2E2A3D"), Text: ui.Hex("#B3A0E8")},
			{Bg: ui.Hex("#32363A"), Text: ui.Hex("#A9B7C6")},
		}
		return styles[(idx%len(styles)+len(styles))%len(styles)]
	}
	styles := []RepoBadgeStyle{
		{Bg: ui.Hex("#EEEDFD"), Text: ui.Hex("#4F46E5")},
		{Bg: ui.Hex("#E0F2FE"), Text: ui.Hex("#0284C7")},
		{Bg: ui.Hex("#E4F6EC"), Text: ui.Hex("#189A58")},
		{Bg: ui.Hex("#FBF0DF"), Text: ui.Hex("#B45309")},
		{Bg: ui.Hex("#FCE7F3"), Text: ui.Hex("#DB2777")},
		{Bg: ui.Hex("#F3E8FF"), Text: ui.Hex("#7C3AED")},
		{Bg: ui.Hex("#F1F5F9"), Text: ui.Hex("#475569")},
	}
	return styles[(idx%len(styles)+len(styles))%len(styles)]
}

// pill renders a Magpie-style pop-up control button.
func pill(c *ui.Context, p Palette, label string, primary bool, onClick func()) {
	bg := p.Surface2
	fg := p.TextPrimary
	hover := p.SurfaceHover
	if primary {
		bg = p.Accent
		fg = p.AccentText
		hover = p.AccentHover
	}
	el := ui.Row(c).Padding(5, 11).Radius(6).Background(bg).Cursor(ui.CursorPointer).AlignItems(ui.Center).Shrink(0)
	if el.Hovered() {
		el.Background(hover)
	}
	if el.Clicked() && onClick != nil {
		onClick()
	}
	el.Children(func() {
		ui.Text(c, label).FontSize(12).FontWeight(weightMedium).TextColor(fg).SingleLine()
	})
}

// iconPill renders a Magpie-style pop-up button with a leading SVG icon.
func iconPill(c *ui.Context, p Palette, svg *ui.SVG, label string, primary bool, onClick func()) {
	bg := p.Surface2
	fg := p.TextPrimary
	iconCol := p.TextControl
	hover := p.SurfaceHover
	if primary {
		bg = p.Accent
		fg = p.AccentText
		iconCol = p.AccentText
		hover = p.AccentHover
	}
	el := ui.Row(c).Padding(5, 10).Gap(5).Radius(6).Background(bg).Cursor(ui.CursorPointer).AlignItems(ui.Center).Shrink(0)
	if el.Hovered() {
		el.Background(hover)
	}
	if el.Clicked() && onClick != nil {
		onClick()
	}
	el.Children(func() {
		if svg != nil {
			drawIcon(c, svg, 13, iconCol)
		}
		if label != "" {
			ui.Text(c, label).FontSize(12).FontWeight(weightMedium).TextColor(fg).SingleLine()
		}
	})
}

// miniBtn renders a quiet compact control button.
func miniBtn(c *ui.Context, p Palette, label string, onClick func()) {
	el := ui.Row(c).Padding(4, 8).Radius(5).Background(p.Surface2).Cursor(ui.CursorPointer).AlignItems(ui.Center).Shrink(0)
	if el.Hovered() {
		el.Background(p.SurfaceHover)
	}
	if el.Clicked() && onClick != nil {
		onClick()
	}
	el.Children(func() {
		ui.Text(c, label).FontSize(11.5).FontWeight(weightMedium).TextColor(p.TextControl).SingleLine()
	})
}

// ghostBtn renders a borderless subtle text/icon button that shows a pill background only on hover.
func ghostBtn(c *ui.Context, p Palette, svg *ui.SVG, label string, active bool, onClick func()) {
	bg := ui.Transparent
	fg := p.TextControl
	if active {
		bg = p.Surface2
		fg = p.TextPrimary
	}
	el := ui.Row(c).Padding(4, 8).Gap(4).Radius(5).Background(bg).Cursor(ui.CursorPointer).AlignItems(ui.Center).Shrink(0)
	if el.Hovered() {
		el.Background(p.SurfaceHover)
		fg = p.TextPrimary
	}
	if el.Clicked() && onClick != nil {
		onClick()
	}
	el.Children(func() {
		if svg != nil {
			drawIcon(c, svg, 13, fg)
		}
		if label != "" {
			ui.Text(c, label).FontSize(11.5).FontWeight(weightMedium).TextColor(fg).SingleLine()
		}
	})
}

// badgeBox renders a subtle status pill without harsh borders.
func badgeBox(c *ui.Context, text string, fg, bg, border ui.Color, fontSize float32, bold bool) {
	el := ui.Row(c).Padding(2, 7).Radius(5).Background(bg).AlignItems(ui.Center).Shrink(0)
	el.Children(func() {
		t := ui.Text(c, text).FontSize(fontSize).TextColor(fg).SingleLine()
		if bold {
			t.FontWeight(weightSemibold)
		} else {
			t.FontWeight(weightMedium)
		}
	})
}

// menuPill renders a compact pill that opens a popover menu.
func menuPill(c *ui.Context, p Palette, label string, active bool, build func(m *ui.Menu)) {
	bg := p.Surface2
	fg := p.TextControl
	if active {
		bg = p.AccentSoft
		fg = p.Accent
	}
	el := ui.Row(c).Padding(4, 9).Gap(4).Radius(5).Background(bg).Cursor(ui.CursorPointer).AlignItems(ui.Center).Shrink(0)
	if el.Hovered() {
		el.Background(p.SurfaceHover)
	}
	el.PopoverMenuButton(build)
	el.Children(func() {
		ui.Text(c, label).FontSize(11.5).FontWeight(weightMedium).TextColor(fg).SingleLine()
	})
}

// pillDisabled renders a control that is visibly unavailable.
func pillDisabled(c *ui.Context, p Palette, label string) {
	el := ui.Row(c).Padding(5, 11).Radius(6).Background(p.Surface2).AlignItems(ui.Center).Shrink(0)
	el.Children(func() {
		ui.Text(c, label).FontSize(12).FontWeight(weightMedium).TextColor(p.TextMuted).SingleLine()
	})
}
