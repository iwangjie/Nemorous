// Package run starts Run/Debug/Test configurations and streams their
// output.
package run

import (
	"bufio"
	"context"
	"errors"
	"fmt"
	"io"
	"os/exec"
	"strings"
	"sync"
	"time"
)

// Spec is one command to run.
type Spec struct {
	Command string
	Args    []string
	Dir     string
	Env     map[string]string
}

// Result is the outcome of a run.
type Result struct {
	Code     int
	Err      error
	Output   string
	Duration time.Duration
	Canceled bool
}

// Run executes spec and calls onLine for each output line. Cancelling ctx
// kills the process group. It returns only after the process and its
// output are finished.
func Run(ctx context.Context, spec Spec, onLine func(string)) Result {
	start := time.Now()
	res := Result{}
	if strings.TrimSpace(spec.Command) == "" {
		res.Code = -1
		res.Err = fmt.Errorf("命令不能为空")
		return res
	}
	command, args := prepareCommand(spec.Command, spec.Args)
	cmd := exec.CommandContext(ctx, command, args...)
	cmd.Dir = spec.Dir
	if len(spec.Env) > 0 {
		env := make([]string, 0, len(spec.Env))
		for k, v := range spec.Env {
			env = append(env, k+"="+v)
		}
		cmd.Env = append(cmd.Environ(), env...)
	}
	configureProcessGroup(cmd)
	cmd.Cancel = func() error {
		killProcessGroup(cmd)
		return nil
	}
	pr, pw := io.Pipe()
	cmd.Stdout = pw
	cmd.Stderr = pw
	if err := cmd.Start(); err != nil {
		pw.Close()
		pr.Close()
		res.Code = -1
		res.Err = err
		res.Duration = time.Since(start)
		return res
	}
	var wg sync.WaitGroup
	var output strings.Builder
	var mu sync.Mutex
	wg.Add(1)
	go func() {
		defer wg.Done()
		reader := bufio.NewReaderSize(pr, 64<<10)
		for {
			line, err := reader.ReadString('\n')
			if len(line) > 0 {
				if len(line) > 1<<20 {
					line = line[:1<<20]
				}
				mu.Lock()
				if output.Len() < 2<<20 {
					output.WriteString(line)
				}
				mu.Unlock()
				if onLine != nil {
					onLine(strings.TrimRight(line, "\r\n"))
				}
			}
			if err != nil {
				return
			}
		}
	}()
	err := cmd.Wait()
	pw.Close()
	wg.Wait()
	res.Duration = time.Since(start)
	res.Code = 0
	if err != nil {
		res.Err = err
		var exitErr *exec.ExitError
		if errors.As(err, &exitErr) {
			res.Code = exitErr.ExitCode()
			if res.Code < 0 {
				res.Code = -1
			}
		} else {
			res.Code = -1
		}
	}
	if ctx.Err() != nil {
		res.Canceled = true
		if res.Code == 0 {
			res.Code = -1
		}
		if res.Err == nil {
			res.Err = ctx.Err()
		}
	}
	mu.Lock()
	res.Output = output.String()
	mu.Unlock()
	return res
}

// Lines splits a configuration's argument text, honoring quotes.
func Lines(s string) []string {
	var out []string
	var current strings.Builder
	var quote rune
	flush := func() {
		if current.Len() > 0 {
			out = append(out, current.String())
			current.Reset()
		}
	}
	for _, r := range s {
		switch {
		case quote != 0:
			if r == quote {
				quote = 0
			} else {
				current.WriteRune(r)
			}
		case r == '"' || r == '\'':
			quote = r
		case r == ' ' || r == '\t' || r == '\n':
			flush()
		default:
			current.WriteRune(r)
		}
	}
	flush()
	return out
}

// Env parses "K=V" lines.
func Env(s string) map[string]string {
	out := map[string]string{}
	for _, line := range strings.Split(s, "\n") {
		line = strings.TrimSpace(line)
		if line == "" {
			continue
		}
		if i := strings.IndexByte(line, '='); i > 0 {
			out[strings.TrimSpace(line[:i])] = strings.TrimSpace(line[i+1:])
		}
	}
	return out
}
