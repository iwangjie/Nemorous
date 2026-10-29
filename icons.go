package main

import "github.com/egoist/mygo/ui"

func mustSVG(s string) *ui.SVG {
	return ui.MustParseSVG([]byte(s))
}

// Crisp 24x24 Lucide vector icons parsed once at startup.
var (
	icoBranch = mustSVG(`<svg viewBox="0 0 24 24"><line x1="6" y1="3" x2="6" y2="15" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round"/><circle cx="18" cy="6" r="3" fill="none" stroke="currentColor" stroke-width="2"/><circle cx="6" cy="18" r="3" fill="none" stroke="currentColor" stroke-width="2"/><path d="M18 9a9 9 0 0 1-9 9" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round"/></svg>`)

	icoMerge = mustSVG(`<svg viewBox="0 0 24 24"><circle cx="18" cy="18" r="3" fill="none" stroke="currentColor" stroke-width="2"/><circle cx="6" cy="6" r="3" fill="none" stroke="currentColor" stroke-width="2"/><path d="M6 21V9a9 9 0 0 0 9 9" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round"/></svg>`)

	icoCommit = mustSVG(`<svg viewBox="0 0 24 24"><circle cx="12" cy="12" r="3" fill="none" stroke="currentColor" stroke-width="2"/><line x1="3" y1="12" x2="9" y2="12" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round"/><line x1="15" y1="12" x2="21" y2="12" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round"/></svg>`)

	icoFolder = mustSVG(`<svg viewBox="0 0 24 24"><path d="M20 20a2 2 0 0 0 2-2V8a2 2 0 0 0-2-2h-7.9a2 2 0 0 1-1.69-.9L9.6 3.9A2 2 0 0 0 7.93 3H4a2 2 0 0 0-2 2v13a2 2 0 0 0 2 2Z" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linejoin="round"/></svg>`)

	icoFile = mustSVG(`<svg viewBox="0 0 24 24"><path d="M15 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V7Z" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linejoin="round"/><path d="M14 2v4a2 2 0 0 0 2 2h4" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linejoin="round"/></svg>`)

	icoCode = mustSVG(`<svg viewBox="0 0 24 24"><polyline points="16 18 22 12 16 6" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"/><polyline points="8 6 2 12 8 18" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"/></svg>`)

	icoSearch = mustSVG(`<svg viewBox="0 0 24 24"><circle cx="11" cy="11" r="8" fill="none" stroke="currentColor" stroke-width="2"/><path d="m21 21-4.3-4.3" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round"/></svg>`)

	icoRefresh = mustSVG(`<svg viewBox="0 0 24 24"><path d="M3 12a9 9 0 0 1 9-9 9.75 9.75 0 0 1 6.74 2.74L21 8" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"/><path d="M21 3v5h-5" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"/><path d="M21 12a9 9 0 0 1-9 9 9.75 9.75 0 0 1-6.74-2.74L3 16" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"/><path d="M8 16H3v5" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"/></svg>`)

	icoPull = mustSVG(`<svg viewBox="0 0 24 24"><path d="M12 5v14" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round"/><path d="m19 12-7 7-7-7" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"/></svg>`)

	icoPush = mustSVG(`<svg viewBox="0 0 24 24"><path d="M12 19V5" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round"/><path d="m5 12 7-7 7 7" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"/></svg>`)

	icoCheck = mustSVG(`<svg viewBox="0 0 24 24"><path d="M20 6 9 17l-5-5" fill="none" stroke="currentColor" stroke-width="2.2" stroke-linecap="round" stroke-linejoin="round"/></svg>`)

	icoChevronDown = mustSVG(`<svg viewBox="0 0 24 24"><path d="m6 9 6 6 6-6" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"/></svg>`)

	icoChevronRight = mustSVG(`<svg viewBox="0 0 24 24"><path d="m9 18 6-6-6-6" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"/></svg>`)

	icoSettings = mustSVG(`<svg viewBox="0 0 24 24"><circle cx="12" cy="12" r="3" fill="none" stroke="currentColor" stroke-width="2"/><path d="M19.4 15a1.65 1.65 0 0 0 .33 1.82l.06.06a2 2 0 0 1-2.83 2.83l-.06-.06a1.65 1.65 0 0 0-1.82-.33 1.65 1.65 0 0 0-1 1.51V21a2 2 0 0 1-4 0v-.09A1.65 1.65 0 0 0 9 19.4a1.65 1.65 0 0 0-1.82.33l-.06.06a2 2 0 0 1-2.83-2.83l.06-.06a1.65 1.65 0 0 0 .33-1.82 1.65 1.65 0 0 0-1.51-1H3a2 2 0 0 1 0-4h.09A1.65 1.65 0 0 0 4.6 9a1.65 1.65 0 0 0-.33-1.82l-.06-.06a2 2 0 0 1 2.83-2.83l.06.06a1.65 1.65 0 0 0 1.82.33H9a1.65 1.65 0 0 0 1-1.51V3a2 2 0 0 1 4 0v.09a1.65 1.65 0 0 0 1 1.51 1.65 1.65 0 0 0 1.82-.33l.06-.06a2 2 0 0 1 2.83 2.83l-.06.06a1.65 1.65 0 0 0-.33 1.82V9a1.65 1.65 0 0 0 1.51 1H21a2 2 0 0 1 0 4h-.09a1.65 1.65 0 0 0-1.51 1Z" fill="none" stroke="currentColor" stroke-width="1.8"/></svg>`)

	icoSun = mustSVG(`<svg viewBox="0 0 24 24"><circle cx="12" cy="12" r="4" fill="none" stroke="currentColor" stroke-width="2"/><path d="M12 2v2M12 20v2M4.93 4.93l1.41 1.41M17.66 17.66l1.41 1.41M2 12h2M20 12h2M6.34 17.66l-1.41 1.41M19.07 4.93l-1.41 1.41" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round"/></svg>`)

	icoMoon = mustSVG(`<svg viewBox="0 0 24 24"><path d="M12 3a6 6 0 0 0 9 9 9 9 0 1 1-9-9Z" fill="none" stroke="currentColor" stroke-width="2" stroke-linejoin="round"/></svg>`)

	icoEdit = mustSVG(`<svg viewBox="0 0 24 24"><path d="M17 3a2.85 2.83 0 1 1 4 4L7.5 20.5 2 22l1.5-5.5Z" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linejoin="round"/></svg>`)

	icoSparkles = mustSVG(`<svg viewBox="0 0 24 24"><path d="m12 3-1.9 5.8a2 2 0 0 1-1.3 1.3L3 12l5.8 1.9a2 2 0 0 1 1.3 1.3L12 21l1.9-5.8a2 2 0 0 1 1.3-1.3L21 12l-5.8-1.9a2 2 0 0 1-1.3-1.3Z" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linejoin="round"/></svg>`)

	icoMore = mustSVG(`<svg viewBox="0 0 24 24"><circle cx="12" cy="12" r="1.5" fill="currentColor"/><circle cx="19" cy="12" r="1.5" fill="currentColor"/><circle cx="5" cy="12" r="1.5" fill="currentColor"/></svg>`)

	icoPlay = mustSVG(`<svg viewBox="0 0 24 24"><polygon points="6 3 20 12 6 21 6 3" fill="currentColor"/></svg>`)
)

func drawIcon(c *ui.Context, svg *ui.SVG, size float32, color ui.Color) {
	ui.Icon(c, svg).Size(size, size).TextColor(color).Shrink(0)
}
