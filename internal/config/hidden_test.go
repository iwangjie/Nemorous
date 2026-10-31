package config

import "testing"

func TestHideAndShowRepo(t *testing.T) {
	var c Config
	c.HideRepo("nested/repo")
	c.HideRepo("nested/repo")
	if len(c.HiddenRepos) != 1 || !c.IsRepoHidden("nested/repo") {
		t.Fatalf("hidden=%v", c.HiddenRepos)
	}
	c.ShowRepo("nested/repo")
	if len(c.HiddenRepos) != 0 || c.IsRepoHidden("nested/repo") {
		t.Fatalf("hidden=%v", c.HiddenRepos)
	}
}

func TestLSPIsOptionalByDefault(t *testing.T) {
	c := Default()
	if c.LSPEnabled {
		t.Fatal("LSP must be off by default")
	}
}
