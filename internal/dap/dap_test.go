package dap

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
)

func TestDAPHelperProcess(t *testing.T) {
	if os.Getenv("NEMOROUS_DAP_HELPER") != "1" {
		return
	}
	logPath := os.Getenv("NEMOROUS_DAP_LOG")
	logf, _ := os.OpenFile(logPath, os.O_CREATE|os.O_APPEND|os.O_WRONLY, 0o644)
	reader := bufio.NewReader(os.Stdin)
	write := func(v any) {
		data, _ := json.Marshal(v)
		fmt.Fprintf(os.Stdout, "Content-Length: %d\r\n\r\n%s", len(data), data)
	}
	seq := 0
	sendEvent := func(name string, body any) {
		seq++
		write(map[string]any{"seq": seq, "type": "event", "event": name, "body": body})
	}
	for {
		length := -1
		for {
			line, err := reader.ReadString('\n')
			if err != nil {
				os.Exit(0)
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
			os.Exit(1)
		}
		data := make([]byte, length)
		if _, err := io.ReadFull(reader, data); err != nil {
			os.Exit(0)
		}
		var req struct {
			Seq     int             `json:"seq"`
			Command string          `json:"command"`
			Args    json.RawMessage `json:"arguments"`
		}
		_ = json.Unmarshal(data, &req)
		if logf != nil {
			fmt.Fprintf(logf, "%s %s\n", req.Command, string(req.Args))
		}
		seq++
		resp := map[string]any{"seq": seq, "type": "response", "request_seq": req.Seq, "command": req.Command, "success": true}
		switch req.Command {
		case "initialize":
			resp["body"] = map[string]any{"supportsConfigurationDoneRequest": true}
		case "setBreakpoints":
			resp["body"] = map[string]any{"breakpoints": []map[string]any{{"verified": true, "line": 10}}}
		case "configurationDone":
			write(resp)
			sendEvent("stopped", map[string]any{"threadId": 1, "reason": "breakpoint"})
			continue
		case "threads":
			resp["body"] = map[string]any{"threads": []map[string]any{{"id": 1, "name": "main"}}}
		case "stackTrace":
			resp["body"] = map[string]any{"stackFrames": []map[string]any{{"id": 100, "name": "main", "line": 10, "source": map[string]any{"path": "/tmp/main.go"}}}}
		case "scopes":
			resp["body"] = map[string]any{"scopes": []map[string]any{{"name": "Locals", "variablesReference": 1}}}
		case "variables":
			resp["body"] = map[string]any{"variables": []map[string]any{{"name": "x", "value": "42", "variablesReference": 0}}}
		case "continue":
			write(resp)
			sendEvent("continued", map[string]any{"threadId": 1})
			sendEvent("terminated", map[string]any{})
			continue
		case "disconnect":
			write(resp)
			os.Exit(0)
		}
		write(resp)
	}
}

func TestClientAgainstMockAdapter(t *testing.T) {
	logPath := t.TempDir() + "/dap.log"
	c, err := Dial(context.Background(), Endpoint{
		Command: os.Args[0],
		Args:    []string{"-test.run=TestDAPHelperProcess"},
		Env:     map[string]string{"NEMOROUS_DAP_HELPER": "1", "NEMOROUS_DAP_LOG": logPath},
	})
	if err != nil {
		t.Fatal(err)
	}
	defer c.Close()
	events := make(chan Event, 8)
	c.SetEventHandler(func(ev Event) { events <- ev })
	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()
	caps, err := c.Initialize(ctx, "mock", nil)
	if err != nil || !caps.SupportsConfigurationDoneRequest {
		t.Fatalf("initialize = %+v %v", caps, err)
	}
	bps, err := c.SetBreakpoints(ctx, "/tmp/main.go", []int{10})
	if err != nil || len(bps) != 1 || !bps[0].Verified {
		t.Fatalf("breakpoints = %+v %v", bps, err)
	}
	if err := c.ConfigurationDone(ctx); err != nil {
		t.Fatal(err)
	}
	select {
	case ev := <-events:
		if ev.Event != "stopped" {
			t.Fatalf("event = %+v", ev)
		}
	case <-ctx.Done():
		t.Fatal("no stopped event")
	}
	threads, err := c.Threads(ctx)
	if err != nil || len(threads) != 1 || threads[0].ID != 1 {
		t.Fatalf("threads = %+v %v", threads, err)
	}
	frames, err := c.StackTrace(ctx, 1)
	if err != nil || len(frames) != 1 || frames[0].Line != 10 {
		t.Fatalf("frames = %+v %v", frames, err)
	}
	scopes, err := c.Scopes(ctx, frames[0].ID)
	if err != nil || len(scopes) != 1 || scopes[0].VariablesReference != 1 {
		t.Fatalf("scopes = %+v %v", scopes, err)
	}
	vars, err := c.Variables(ctx, scopes[0].VariablesReference)
	if err != nil || len(vars) != 1 || vars[0].Value != "42" {
		t.Fatalf("vars = %+v %v", vars, err)
	}
	if err := c.Continue(ctx, 1); err != nil {
		t.Fatal(err)
	}
	if err := c.Pause(ctx, 1); err != nil {
		t.Fatalf("pause after termination should not hang: %v", err)
	}
}
