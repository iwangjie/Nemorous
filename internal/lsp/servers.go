package lsp

import (
	"os/exec"
	"path/filepath"
	"strings"
)

// ServerSpec describes a language server and the files it handles.
type ServerSpec struct {
	LanguageID string
	Command    string
	Extensions []string
}

// BuiltinServers are used only when the optional LSP setting is on and no
// override is configured for the language.
var BuiltinServers = []ServerSpec{
	{LanguageID: "go", Command: "gopls", Extensions: []string{".go"}},
	{LanguageID: "typescript", Command: "typescript-language-server --stdio", Extensions: []string{".ts", ".tsx", ".mts", ".cts"}},
	{LanguageID: "javascript", Command: "typescript-language-server --stdio", Extensions: []string{".js", ".jsx", ".mjs", ".cjs"}},
	{LanguageID: "python", Command: "pyright-langserver --stdio", Extensions: []string{".py", ".pyi"}},
	{LanguageID: "rust", Command: "rust-analyzer", Extensions: []string{".rs"}},
	{LanguageID: "c", Command: "clangd", Extensions: []string{".c", ".h"}},
	{LanguageID: "cpp", Command: "clangd", Extensions: []string{".cc", ".cpp", ".cxx", ".hpp", ".hh"}},
	{LanguageID: "java", Command: "jdtls", Extensions: []string{".java"}},
}

// Detect returns the server for a path, preferring overrides keyed by language
// ID. available reports whether the executable is installed.
func Detect(path string, overrides map[string]string) (ServerSpec, bool) {
	ext := strings.ToLower(filepath.Ext(path))
	for _, spec := range BuiltinServers {
		for _, candidate := range spec.Extensions {
			if candidate != ext {
				continue
			}
			if override := strings.TrimSpace(overrides[spec.LanguageID]); override != "" {
				return ServerSpec{LanguageID: spec.LanguageID, Command: override, Extensions: spec.Extensions}, commandAvailable(override)
			}
			return spec, commandAvailable(spec.Command)
		}
	}
	return ServerSpec{}, false
}

// LanguageForPath returns the LSP language ID for a file, or empty.
func LanguageForPath(path string) string {
	ext := strings.ToLower(filepath.Ext(path))
	for _, spec := range BuiltinServers {
		for _, candidate := range spec.Extensions {
			if candidate == ext {
				return spec.LanguageID
			}
		}
	}
	return ""
}

func commandAvailable(command string) bool {
	fields := SplitCommand(strings.TrimSpace(command))
	if len(fields) == 0 {
		return false
	}
	_, err := exec.LookPath(fields[0])
	return err == nil
}

// Available lists built-in servers and whether their executables are present.
func Available() []ServerSpec {
	out := make([]ServerSpec, len(BuiltinServers))
	copy(out, BuiltinServers)
	return out
}

// ApplyTextEdits applies LSP text edits to a document. Edits are applied from
// the end so earlier offsets stay valid.
func ApplyTextEdits(text string, edits []TextEdit) string {
	offsets := make([]struct {
		start, end int
		text       string
	}, 0, len(edits))
	lines := strings.Split(text, "\n")
	for _, e := range edits {
		start := positionOffset(lines, e.Range.Start)
		end := positionOffset(lines, e.Range.End)
		if end < start {
			end = start
		}
		offsets = append(offsets, struct {
			start, end int
			text       string
		}{start, end, e.NewText})
	}
	// Simple insertion sort by start descending; edit lists are small.
	for i := 1; i < len(offsets); i++ {
		for j := i; j > 0 && offsets[j].start > offsets[j-1].start; j-- {
			offsets[j], offsets[j-1] = offsets[j-1], offsets[j]
		}
	}
	for _, e := range offsets {
		if e.start < 0 || e.end > len(text) || e.start > e.end {
			continue
		}
		text = text[:e.start] + e.text + text[e.end:]
	}
	return text
}

func positionOffset(lines []string, p Position) int {
	if p.Line < 0 {
		return 0
	}
	if p.Line >= len(lines) {
		return len(strings.Join(lines, "\n"))
	}
	offset := 0
	for i := 0; i < p.Line; i++ {
		offset += len(lines[i]) + 1
	}
	return offset + UTF16ToByte(lines[p.Line], p.Character)
}

// ServerInstalled reports whether a server's executable is on PATH.
func ServerInstalled(spec ServerSpec) bool { return commandAvailable(spec.Command) }
