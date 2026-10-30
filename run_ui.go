package main

import (
	"context"
	"encoding/json"
	"fmt"
	"math"
	"os/exec"
	"path/filepath"
	"strings"
	"sync"
	"time"

	"github.com/egoist/mygo/ui"
	"github.com/iwangjie/nemorous/internal/config"
	"github.com/iwangjie/nemorous/internal/dap"
	"github.com/iwangjie/nemorous/internal/gitops"
	runpkg "github.com/iwangjie/nemorous/internal/run"
)

func (a *App) installRunDefaults() {
	if a.runPanelPct == 0 {
		a.runPanelPct = 42
	}
}

func (a *App) runConfigList() []config.RunConfig { return a.cfg.RunConfigs }

func (a *App) selectedRunConfig() *config.RunConfig {
	for i := range a.cfg.RunConfigs {
		if a.cfg.RunConfigs[i].ID == a.runConfigID {
			return &a.cfg.RunConfigs[i]
		}
	}
	if len(a.cfg.RunConfigs) > 0 {
		a.runConfigID = a.cfg.RunConfigs[0].ID
		return &a.cfg.RunConfigs[0]
	}
	return nil
}

func (a *App) toggleRunPanel() {
	if a.showRunPanel {
		a.showRunPanel = false
		a.stopRun()
		a.stopDebug()
		return
	}
	a.showRunPanel = true
	a.showGitPanel = false
	a.showLogPanel = false
	a.showTerminalPanel = false
	a.installRunDefaults()
}

func (a *App) openRunConfig(id string) {
	a.showRunConfig = true
	a.runCfgErr = ""
	a.runCfgAdvanced = false
	if id == "" {
		a.runCfgIsNew = true
		a.runCfgDraft = config.RunConfig{
			ID:      fmt.Sprintf("run-%d", time.Now().UnixNano()),
			Name:    "新运行配置",
			Kind:    "run",
			Command: "go",
			Args:    "run .",
			Cwd:     ".",
		}
		a.runCfgEnvText = ""
		a.runCfgLaunchText = ""
		return
	}
	a.runCfgIsNew = false
	for _, cfg := range a.cfg.RunConfigs {
		if cfg.ID == id {
			a.runCfgDraft = cfg
			a.runCfgEnvText = envText(cfg.Env)
			a.runCfgLaunchText = cfg.Launch
			return
		}
	}
}

func envText(env map[string]string) string {
	if len(env) == 0 {
		return ""
	}
	var lines []string
	for k, v := range env {
		lines = append(lines, k+"="+v)
	}
	return strings.Join(lines, "\n")
}

func (a *App) saveRunConfigDraft() {
	cfg := a.runCfgDraft
	cfg.Name = strings.TrimSpace(cfg.Name)
	cfg.Command = strings.TrimSpace(cfg.Command)
	if cfg.Name == "" {
		a.runCfgErr = "名称不能为空"
		return
	}
	if cfg.Kind == "debug" {
		if strings.TrimSpace(cfg.Adapter) == "" {
			a.runCfgErr = "调试配置需要 Adapter 命令（如 dlv、debugpy、node）"
			return
		}
	} else if cfg.Command == "" {
		a.runCfgErr = "命令不能为空"
		return
	}
	cfg.Env = runpkg.Env(a.runCfgEnvText)
	cfg.Launch = strings.TrimSpace(a.runCfgLaunchText)
	if cfg.Launch != "" {
		var probe map[string]any
		if err := json.Unmarshal([]byte(cfg.Launch), &probe); err != nil {
			a.runCfgErr = "Launch JSON 无效：" + err.Error()
			return
		}
	}
	replaced := false
	for i := range a.cfg.RunConfigs {
		if a.cfg.RunConfigs[i].ID == cfg.ID {
			a.cfg.RunConfigs[i] = cfg
			replaced = true
			break
		}
	}
	if !replaced {
		a.cfg.RunConfigs = append(a.cfg.RunConfigs, cfg)
	}
	a.runConfigID = cfg.ID
	_ = a.cfg.Save()
	a.showRunConfig = false
	a.showToast("ok", "已保存运行配置 "+cfg.Name)
}

func (a *App) deleteRunConfig() {
	id := a.runCfgDraft.ID
	out := a.cfg.RunConfigs[:0]
	for _, cfg := range a.cfg.RunConfigs {
		if cfg.ID != id {
			out = append(out, cfg)
		}
	}
	a.cfg.RunConfigs = out
	_ = a.cfg.Save()
	a.showRunConfig = false
	if a.runConfigID == id {
		a.runConfigID = ""
	}
}

func (a *App) appendRunOutput(lines []string) {
	a.runOutput = append(a.runOutput, lines...)
	if len(a.runOutput) > 4000 {
		a.runOutput = a.runOutput[len(a.runOutput)-4000:]
	}
}

func (a *App) launchSelectedConfig() {
	cfg := a.selectedRunConfig()
	if cfg == nil {
		a.showToast("warn", "请先新建运行配置")
		return
	}
	if cfg.Kind == "debug" {
		a.startDebug(*cfg)
		return
	}
	a.startRun(*cfg)
}

func (a *App) startRun(cfg config.RunConfig) {
	if a.runRunning {
		a.showToast("warn", "已有运行中的任务")
		return
	}
	workspace := a.cfg.WorkspacePath
	cwd := cfg.Cwd
	if cwd == "" {
		cwd = "."
	}
	if !filepath.IsAbs(cwd) {
		cwd = joinPath(workspace, cwd)
	}
	spec := runpkg.Spec{Command: cfg.Command, Args: runpkg.Lines(cfg.Args), Dir: cwd, Env: cfg.Env}
	ctx, cancel := context.WithCancel(context.Background())
	a.runCancel = cancel
	a.runRunning = true
	a.runStatus = "运行中"
	a.runExit = -1
	a.runOutput = nil
	a.runStartedAt = time.Now()
	a.showRunPanel = true
	a.showGitPanel = false
	a.showLogPanel = false
	a.showTerminalPanel = false
	var mu sync.Mutex
	var pending []string
	last := time.Now()
	flush := func(final bool) {
		mu.Lock()
		if len(pending) == 0 {
			mu.Unlock()
			return
		}
		batch := append([]string(nil), pending...)
		pending = pending[:0]
		last = time.Now()
		mu.Unlock()
		a.runUI(func() {
			a.appendRunOutput(batch)
			if a.win != nil {
				a.win.Invalidate()
			}
		})
	}
	go func() {
		res := runpkg.Run(ctx, spec, func(line string) {
			mu.Lock()
			pending = append(pending, line)
			due := time.Since(last) > 100*time.Millisecond
			mu.Unlock()
			if due {
				flush(false)
			}
		})
		flush(true)
		a.runUI(func() {
			a.runRunning = false
			a.runCancel = nil
			a.runExit = res.Code
			switch {
			case res.Canceled:
				a.runStatus = "已停止"
				a.runOutput = append(a.runOutput, "— 已停止 —")
			case res.Code == 0:
				a.runStatus = "成功"
				a.runOutput = append(a.runOutput, fmt.Sprintf("— 成功（%s）—", res.Duration.Round(time.Millisecond)))
			default:
				a.runStatus = "失败"
				msg := ""
				if res.Err != nil {
					msg = "：" + res.Err.Error()
				}
				a.runOutput = append(a.runOutput, fmt.Sprintf("— 退出码 %d%s —", res.Code, msg))
			}
			a.appendRunOutput(nil)
			if a.runExit == 0 {
				a.showToast("ok", cfg.Name+" 运行成功")
			} else if !res.Canceled {
				a.showToast("warn", fmt.Sprintf("%s 运行失败（exit %d）", cfg.Name, res.Code))
			}
		})
	}()
}

func (a *App) stopRun() {
	if a.runCancel != nil {
		a.runCancel()
		a.runStatus = "停止中…"
	}
}

func (a *App) debugConfig() *config.RunConfig {
	return a.selectedRunConfig()
}

func (a *App) startDebug(cfg config.RunConfig) {
	if a.debugClient != nil {
		a.stopDebug()
	}
	workspace := a.cfg.WorkspacePath
	adapterArgs := runpkg.Lines(cfg.AdapterArgs)
	endpoint := dap.Endpoint{
		Command: cfg.Adapter,
		Args:    adapterArgs,
		Network: cfg.Network,
		Address: cfg.Address,
		Dir:     joinPath(workspace, cfg.Cwd),
		Env:     cfg.Env,
	}
	if endpoint.Dir == "" {
		endpoint.Dir = workspace
	}
	breakpoints := make(map[string][]int, len(a.breakpoints))
	bpFiles := make(map[string]gitops.WorkspaceFile)
	for key, lines := range a.breakpoints {
		breakpoints[key] = append([]int(nil), lines...)
		parts := strings.SplitN(key, "::", 2)
		if len(parts) == 2 {
			if wf, ok := a.workspaceFileByKey(parts[0], parts[1]); ok {
				bpFiles[key] = wf
			}
		}
	}
	a.runStatus = "启动调试适配器…"
	a.runOutput = nil
	a.runExit = -1
	a.runRunning = true
	go func() {
		ctx, cancel := context.WithTimeout(context.Background(), 30*time.Second)
		defer cancel()
		client, err := dap.Dial(ctx, endpoint)
		if err != nil {
			a.runUI(func() {
				a.runRunning = false
				a.runStatus = "调试启动失败"
				a.appendRunOutput([]string{err.Error()})
			})
			return
		}
		events := make(chan dap.Event, 64)
		client.SetEventHandler(func(ev dap.Event) {
			select {
			case events <- ev:
			default:
			}
		})
		client.SetReverseRequestHandler(func(command string, body json.RawMessage) (any, error) {
			if command != "runInTerminal" {
				return nil, fmt.Errorf("不支持适配器请求 %s", command)
			}
			var req struct {
				Args []string `json:"args"`
				Cwd  string   `json:"cwd"`
			}
			if err := json.Unmarshal(body, &req); err != nil || len(req.Args) == 0 {
				return nil, fmt.Errorf("runInTerminal 参数无效")
			}
			cmd := exec.Command(req.Args[0], req.Args[1:]...)
			if req.Cwd != "" {
				cmd.Dir = req.Cwd
			}
			if err := cmd.Start(); err != nil {
				return nil, err
			}
			return map[string]any{"processId": cmd.Process.Pid}, nil
		})
		if _, err := client.Initialize(ctx, cfg.Adapter, nil); err != nil {
			client.Close()
			a.runUI(func() {
				a.runRunning = false
				a.runStatus = "调试初始化失败"
				a.appendRunOutput([]string{err.Error()})
			})
			return
		}
		// Send breakpoints before launch.
		for key, lines := range breakpoints {
			if wf, ok := bpFiles[key]; ok {
				_, _ = client.SetBreakpoints(ctx, wf.AbsPath, lines)
			}
		}
		launchArgs := map[string]any{}
		if strings.TrimSpace(cfg.Launch) != "" {
			_ = json.Unmarshal([]byte(cfg.Launch), &launchArgs)
		}
		if cfg.Program != "" {
			launchArgs["program"] = cfg.Program
		}
		if strings.TrimSpace(cfg.ProgramArgs) != "" {
			launchArgs["args"] = runpkg.Lines(cfg.ProgramArgs)
		}
		if cfg.Cwd != "" {
			launchArgs["cwd"] = joinPath(workspace, cfg.Cwd)
		}
		if _, ok := launchArgs["cwd"]; !ok {
			launchArgs["cwd"] = workspace
		}
		if err := client.Launch(ctx, launchArgs); err != nil {
			client.Close()
			a.runUI(func() {
				a.runRunning = false
				a.runStatus = "调试启动失败"
				a.appendRunOutput([]string{err.Error()})
			})
			return
		}
		if err := client.ConfigurationDone(ctx); err != nil {
			client.Close()
			a.runUI(func() {
				a.runRunning = false
				a.runStatus = "调试启动失败"
				a.appendRunOutput([]string{err.Error()})
			})
			return
		}
		eventCtx, eventCancel := context.WithCancel(context.Background())
		a.runUI(func() {
			a.debugClient = client
			a.debugEventCancel = eventCancel
			a.runStatus = "调试中"
			a.debugStopped = false
			a.debugMsg = "已连接 " + cfg.Adapter
			a.showRunPanel = true
			a.showGitPanel = false
			a.showTerminalPanel = false
			a.showLogPanel = false
			go func() {
				for {
					select {
					case ev := <-events:
						a.runUI(func() { a.handleDebugEvent(client, ev) })
					case <-eventCtx.Done():
						return
					case <-client.Done():
						a.runUI(func() {
							if a.debugClient == client {
								a.debugClient = nil
								a.runRunning = false
								if a.runStatus == "调试中" {
									a.runStatus = "调试连接已断开"
								}
							}
						})
						return
					}
				}
			}()
		})
	}()
}

func (a *App) handleDebugEvent(client *dap.Client, ev dap.Event) {
	if a.debugClient != client {
		return
	}
	switch ev.Event {
	case "stopped":
		var body struct {
			ThreadID int    `json:"threadId"`
			Reason   string `json:"reason"`
		}
		_ = json.Unmarshal(ev.Body, &body)
		a.debugStopped = true
		a.runStatus = "已暂停"
		a.debugMsg = "已停止"
		if body.Reason != "" {
			a.debugMsg += "：" + body.Reason
		}
		if body.ThreadID > 0 {
			a.debugThreadID = body.ThreadID
		}
		a.refreshDebugState()
	case "continued":
		a.debugStopped = false
		a.runStatus = "调试中"
		a.debugMsg = "运行中"
		a.debugFrames = nil
		a.debugVars = nil
	case "terminated", "exited":
		a.debugStopped = false
		a.runStatus = "调试结束"
		a.runRunning = false
		a.debugMsg = "调试会话已结束"
		if a.debugEventCancel != nil {
			a.debugEventCancel()
			a.debugEventCancel = nil
		}
		client.Close()
		if a.debugClient == client {
			a.debugClient = nil
		}
	case "output":
		var body struct {
			Output string `json:"output"`
		}
		_ = json.Unmarshal(ev.Body, &body)
		if strings.TrimSpace(body.Output) != "" {
			a.appendRunOutput(strings.Split(strings.TrimRight(body.Output, "\n"), "\n"))
		}
	}
}

func (a *App) refreshDebugState() {
	client := a.debugClient
	if client == nil {
		return
	}
	threadID := a.debugThreadID
	if threadID == 0 {
		threadID = 1
	}
	go func() {
		ctx, cancel := context.WithTimeout(context.Background(), 10*time.Second)
		defer cancel()
		frames, err := client.StackTrace(ctx, threadID)
		if err != nil {
			a.runUI(func() { a.debugMsg = err.Error() })
			return
		}
		var scopes []dap.Scope
		var vars []dap.Variable
		if len(frames) > 0 {
			scopes, _ = client.Scopes(ctx, frames[0].ID)
			for _, scope := range scopes {
				vs, _ := client.Variables(ctx, scope.VariablesReference)
				vars = append(vars, vs...)
			}
		}
		a.runUI(func() {
			if a.debugClient != client {
				return
			}
			a.debugFrames = frames
			a.debugScopes = scopes
			a.debugVars = vars
			if len(frames) > 0 {
				a.debugFrameID = frames[0].ID
			}
		})
	}()
}

func (a *App) stopDebug() {
	client := a.debugClient
	if client == nil {
		return
	}
	if a.debugEventCancel != nil {
		a.debugEventCancel()
		a.debugEventCancel = nil
	}
	a.debugClient = nil
	a.runRunning = false
	a.runStatus = "调试已停止"
	a.debugStopped = false
	a.debugFrames = nil
	a.debugVars = nil
	go func() {
		ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
		defer cancel()
		_ = client.Disconnect(ctx)
		client.Close()
	}()
}

func (a *App) debugCommand(name string) {
	client := a.debugClient
	if client == nil {
		return
	}
	threadID := a.debugThreadID
	if threadID == 0 {
		threadID = 1
	}
	go func() {
		ctx, cancel := context.WithTimeout(context.Background(), 10*time.Second)
		defer cancel()
		var err error
		switch name {
		case "continue":
			err = client.Continue(ctx, threadID)
		case "next":
			err = client.Next(ctx, threadID)
		case "stepIn":
			err = client.StepIn(ctx, threadID)
		case "stepOut":
			err = client.StepOut(ctx, threadID)
		case "pause":
			err = client.Pause(ctx, threadID)
		}
		if err != nil {
			a.runUI(func() { a.debugMsg = err.Error() })
		}
	}()
}

func joinPath(base, p string) string {
	if p == "" {
		return base
	}
	if filepath.IsAbs(p) {
		return p
	}
	return filepath.Join(base, p)
}

func (a *App) runStatusColor(p Palette) ui.Color {
	switch a.runStatus {
	case "成功", "调试中":
		return p.Green
	case "已暂停":
		return p.Amber
	case "失败", "调试启动失败", "调试初始化失败", "调试已停止":
		return p.Danger
	case "运行中", "启动调试适配器…":
		return p.Accent
	default:
		return p.TextMuted
	}
}

func runKindIcon(kind string) *ui.SVG {
	switch kind {
	case "debug":
		return icoBug
	case "test":
		return icoCheck
	default:
		return icoPlay
	}
}

func runStatusIcon(status string) *ui.SVG {
	switch status {
	case "成功", "调试中":
		return icoCheck
	case "已暂停":
		return icoPause
	case "失败", "调试启动失败", "调试初始化失败", "调试已停止", "调试连接已断开":
		return icoAlert
	case "运行中", "启动调试适配器…":
		return icoPlay
	default:
		return icoStop
	}
}

func (a *App) buildRunPanel(c *ui.Context, p Palette) {
	cfg := a.selectedRunConfig()
	kind := "run"
	kindLabel := "运行"
	if cfg != nil {
		kind = cfg.Kind
		switch kind {
		case "debug":
			kindLabel = "调试"
		case "test":
			kindLabel = "测试"
		}
	}
	active := a.runRunning || a.debugClient != nil
	ui.Column(c).Grow(1).FillHeight().Background(p.RailBg).BorderWidth(0, 0, 1, 0).BorderColor(p.Border).Children(func() {
		ui.Row(c).Height(34).Padding(0, 8).Gap(2).AlignItems(ui.Center).BorderWidth(0, 0, 1, 0).BorderColor(p.Border).Children(func() {
			configLabel := "运行"
			if cfg != nil {
				configLabel = cfg.Name
			}
			menuIconPill(c, p, "run-config-menu", runKindIcon(kind), configLabel, false, func(m *ui.Menu) {
				for _, rc := range a.cfg.RunConfigs {
					rc := rc
					label := rc.Name
					switch rc.Kind {
					case "test":
						label = "Test · " + label
					case "debug":
						label = "Debug · " + label
					}
					if m.Item(label).Radio(a.runConfigID == rc.ID).Chosen() {
						a.runConfigID = rc.ID
					}
				}
				if len(a.cfg.RunConfigs) > 0 {
					m.Separator()
				}
				if m.Item("新建配置…").Chosen() {
					a.openRunConfig("")
				}
			})
			if a.runStatus != "" && a.debugClient == nil {
				ui.Icon(c, runStatusIcon(a.runStatus)).Size(13, 13).TextColor(a.runStatusColor(p)).Tooltip(a.runStatus).Label(a.runStatus)
			}
			ui.Spacer(c)
			iconAction(c, p, "run-launch", runKindIcon(kind), kindLabel, true, func() { a.launchSelectedConfig() })
			toolButton(c, p, "run-stop", icoStop, "停止", false, active, func() {
				a.stopRun()
				a.stopDebug()
			})
			toolButton(c, p, "run-edit", icoSettings, "编辑配置", false, cfg != nil, func() {
				if cfg != nil {
					a.openRunConfig(cfg.ID)
				} else {
					a.openRunConfig("")
				}
			})
			toolButton(c, p, "run-new", icoPlus, "新建配置", false, true, func() { a.openRunConfig("") })
			toolButton(c, p, "run-close", icoClose, "关闭", false, true, func() {
				a.stopRun()
				a.stopDebug()
				a.showRunPanel = false
			})
		})
		if a.debugModeActive() && len(a.breakpoints) > 0 {
			ui.Row(c).FillWidth().Height(26).Padding(0, 8).Gap(4).AlignItems(ui.Center).BorderWidth(0, 0, 1, 0).BorderColor(p.BorderSubtle).Children(func() {
				drawIcon(c, icoAlert, 11, p.Danger)
				count := 0
				for _, lines := range a.breakpoints {
					count += len(lines)
				}
				ui.Text(c, fmt.Sprint(count)).FontSize(9.5).TextColor(p.TextMuted)
				ui.ScrollHorizontal(c).Grow(1).Children(func() {
					ui.Row(c).Gap(4).Children(func() {
						for key, lines := range a.breakpoints {
							parts := strings.SplitN(key, "::", 2)
							if len(parts) != 2 {
								continue
							}
							for _, line := range lines {
								key, parts, line := key, parts, line
								chip := ui.Row(c.Key(fmt.Sprintf("bp-%s-%d", key, line))).Padding(1, 5).Radius(3).Background(p.DangerSoft).Children(func() {
									ui.Text(c, fmt.Sprintf("%s:%d", shortenPath(parts[1]), line)).Font("monospace").FontSize(8.5).TextColor(p.Danger)
								})
								chip.Cursor(ui.CursorPointer)
								if chip.Clicked() {
									a.toggleBreakpointKey(key, "", parts[1], line)
								}
							}
						}
					})
				})
				toolButton(c, p, "bp-clear", icoTrash, "清空断点", false, true, func() {
					a.breakpoints = make(map[string][]int)
					a.cfg.Breakpoints = nil
					_ = a.cfg.Save()
					a.syncBreakpoints()
				})
			})
		}
		if a.debugClient != nil {
			ui.Row(c).FillWidth().Height(30).Padding(0, 8).Gap(2).AlignItems(ui.Center).BorderWidth(0, 0, 1, 0).BorderColor(p.BorderSubtle).Children(func() {
				for _, ctl := range []struct {
					id, tip string
					svg     *ui.SVG
				}{
					{"continue", "继续", icoPlay}, {"next", "单步跳过", icoStepOver}, {"stepIn", "单步进入", icoStepInto},
					{"stepOut", "单步返回", icoStepOut}, {"pause", "暂停", icoPause},
				} {
					ctl := ctl
					toolButton(c, p, "dbg-"+ctl.id, ctl.svg, ctl.tip, false, true, func() { a.debugCommand(ctl.id) })
				}
				ui.Spacer(c)
				if a.debugMsg != "" {
					ui.Text(c, a.debugMsg).FontSize(9.5).TextColor(p.TextMuted).MaxLines(1)
				}
			})
			if len(a.debugFrames) > 0 {
				ui.Row(c).Grow(1).Gap(10).Padding(6, 8).Children(func() {
					ui.Column(c).Width(300).FillHeight().Gap(2).Children(func() {
						ui.Text(c, "调用栈").FontSize(10).FontWeight(weightSemibold).TextColor(p.TextSecondary)
						ui.Scroll(c).Grow(1).Children(func() {
							for i, frame := range a.debugFrames {
								frame := frame
								kc := c.Key(fmt.Sprintf("dbg-frame-%d", i))
								row := ui.Row(kc).Padding(2, 4).Gap(4).Radius(3).Cursor(ui.CursorPointer)
								if row.Hovered() {
									row.Background(p.SurfaceHover)
								}
								if row.Clicked() {
									a.debugFrameID = frame.ID
									if frame.Source.Path != "" {
										if wf, ok := a.workspaceFileForSearch(frame.Source.Path); ok {
											a.openCodeAtLine(wf.RepoID, wf.RepoName, wf.RepoPath, wf.RelPath, wf.AbsPath, frame.Line)
										}
									}
									go func() {
										ctx, cancel := context.WithTimeout(context.Background(), 10*time.Second)
										defer cancel()
										scopes, _ := a.debugClient.Scopes(ctx, frame.ID)
										var vars []dap.Variable
										for _, scope := range scopes {
											vs, _ := a.debugClient.Variables(ctx, scope.VariablesReference)
											vars = append(vars, vs...)
										}
										a.runUI(func() {
											a.debugScopes = scopes
											a.debugVars = vars
										})
									}()
								}
								row.Children(func() {
									ui.Text(kc, frame.Name).Font("monospace").FontSize(9.5).TextColor(p.TextPrimary).Grow(1).MaxLines(1)
									ui.Text(kc, fmt.Sprintf("%s:%d", shortenPath(frame.Source.Path), frame.Line)).FontSize(8.5).TextColor(p.TextMuted).MaxLines(1)
								})
							}
						})
					})
					ui.Column(c).Grow(1).FillHeight().Gap(2).Children(func() {
						ui.Text(c, "变量").FontSize(10).FontWeight(weightSemibold).TextColor(p.TextSecondary)
						ui.Scroll(c).Grow(1).Children(func() {
							for _, v := range a.debugVars {
								ui.Row(c).Gap(5).Children(func() {
									ui.Text(c, v.Name).Font("monospace").FontSize(9.5).TextColor(p.AccentSoftText)
									ui.Text(c, "=").FontSize(9).TextColor(p.TextMuted)
									ui.Text(c, v.Value).Font("monospace").FontSize(9.5).TextColor(p.TextPrimary).MaxLines(1)
								})
							}
						})
					})
				})
			}
		}
		outScroll := ui.Scroll(c).Grow(1).Padding(6).Background(p.Surface)
		st := a.runScroll
		if st == nil {
			st = &ui.ScrollState{}
			a.runScroll = st
		}
		outScroll.TrackScroll(st)
		if len(a.runOutput) != a.runScrollLen {
			a.runScrollLen = len(a.runOutput)
			st.Y = math.MaxFloat32
		}
		outScroll.Children(func() {
			if len(a.runOutput) == 0 {
				emptyState(c, p, icoTerminal, "暂无输出", "")
				return
			}
			ui.Text(c, strings.Join(a.runOutput, "\n")).Font("monospace").FontSize(10).TextColor(p.TextSecondary)
		})
	})
}

func shortenPath(p string) string {
	if p == "" {
		return ""
	}
	parts := strings.FieldsFunc(p, func(r rune) bool { return r == '/' || r == '\\' })
	if len(parts) <= 2 {
		return p
	}
	return strings.Join(parts[len(parts)-2:], "/")
}

func (a *App) buildRunConfigModal(c *ui.Context, p Palette) {
	ui.DialogBase(c, &a.showRunConfig, func(backdrop, panel ui.Element) {
		backdrop.Background(ui.RGBA(20, 22, 30, 0.38))
		panel.Width(620).Height(580).Padding(14).Gap(8).Radius(12).Background(p.Surface).Border(1, p.Border)
		modalHeader(c, p, icoSettings, "运行配置", a.runCfgDraft.Name, nil, func() { a.showRunConfig = false })
		ui.Scroll(c).Grow(1).Children(func() {
			ui.Column(c).FillWidth().Gap(6).Children(func() {
				formRow(c, p, "名称", func() {
					ui.TextInput(c.Key("rc-name"), &a.runCfgDraft.Name).FillWidth()
				})
				formRow(c, p, "类型", func() {
					ui.Row(c).Gap(2).Children(func() {
						for _, kind := range []struct {
							id, label string
							svg       *ui.SVG
						}{{"run", "运行", icoPlay}, {"test", "测试", icoCheck}, {"debug", "调试", icoBug}} {
							kind := kind
							active := a.runCfgDraft.Kind == kind.id
							kc := c.Key("rc-kind-" + kind.id)
							el := ui.Row(kc).Padding(3, 9).Gap(4).Radius(4).Cursor(ui.CursorPointer).AlignItems(ui.Center)
							fg := p.TextSecondary
							if active {
								el.Background(p.AccentSoft)
								fg = p.AccentSoftText
							}
							if el.Hovered() && !active {
								el.Background(p.SurfaceHover)
								fg = p.TextPrimary
							}
							if el.Clicked() {
								a.runCfgDraft.Kind = kind.id
							}
							el.Children(func() {
								drawIcon(kc, kind.svg, 12, fg)
								ui.Text(kc, kind.label).FontSize(10.5).FontWeight(weightMedium).TextColor(fg)
							})
						}
					})
				})
				if a.runCfgDraft.Kind == "debug" {
					formRow(c, p, "程序", func() {
						ui.TextInput(c.Key("rc-program"), &a.runCfgDraft.Program).FillWidth().Placeholder("./cmd/app 或 main.py")
					})
					formRow(c, p, "程序参数", func() {
						ui.TextInput(c.Key("rc-program-args"), &a.runCfgDraft.ProgramArgs).FillWidth().Placeholder("--flag value")
					})
				} else {
					formRow(c, p, "命令", func() {
						ui.TextInput(c.Key("rc-command"), &a.runCfgDraft.Command).FillWidth().Placeholder("go / npm / mvn")
					})
					formRow(c, p, "参数", func() {
						ui.TextInput(c.Key("rc-args"), &a.runCfgDraft.Args).FillWidth().Placeholder("run . / test ./...")
					})
				}
				formRow(c, p, "目录", func() {
					ui.TextInput(c.Key("rc-cwd"), &a.runCfgDraft.Cwd).FillWidth().Placeholder(".")
				})
				formRow(c, p, "环境", func() {
					ui.TextArea(c.Key("rc-env"), &a.runCfgEnvText).Height(56).FillWidth().Placeholder("KEY=VALUE，每行一个")
				})
				if a.runCfgDraft.Kind == "debug" {
					ui.Box(c).Height(1).FillWidth().Background(p.BorderSubtle)
					advRow := ui.Row(c).Height(26).Gap(4).AlignItems(ui.Center).Cursor(ui.CursorPointer)
					if advRow.Clicked() {
						a.runCfgAdvanced = !a.runCfgAdvanced
					}
					advRow.Children(func() {
						chev := icoChevronRight
						if a.runCfgAdvanced {
							chev = icoChevronDown
						}
						drawIcon(c, chev, 12, p.TextMuted)
						ui.Text(c, "高级").FontSize(10.5).TextColor(p.TextSecondary)
						ui.Spacer(c)
						menuIconPill(c, p, "rc-template", icoSparkles, "模板", false, func(m *ui.Menu) {
							if m.Item("Go · dlv").Chosen() {
								a.runCfgDraft.Adapter = "dlv"
								a.runCfgDraft.AdapterArgs = "dap --listen=127.0.0.1:38697"
								a.runCfgDraft.Network = "tcp"
								a.runCfgDraft.Address = "127.0.0.1:38697"
								a.runCfgDraft.Launch = `{"mode":"debug"}`
								a.runCfgAdvanced = true
							}
							if m.Item("Python · debugpy").Chosen() {
								a.runCfgDraft.Adapter = "python3"
								a.runCfgDraft.AdapterArgs = "-m debugpy --listen 127.0.0.1:5678 --wait-for-client"
								a.runCfgDraft.Network = "tcp"
								a.runCfgDraft.Address = "127.0.0.1:5678"
								a.runCfgDraft.Program = "main.py"
								a.runCfgAdvanced = true
							}
						})
					})
					if a.runCfgAdvanced {
						formRow(c, p, "适配器", func() {
							ui.TextInput(c.Key("rc-adapter"), &a.runCfgDraft.Adapter).FillWidth().Placeholder("dlv / debugpy / node")
						})
						formRow(c, p, "适配器参数", func() {
							ui.TextInput(c.Key("rc-adapter-args"), &a.runCfgDraft.AdapterArgs).FillWidth().Placeholder("dap --listen=127.0.0.1:0")
						})
						formRow(c, p, "网络", func() {
							ui.Row(c).Gap(6).Children(func() {
								ui.TextInput(c.Key("rc-network"), &a.runCfgDraft.Network).Width(96).Placeholder("stdio")
								ui.TextInput(c.Key("rc-address"), &a.runCfgDraft.Address).Grow(1).Placeholder("127.0.0.1:38697")
							})
						})
						formRow(c, p, "Launch JSON", func() {
							ui.TextArea(c.Key("rc-launch"), &a.runCfgLaunchText).Height(52).FillWidth().Placeholder(`{"mode":"debug","stopOnEntry":true}`)
						})
					}
				}
				if a.runCfgErr != "" {
					ui.Text(c, a.runCfgErr).FontSize(10).TextColor(p.Danger)
				}
			})
		})
		ui.Row(c).FillWidth().AlignItems(ui.Center).Children(func() {
			if !a.runCfgIsNew {
				toolButton(c, p, "rc-delete", icoTrash, "删除配置", false, true, func() { a.deleteRunConfig() })
			}
			ui.Spacer(c)
			iconPill(c, p, icoCheck, "保存", true, func() { a.saveRunConfigDraft() })
		})
	})
}
