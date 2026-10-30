package main

import (
	"testing"

	"github.com/egoist/mygo/ui/editor"
	"github.com/iwangjie/nemorous/internal/config"
)

func TestUpdateSkipVersion(t *testing.T) {
	a := &App{cfg: config.Config{SkippedUpdateVersion: "v1.2.3"}}
	if !a.updateIsSkipped("v1.2.3") {
		t.Fatal("matching skipped version was not recognized")
	}
	if a.updateIsSkipped("v1.2.4") || a.updateIsSkipped("") {
		t.Fatal("newer or empty version must not be skipped")
	}
}

func TestHasUnsavedEdits(t *testing.T) {
	buf := editor.NewBuffer("original")
	buf.Set("changed")
	a := &App{codeTabs: []*codeTab{{state: &editor.State{Buffer: buf}}}}
	if !a.hasUnsavedEdits() {
		t.Fatal("modified code buffer must block an automatic restart")
	}
	buf.MarkSaved()
	if a.hasUnsavedEdits() {
		t.Fatal("saved code buffer must not block an update")
	}
}

func TestHasActiveWork(t *testing.T) {
	if !(&App{busyLabel: "fetching"}).hasActiveWork() {
		t.Fatal("running command must defer an automatic restart")
	}
	if (&App{}).hasActiveWork() {
		t.Fatal("idle app must allow an automatic restart")
	}
}
