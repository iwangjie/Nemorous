//go:build windows

package run

import (
	"os/exec"
	"path/filepath"
	"strings"
	"syscall"
)

const createNewProcessGroup = 0x00000200

func prepareCommand(command string, args []string) (string, []string) {
	path, err := exec.LookPath(command)
	if err != nil {
		return command, args
	}
	switch strings.ToLower(filepath.Ext(path)) {
	case ".cmd", ".bat":
		return "cmd.exe", append([]string{"/c", path}, args...)
	default:
		return command, args
	}
}

func configureProcessGroup(cmd *exec.Cmd) {
	cmd.SysProcAttr = &syscall.SysProcAttr{CreationFlags: createNewProcessGroup}
}

func killProcessGroup(cmd *exec.Cmd) {
	if cmd.Process == nil {
		return
	}
	_ = cmd.Process.Kill()
}
