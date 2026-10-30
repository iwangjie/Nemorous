// Package dap is a Debug Adapter Protocol client over stdio or TCP.
package dap

import (
	"bufio"
	"context"
	"encoding/json"
	"fmt"
	"io"
	"net"
	"os/exec"
	"strconv"
	"strings"
	"sync"
	"time"
)

// Endpoint says how to reach the debug adapter.
type Endpoint struct {
	// Command/Args start the adapter. When Network is "tcp", the adapter
	// may listen itself and Command may be empty.
	Command string
	Args    []string
	// Network is "stdio" (default) or "tcp".
	Network string
	Address string
	// Dir is the adapter process's working directory.
	Dir string
	// Env adds environment values to the adapter process.
	Env map[string]string
}

// Message is one DAP protocol message.
type Message struct {
	Seq        int             `json:"seq"`
	Type       string          `json:"type"`
	Command    string          `json:"command,omitempty"`
	RequestSeq int             `json:"request_seq,omitempty"`
	Success    bool            `json:"success,omitempty"`
	Message    string          `json:"message,omitempty"`
	Event      string          `json:"event,omitempty"`
	Body       json.RawMessage `json:"body,omitempty"`
}

// Event is a DAP event.
type Event struct {
	Event string          `json:"event"`
	Body  json.RawMessage `json:"body"`
}

// Thread is a debug thread.
type Thread struct {
	ID   int    `json:"id"`
	Name string `json:"name"`
}

// Frame is one stack frame.
type Frame struct {
	ID     int    `json:"id"`
	Name   string `json:"name"`
	Line   int    `json:"line"`
	Column int    `json:"column"`
	Source struct {
		Name string `json:"name"`
		Path string `json:"path"`
	} `json:"source"`
}

// Scope is one variable scope.
type Scope struct {
	Name               string `json:"name"`
	VariablesReference int    `json:"variablesReference"`
}

// Variable is one variable.
type Variable struct {
	Name               string `json:"name"`
	Value              string `json:"value"`
	Type               string `json:"type"`
	VariablesReference int    `json:"variablesReference"`
}

// Breakpoint is one verified source breakpoint.
type Breakpoint struct {
	ID       int    `json:"id"`
	Verified bool   `json:"verified"`
	Line     int    `json:"line"`
	Message  string `json:"message"`
}

// Capabilities is the adapter's initialize response.
type Capabilities struct {
	SupportsConfigurationDoneRequest bool `json:"supportsConfigurationDoneRequest"`
	SupportsFunctionBreakpoints      bool `json:"supportsFunctionBreakpoints"`
	SupportsStepInTargetsRequest     bool `json:"supportsStepInTargetsRequest"`
}

// Client is a connected debug adapter.
type Client struct {
	mu      sync.Mutex
	seq     int
	pending map[int]chan Message
	write   io.WriteCloser
	reader  *bufio.Reader
	cmd     *exec.Cmd
	conn    net.Conn
	done    chan struct{}
	closed  bool

	eventMu sync.RWMutex
	onEvent func(Event)
	onReq   func(command string, body json.RawMessage) (any, error)
}

// Dial starts and/or connects to an adapter.
func Dial(ctx context.Context, ep Endpoint) (*Client, error) {
	c := &Client{pending: make(map[int]chan Message), done: make(chan struct{})}
	var reader io.Reader
	var writer io.WriteCloser
	switch strings.ToLower(ep.Network) {
	case "", "stdio":
		if strings.TrimSpace(ep.Command) == "" {
			return nil, fmt.Errorf("debug adapter command is required")
		}
		cmd := exec.Command(ep.Command, ep.Args...)
		cmd.Dir = ep.Dir
		applyEnv(cmd, ep.Env)
		stdin, err := cmd.StdinPipe()
		if err != nil {
			return nil, err
		}
		stdout, err := cmd.StdoutPipe()
		if err != nil {
			return nil, err
		}
		if err := cmd.Start(); err != nil {
			return nil, err
		}
		c.cmd = cmd
		reader, writer = stdout, stdin
	case "tcp":
		if ep.Command != "" {
			cmd := exec.Command(ep.Command, ep.Args...)
			cmd.Dir = ep.Dir
			applyEnv(cmd, ep.Env)
			if err := cmd.Start(); err != nil {
				return nil, err
			}
			c.cmd = cmd
		}
		conn, err := dialTCP(ctx, ep.Address)
		if err != nil {
			if c.cmd != nil {
				_ = c.cmd.Process.Kill()
				_, _ = c.cmd.Process.Wait()
			}
			return nil, err
		}
		c.conn = conn
		reader, writer = conn, conn
	default:
		return nil, fmt.Errorf("unknown debug adapter network %q", ep.Network)
	}
	c.reader = bufio.NewReader(reader)
	c.write = writer
	go c.readLoop()
	return c, nil
}

func applyEnv(cmd *exec.Cmd, env map[string]string) {
	if len(env) == 0 {
		return
	}
	values := make([]string, 0, len(env))
	for k, v := range env {
		values = append(values, k+"="+v)
	}
	cmd.Env = append(cmd.Environ(), values...)
}

func dialTCP(ctx context.Context, address string) (net.Conn, error) {
	deadline := time.Now().Add(10 * time.Second)
	for {
		d := net.Dialer{Timeout: 500 * time.Millisecond}
		conn, err := d.DialContext(ctx, "tcp", address)
		if err == nil {
			return conn, nil
		}
		if time.Now().After(deadline) || ctx.Err() != nil {
			return nil, fmt.Errorf("连接调试适配器 %s 失败：%w", address, err)
		}
		time.Sleep(150 * time.Millisecond)
	}
}

// SetEventHandler receives adapter events in the reader goroutine; it must
// not block.
func (c *Client) SetEventHandler(fn func(Event)) {
	c.eventMu.Lock()
	c.onEvent = fn
	c.eventMu.Unlock()
}

// SetReverseRequestHandler answers requests the adapter sends (for
// example runInTerminal). Returning an error sends an unsuccessful
// response.
func (c *Client) SetReverseRequestHandler(fn func(command string, body json.RawMessage) (any, error)) {
	c.eventMu.Lock()
	c.onReq = fn
	c.eventMu.Unlock()
}

func (c *Client) readLoop() {
	defer close(c.done)
	for {
		msg, err := c.readMessage()
		if err != nil {
			return
		}
		switch msg.Type {
		case "response":
			c.mu.Lock()
			ch := c.pending[msg.RequestSeq]
			delete(c.pending, msg.RequestSeq)
			c.mu.Unlock()
			if ch != nil {
				ch <- msg
			}
		case "event":
			c.eventMu.RLock()
			fn := c.onEvent
			c.eventMu.RUnlock()
			if fn != nil {
				fn(Event{Event: msg.Event, Body: msg.Body})
			}
		case "request":
			go c.answerReverse(msg)
		}
	}
}

func (c *Client) answerReverse(msg Message) {
	var body any
	var err error
	c.eventMu.RLock()
	fn := c.onReq
	c.eventMu.RUnlock()
	if fn != nil {
		body, err = fn(msg.Command, msg.Body)
	} else {
		err = fmt.Errorf("不支持适配器请求 %s", msg.Command)
	}
	response := map[string]any{
		"seq":         c.nextSeq(),
		"type":        "response",
		"request_seq": msg.Seq,
		"command":     msg.Command,
		"success":     err == nil,
	}
	if err != nil {
		response["message"] = err.Error()
	} else if body != nil {
		response["body"] = body
	}
	_ = c.writeMessage(response)
}

func (c *Client) readMessage() (Message, error) {
	length := -1
	for {
		line, err := c.reader.ReadString('\n')
		if err != nil {
			return Message{}, err
		}
		line = strings.TrimRight(line, "\r\n")
		if line == "" {
			break
		}
		if strings.HasPrefix(strings.ToLower(line), "content-length:") {
			n, err := strconv.Atoi(strings.TrimSpace(line[len("content-length:"):]))
			if err != nil || n < 0 || n > 16<<20 {
				return Message{}, fmt.Errorf("invalid Content-Length")
			}
			length = n
		}
	}
	if length < 0 {
		return Message{}, fmt.Errorf("missing Content-Length")
	}
	data := make([]byte, length)
	if _, err := io.ReadFull(c.reader, data); err != nil {
		return Message{}, err
	}
	var msg Message
	if err := json.Unmarshal(data, &msg); err != nil {
		return Message{}, err
	}
	return msg, nil
}

func (c *Client) nextSeq() int {
	c.mu.Lock()
	defer c.mu.Unlock()
	c.seq++
	return c.seq
}

func (c *Client) writeMessage(v any) error {
	data, err := json.Marshal(v)
	if err != nil {
		return err
	}
	c.mu.Lock()
	defer c.mu.Unlock()
	if c.closed {
		return fmt.Errorf("debug adapter is closed")
	}
	_, err = fmt.Fprintf(c.write, "Content-Length: %d\r\n\r\n%s", len(data), data)
	return err
}

// Request sends a request and waits for its response.
func (c *Client) Request(ctx context.Context, command string, args any) (json.RawMessage, error) {
	seq := c.nextSeq()
	msg := map[string]any{
		"seq":     seq,
		"type":    "request",
		"command": command,
	}
	if args != nil {
		msg["arguments"] = args
	}
	ch := make(chan Message, 1)
	c.mu.Lock()
	c.pending[seq] = ch
	c.mu.Unlock()
	if err := c.writeMessage(msg); err != nil {
		c.mu.Lock()
		delete(c.pending, seq)
		c.mu.Unlock()
		return nil, err
	}
	select {
	case resp, ok := <-ch:
		if !ok {
			return nil, fmt.Errorf("debug adapter closed")
		}
		if !resp.Success {
			return resp.Body, fmt.Errorf("%s", resp.Message)
		}
		return resp.Body, nil
	case <-ctx.Done():
		c.mu.Lock()
		delete(c.pending, seq)
		c.mu.Unlock()
		return nil, ctx.Err()
	case <-c.done:
		return nil, fmt.Errorf("debug adapter closed")
	}
}

// Decode decodes a response body.
func Decode[T any](raw json.RawMessage) (T, error) {
	var v T
	if len(raw) == 0 {
		return v, nil
	}
	err := json.Unmarshal(raw, &v)
	return v, err
}

// Initialize performs the DAP handshake.
func (c *Client) Initialize(ctx context.Context, adapterID string, args map[string]any) (Capabilities, error) {
	body := map[string]any{
		"clientID":        "nemorous",
		"clientName":      "Nemorous",
		"adapterID":       adapterID,
		"linesStartAt1":   true,
		"columnsStartAt1": true,
		"pathFormat":      "path",
	}
	for k, v := range args {
		body[k] = v
	}
	raw, err := c.Request(ctx, "initialize", body)
	if err != nil {
		return Capabilities{}, err
	}
	return Decode[Capabilities](raw)
}

// Launch starts the debuggee.
func (c *Client) Launch(ctx context.Context, args map[string]any) error {
	_, err := c.Request(ctx, "launch", args)
	return err
}

// SetBreakpoints sets source breakpoints in one file.
func (c *Client) SetBreakpoints(ctx context.Context, path string, lines []int) ([]Breakpoint, error) {
	bps := make([]map[string]any, 0, len(lines))
	for _, line := range lines {
		bps = append(bps, map[string]any{"line": line})
	}
	raw, err := c.Request(ctx, "setBreakpoints", map[string]any{
		"source":      map[string]any{"path": path},
		"breakpoints": bps,
		"lines":       lines,
	})
	if err != nil {
		return nil, err
	}
	var body struct {
		Breakpoints []Breakpoint `json:"breakpoints"`
	}
	err = json.Unmarshal(raw, &body)
	return body.Breakpoints, err
}

// ConfigurationDone finishes configuration.
func (c *Client) ConfigurationDone(ctx context.Context) error {
	_, err := c.Request(ctx, "configurationDone", map[string]any{})
	return err
}

// Threads lists debug threads.
func (c *Client) Threads(ctx context.Context) ([]Thread, error) {
	raw, err := c.Request(ctx, "threads", map[string]any{})
	if err != nil {
		return nil, err
	}
	var body struct {
		Threads []Thread `json:"threads"`
	}
	err = json.Unmarshal(raw, &body)
	return body.Threads, err
}

// StackTrace lists one thread's frames.
func (c *Client) StackTrace(ctx context.Context, threadID int) ([]Frame, error) {
	raw, err := c.Request(ctx, "stackTrace", map[string]any{"threadId": threadID, "startFrame": 0, "levels": 50})
	if err != nil {
		return nil, err
	}
	var body struct {
		StackFrames []Frame `json:"stackFrames"`
	}
	err = json.Unmarshal(raw, &body)
	return body.StackFrames, err
}

// Scopes lists one frame's scopes.
func (c *Client) Scopes(ctx context.Context, frameID int) ([]Scope, error) {
	raw, err := c.Request(ctx, "scopes", map[string]any{"frameId": frameID})
	if err != nil {
		return nil, err
	}
	var body struct {
		Scopes []Scope `json:"scopes"`
	}
	err = json.Unmarshal(raw, &body)
	return body.Scopes, err
}

// Variables lists variables of a reference.
func (c *Client) Variables(ctx context.Context, reference int) ([]Variable, error) {
	raw, err := c.Request(ctx, "variables", map[string]any{"variablesReference": reference})
	if err != nil {
		return nil, err
	}
	var body struct {
		Variables []Variable `json:"variables"`
	}
	err = json.Unmarshal(raw, &body)
	return body.Variables, err
}

func (c *Client) simple(ctx context.Context, command string, args map[string]any) error {
	_, err := c.Request(ctx, command, args)
	return err
}

// Continue resumes execution.
func (c *Client) Continue(ctx context.Context, threadID int) error {
	return c.simple(ctx, "continue", map[string]any{"threadId": threadID})
}

// Next steps over.
func (c *Client) Next(ctx context.Context, threadID int) error {
	return c.simple(ctx, "next", map[string]any{"threadId": threadID})
}

// StepIn steps into.
func (c *Client) StepIn(ctx context.Context, threadID int) error {
	return c.simple(ctx, "stepIn", map[string]any{"threadId": threadID})
}

// StepOut steps out.
func (c *Client) StepOut(ctx context.Context, threadID int) error {
	return c.simple(ctx, "stepOut", map[string]any{"threadId": threadID})
}

// Pause pauses execution.
func (c *Client) Pause(ctx context.Context, threadID int) error {
	return c.simple(ctx, "pause", map[string]any{"threadId": threadID})
}

// Disconnect ends the debug session.
func (c *Client) Disconnect(ctx context.Context) error {
	err := c.simple(ctx, "disconnect", map[string]any{"terminateDebuggee": true})
	c.Close()
	return err
}

// Done closes when the adapter's transport ends.
func (c *Client) Done() <-chan struct{} { return c.done }

// Close closes the transport and stops the adapter process.
func (c *Client) Close() {
	c.mu.Lock()
	if c.closed {
		c.mu.Unlock()
		return
	}
	c.closed = true
	pending := make([]chan Message, 0, len(c.pending))
	for _, ch := range c.pending {
		pending = append(pending, ch)
	}
	c.pending = map[int]chan Message{}
	write := c.write
	conn := c.conn
	cmd := c.cmd
	c.mu.Unlock()
	if write != nil {
		_ = write.Close()
	}
	if conn != nil {
		_ = conn.Close()
	}
	if cmd != nil && cmd.Process != nil {
		_ = cmd.Process.Kill()
		_, _ = cmd.Process.Wait()
	}
	for _, ch := range pending {
		close(ch)
	}
}
