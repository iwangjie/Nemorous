// Sign a macOS update archive in the format understood by the Go and Rust apps.
package main

import (
	"bytes"
	"crypto/ed25519"
	"crypto/sha256"
	"encoding/base64"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"os"
	"path/filepath"
	"regexp"
	"strings"
)

func main() {
	if err := run(os.Args[1:]); err != nil {
		fmt.Fprintln(os.Stderr, err)
		os.Exit(1)
	}
}

func run(args []string) error {
	if len(args) != 5 {
		return errors.New("usage: sign-macos-update VERSION ARCH ARCHIVE NOTES MANIFEST")
	}
	version, arch, archive, notesPath, manifestPath := args[0], args[1], args[2], args[3], args[4]
	if !regexp.MustCompile(`^\d+\.\d+\.\d+$`).MatchString(version) || (arch != "arm64" && arch != "amd64") {
		return errors.New("invalid macOS update version or architecture")
	}
	target := "darwin-" + arch
	name := "nemorous-" + version + "-" + target + ".tar.gz"
	if filepath.Base(archive) != name || filepath.Base(manifestPath) != "update-"+target+".json" {
		return errors.New("update archive or manifest has an unexpected filename")
	}
	key, err := base64.StdEncoding.DecodeString(strings.TrimSpace(os.Getenv("MYGO_UPDATER_PRIVATE_KEY")))
	if err != nil || len(key) != ed25519.PrivateKeySize {
		return errors.New("missing or invalid update signing key")
	}
	public, err := base64.StdEncoding.DecodeString(strings.TrimSpace(os.Getenv("NEMOROUS_UPDATER_PUBLIC_KEY")))
	if err != nil || len(public) != ed25519.PublicKeySize || !bytes.Equal(key, ed25519.NewKeyFromSeed(key[:ed25519.SeedSize])) || !bytes.Equal(key[ed25519.SeedSize:], public) {
		return errors.New("update signing key does not match the installed public key")
	}
	file, err := os.Open(archive)
	if err != nil {
		return err
	}
	defer file.Close()
	digest := sha256.New()
	const maxArchiveSize = 1 << 30
	size, err := io.Copy(digest, io.LimitReader(file, maxArchiveSize+1))
	if err != nil {
		return err
	}
	if size == 0 || size > maxArchiveSize {
		return errors.New("invalid update archive size")
	}
	notes, err := os.ReadFile(notesPath)
	if err != nil {
		return err
	}
	signature := ed25519.Sign(ed25519.PrivateKey(key), digest.Sum(nil))
	manifest := struct {
		Version   string `json:"version"`
		Notes     string `json:"notes"`
		URL       string `json:"url"`
		Size      int64  `json:"size"`
		Signature string `json:"signature"`
	}{
		Version:   version,
		Notes:     strings.TrimSpace(string(notes)),
		URL:       "https://github.com/iwangjie/Nemorous/releases/download/v" + version + "/" + name,
		Size:      size,
		Signature: base64.StdEncoding.EncodeToString(signature),
	}
	data, err := json.MarshalIndent(manifest, "", "  ")
	if err != nil {
		return err
	}
	return os.WriteFile(manifestPath, append(data, '\n'), 0o644)
}
