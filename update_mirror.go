package main

import (
	"bytes"
	"context"
	"encoding/json"
	"fmt"
	"io"
	"net/http"
	"net/url"
	"runtime"
	"strings"
	"time"
)

// updateRepo is the public repository whose Releases hold signed updates.
const updateRepo = "iwangjie/Nemorous"

// mirrorUpdateSource fetches the small update manifest and the signed archive
// through a GitHub proxy (ghfast.top by default), falling back to GitHub
// directly. Signatures still come from the installed public key, so a mirror
// can speed up the transfer but cannot replace an update with its own bytes.
type mirrorUpdateSource struct {
	mirror string
}

func (s mirrorUpdateSource) Manifest(ctx context.Context, prereleases bool) (io.ReadCloser, error) {
	if prereleases {
		return nil, fmt.Errorf("update mirror does not support prereleases")
	}
	target := runtime.GOOS + "-" + runtime.GOARCH
	direct := fmt.Sprintf("https://github.com/%s/releases/latest/download/update-%s.json", updateRepo, target)
	return fetchUpdateFile(ctx, mirrorURLs(s.mirror, direct), true)
}

func (s mirrorUpdateSource) Open(ctx context.Context, rawURL string) (io.ReadCloser, error) {
	return fetchUpdateFile(ctx, mirrorURLs(s.mirror, rawURL), false)
}

var (
	updateHTTPClient         = &http.Client{Timeout: 10 * time.Minute}
	updateManifestHTTPClient = &http.Client{Timeout: 15 * time.Second}
)

// mirrorURLs returns the mirror-prefixed URL first, then the original GitHub
// URL as a fallback. Only https proxy prefixes are accepted.
func mirrorURLs(mirror, raw string) []string {
	mirror = strings.TrimSpace(mirror)
	urls := make([]string, 0, 2)
	if mirror != "" {
		if u, err := url.Parse(mirror); err == nil && u.Scheme == "https" && u.Host != "" {
			urls = append(urls, strings.TrimRight(mirror, "/")+"/"+raw)
		}
	}
	urls = append(urls, raw)
	return urls
}

// fetchUpdateFile tries URLs in order. Manifests are validated as JSON before
// a mirror's response is accepted.
func fetchUpdateFile(ctx context.Context, urls []string, manifest bool) (io.ReadCloser, error) {
	var lastErr error
	for _, u := range urls {
		req, err := http.NewRequestWithContext(ctx, http.MethodGet, u, nil)
		if err != nil {
			lastErr = err
			continue
		}
		req.Header.Set("User-Agent", "Nemorous-updater")
		client := updateHTTPClient
		if manifest {
			client = updateManifestHTTPClient
		}
		resp, err := client.Do(req)
		if err != nil {
			lastErr = err
			continue
		}
		if resp.StatusCode != http.StatusOK {
			lastErr = fmt.Errorf("%s: HTTP %s", u, resp.Status)
			resp.Body.Close()
			continue
		}
		if !manifest {
			return resp.Body, nil
		}
		data, err := io.ReadAll(io.LimitReader(resp.Body, 1<<20))
		resp.Body.Close()
		if err != nil {
			lastErr = err
			continue
		}
		if !validUpdateManifest(data) {
			lastErr = fmt.Errorf("%s: response is not an update manifest", u)
			continue
		}
		return io.NopCloser(bytes.NewReader(data)), nil
	}
	if lastErr == nil {
		lastErr = fmt.Errorf("no update URL configured")
	}
	return nil, lastErr
}

func validUpdateManifest(data []byte) bool {
	var m struct {
		Version   string `json:"version"`
		URL       string `json:"url"`
		Signature string `json:"signature"`
	}
	return json.Unmarshal(data, &m) == nil && m.Version != "" && m.URL != "" && m.Signature != ""
}
