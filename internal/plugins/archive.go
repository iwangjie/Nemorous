package plugins

import (
	"archive/zip"
	"bytes"
	"fmt"
	"io"
	"io/fs"
	"os"
	"path"
	"path/filepath"
	"strings"
)

const (
	maxArchiveFiles  = 512
	maxArchiveMember = 8 << 20
	maxArchiveTotal  = 32 << 20
)

// extractZip unpacks data into dest, refusing entries that would leave
// dest, links, and oversized packages. It returns the directory holding
// plugin.json, which is dest unless the archive wraps everything in one
// top-level folder.
func extractZip(data []byte, dest string) (string, error) {
	zr, err := zip.NewReader(bytes.NewReader(data), int64(len(data)))
	if err != nil {
		return "", fmt.Errorf("plugin package is not a zip file: %w", err)
	}
	if len(zr.File) > maxArchiveFiles {
		return "", fmt.Errorf("plugin package has too many files")
	}
	var total int64
	for _, f := range zr.File {
		name := strings.ReplaceAll(f.Name, "\\", "/")
		clean := path.Clean(name)
		if clean == "." {
			continue
		}
		if strings.HasPrefix(clean, "/") || clean == ".." || strings.HasPrefix(clean, "../") {
			return "", fmt.Errorf("plugin package entry %q escapes the package", f.Name)
		}
		if strings.Contains(clean, "/../") {
			return "", fmt.Errorf("plugin package entry %q escapes the package", f.Name)
		}
		mode := f.Mode()
		if mode&fs.ModeSymlink != 0 {
			return "", fmt.Errorf("plugin package entry %q is a symlink", f.Name)
		}
		if f.FileInfo().IsDir() {
			if err := os.MkdirAll(filepath.Join(dest, filepath.FromSlash(clean)), 0o755); err != nil {
				return "", err
			}
			continue
		}
		if f.UncompressedSize64 > maxArchiveMember {
			return "", fmt.Errorf("plugin package entry %q is too large", f.Name)
		}
		if int64(f.UncompressedSize64) > maxArchiveTotal {
			return "", fmt.Errorf("plugin package is too large")
		}
		target := filepath.Join(dest, filepath.FromSlash(clean))
		if err := os.MkdirAll(filepath.Dir(target), 0o755); err != nil {
			return "", err
		}
		rc, err := f.Open()
		if err != nil {
			return "", err
		}
		w, err := os.OpenFile(target, os.O_CREATE|os.O_WRONLY|os.O_TRUNC, 0o644)
		if err != nil {
			rc.Close()
			return "", err
		}
		n, copyErr := io.Copy(w, io.LimitReader(rc, maxArchiveMember+1))
		cerr := w.Close()
		rc.Close()
		if copyErr != nil {
			return "", copyErr
		}
		if cerr != nil {
			return "", cerr
		}
		if n > maxArchiveMember {
			return "", fmt.Errorf("plugin package entry %q is too large", f.Name)
		}
		total += n
		if total > maxArchiveTotal {
			return "", fmt.Errorf("plugin package is too large")
		}
	}
	root, err := findPackageRoot(dest)
	if err != nil {
		return "", err
	}
	return root, nil
}

// findPackageRoot returns dir if it holds plugin.json, or the only
// immediate child directory that does (the usual GitHub zip layout).
func findPackageRoot(dir string) (string, error) {
	if fileExists(filepath.Join(dir, "plugin.json")) {
		return dir, nil
	}
	entries, err := os.ReadDir(dir)
	if err != nil {
		return "", err
	}
	var candidates []string
	for _, e := range entries {
		if !e.IsDir() {
			continue
		}
		candidate := filepath.Join(dir, e.Name())
		if fileExists(filepath.Join(candidate, "plugin.json")) {
			candidates = append(candidates, candidate)
		}
	}
	switch len(candidates) {
	case 1:
		return candidates[0], nil
	case 0:
		return "", fmt.Errorf("plugin package has no plugin.json")
	default:
		return "", fmt.Errorf("plugin package has more than one plugin.json")
	}
}

func fileExists(p string) bool {
	info, err := os.Stat(p)
	return err == nil && !info.IsDir()
}

// copyTree copies a plugin package directory into dst.
func copyTree(src, dst string) error {
	return filepath.WalkDir(src, func(p string, d fs.DirEntry, err error) error {
		if err != nil {
			return err
		}
		rel, err := filepath.Rel(src, p)
		if err != nil {
			return err
		}
		if rel == "." {
			return nil
		}
		if d.Type()&fs.ModeSymlink != 0 {
			return fmt.Errorf("plugin package contains a symlink: %s", rel)
		}
		target := filepath.Join(dst, rel)
		if d.IsDir() {
			return os.MkdirAll(target, 0o755)
		}
		if !d.Type().IsRegular() {
			return fmt.Errorf("plugin package contains a special file: %s", rel)
		}
		in, err := os.Open(p)
		if err != nil {
			return err
		}
		defer in.Close()
		out, err := os.OpenFile(target, os.O_CREATE|os.O_WRONLY|os.O_TRUNC, 0o644)
		if err != nil {
			return err
		}
		_, copyErr := io.Copy(out, io.LimitReader(in, maxArchiveMember))
		closeErr := out.Close()
		if copyErr != nil {
			return copyErr
		}
		return closeErr
	})
}
