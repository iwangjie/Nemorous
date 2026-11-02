package plugins

import (
	"bufio"
	"context"
	"errors"
	"fmt"
	"io"
	"os/exec"
	"strings"
	"sync"
)

const maxOutputLine = 1 << 20

// jobEnv starts from the app's environment (with the login shell's PATH) and
// applies the plugin's overrides, replacing duplicates instead of relying on
// platform-specific last-wins behaviour.
func jobEnv(extra map[string]string) []string {
	env := defaultEnv()
	if len(extra) == 0 {
		return env
	}
	merged := make(map[string]string, len(env)+len(extra))
	order := make([]string, 0, len(env)+len(extra))
	for _, entry := range env {
		k, v, ok := strings.Cut(entry, "=")
		if !ok {
			continue
		}
		if _, seen := merged[k]; !seen {
			order = append(order, k)
		}
		merged[k] = v
	}
	for k, v := range extra {
		if _, seen := merged[k]; !seen {
			order = append(order, k)
		}
		merged[k] = v
	}
	out := make([]string, 0, len(order))
	for _, k := range order {
		out = append(out, k+"="+merged[k])
	}
	return out
}

type jobSpec struct {
	Command string
	Args    []string
	Cwd     string
	Env     map[string]string
}

// runJob starts spec and streams its merged output. onOutput and onExit
// are called from this goroutine; the runtime marshals them onto the
// plugin's JavaScript loop. Cancelling the returned function kills the
// process group.
func runJob(ctx context.Context, spec jobSpec, onOutput func(string), onExit func(code int, message string)) (func(), error) {
	runCtx, cancel := context.WithCancel(ctx)
	command, args := prepareCommand(spec.Command, spec.Args)
	cmd := exec.CommandContext(runCtx, command, args...)
	cmd.Dir = spec.Cwd
	cmd.Env = jobEnv(spec.Env)
	configureProcessGroup(cmd)
	cmd.Cancel = func() error {
		killProcessGroup(cmd)
		return nil
	}
	if runCtx.Err() != nil {
		cancel()
		return nil, runCtx.Err()
	}
	pr, pw := io.Pipe()
	cmd.Stdout = pw
	cmd.Stderr = pw
	if err := cmd.Start(); err != nil {
		cancel()
		pw.Close()
		pr.Close()
		go onExit(-1, err.Error())
		return func() {}, nil
	}
	var scanDone sync.WaitGroup
	scanDone.Add(1)
	go func() {
		defer scanDone.Done()
		reader := bufio.NewReaderSize(pr, 64<<10)
		for {
			line, err := reader.ReadString('\n')
			if len(line) > 0 {
				onOutput(trimText(line, maxOutputLine))
			}
			if err != nil {
				return
			}
		}
	}()
	go func() {
		err := cmd.Wait()
		pw.Close()
		scanDone.Wait()
		code := 0
		message := ""
		if err != nil {
			var exitErr *exec.ExitError
			switch {
			case errors.As(err, &exitErr):
				code = exitErr.ExitCode()
				if code < 0 {
					code = -1
				}
			case errors.Is(runCtx.Err(), context.Canceled):
				code = -1
				message = "已取消"
			default:
				code = -1
			}
		}
		if runCtx.Err() != nil && message == "" {
			message = "已取消"
			if code == 0 {
				code = -1
			}
		}
		onExit(code, message)
	}()
	return cancel, nil
}

func normalizeArgs(args []any) []string {
	out := make([]string, 0, len(args))
	for _, a := range args {
		out = append(out, fmt.Sprint(a))
	}
	return out
}

func joinCommand(command string, args []string) string {
	if len(args) == 0 {
		return command
	}
	return command + " " + strings.Join(args, " ")
}
