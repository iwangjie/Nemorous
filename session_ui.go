package main

import (
	"crypto/sha1"
	"encoding/hex"
	"os"
	"os/exec"
	"path/filepath"
	"time"

	"github.com/egoist/mygo"
	"github.com/egoist/mygo/ui/editor"
	"github.com/iwangjie/nemorous/internal/config"
	"github.com/iwangjie/nemorous/internal/session"
)

func (a *App) sessionPath() string {
	sum := sha1.Sum([]byte(filepath.Clean(a.cfg.WorkspacePath)))
	name := hex.EncodeToString(sum[:8]) + ".json"
	return filepath.Join(filepath.Dir(config.ConfigPath()), "sessions", name)
}

func (a *App) maybeRestoreSession() {
	if a.sessionRestored || len(a.workspaceFiles) == 0 {
		return
	}
	a.sessionRestored = true
	a.restoringSession = true
	defer func() { a.restoringSession = false }()
	st := session.Load(a.sessionPath())
	if st.Workspace != "" && filepath.Clean(st.Workspace) != filepath.Clean(a.cfg.WorkspacePath) {
		return
	}
	if len(st.OpenFiles) == 0 && st.ActiveTab == "" {
		return
	}
	for _, file := range st.OpenFiles {
		wf, ok := a.workspaceFileByKey(file.RepoID, file.RelPath)
		if !ok {
			continue
		}
		a.selectCodeFile(wf)
		if tab := a.activeCodeTab(); tab != nil && tab.state != nil && file.Line > 0 {
			tab.state.GoTo(editor.Pos{Line: file.Line - 1})
		}
	}
	if st.ActiveTab == string(TabCode) && len(a.codeTabs) > 0 {
		a.activeTab = TabCode
		a.codeTabIdx = st.ActiveFile
		if a.codeTabIdx < 0 || a.codeTabIdx >= len(a.codeTabs) {
			a.codeTabIdx = len(a.codeTabs) - 1
		}
	}
	if st.ChangesSplit > 80 {
		a.changesSplit = st.ChangesSplit
	}
	if st.CodeSplit > 80 {
		a.codeSplit = st.CodeSplit
	}
	if st.GitEditorPct > 10 && st.GitEditorPct < 95 {
		a.gitEditorPct = st.GitEditorPct
	}
	a.showGitPanel = st.ShowGitPanel
	if a.showGitPanel {
		a.expandGitForMerge()
	}
	if st.ShowManiPanel && a.maniAvailable() {
		a.showManiPanel = true
	}
	if a.pluginMgr != nil && st.PluginPanel != "" {
		if _, ok := a.pluginMgr.Panel(st.PluginPanel); ok {
			a.pluginPanel = st.PluginPanel
			a.pluginMgr.Dispatch(st.PluginPanel, "open", nil)
		}
	}
	if a.win != nil {
		a.win.Invalidate()
	}
}

func (a *App) saveSession() {
	st := session.State{
		Workspace:         a.cfg.WorkspacePath,
		ActiveTab:         string(a.activeTab),
		ActiveFile:        a.codeTabIdx,
		ShowGitPanel:      a.showGitPanel,
		ShowManiPanel:     a.showManiPanel,
		PluginPanel:       a.pluginPanel,
		ChangesSplit:      a.changesSplit,
		CodeSplit:         a.codeSplit,
		GitEditorPct:      a.gitEditorPct,
		ShowTerminalPanel: false,
		ShowLogPanel:      false,
	}
	for _, tab := range a.codeTabs {
		line := 0
		if tab.state != nil {
			line = tab.state.CursorPos().Line + 1
		}
		st.OpenFiles = append(st.OpenFiles, session.File{RepoID: tab.file.RepoID, RelPath: tab.file.RelPath, Line: line})
	}
	_ = session.Save(a.sessionPath(), st)
}

func (a *App) scheduleSessionSave() {
	if a.restoringSession {
		return
	}
	if a.sessionTimer != nil {
		a.sessionTimer.Stop()
	}
	a.sessionTimer = time.AfterFunc(400*time.Millisecond, func() {
		a.runUI(func() { a.saveSession() })
	})
}

var executablePath = os.Executable
var startProcess = func(cmd *exec.Cmd) error { return cmd.Start() }

// newWindow launches another Nemorous process, giving each workspace its
// own window and state while sharing the plugin store.
func (a *App) newWindow(workspace string) {
	exe, err := executablePath()
	if err != nil {
		a.showToast("err", "无法定位应用可执行文件")
		return
	}
	cmd := exec.Command(exe)
	env := os.Environ()
	env = append(env, "NEMOROUS_WINDOW=1")
	if workspace != "" {
		env = append(env, "NEMOROUS_WORKSPACE="+workspace)
	}
	if custom := os.Getenv("NEMOROUS_CONFIG"); custom != "" {
		env = append(env, "NEMOROUS_CONFIG="+custom)
	}
	cmd.Env = env
	if err := startProcess(cmd); err != nil {
		a.showToast("err", "新建窗口失败："+err.Error())
		return
	}
	go func() { _ = cmd.Wait() }()
	a.showToast("ok", "已打开新窗口")
}

func (a *App) newWindowForWorkspace() {
	go func() {
		paths, err := mygo.Dialog.Open(mygo.OpenDialogOptions{
			Parent:    a.win,
			Title:     "在新窗口打开工作区",
			Directory: true,
		})
		if err != nil || len(paths) == 0 {
			return
		}
		a.runUI(func() { a.newWindow(paths[0]) })
	}()
}

func (a *App) syncWindowTitle() {
	if a.win == nil {
		return
	}
	title := "Nemorous"
	if name := a.workspaceName(); name != "" && name != "workspace" {
		title += " — " + name
	}
	a.win.SetTitle(title)
}

func (a *App) workspaceName() string {
	if name := filepath.Base(a.cfg.WorkspacePath); name != "" && name != "." && name != string(filepath.Separator) {
		return name
	}
	return "workspace"
}
