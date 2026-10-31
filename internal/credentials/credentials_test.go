package credentials

import (
	"context"
	"os"
	"os/exec"
	"path/filepath"
	"runtime"
	"strings"
	"testing"
)

func TestHTTPSHost(t *testing.T) {
	cases := map[string]string{
		"https://github.com/owner/repo.git": "github.com",
		"git@github.com:owner/repo.git":     "github.com",
		"github.com/owner/repo":             "github.com",
		"  https://user@gitlab.com/x  ":     "gitlab.com",
	}
	for in, want := range cases {
		if got := HTTPSHost(in); got != want {
			t.Errorf("HTTPSHost(%q)=%q want %q", in, got, want)
		}
	}
}

func TestSaveAndEraseHTTPSViaStoreHelper(t *testing.T) {
	if runtime.GOOS == "windows" {
		t.Skip("HOME-based git config isolation is Unix-specific")
	}
	home := t.TempDir()
	t.Setenv("HOME", home)
	t.Setenv("XDG_CONFIG_HOME", filepath.Join(home, ".config"))
	store := filepath.Join(home, "credentials")
	cmd := exec.Command("git", "config", "--global", "credential.helper", "store --file "+store)
	cmd.Env = append(os.Environ(), "HOME="+home)
	if out, err := cmd.CombinedOutput(); err != nil {
		t.Fatalf("configure helper: %v %s", err, out)
	}
	ctx := context.Background()
	if err := SaveHTTPS(ctx, "https://github.com/owner/repo", "alice", "s3cret-token"); err != nil {
		t.Fatal(err)
	}
	data, err := os.ReadFile(store)
	if err != nil {
		t.Fatal(err)
	}
	if !strings.Contains(string(data), "github.com") || !strings.Contains(string(data), "s3cret-token") {
		t.Fatalf("credential store=%q", data)
	}
	if err := EraseHTTPS(ctx, "github.com", "alice"); err != nil {
		t.Fatal(err)
	}
	data, _ = os.ReadFile(store)
	if strings.Contains(string(data), "s3cret-token") {
		t.Fatalf("credential was not erased: %q", data)
	}
}

func TestListSSHKeys(t *testing.T) {
	home := t.TempDir()
	t.Setenv("HOME", home)
	t.Setenv("USERPROFILE", home)
	sshDir := filepath.Join(home, ".ssh")
	if err := os.MkdirAll(sshDir, 0o700); err != nil {
		t.Fatal(err)
	}
	pub := "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAITestKey test@example.com\n"
	if err := os.WriteFile(filepath.Join(sshDir, "id_test.pub"), []byte(pub), 0o644); err != nil {
		t.Fatal(err)
	}
	keys, err := ListSSHKeys(context.Background())
	if err != nil {
		t.Fatal(err)
	}
	if len(keys) != 1 || keys[0].Type != "ssh-ed25519" || keys[0].Comment != "test@example.com" {
		t.Fatalf("keys=%+v", keys)
	}
}
