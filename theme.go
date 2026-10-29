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
		AppBg:        ui.Hex("#1A1A1E"),
		RailBg:       ui.Hex("#1F1F23"),
		Surface:      ui.Hex("#232327"),
		Surface2:     ui.Hex("#2D2D33"),
		Surface3:     ui.Hex("#151518"),
		SurfaceHover: ui.Hex("#36363D"),
		ActiveRowBg:  ui.Hex("#33345A"),

		TextPrimary:   ui.Hex("#EDEDF1"),
		TextSecondary: ui.Hex("#B9B9C1"),
		TextControl:   ui.Hex("#A1A1AA"),
		TextMuted:     ui.Hex("#8B8B94"),

		Border:       ui.Hex("#333339"),
		BorderSubtle: ui.Hex("#2C2C32"),
		BorderStrong: ui.Hex("#42424A"),

		Accent:       ui.Hex("#6366F1"),
		AccentHover:  ui.Hex("#818CF8"),
		AccentSoft:   ui.Hex("#2B2C47"),
		AccentBorder: ui.Hex("#4338CA"),
		AccentText:   ui.Hex("#FFFFFF"),

		Green:       ui.Hex("#6FD99B"),
		GreenSoft:   ui.Hex("#1E3A2B"),
		GreenBorder: ui.Hex("#27533B"),

		Amber:       ui.Hex("#F2B544"),
		AmberSoft:   ui.Hex("#3F3220"),
		AmberBorder: ui.Hex("#5C482B"),

		Danger:       ui.Hex("#F28B82"),
		DangerSoft:   ui.Hex("#45272A"),
		DangerBorder: ui.Hex("#63363A"),

		Blue:       ui.Hex("#7DD3FC"),
		BlueSoft:   ui.Hex("#1E2F3D"),
		BlueBorder: ui.Hex("#2B485E"),

		Purple:       ui.Hex("#C4B5FD"),
		PurpleSoft:   ui.Hex("#2E2640"),
		PurpleBorder: ui.Hex("#463963"),
	}
}

// repoBadgeColors uses Magpie's qualitative series (--c1 to --c7).
func repoBadgeColors(idx int, isDark bool) RepoBadgeStyle {
	if isDark {
		styles := []RepoBadgeStyle{
			{Bg: ui.Hex("#2B2C47"), Text: ui.Hex("#A5B4FC")},
			{Bg: ui.Hex("#1E2F3D"), Text: ui.Hex("#7DD3FC")},
			{Bg: ui.Hex("#1E3A2B"), Text: ui.Hex("#6EE7B7")},
			{Bg: ui.Hex("#3F3220"), Text: ui.Hex("#FCD34D")},
			{Bg: ui.Hex("#3D2431"), Text: ui.Hex("#F9A8D4")},
			{Bg: ui.Hex("#2E2640"), Text: ui.Hex("#C4B5FD")},
			{Bg: ui.Hex("#272B33"), Text: ui.Hex("#94A3B8")},
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
