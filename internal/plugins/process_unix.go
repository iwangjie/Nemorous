//go:build !windows

package plugins

import (
	"os/exec"
	"syscall"
)

func prepareCommand(command string, args []string) (string, []string) {
	return command, args
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
