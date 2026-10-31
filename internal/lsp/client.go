// Package lsp implements the small stdio JSON-RPC client Nemorous needs for
// optional language-server features. It is disabled by default; servers are
// started only for the language of an opened file.
package lsp

import (
	"bufio"
	"bytes"
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"net/url"
	"os"
	"os/exec"
	"path/filepath"
	"runtime"
	"strconv"
	"strings"
	"sync"
	"time"
)

// Diagnostic is one problem reported by a language server.
type Diagnostic struct {
	Start, End int // byte offsets in the document
	Severity   int // 1 error, 2 warning, 3 info, 4 hint
	Message    string
	Source     string
}

type Position struct {
	Line      int `json:"line"`
	Character int `json:"character"`
}

type Range struct {
	Start Position `json:"start"`
	End   Position `json:"end"`
}

type Location struct {
	URI   string `json:"uri"`
	Range Range  `json:"range"`
}

type TextEdit struct {
	Range   Range  `json:"range"`
	NewText string `json:"newText"`
}

type CompletionItem struct {
	Label         string `json:"label"`
	Detail        string `json:"detail"`
	Kind          int    `json:"kind"`
	InsertText    string `json:"insertText"`
	Documentation any    `json:"documentation"`
	FilterText    string `json:"filterText"`
	TextEdit      *struct {
		NewText string `json:"newText"`
		Range   Range  `json:"range"`
	} `json:"textEdit"`
}

type Hover struct {
	Contents any    `json:"contents"`
	Range    *Range `json:"range"`
}

// Client is one language-server process.
type Client struct {
	cmd        *exec.Cmd
	stdin      io.WriteCloser
	root       string
	langID     string
	writeMu    sync.Mutex
	mu         sync.Mutex
	nextID     int
	pending    map[int]chan rpcResponse
	closed     bool
	diags      func(uri string, ds []Diagnostic)
	open       map[string]bool
	docs       map[string]string
	docVersion int
}

type rpcMessage struct {
	JSONRPC string          `json:"jsonrpc"`
	ID      *int            `json:"id,omitempty"`
	Method  string          `json:"method,omitempty"`
	Params  json.RawMessage `json:"params,omitempty"`
	Result  json.RawMessage `json:"result,omitempty"`
	Error   *rpcError       `json:"error,omitempty"`
}

type rpcError struct {
	Code    int    `json:"code"`
	Message string `json:"message"`
}

type rpcResponse struct {
	result json.RawMessage
	err    error
}

// Start launches and initializes a language server rooted at root.
func Start(ctx context.Context, root, command, langID string, onDiagnostics func(uri string, ds []Diagnostic)) (*Client, error) {
	command = strings.TrimSpace(command)
	if command == "" {
		return nil, fmt.Errorf("LSP 服务命令为空")
	}
	fields := SplitCommand(command)
	if len(fields) == 0 {
		return nil, fmt.Errorf("无效的 LSP 服务命令")
	}
	cmd := exec.Command(fields[0], fields[1:]...)
	cmd.Dir = root
	cmd.Env = os.Environ()
	stdin, err := cmd.StdinPipe()
	if err != nil {
		return nil, err
	}
	stdout, err := cmd.StdoutPipe()
	if err != nil {
		return nil, err
	}
	cmd.Stderr = io.Discard
	if err := cmd.Start(); err != nil {
		return nil, fmt.Errorf("启动 LSP 服务失败：%w", err)
	}
	c := &Client{cmd: cmd, stdin: stdin, root: root, langID: langID, pending: make(map[int]chan rpcResponse), diags: onDiagnostics, open: make(map[string]bool), docs: make(map[string]string)}
	go c.readLoop(stdout)
	initCtx, cancel := context.WithTimeout(ctx, 12*time.Second)
	defer cancel()
	rootURI := FileURI(root)
	params := map[string]any{
		"processId":        os.Getpid(),
		"clientInfo":       map[string]any{"name": "Nemorous"},
		"rootUri":          rootURI,
		"workspaceFolders": []map[string]string{{"uri": rootURI, "name": filepath.Base(root)}},
		"capabilities": map[string]any{
			"textDocument": map[string]any{
				"synchronization":    map[string]any{"didSave": true},
				"completion":         map[string]any{"completionItem": map[string]any{"snippetSupport": false}},
				"hover":              map[string]any{"contentFormat": []string{"markdown", "plaintext"}},
				"definition":         map[string]any{},
				"references":         map[string]any{},
				"rename":             map[string]any{"prepareSupport": false},
				"publishDiagnostics": map[string]any{},
			},
			"workspace": map[string]any{"applyEdit": false, "workspaceEdit": map[string]any{"documentChanges": false}},
		},
	}
	if _, err := c.request(initCtx, "initialize", params); err != nil {
		_ = c.Close()
		return nil, fmt.Errorf("LSP initialize 失败：%w", err)
	}
	if err := c.notify("initialized", map[string]any{}); err != nil {
		_ = c.Close()
		return nil, err
	}
	return c, nil
}

func (c *Client) readLoop(stdout io.Reader) {
	br := bufio.NewReaderSize(stdout, 256*1024)
	for {
		msg, err := readFrame(br)
		if err != nil {
			c.failPending(err)
			return
		}
		var m rpcMessage
		if err := json.Unmarshal(msg, &m); err != nil {
			continue
		}
		if m.ID != nil && m.Method != "" {
			// Server-initiated request (configuration, progress, etc.).
			// Answer null so a server never blocks waiting for the client.
			_ = c.writeFrame(mustJSON(map[string]any{"jsonrpc": "2.0", "id": *m.ID, "result": nil}))
			continue
		}
		if m.ID != nil && m.Method == "" {
			c.mu.Lock()
			ch := c.pending[*m.ID]
			delete(c.pending, *m.ID)
			c.mu.Unlock()
			if ch != nil {
				if m.Error != nil {
					ch <- rpcResponse{err: fmt.Errorf("%s", m.Error.Message)}
				} else {
					ch <- rpcResponse{result: m.Result}
				}
			}
			continue
		}
		c.handleNotification(m)
	}
}

func (c *Client) handleNotification(m rpcMessage) {
	switch m.Method {
	case "textDocument/publishDiagnostics":
		var p struct {
			URI         string `json:"uri"`
			Diagnostics []struct {
				Range struct {
					Start Position `json:"start"`
					End   Position `json:"end"`
				} `json:"range"`
				Severity int    `json:"severity"`
				Message  string `json:"message"`
				Source   string `json:"source"`
			} `json:"diagnostics"`
		}
		if json.Unmarshal(m.Params, &p) != nil || c.diags == nil {
			return
		}
		text := c.documentText(p.URI)
		var ds []Diagnostic
		for _, d := range p.Diagnostics {
			start := offsetFor(text, d.Range.Start)
			end := offsetFor(text, d.Range.End)
			if end < start {
				end = start
			}
			ds = append(ds, Diagnostic{Start: start, End: end, Severity: d.Severity, Message: d.Message, Source: d.Source})
		}
		c.diags(p.URI, ds)
	}
}

// documents caches no text; open documents are tracked by the app, so this is
// intentionally empty. Diagnostics ranges are converted by the caller with
// offsets in the current buffer when possible; without text they collapse to
// zero and are ignored by the app.
func (c *Client) documentText(uri string) string {
	c.mu.Lock()
	defer c.mu.Unlock()
	return c.docs[uri]
}

func (c *Client) failPending(err error) {
	c.mu.Lock()
	defer c.mu.Unlock()
	c.closed = true
	for id, ch := range c.pending {
		ch <- rpcResponse{err: err}
		delete(c.pending, id)
	}
}

func (c *Client) request(ctx context.Context, method string, params any) (json.RawMessage, error) {
	c.mu.Lock()
	if c.closed {
		c.mu.Unlock()
		return nil, errors.New("LSP 服务已关闭")
	}
	c.nextID++
	id := c.nextID
	ch := make(chan rpcResponse, 1)
	c.pending[id] = ch
	c.mu.Unlock()
	payload, err := json.Marshal(map[string]any{"jsonrpc": "2.0", "id": id, "method": method, "params": params})
	if err != nil {
		return nil, err
	}
	if err := c.writeFrame(payload); err != nil {
		c.mu.Lock()
		delete(c.pending, id)
		c.mu.Unlock()
		return nil, err
	}
	select {
	case res := <-ch:
		return res.result, res.err
	case <-ctx.Done():
		c.mu.Lock()
		delete(c.pending, id)
		c.mu.Unlock()
		return nil, ctx.Err()
	}
}

func (c *Client) notify(method string, params any) error {
	payload, err := json.Marshal(map[string]any{"jsonrpc": "2.0", "method": method, "params": params})
	if err != nil {
		return err
	}
	return c.writeFrame(payload)
}

func (c *Client) writeFrame(payload []byte) error {
	c.writeMu.Lock()
	defer c.writeMu.Unlock()
	_, err := fmt.Fprintf(c.stdin, "Content-Length: %d\r\n\r\n", len(payload))
	if err != nil {
		return err
	}
	_, err = c.stdin.Write(payload)
	return err
}

func readFrame(br *bufio.Reader) ([]byte, error) {
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
			n, err := strconv.Atoi(strings.TrimSpace(line[len("content-length:"):]))
			if err != nil {
				return nil, err
			}
			length = n
		}
	}
	if length < 0 || length > 64<<20 {
		return nil, fmt.Errorf("无效的 LSP 消息长度")
	}
	buf := make([]byte, length)
	_, err := io.ReadFull(br, buf)
	return buf, err
}

// FileURI converts an absolute path to a file URI.
func FileURI(path string) string {
	path = filepath.ToSlash(path)
	if runtime.GOOS == "windows" && !strings.HasPrefix(path, "/") {
		path = "/" + path
	}
	return (&url.URL{Scheme: "file", Path: path}).String()
}

// PathFromURI converts a file URI back to a filesystem path.
func PathFromURI(uri string) (string, error) {
	u, err := url.Parse(uri)
	if err != nil || u.Scheme != "file" {
		return "", fmt.Errorf("不是文件 URI：%s", uri)
	}
	p, err := url.PathUnescape(u.Path)
	if err != nil {
		return "", err
	}
	if runtime.GOOS == "windows" && len(p) > 2 && p[0] == '/' && p[2] == ':' {
		p = p[1:]
	}
	return filepath.FromSlash(p), nil
}

func offsetFor(text string, p Position) int {
	if text == "" {
		return 0
	}
	lines := strings.Split(text, "\n")
	if p.Line < 0 || p.Line >= len(lines) {
		return len(text)
	}
	line := lines[p.Line]
	col := UTF16ToByte(line, p.Character)
	offset := 0
	for i := 0; i < p.Line; i++ {
		offset += len(lines[i]) + 1
	}
	return offset + col
}

// UTF16ToByte converts an LSP UTF-16 column to a byte column.
func UTF16ToByte(line string, character int) int {
	units := 0
	for i, r := range line {
		if units >= character {
			return i
		}
		if r > 0xFFFF {
			units += 2
		} else {
			units++
		}
	}
	return len(line)
}

// ByteToUTF16 converts a byte column to an LSP UTF-16 column.
func ByteToUTF16(line string, byteCol int) int {
	if byteCol > len(line) {
		byteCol = len(line)
	}
	units := 0
	for i, r := range line {
		if i >= byteCol {
			break
		}
		if r > 0xFFFF {
			units += 2
		} else {
			units++
		}
	}
	return units
}

func (c *Client) didOpen(uri, text string) error {
	c.mu.Lock()
	c.docs[uri] = text
	already := c.open[uri]
	c.open[uri] = true
	c.mu.Unlock()
	if already {
		return c.didChange(uri, text)
	}
	return c.notify("textDocument/didOpen", map[string]any{"textDocument": map[string]any{
		"uri": uri, "languageId": c.langID, "version": 1, "text": text,
	}})
}

func (c *Client) didChange(uri, text string) error {
	c.mu.Lock()
	c.docVersion++
	version := c.docVersion
	c.docs[uri] = text
	c.mu.Unlock()
	return c.notify("textDocument/didChange", map[string]any{
		"textDocument":   map[string]any{"uri": uri, "version": version},
		"contentChanges": []map[string]any{{"text": text}},
	})
}

// DidOpen sends the full text of a newly opened document.
func (c *Client) DidOpen(uri, text string) error { return c.didOpen(uri, text) }

// DidChange sends a full-document change.
func (c *Client) DidChange(uri, text string) error { return c.didChange(uri, text) }

// DidSave tells the server the document was saved.
func (c *Client) DidSave(uri, text string) error {
	c.mu.Lock()
	c.docs[uri] = text
	c.mu.Unlock()
	return c.notify("textDocument/didSave", map[string]any{"textDocument": map[string]any{"uri": uri}, "text": text})
}

func (c *Client) textPosition(uri string, lineText string, byteCol int) Position {
	// Callers provide the full document through the URI cache; line number is
	// derived by the app. This helper only maps the column.
	return Position{Character: ByteToUTF16(lineText, byteCol)}
}

// Completion asks for completions at a zero-based line and byte column.
func (c *Client) Completion(ctx context.Context, uri, lineText string, line, byteCol int) ([]CompletionItem, error) {
	pos := Position{Line: line, Character: ByteToUTF16(lineText, byteCol)}
	res, err := c.request(ctx, "textDocument/completion", map[string]any{
		"textDocument": map[string]any{"uri": uri},
		"position":     pos,
		"context":      map[string]any{"triggerKind": 1},
	})
	if err != nil {
		return nil, err
	}
	var list []CompletionItem
	if err := json.Unmarshal(res, &list); err == nil {
		return list, nil
	}
	var wrapped struct {
		Items []CompletionItem `json:"items"`
	}
	if err := json.Unmarshal(res, &wrapped); err != nil {
		return nil, err
	}
	return wrapped.Items, nil
}

// Hover asks for documentation at a position.
func (c *Client) Hover(ctx context.Context, uri, lineText string, line, byteCol int) (*Hover, error) {
	res, err := c.request(ctx, "textDocument/hover", map[string]any{
		"textDocument": map[string]any{"uri": uri},
		"position":     Position{Line: line, Character: ByteToUTF16(lineText, byteCol)},
	})
	if err != nil {
		return nil, err
	}
	if len(res) == 0 || string(res) == "null" {
		return nil, nil
	}
	var h Hover
	if err := json.Unmarshal(res, &h); err != nil {
		return nil, err
	}
	return &h, nil
}

// Definition asks for definition locations at a position.
func (c *Client) Definition(ctx context.Context, uri, lineText string, line, byteCol int) ([]Location, error) {
	res, err := c.request(ctx, "textDocument/definition", map[string]any{
		"textDocument": map[string]any{"uri": uri},
		"position":     Position{Line: line, Character: ByteToUTF16(lineText, byteCol)},
	})
	if err != nil {
		return nil, err
	}
	return parseLocations(res)
}

// References asks for references at a position.
func (c *Client) References(ctx context.Context, uri, lineText string, line, byteCol int) ([]Location, error) {
	res, err := c.request(ctx, "textDocument/references", map[string]any{
		"textDocument": map[string]any{"uri": uri},
		"position":     Position{Line: line, Character: ByteToUTF16(lineText, byteCol)},
		"context":      map[string]any{"includeDeclaration": true},
	})
	if err != nil {
		return nil, err
	}
	return parseLocations(res)
}

func parseLocations(res json.RawMessage) ([]Location, error) {
	if len(res) == 0 || string(res) == "null" {
		return nil, nil
	}
	var list []Location
	if err := json.Unmarshal(res, &list); err == nil {
		return list, nil
	}
	var one Location
	if err := json.Unmarshal(res, &one); err != nil {
		return nil, err
	}
	return []Location{one}, nil
}

// Rename returns the workspace edit for renaming the symbol at a position.
func (c *Client) Rename(ctx context.Context, uri, lineText string, line, byteCol int, newName string) (map[string][]TextEdit, error) {
	res, err := c.request(ctx, "textDocument/rename", map[string]any{
		"textDocument": map[string]any{"uri": uri},
		"position":     Position{Line: line, Character: ByteToUTF16(lineText, byteCol)},
		"newName":      newName,
	})
	if err != nil {
		return nil, err
	}
	var edit struct {
		Changes map[string][]TextEdit `json:"changes"`
	}
	if err := json.Unmarshal(res, &edit); err != nil {
		return nil, err
	}
	return edit.Changes, nil
}

// Close shuts the server down and kills it if it does not exit promptly.
func (c *Client) Close() error {
	ctx, cancel := context.WithTimeout(context.Background(), 2*time.Second)
	defer cancel()
	_, _ = c.request(ctx, "shutdown", nil)
	_ = c.notify("exit", nil)
	_ = c.stdin.Close()
	done := make(chan error, 1)
	go func() { done <- c.cmd.Wait() }()
	select {
	case err := <-done:
		return err
	case <-time.After(2 * time.Second):
		_ = c.cmd.Process.Kill()
		return nil
	}
}

// TextDocument is a convenience for callers that only need an LSP URI and text.
type TextDocument struct {
	URI  string
	Text string
}

// MessageText flattens Hover contents into plain text or Markdown.
func MessageText(v any) string {
	switch x := v.(type) {
	case string:
		return x
	case map[string]any:
		if s, ok := x["value"].(string); ok {
			return s
		}
		if s, ok := x["language"].(string); ok {
			return s
		}
		if arr, ok := x["contents"]; ok {
			return MessageText(arr)
		}
	case []any:
		var b bytes.Buffer
		for _, item := range x {
			if s := MessageText(item); s != "" {
				if b.Len() > 0 {
					b.WriteString("\n\n")
				}
				b.WriteString(s)
			}
		}
		return b.String()
	}
	return ""
}

var _ = strconv.Itoa

func mustJSON(v any) []byte {
	b, _ := json.Marshal(v)
	return b
}

// SplitCommand splits a server command while honoring single and double quotes.
func SplitCommand(s string) []string {
	var out []string
	var cur strings.Builder
	quote := rune(0)
	flush := func() {
		if cur.Len() > 0 {
			out = append(out, cur.String())
			cur.Reset()
		}
	}
	for _, r := range s {
		if quote != 0 {
			if r == quote {
				quote = 0
			} else {
				cur.WriteRune(r)
			}
			continue
		}
		switch r {
		case '\'', '"':
			quote = r
		case ' ', '\t', '\n':
			flush()
		default:
			cur.WriteRune(r)
		}
	}
	flush()
	return out
}
