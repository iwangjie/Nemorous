package run

import (
	"context"
	"os"
	"strings"
	"testing"
	"time"
)

func TestRunStreamsOutput(t *testing.T) {
	var lines []string
	res := Run(context.Background(), Spec{Command: "go", Args: []string{"env", "GOROOT"}}, func(line string) {
		lines = append(lines, line)
	})
	if res.Code != 0 || res.Err != nil {
		t.Fatalf("res = %+v", res)
	}
	if len(lines) == 0 || strings.TrimSpace(res.Output) == "" {
		t.Fatalf("no output: %+v", res)
	}
}

func TestRunCancel(t *testing.T) {
	ctx, cancel := context.WithCancel(context.Background())
	go func() {
		time.Sleep(200 * time.Millisecond)
		cancel()
	}()
	res := Run(ctx, Spec{
		Command: os.Args[0],
		Args:    []string{"-test.run=TestRunHelperProcess"},
		Env:     map[string]string{"NEMOROUS_RUN_HELPER": "1"},
	}, nil)
	if !res.Canceled || res.Code == 0 {
		t.Fatalf("expected cancellation, got %+v", res)
	}
}

func TestRunHelperProcess(t *testing.T) {
	if os.Getenv("NEMOROUS_RUN_HELPER") != "1" {
		return
	}
	time.Sleep(10 * time.Second)
}

func TestParseArgsAndEnv(t *testing.T) {
	args := Lines(`-run "a b" 'c d'`)
	if len(args) != 3 || args[1] != "a b" || args[2] != "c d" {
		t.Fatalf("args = %#v", args)
	}
	env := Env("A=1\nB = two words \n")
	if env["A"] != "1" || env["B"] != "two words" {
		t.Fatalf("env = %#v", env)
	}
}
