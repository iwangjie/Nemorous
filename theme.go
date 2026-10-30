package main

import (
	"github.com/egoist/mygo"
	"github.com/egoist/mygo/ui"
)

const (
	weightMedium   = 500
	weightSemibold = 600
	weightBold     = 700
)

// themeChoice is one entry in the appearance selector.
type themeChoice struct {
	ID    string
	Label string
}

var themeChoices = []themeChoice{
	{"light", "明亮"},
	{"dark", "暗色"},
	{"darcula", "Darcula"},
	{"system", "跟随系统"},
}

func normalizeTheme(theme string) string {
	for _, choice := range themeChoices {
		if theme == choice.ID {
			return theme
		}
	}
	return "light"
}

func themeLabel(theme string) string {
	for _, choice := range themeChoices {
		if theme == choice.ID {
			return choice.Label
		}
	}
	return "明亮"
}

// nativeThemeSource maps the app theme to the native title bar and menu
// appearance. Darcula is a dark palette, so it forces a dark native chrome.
func nativeThemeSource(theme string) mygo.ThemeSource {
	switch normalizeTheme(theme) {
	case "light":
		return mygo.ThemeLight
	case "dark", "darcula":
		return mygo.ThemeDark
	default:
		return mygo.ThemeSystem
	}
}

// Palette implements the Magpie color system:
// "one quiet surface, real logos, controls that read as pop-up buttons."
type Palette struct {
	IsDark       bool
	AppBg        ui.Color // --bg: editor/content canvas (IDEA paints the editor dark)
	RailBg       ui.Color // --card-2: tool window chrome (sidebar, toolbars, status bar)
	Surface      ui.Color // --card: elevated primary card / content surface (editor cards)
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

	Accent         ui.Color // --accent (indigo)
	AccentHover    ui.Color
	AccentSoft     ui.Color // --accent-soft
	AccentSoftText ui.Color // readable accent text on AccentSoft
	AccentBorder   ui.Color
	AccentText     ui.Color // --accent-fg

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
		TextMuted:     ui.Hex("#6F6F78"),

		Border:       ui.Hex("#E3E3E8"),
		BorderSubtle: ui.Hex("#ECECF0"),
		BorderStrong: ui.Hex("#D4D4DC"),

		Accent:         ui.Hex("#4F46E5"),
		AccentHover:    ui.Hex("#4338CA"),
		AccentSoft:     ui.Hex("#EEEDFD"),
		AccentSoftText: ui.Hex("#4338CA"),
		AccentBorder:   ui.Hex("#C7D2FE"),
		AccentText:     ui.Hex("#FFFFFF"),

		Green:       ui.Hex("#0F7A45"),
		GreenSoft:   ui.Hex("#E4F6EC"),
		GreenBorder: ui.Hex("#A7F3D0"),

		Amber:       ui.Hex("#B45309"),
		AmberSoft:   ui.Hex("#FBF0DF"),
		AmberBorder: ui.Hex("#FDE68A"),

		Danger:       ui.Hex("#C43D3D"),
		DangerSoft:   ui.Hex("#FBE9E9"),
		DangerBorder: ui.Hex("#FECACA"),

		Blue:       ui.Hex("#0369A1"),
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
		AppBg:        ui.Hex("#101114"),
		RailBg:       ui.Hex("#17181C"),
		Surface:      ui.Hex("#1B1D22"),
		Surface2:     ui.Hex("#24262D"),
		Surface3:     ui.Hex("#0E0F12"),
		SurfaceHover: ui.Hex("#30333B"),
		ActiveRowBg:  ui.Hex("#22344A"),

		TextPrimary:   ui.Hex("#F1F3F5"),
		TextSecondary: ui.Hex("#C3C8D0"),
		TextControl:   ui.Hex("#AAB1BB"),
		TextMuted:     ui.Hex("#8B929C"),

		Border:       ui.Hex("#2B2F36"),
		BorderSubtle: ui.Hex("#24272D"),
		BorderStrong: ui.Hex("#41464F"),

		Accent:         ui.Hex("#2563EB"),
		AccentHover:    ui.Hex("#1D4ED8"),
		AccentSoft:     ui.Hex("#16304F"),
		AccentSoftText: ui.Hex("#9CC5FF"),
		AccentBorder:   ui.Hex("#2E5F9E"),
		AccentText:     ui.Hex("#FFFFFF"),

		Green:       ui.Hex("#5BD18B"),
		GreenSoft:   ui.Hex("#153A26"),
		GreenBorder: ui.Hex("#2A6A45"),

		Amber:       ui.Hex("#F5C451"),
		AmberSoft:   ui.Hex("#3A2E16"),
		AmberBorder: ui.Hex("#6A5528"),

		Danger:       ui.Hex("#FF7B85"),
		DangerSoft:   ui.Hex("#3A1D24"),
		DangerBorder: ui.Hex("#6E303B"),

		Blue:       ui.Hex("#6CB8FF"),
		BlueSoft:   ui.Hex("#172E45"),
		BlueBorder: ui.Hex("#2B5378"),

		Purple:       ui.Hex("#C4A7FF"),
		PurpleSoft:   ui.Hex("#2C2440"),
		PurpleBorder: ui.Hex("#4F3E70"),
	}
}

// darculaPalette follows IntelliJ IDEA's classic dark editor chrome: #3D3F41
// tool windows and toolbars wrapped around a #2B2B2B editor, muted blue-grey
// text, #45494A buttons and #3574C8 actions. Regions keep IDEA's mapping:
// tool windows, toolbars and the status bar use RailBg, while editor content
// and cards use Surface/AppBg.
func darculaPalette() Palette {
	return Palette{
		IsDark:       true,
		AppBg:        ui.Hex("#2B2B2B"),
		RailBg:       ui.Hex("#3D3F41"),
		Surface:      ui.Hex("#2B2B2B"),
		Surface2:     ui.Hex("#45494A"),
		Surface3:     ui.Hex("#323232"),
		SurfaceHover: ui.Hex("#4C5052"),
		ActiveRowBg:  ui.Hex("#0D293E"),

		TextPrimary:   ui.Hex("#BCBEC4"),
		TextSecondary: ui.Hex("#A9B7C6"),
		TextControl:   ui.Hex("#AEB6C0"),
		TextMuted:     ui.Hex("#8A919A"),

		Border:       ui.Hex("#323232"),
		BorderSubtle: ui.Hex("#2C2C2C"),
		BorderStrong: ui.Hex("#4C5052"),

		Accent:         ui.Hex("#3574C8"),
		AccentHover:    ui.Hex("#2F6DB5"),
		AccentSoft:     ui.Hex("#1E3A5F"),
		AccentSoftText: ui.Hex("#9CC5EA"),
		AccentBorder:   ui.Hex("#2F5C86"),
		AccentText:     ui.Hex("#FFFFFF"),

		Green:       ui.Hex("#82C98F"),
		GreenSoft:   ui.Hex("#294436"),
		GreenBorder: ui.Hex("#3B5F45"),

		Amber:       ui.Hex("#D6A45A"),
		AmberSoft:   ui.Hex("#3A3024"),
		AmberBorder: ui.Hex("#5A4A32"),

		Danger:       ui.Hex("#F07178"),
		DangerSoft:   ui.Hex("#3A2428"),
		DangerBorder: ui.Hex("#6A3A3D"),

		Blue:       ui.Hex("#6CB8FF"),
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

// iconAction renders a square icon-only action with a tooltip in place of a
// visible text label.
func iconAction(c *ui.Context, p Palette, key string, svg *ui.SVG, tooltip string, primary bool, onClick func()) {
	bg := p.Surface2
	iconCol := p.TextControl
	hover := p.SurfaceHover
	if primary {
		bg = p.Accent
		iconCol = p.AccentText
		hover = p.AccentHover
	}
	kc := c.Key(key)
	el := ui.Row(kc).Size(28, 28).Radius(6).Background(bg).Cursor(ui.CursorPointer).
		AlignItems(ui.Center).Justify(ui.Center).Tooltip(tooltip).Label(tooltip)
	if el.Hovered() {
		el.Background(hover)
	}
	if el.Clicked() && onClick != nil {
		onClick()
	}
	el.Children(func() {
		drawIcon(kc, svg, 14, iconCol)
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
		fg = p.AccentSoftText
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
