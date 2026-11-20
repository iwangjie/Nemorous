package main

import (
	"strings"

	"github.com/egoist/mygo/nemorouseditor"
)

// Remember the editing surface before the search field takes focus. A
// tree or tool window selection cannot seed a text search.
func (a *App) searchSelection() (text string, restoreFocus func()) {
	if a.showGitPanel {
		var diff *editor.DiffState
		if a.gitSubTab == "file-history" {
			if history := a.gitRevisionHistory; history != nil && history.preview != nil {
				diff = history.preview.diff
			}
		} else if a.cfg.GitLogDiffPreview && a.gitLogPreview != nil {
			diff = a.gitLogPreview.diff
		}
		if diff != nil && diff.Focused() {
			return diff.SelectedText(), diff.Focus
		}
	}
	var diff *editor.DiffState
	switch a.editorTab {
	case "file":
		if tab := a.activeCodeTab(); tab != nil {
			if state := a.activeCodeEditor(); state.Focused() {
				return state.SelectedText(), func() { a.activeCodeViewHandle(tab).Focus() }
			}
		}
	case "changes":
		diff = a.diffState
	case "commit":
		diff = a.gitCommitDiff
	case "compare":
		if a.comparison != nil {
			diff = a.comparison.diff
		}
	case "staged":
		if view := a.stagedReview; view != nil && view.state != nil {
			for _, state := range view.state.Versions {
				if state.Focused() {
					return state.SelectedText(), nil
				}
			}
		}
	}
	if diff != nil && diff.Focused() {
		return diff.SelectedText(), diff.Focus
	}
	return "", nil
}

func singleLineSearchSelection(text string) string {
	if strings.ContainsAny(text, "\r\n") {
		return ""
	}
	return text
}
