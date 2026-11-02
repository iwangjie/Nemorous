//go:build !windows

package plugins

import (
	"context"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"sync"
	"syscall"
	"time"
)

// prepareCommand resolves bare commands from the user's login shell when the
// app was launched without the shell's PATH, as Finder and Dock do on macOS.
// Without this, installed toolchains such as Homebrew, SDKMAN or ~/bin are
// invisible to plugins even though they work in a terminal.
func prepareCommand(command string, args []string) (string, []string) {
	return resolveCommand(command), args
}

func resolveCommand(command string) string {
	if command == "" || strings.ContainsRune(command, os.PathSeparator) {
		return command
	}
	if path, err := exec.LookPath(command); err == nil {
		return path
	}
	for _, dir := range filepath.SplitList(loginShellPath()) {
		if dir == "" {
			continue
		}
		candidate := filepath.Join(dir, command)
		info, err := os.Stat(candidate)
		if err == nil && !info.IsDir() && info.Mode()&0o111 != 0 {
			return candidate
		}
	}
	return command
}

var (
	loginPathOnce sync.Once
	loginPath     string
)

// loginShellPath reads PATH from the login shell once. Interactive prompts
// are avoided because the command runs with -l -c, and a timeout keeps a
// hung profile from blocking the first plugin job.
func loginShellPath() string {
	loginPathOnce.Do(func() {
		ctx, cancel := context.WithTimeout(context.Background(), 3*time.Second)
		defer cancel()
		cmd := exec.CommandContext(ctx, loginShell(), "-l", "-c", `printf '%s' "$PATH"`)
		cmd.WaitDelay = time.Second
		out, err := cmd.Output()
		if err != nil {
			return
		}
		raw := strings.TrimSpace(string(out))
		if i := strings.LastIndexByte(raw, '\n'); i >= 0 {
			raw = strings.TrimSpace(raw[i+1:])
		}
		loginPath = raw
	})
	return loginPath
}

// loginShell prefers SHELL, which Finder and Dock may leave unset, then the
// usual interactive shells. zsh first matches modern macOS defaults.
func loginShell() string {
	if shell := os.Getenv("SHELL"); shell != "" {
		return shell
	}
	for _, candidate := range []string{"/bin/zsh", "/bin/bash", "/bin/sh"} {
		if info, err := os.Stat(candidate); err == nil && !info.IsDir() && info.Mode()&0o111 != 0 {
			return candidate
		}
	}
	return "/bin/sh"
}

// defaultEnv is the environment plugin jobs inherit: the app's own, with the
// login shell's PATH appended so tools resolved from it can also find their
// own siblings (node, java, toolchain helpers) at run time.
func defaultEnv() []string {
	env := os.Environ()
	extra := loginShellPath()
	if extra == "" {
		return env
	}
	for i, entry := range env {
		if strings.HasPrefix(entry, "PATH=") {
			current := strings.TrimPrefix(entry, "PATH=")
			env[i] = "PATH=" + current + string(os.PathListSeparator) + extra
			return env
		}
	}
	return append(env, "PATH="+extra)
}

func configureProcessGroup(cmd *exec.Cmd) {
	cmd.SysProcAttr = &syscall.SysProcAttr{Setpgid: true}
}

func killProcessGroup(cmd *exec.Cmd) {
	if cmd.Process == nil {
		return
	}
	_ = syscall.Kill(-cmd.Process.Pid, syscall.SIGKILL)
}
