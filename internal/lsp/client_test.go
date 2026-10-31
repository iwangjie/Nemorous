package lsp_test

import (
	"bufio"
	"context"
	"encoding/json"
	"fmt"
	"io"
	"os"
	"strconv"
	"strings"
	"testing"
	"time"

	"github.com/iwangjie/nemorous/internal/lsp"
)

func TestSplitCommand(t *testing.T) {
	got := lsp.SplitCommand(`"/Applications/My Server/bin/gopls" -remote=auto`)
	if len(got) != 2 || got[0] != "/Applications/My Server/bin/gopls" || got[1] != "-remote=auto" {
		t.Fatalf("splitCommand=%q", got)
	}
}

func TestUTF16Conversions(t *testing.T) {
	line := "a😀b"
	if got := lsp.ByteToUTF16(line, len("a😀")); got != 3 {
		t.Fatalf("ByteToUTF16=%d", got)
	}
	if got := lsp.UTF16ToByte(line, 3); got != len("a😀") {
		t.Fatalf("UTF16ToByte=%d", got)
	}
}

func TestApplyTextEdits(t *testing.T) {
	text := "one\ntwo\nthree\n"
	edits := []lsp.TextEdit{
		{Range: lsp.Range{Start: lsp.Position{Line: 1, Character: 0}, End: lsp.Position{Line: 1, Character: 3}}, NewText: "TWO"},
		{Range: lsp.Range{Start: lsp.Position{Line: 2, Character: 0}, End: lsp.Position{Line: 2, Character: 5}}, NewText: "3"},
	}
	if got := lsp.ApplyTextEdits(text, edits); got != "one\nTWO\n3\n" {
		t.Fatalf("ApplyTextEdits=%q", got)
	}
}

func TestClientAgainstFakeServer(t *testing.T) {
	t.Setenv("NEMOROUS_FAKE_LSP", "1")
	root := t.TempDir()
	diagCh := make(chan []lsp.Diagnostic, 4)
	client, err := lsp.Start(context.Background(), root, os.Args[0]+" -test.run=TestFakeLSPServer", "go", func(uri string, ds []lsp.Diagnostic) {
		diagCh <- ds
	})
	if err != nil {
		t.Fatal(err)
	}
	defer client.Close()
	uri := lsp.FileURI(root + "/main.go")
	if err := client.DidOpen(uri, "x\n"); err != nil {
		t.Fatal(err)
	}
	select {
	case ds := <-diagCh:
		if len(ds) != 1 || ds[0].Message != "bad" || ds[0].Start != 0 || ds[0].End != 1 {
			t.Fatalf("diagnostics=%+v", ds)
		}
	case <-time.After(5 * time.Second):
		t.Fatal("diagnostic timeout")
	}
	items, err := client.Completion(context.Background(), uri, "x", 0, 0)
	if err != nil || len(items) != 1 || items[0].Label != "hello" {
		t.Fatalf("completion=%+v err=%v", items, err)
	}
	hover, err := client.Hover(context.Background(), uri, "x", 0, 0)
	if err != nil || lsp.MessageText(hover.Contents) != "**doc**" {
		t.Fatalf("hover=%+v err=%v", hover, err)
	}
	locs, err := client.Definition(context.Background(), uri, "x", 0, 0)
	if err != nil || len(locs) != 1 || locs[0].Range.Start.Line != 0 {
		t.Fatalf("definition=%+v err=%v", locs, err)
	}
	changes, err := client.Rename(context.Background(), uri, "x", 0, 0, "y")
	if err != nil || len(changes[uri]) != 1 || changes[uri][0].NewText != "y" {
		t.Fatalf("rename=%+v err=%v", changes, err)
	}
}

// TestFakeLSPServer is both a normal (skipped) test and the fake server process.
func TestFakeLSPServer(t *testing.T) {
	if os.Getenv("NEMOROUS_FAKE_LSP") != "1" {
		t.Skip("helper process")
	}
	runFakeServer()
	os.Exit(0)
}

func runFakeServer() {
	in := bufio.NewReader(os.Stdin)
	out := os.Stdout
	uri := ""
	for {
		body, err := readTestFrame(in)
		if err != nil {
			return
		}
		var msg struct {
			ID     *int            `json:"id"`
			Method string          `json:"method"`
			Params json.RawMessage `json:"params"`
		}
		if json.Unmarshal(body, &msg) != nil {
			continue
		}
		if msg.Method == "exit" {
			return
		}
		if msg.Method == "textDocument/didOpen" {
			var p struct {
				TextDocument struct{ URI string } `json:"textDocument"`
			}
			_ = json.Unmarshal(msg.Params, &p)
			uri = p.TextDocument.URI
			writeTestFrame(out, map[string]any{
				"jsonrpc": "2.0", "method": "textDocument/publishDiagnostics",
				"params": map[string]any{
					"uri": uri,
					"diagnostics": []map[string]any{{
						"range":    map[string]any{"start": map[string]int{"line": 0, "character": 0}, "end": map[string]int{"line": 0, "character": 1}},
						"severity": 1, "message": "bad", "source": "fake",
					}},
				},
			})
			continue
		}
		if msg.ID == nil {
			continue
		}
		result := any(nil)
		switch msg.Method {
		case "initialize":
			result = map[string]any{"capabilities": map[string]any{
				"completionProvider": map[string]any{}, "hoverProvider": true,
				"definitionProvider": true, "renameProvider": true,
			}}
		case "textDocument/completion":
			result = []map[string]any{{"label": "hello", "kind": 3, "insertText": "hello"}}
		case "textDocument/hover":
			result = map[string]any{"contents": map[string]any{"kind": "markdown", "value": "**doc**"}}
		case "textDocument/definition":
			result = []map[string]any{{"uri": uri, "range": map[string]any{"start": map[string]int{"line": 0, "character": 0}, "end": map[string]int{"line": 0, "character": 1}}}}
		case "textDocument/rename":
			result = map[string]any{"changes": map[string]any{uri: []map[string]any{{"range": map[string]any{"start": map[string]int{"line": 0, "character": 0}, "end": map[string]int{"line": 0, "character": 1}}, "newText": "y"}}}}
		}
		writeTestFrame(out, map[string]any{"jsonrpc": "2.0", "id": *msg.ID, "result": result})
	}
}

func readTestFrame(br *bufio.Reader) ([]byte, error) {
	length := -1
	for {
		line, err := br.ReadString('\n')
		if err != nil {
			return nil, err
		}
		line = strings.TrimRight(line, "\r\n")
		if line == "" {
			break
		}
		if strings.HasPrefix(strings.ToLower(line), "content-length:") {
			length, _ = strconv.Atoi(strings.TrimSpace(line[len("content-length:"):]))
		}
	}
	if length < 0 {
		return nil, fmt.Errorf("missing length")
	}
	buf := make([]byte, length)
	_, err := io.ReadFull(br, buf)
	return buf, err
}

func writeTestFrame(w io.Writer, v any) {
	b, _ := json.Marshal(v)
	fmt.Fprintf(w, "Content-Length: %d\r\n\r\n", len(b))
	_, _ = w.Write(b)
}
