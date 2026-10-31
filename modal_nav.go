package main

import "github.com/egoist/mygo/ui"

// modalNavItem is one destination in a modal's vertical tool-window rail.
type modalNavItem struct {
	ID    string
	Label string
	Group string
	Icon  *ui.SVG
}

// buildModalNav renders an IDEA-style vertical navigation rail. Grouping the
// destinations keeps rarely used operations reachable without a long row of
// tabs across the dialog.
func buildModalNav(c *ui.Context, p Palette, width float32, items []modalNavItem, active string, onSelect func(string)) {
	ui.Column(c).Width(width).FillHeight().Padding(6).Gap(1).Background(p.RailBg).
		BorderWidth(0, 0, 0, 1).BorderColor(p.Border).Children(func() {
		lastGroup := ""
		for _, item := range items {
			item := item
			if item.Group != lastGroup {
				if lastGroup != "" {
					ui.Box(c).Height(1).FillWidth().Background(p.BorderSubtle).Margin(5, 2)
				}
				label := item.Group
				if label == "" {
					label = "常用"
				}
				ui.Text(c, label).FontSize(9).FontWeight(weightSemibold).TextColor(p.TextMuted).Padding(6, 4, 2, 4)
				lastGroup = item.Group
			}
			selected := active == item.ID
			kc := c.Key("modalnav-" + item.ID)
			row := ui.Row(kc).Height(30).FillWidth().Padding(0, 8).Gap(8).Radius(5).Cursor(ui.CursorPointer).AlignItems(ui.Center)
			fg := p.TextSecondary
			if selected {
				row.Background(p.AccentSoft)
				fg = p.AccentSoftText
			} else if row.Hovered() {
				row.Background(p.SurfaceHover)
				fg = p.TextPrimary
			}
			if row.Clicked() {
				onSelect(item.ID)
			}
			row.Children(func() {
				if item.Icon != nil {
					drawIcon(kc, item.Icon, 13, fg)
				}
				ui.Text(kc, item.Label).FontSize(11.5).FontWeight(weightMedium).TextColor(fg).SingleLine().Grow(1)
			})
		}
	})
}
