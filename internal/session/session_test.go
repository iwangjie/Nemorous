package session

import (
	"os"
	"path/filepath"
	"testing"
)

func TestRoundTrip(t *testing.T) {
	path := filepath.Join(t.TempDir(), "session.json")
	st := State{
		OpenFiles:    []File{{RepoID: "a", RelPath: "x.go", Line: 12}},
		ActiveFile:   0,
		ActiveTab:    "code",
		ShowGitPanel: true,
		ChangesSplit: 320,
	}
	if err := Save(path, st); err != nil {
		t.Fatal(err)
	}
	got := Load(path)
	if len(got.OpenFiles) != 1 || got.OpenFiles[0].RelPath != "x.go" || got.OpenFiles[0].Line != 12 ||
		got.ActiveTab != "code" || !got.ShowGitPanel || got.ChangesSplit != 320 {
		t.Fatalf("got %+v", got)
	}
	if err := os.WriteFile(path, []byte("{broken"), 0o644); err != nil {
		t.Fatal(err)
	}
	if got := Load(path); len(got.OpenFiles) != 0 {
		t.Fatalf("broken session = %+v", got)
	}
}
