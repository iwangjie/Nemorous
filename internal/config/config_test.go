package config

import (
	"os"
	"path/filepath"
	"testing"
)

func TestDefaultUpdateSettings(t *testing.T) {
	c := Default()
	if !c.AutoUpdate || !c.AutoUpdateIdle {
		t.Fatalf("auto update should default on: %+v", c)
	}
	if c.UpdateCheckMinutes != 30 {
		t.Fatalf("check interval = %d, want 30", c.UpdateCheckMinutes)
	}
	if c.UpdateMirror != "https://ghfast.top" {
		t.Fatalf("update mirror = %q", c.UpdateMirror)
	}
}

func TestLoadClampsUpdateInterval(t *testing.T) {
	dir := t.TempDir()
	path := filepath.Join(dir, "config.json")
	if err := os.WriteFile(path, []byte(`{"update_check_minutes": 1, "auto_update": false}`), 0o644); err != nil {
		t.Fatal(err)
	}
	t.Setenv("NEMOROUS_CONFIG", path)
	c := Load()
	if c.UpdateCheckMinutes != 5 {
		t.Fatalf("check interval = %d, want 5", c.UpdateCheckMinutes)
	}
	if c.AutoUpdate {
		t.Fatal("explicit auto_update=false must be preserved")
	}
}

func TestLoadClampsUpdateIntervalUpperBound(t *testing.T) {
	dir := t.TempDir()
	path := filepath.Join(dir, "config.json")
	if err := os.WriteFile(path, []byte(`{"update_check_minutes": 99999}`), 0o644); err != nil {
		t.Fatal(err)
	}
	t.Setenv("NEMOROUS_CONFIG", path)
	if got := Load().UpdateCheckMinutes; got != 24*60 {
		t.Fatalf("check interval = %d, want %d", got, 24*60)
	}
}
