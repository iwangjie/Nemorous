//go:build !windows

package plugins

import (
	"os"
	"os/exec"
	"path/filepath"
	"testing"
)

// Launching from Finder gives the app a minimal PATH; a bare toolchain name
// must still resolve through the login shell.
func TestResolveCommandUsesLoginPath(t *testing.T) {
	t.Setenv("PATH", t.TempDir())
	path := resolveCommand("go")
	if path == "go" {
		if loginShellPath() == "" {
			t.Skip("login shell PATH unavailable")
		}
		t.Skip("go is not on the login shell PATH")
	}
	if !filepath.IsAbs(path) {
		t.Fatalf("resolveCommand returned %q, want an absolute path", path)
	}
	if out, err := exec.Command(path, "version").CombinedOutput(); err != nil {
		t.Fatalf("resolved go does not run: %v (%s)", err, out)
	}
	if _, err := os.Stat(path); err != nil {
		t.Fatal(err)
	}
}

func TestResolveCommandKeepsPathsAndUnknownNames(t *testing.T) {
	if got := resolveCommand("/bin/sh"); got != "/bin/sh" {
		t.Fatalf("absolute command changed to %q", got)
	}
	if got := resolveCommand("./script"); got != "./script" {
		t.Fatalf("relative command changed to %q", got)
	}
	if got := resolveCommand("nemorous-missing-tool-xyz"); got != "nemorous-missing-tool-xyz" {
		t.Fatalf("unknown command changed to %q", got)
	}
}
