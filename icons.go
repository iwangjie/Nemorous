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

	icoMinus = mustSVG(`<svg viewBox="0 0 24 24"><path d="M5 12h14" fill="none" stroke="currentColor" stroke-width="2.2" stroke-linecap="round"/></svg>`)

	icoArrowLeft = mustSVG(`<svg viewBox="0 0 24 24"><path d="M19 12H5" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round"/><path d="m12 19-7-7 7-7" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"/></svg>`)

	icoArrowRight = mustSVG(`<svg viewBox="0 0 24 24"><path d="M5 12h14" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round"/><path d="m12 5 7 7-7 7" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"/></svg>`)

	icoChevronDown = mustSVG(`<svg viewBox="0 0 24 24"><path d="m6 9 6 6 6-6" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"/></svg>`)

	icoChevronRight = mustSVG(`<svg viewBox="0 0 24 24"><path d="m9 18 6-6-6-6" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"/></svg>`)
	icoChevronUp    = mustSVG(`<svg viewBox="0 0 24 24"><path d="m6 15 6-6 6 6" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"/></svg>`)
	icoClose        = mustSVG(`<svg viewBox="0 0 24 24"><path d="M18 6 6 18M6 6l12 12" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round"/></svg>`)
	icoPlus         = mustSVG(`<svg viewBox="0 0 24 24"><path d="M12 5v14M5 12h14" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round"/></svg>`)
	icoTrash        = mustSVG(`<svg viewBox="0 0 24 24"><path d="M3 6h18M8 6V4h8v2M19 6l-1 14H6L5 6M10 11v5M14 11v5" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"/></svg>`)
	icoStop         = mustSVG(`<svg viewBox="0 0 24 24"><rect x="6" y="6" width="12" height="12" rx="1" fill="currentColor"/></svg>`)
	icoPause        = mustSVG(`<svg viewBox="0 0 24 24"><path d="M8 5v14M16 5v14" fill="none" stroke="currentColor" stroke-width="2.4" stroke-linecap="round"/></svg>`)
	icoStepOver     = mustSVG(`<svg viewBox="0 0 24 24"><path d="M4 4v5a4 4 0 0 0 4 4h11" fill="none" stroke="currentColor" stroke-width="1.9" stroke-linecap="round"/><path d="m15 9 4 4-4 4" fill="none" stroke="currentColor" stroke-width="1.9" stroke-linecap="round" stroke-linejoin="round"/></svg>`)
	icoStepInto     = mustSVG(`<svg viewBox="0 0 24 24"><path d="M12 3v12" fill="none" stroke="currentColor" stroke-width="1.9" stroke-linecap="round"/><path d="m8 11 4 4 4-4M5 21h14" fill="none" stroke="currentColor" stroke-width="1.9" stroke-linecap="round" stroke-linejoin="round"/></svg>`)
	icoStepOut      = mustSVG(`<svg viewBox="0 0 24 24"><path d="M12 21V9" fill="none" stroke="currentColor" stroke-width="1.9" stroke-linecap="round"/><path d="m8 13 4-4 4 4M5 3h14" fill="none" stroke="currentColor" stroke-width="1.9" stroke-linecap="round" stroke-linejoin="round"/></svg>`)
	icoSend         = mustSVG(`<svg viewBox="0 0 24 24"><path d="m22 2-7 20-4-9-9-4Z" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linejoin="round"/><path d="M22 2 11 13" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round"/></svg>`)
	icoExternal     = mustSVG(`<svg viewBox="0 0 24 24"><path d="M15 3h6v6M10 14 21 3M18 13v6a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2V8a2 2 0 0 1 2-2h6" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"/></svg>`)
	icoComment      = mustSVG(`<svg viewBox="0 0 24 24"><path d="M21 15a2 2 0 0 1-2 2H7l-4 4V5a2 2 0 0 1 2-2h14a2 2 0 0 1 2 2Z" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linejoin="round"/></svg>`)
	icoArchive      = mustSVG(`<svg viewBox="0 0 24 24"><rect x="2" y="4" width="20" height="5" rx="1" fill="none" stroke="currentColor" stroke-width="1.8"/><path d="M4 9v10a1 1 0 0 0 1 1h14a1 1 0 0 0 1-1V9M10 13h4" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round"/></svg>`)
	icoTag          = mustSVG(`<svg viewBox="0 0 24 24"><path d="M12.6 2.6A2 2 0 0 0 11.2 2H4a2 2 0 0 0-2 2v7.2a2 2 0 0 0 .6 1.4l8.7 8.7a2.4 2.4 0 0 0 3.4 0l6.6-6.6a2.4 2.4 0 0 0 0-3.4Z" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linejoin="round"/><circle cx="7.5" cy="7.5" r="1" fill="currentColor"/></svg>`)
	icoKey          = mustSVG(`<svg viewBox="0 0 24 24"><circle cx="7.5" cy="15.5" r="4" fill="none" stroke="currentColor" stroke-width="1.8"/><path d="m10.5 13 8-8m-3 3 3 3m-6 0 3 3" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"/></svg>`)
	icoShield       = mustSVG(`<svg viewBox="0 0 24 24"><path d="M12 22s8-4 8-10V5l-8-3-8 3v7c0 6 8 10 8 10Z" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linejoin="round"/><path d="m9 12 2 2 4-4" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"/></svg>`)
	icoCloud        = mustSVG(`<svg viewBox="0 0 24 24"><path d="M17.5 19H9a7 7 0 1 1 6.7-9h1.8a4.5 4.5 0 1 1 0 9Z" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linejoin="round"/></svg>`)
	icoBox          = mustSVG(`<svg viewBox="0 0 24 24"><path d="m3.3 7 8.7 5 8.7-5M12 22V12" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linejoin="round"/><path d="M21 16V8a2 2 0 0 0-1-1.7l-7-4a2 2 0 0 0-2 0l-7 4A2 2 0 0 0 3 8v8a2 2 0 0 0 1 1.7l7 4a2 2 0 0 0 2 0l7-4A2 2 0 0 0 21 16Z" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linejoin="round"/></svg>`)
	icoList         = mustSVG(`<svg viewBox="0 0 24 24"><path d="M8 6h13M8 12h13M8 18h13" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round"/><circle cx="3.5" cy="6" r="1" fill="currentColor"/><circle cx="3.5" cy="12" r="1" fill="currentColor"/><circle cx="3.5" cy="18" r="1" fill="currentColor"/></svg>`)
	icoRestore      = mustSVG(`<svg viewBox="0 0 24 24"><path d="M3 12a9 9 0 1 0 2.6-6.4L3 8" fill="none" stroke="currentColor" stroke-width="1.9" stroke-linecap="round" stroke-linejoin="round"/><path d="M3 3v5h5" fill="none" stroke="currentColor" stroke-width="1.9" stroke-linecap="round" stroke-linejoin="round"/></svg>`)
	icoBoth         = mustSVG(`<svg viewBox="0 0 24 24"><path d="M4 12h6m-3-3 3 3-3 3M20 12h-6m3-3-3 3 3 3" fill="none" stroke="currentColor" stroke-width="1.9" stroke-linecap="round" stroke-linejoin="round"/></svg>`)
	icoAcceptLeft   = mustSVG(`<svg viewBox="0 0 24 24"><path d="M20 12H8" fill="none" stroke="currentColor" stroke-width="1.9" stroke-linecap="round"/><path d="m13 7-5 5 5 5M4 4v16" fill="none" stroke="currentColor" stroke-width="1.9" stroke-linecap="round" stroke-linejoin="round"/></svg>`)
	icoAcceptRight  = mustSVG(`<svg viewBox="0 0 24 24"><path d="M4 12h12" fill="none" stroke="currentColor" stroke-width="1.9" stroke-linecap="round"/><path d="m11 7 5 5-5 5M20 4v16" fill="none" stroke="currentColor" stroke-width="1.9" stroke-linecap="round" stroke-linejoin="round"/></svg>`)
	icoResolve      = mustSVG(`<svg viewBox="0 0 24 24"><circle cx="12" cy="12" r="9" fill="none" stroke="currentColor" stroke-width="1.8"/><path d="m8 12 3 3 5-6" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"/></svg>`)
	icoColumns      = mustSVG(`<svg viewBox="0 0 24 24"><rect x="3" y="4" width="18" height="16" rx="2" fill="none" stroke="currentColor" stroke-width="1.8"/><path d="M12 4v16" fill="none" stroke="currentColor" stroke-width="1.8"/></svg>`)
	icoBug          = mustSVG(`<svg viewBox="0 0 24 24"><path d="m8 2 1.9 1.9M16 2l-1.9 1.9M9 7V6a3 3 0 0 1 6 0v1" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round"/><path d="M6 13H3M21 13h-3M6.5 8.5 4 6M17.5 8.5 20 6M6.5 17.5 4 20M17.5 17.5 20 20" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round"/><rect x="6" y="7" width="12" height="13" rx="6" fill="none" stroke="currentColor" stroke-width="1.7"/></svg>`)

	icoSettings = mustSVG(`<svg viewBox="0 0 24 24"><circle cx="12" cy="12" r="3" fill="none" stroke="currentColor" stroke-width="2"/><path d="M19.4 15a1.65 1.65 0 0 0 .33 1.82l.06.06a2 2 0 0 1-2.83 2.83l-.06-.06a1.65 1.65 0 0 0-1.82-.33 1.65 1.65 0 0 0-1 1.51V21a2 2 0 0 1-4 0v-.09A1.65 1.65 0 0 0 9 19.4a1.65 1.65 0 0 0-1.82.33l-.06.06a2 2 0 0 1-2.83-2.83l.06-.06a1.65 1.65 0 0 0 .33-1.82 1.65 1.65 0 0 0-1.51-1H3a2 2 0 0 1 0-4h.09A1.65 1.65 0 0 0 4.6 9a1.65 1.65 0 0 0-.33-1.82l-.06-.06a2 2 0 0 1 2.83-2.83l.06.06a1.65 1.65 0 0 0 1.82.33H9a1.65 1.65 0 0 0 1-1.51V3a2 2 0 0 1 4 0v.09a1.65 1.65 0 0 0 1 1.51 1.65 1.65 0 0 0 1.82-.33l.06-.06a2 2 0 0 1 2.83 2.83l-.06.06a1.65 1.65 0 0 0-.33 1.82V9a1.65 1.65 0 0 0 1.51 1H21a2 2 0 0 1 0 4h-.09a1.65 1.65 0 0 0-1.51 1Z" fill="none" stroke="currentColor" stroke-width="1.8"/></svg>`)

	icoSun = mustSVG(`<svg viewBox="0 0 24 24"><circle cx="12" cy="12" r="4" fill="none" stroke="currentColor" stroke-width="2"/><path d="M12 2v2M12 20v2M4.93 4.93l1.41 1.41M17.66 17.66l1.41 1.41M2 12h2M20 12h2M6.34 17.66l-1.41 1.41M19.07 4.93l-1.41 1.41" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round"/></svg>`)

	icoMoon = mustSVG(`<svg viewBox="0 0 24 24"><path d="M12 3a6 6 0 0 0 9 9 9 9 0 1 1-9-9Z" fill="none" stroke="currentColor" stroke-width="2" stroke-linejoin="round"/></svg>`)

	icoMonitor = mustSVG(`<svg viewBox="0 0 24 24"><rect x="2" y="3" width="20" height="14" rx="2" fill="none" stroke="currentColor" stroke-width="2"/><path d="M8 21h8M12 17v4" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round"/></svg>`)

	icoEdit = mustSVG(`<svg viewBox="0 0 24 24"><path d="M17 3a2.85 2.83 0 1 1 4 4L7.5 20.5 2 22l1.5-5.5Z" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linejoin="round"/></svg>`)

	icoCopy = mustSVG(`<svg viewBox="0 0 24 24"><rect x="9" y="9" width="13" height="13" rx="2" ry="2" fill="none" stroke="currentColor" stroke-width="1.8"/><path d="M5 15H4a2 2 0 0 1-2-2V4a2 2 0 0 1 2-2h9a2 2 0 0 1 2 2v1" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"/></svg>`)

	icoLog = mustSVG(`<svg viewBox="0 0 24 24"><path d="M8 6h13M8 12h13M8 18h13" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round"/><circle cx="3.5" cy="6" r="1" fill="currentColor"/><circle cx="3.5" cy="12" r="1" fill="currentColor"/><circle cx="3.5" cy="18" r="1" fill="currentColor"/></svg>`)

	icoSparkles = mustSVG(`<svg viewBox="0 0 24 24"><path d="m12 3-1.9 5.8a2 2 0 0 1-1.3 1.3L3 12l5.8 1.9a2 2 0 0 1 1.3 1.3L12 21l1.9-5.8a2 2 0 0 1 1.3-1.3L21 12l-5.8-1.9a2 2 0 0 1-1.3-1.3Z" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linejoin="round"/></svg>`)

	icoMore     = mustSVG(`<svg viewBox="0 0 24 24"><circle cx="12" cy="12" r="1.5" fill="currentColor"/><circle cx="19" cy="12" r="1.5" fill="currentColor"/><circle cx="5" cy="12" r="1.5" fill="currentColor"/></svg>`)
	icoTerminal = mustSVG(`<svg viewBox="0 0 24 24"><rect x="3" y="4" width="18" height="16" rx="2" fill="none" stroke="currentColor" stroke-width="1.8"/><path d="m7 9 3 3-3 3M12 15h5" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"/></svg>`)
	icoAlert    = mustSVG(`<svg viewBox="0 0 24 24"><path d="M12 3 2.5 20h19Z" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linejoin="round"/><path d="M12 9v5" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round"/><circle cx="12" cy="17" r="1" fill="currentColor"/></svg>`)
	icoFetch    = mustSVG(`<svg viewBox="0 0 24 24"><path d="M12 3v12" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round"/><path d="m7 10 5 5 5-5" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"/><path d="M4 20h16" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round"/></svg>`)
	icoHome     = mustSVG(`<svg viewBox="0 0 24 24"><path d="m3 11 9-8 9 8" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"/><path d="M5 10v10h14V10" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linejoin="round"/></svg>`)

	icoPlay = mustSVG(`<svg viewBox="0 0 24 24"><polygon points="6 3 20 12 6 21 6 3" fill="currentColor"/></svg>`)
)

func drawIcon(c *ui.Context, svg *ui.SVG, size float32, color ui.Color) {
	ui.Icon(c, svg).Size(size, size).TextColor(color).Shrink(0)
}
