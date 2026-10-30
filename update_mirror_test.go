package main

import (
	"testing"
)

func TestMirrorURLs(t *testing.T) {
	raw := "https://github.com/iwangjie/Nemorous/releases/latest/download/update-darwin-arm64.json"
	got := mirrorURLs("https://ghfast.top/", raw)
	if len(got) != 2 || got[0] != "https://ghfast.top/"+raw || got[1] != raw {
		t.Fatalf("mirrorURLs = %#v", got)
	}
	if got := mirrorURLs("http://insecure.example", raw); len(got) != 1 || got[0] != raw {
		t.Fatalf("insecure mirror was not ignored: %#v", got)
	}
	if got := mirrorURLs("", raw); len(got) != 1 || got[0] != raw {
		t.Fatalf("empty mirror = %#v", got)
	}
}

func TestValidUpdateManifest(t *testing.T) {
	if !validUpdateManifest([]byte(`{"version":"1.0.0","url":"https://example.test/a.tgz","signature":"abc"}`)) {
		t.Fatal("valid manifest rejected")
	}
	for _, body := range []string{"<html>blocked</html>", `{"version":"1.0.0"}`, "{}"} {
		if validUpdateManifest([]byte(body)) {
			t.Fatalf("invalid manifest accepted: %s", body)
		}
	}
}
