package plugins

import (
	"context"
	"encoding/json"
	"fmt"
	"log"
	"os"
	"path/filepath"
	"strings"
	"sync"
	"sync/atomic"
	"time"

	"github.com/dop251/goja"
)

const (
	runtimeRenderInterval = 120 * time.Millisecond
	runtimeQueueSize      = 512
	maxFSFileSize         = 4 << 20
	maxFSEntries          = 5000
	maxStorageSize        = 1 << 20
	maxJobs               = 4
)

type runtimeJob struct {
	id       string
	cancel   context.CancelFunc
	onOutput goja.Value
	onExit   goja.Value
}

// Runtime is one plugin's JavaScript VM and its event loop. Everything
// that touches the VM runs on the loop goroutine.
type Runtime struct {
	mgr    *Manager
	plugin Plugin

	loop   chan func()
	stopCh chan struct{}
	stopMu sync.Once
	done   chan struct{}

	vmPtr atomic.Pointer[goja.Runtime]
	vm    *goja.Runtime

	dirty          bool
	lastInvalidate time.Time
	seq            uint64
	jobSeq         int
	jobs           map[string]*runtimeJob
	storage        map[string]any
	logs           []string
	evalErr        error
}

func newRuntime(mgr *Manager, p Plugin) *Runtime {
	return &Runtime{
		mgr:     mgr,
		plugin:  p,
		loop:    make(chan func(), runtimeQueueSize),
		stopCh:  make(chan struct{}),
		done:    make(chan struct{}),
		jobs:    make(map[string]*runtimeJob),
		storage: make(map[string]any),
	}
}

func (rt *Runtime) run() {
	defer close(rt.done)
	vm := goja.New()
	vm.SetFieldNameMapper(goja.TagFieldNameMapper("json", true))
	rt.vm = vm
	rt.vmPtr.Store(vm)
	rt.loadStorage()
	if err := rt.installAPI(vm); err != nil {
		rt.publishError(err.Error())
		return
	}
	script, err := os.ReadFile(filepath.Join(rt.plugin.Dir, rt.plugin.Manifest.Main))
	if err != nil {
		rt.publishError("读取入口文件失败：" + err.Error())
		return
	}
	if _, err := vm.RunScript(rt.plugin.Manifest.Main, string(script)); err != nil {
		rt.publishError("加载插件失败：" + err.Error())
		return
	}
	rt.callOptional("activate")
	rt.renderAll()
	ticker := time.NewTicker(runtimeRenderInterval)
	defer ticker.Stop()
	for {
		select {
		case <-rt.stopCh:
			rt.callOptional("deactivate")
			for _, job := range rt.jobs {
				job.cancel()
			}
			return
		case fn := <-rt.loop:
			fn()
		case <-ticker.C:
			if rt.dirty {
				rt.dirty = false
				rt.renderAll()
			}
		}
	}
}

func (rt *Runtime) stop() {
	rt.stopMu.Do(func() {
		close(rt.stopCh)
		if vm := rt.vmPtr.Load(); vm != nil {
			vm.Interrupt("plugin stopped")
		}
	})
	select {
	case <-rt.done:
	case <-time.After(4 * time.Second):
		log.Printf("[plugin %s] runtime did not stop in time", rt.plugin.Manifest.ID)
	}
}

func (rt *Runtime) post(fn func()) {
	select {
	case rt.loop <- fn:
	case <-rt.stopCh:
	}
}

func (rt *Runtime) action(panelID, action string, payload any) {
	rt.post(func() {
		rt.dirty = false
		rt.call("handleAction", action, payload, panelID)
		rt.renderAll()
		rt.mgr.invalidate()
	})
}

func (rt *Runtime) workspaceChanged() {
	rt.post(func() {
		rt.callOptional("onWorkspaceChanged")
		rt.dirty = false
		rt.renderAll()
	})
}

func (rt *Runtime) report(where string, err error) {
	if err == nil {
		return
	}
	select {
	case <-rt.stopCh:
		return // the VM was interrupted because the plugin is shutting down
	default:
	}
	msg := where + ": " + err.Error()
	rt.logs = append(rt.logs, msg)
	if len(rt.logs) > 100 {
		rt.logs = rt.logs[len(rt.logs)-100:]
	}
	log.Printf("[plugin %s] %s", rt.plugin.Manifest.ID, msg)
	rt.mgr.notify("err", rt.plugin.Manifest.Name+" 插件："+err.Error())
}

func (rt *Runtime) call(name string, args ...any) {
	rt.callOptionalArgs(name, args...)
}

func (rt *Runtime) callOptional(name string) {
	rt.callOptionalArgs(name)
}

func (rt *Runtime) callOptionalArgs(name string, args ...any) {
	fn, ok := goja.AssertFunction(rt.vm.Get(name))
	if !ok {
		return
	}
	values := make([]goja.Value, len(args))
	for i, a := range args {
		values[i] = rt.vm.ToValue(a)
	}
	if _, err := fn(goja.Undefined(), values...); err != nil {
		rt.report(name, err)
	}
}

func (rt *Runtime) renderAll() {
	renderFn, ok := goja.AssertFunction(rt.vm.Get("render"))
	if !ok {
		rt.publishError("插件没有定义 render(panelId)")
		return
	}
	for _, panel := range rt.plugin.Manifest.Contributes.Panels {
		snap := &PanelSnapshot{
			Ref:       panelRef(rt.plugin.Manifest.ID, panel.ID),
			PluginID:  rt.plugin.Manifest.ID,
			PanelID:   panel.ID,
			Title:     panel.Title,
			UpdatedAt: time.Now(),
		}
		value, err := renderFn(goja.Undefined(), rt.vm.ToValue(panel.ID))
		if err != nil {
			snap.Err = err.Error()
			snap.Root = errorNode(err.Error())
			rt.mgr.publish(snap)
			continue
		}
		node, err := decodeNode(value.Export())
		if err != nil {
			snap.Err = err.Error()
			snap.Root = errorNode(err.Error())
			rt.mgr.publish(snap)
			continue
		}
		rt.seq++
		snap.Version = rt.seq
		snap.Root = node
		rt.mgr.publish(snap)
	}
}

// markDirty asks for a render and wakes the UI at most every 80 ms, so a
// build's output lines cannot flood the window with frames.
func (rt *Runtime) markDirty() {
	rt.dirty = true
	now := time.Now()
	if now.Sub(rt.lastInvalidate) < 80*time.Millisecond {
		return
	}
	rt.lastInvalidate = now
	rt.mgr.invalidate()
}

func (rt *Runtime) publishError(msg string) {
	rt.evalErr = fmt.Errorf("%s", msg)
	for _, panel := range rt.plugin.Manifest.Contributes.Panels {
		rt.seq++
		rt.mgr.publish(&PanelSnapshot{
			Ref:       panelRef(rt.plugin.Manifest.ID, panel.ID),
			PluginID:  rt.plugin.Manifest.ID,
			PanelID:   panel.ID,
			Title:     panel.Title,
			Version:   rt.seq,
			Err:       msg,
			Root:      errorNode(msg),
			UpdatedAt: time.Now(),
		})
	}
}

func (rt *Runtime) loadStorage() {
	rt.storage = make(map[string]any)
	path := filepath.Join(rt.plugin.DataDir, "storage.json")
	data, err := os.ReadFile(path)
	if err != nil {
		return
	}
	_ = json.Unmarshal(data, &rt.storage)
	if rt.storage == nil {
		rt.storage = make(map[string]any)
	}
}

func (rt *Runtime) saveStorage() {
	data, err := json.MarshalIndent(rt.storage, "", "  ")
	if err != nil {
		rt.report("storage", err)
		return
	}
	if len(data) > maxStorageSize {
		rt.report("storage", fmt.Errorf("存储超过 1 MB"))
		return
	}
	if err := os.MkdirAll(rt.plugin.DataDir, 0o755); err != nil {
		rt.report("storage", err)
		return
	}
	tmp := filepath.Join(rt.plugin.DataDir, "storage.json.tmp")
	if err := os.WriteFile(tmp, data, 0o644); err != nil {
		rt.report("storage", err)
		return
	}
	if err := os.Rename(tmp, filepath.Join(rt.plugin.DataDir, "storage.json")); err != nil {
		rt.report("storage", err)
	}
}

func (rt *Runtime) installAPI(vm *goja.Runtime) error {
	api := vm.NewObject()
	api.Set("appVersion", rt.mgr.AppVersion())
	api.Set("plugin", map[string]any{
		"id":      rt.plugin.Manifest.ID,
		"name":    rt.plugin.Manifest.Name,
		"version": rt.plugin.Manifest.Version,
		"dir":     rt.plugin.Dir,
	})
	api.Set("workspace", func(goja.FunctionCall) goja.Value {
		return vm.ToValue(rt.mgr.Workspace())
	})
	api.Set("log", func(call goja.FunctionCall) goja.Value {
		msg := call.Argument(0).String()
		rt.logs = append(rt.logs, msg)
		if len(rt.logs) > 200 {
			rt.logs = rt.logs[len(rt.logs)-200:]
		}
		log.Printf("[plugin %s] %s", rt.plugin.Manifest.ID, msg)
		return goja.Undefined()
	})
	api.Set("notify", func(call goja.FunctionCall) goja.Value {
		kind := "ok"
		msg := call.Argument(0).String()
		if len(call.Arguments) > 1 {
			kind = call.Argument(0).String()
			msg = call.Argument(1).String()
		}
		if kind != "ok" && kind != "warn" && kind != "err" && kind != "info" {
			kind = "info"
		}
		rt.mgr.notify(kind, msg)
		return goja.Undefined()
	})
	api.Set("invalidate", func(goja.FunctionCall) goja.Value {
		rt.markDirty()
		return goja.Undefined()
	})

	fsAPI := vm.NewObject()
	fsAPI.Set("read", func(call goja.FunctionCall) goja.Value {
		p := call.Argument(0).String()
		data, err := rt.fsRead(p)
		if err != nil {
			panic(vm.NewGoError(err))
		}
		return vm.ToValue(string(data))
	})
	fsAPI.Set("list", func(call goja.FunctionCall) goja.Value {
		p := call.Argument(0).String()
		entries, err := rt.fsList(p)
		if err != nil {
			panic(vm.NewGoError(err))
		}
		return vm.ToValue(entries)
	})
	fsAPI.Set("exists", func(call goja.FunctionCall) goja.Value {
		p := call.Argument(0).String()
		ok, err := rt.fsExists(p)
		if err != nil {
			panic(vm.NewGoError(err))
		}
		return vm.ToValue(ok)
	})
	fsAPI.Set("resolve", func(call goja.FunctionCall) goja.Value {
		base := call.Argument(0).String()
		rel := call.Argument(1).String()
		p, err := rt.fsResolve(base, rel)
		if err != nil {
			panic(vm.NewGoError(err))
		}
		return vm.ToValue(p)
	})
	api.Set("fs", fsAPI)

	storageAPI := vm.NewObject()
	storageAPI.Set("get", func(call goja.FunctionCall) goja.Value {
		key := call.Argument(0).String()
		v, ok := rt.storage[key]
		if !ok {
			return goja.Null()
		}
		return vm.ToValue(v)
	})
	storageAPI.Set("set", func(call goja.FunctionCall) goja.Value {
		key := call.Argument(0).String()
		value := call.Argument(1).Export()
		if !jsonSafe(value) {
			panic(vm.NewGoError(fmt.Errorf("storage.set: value must be JSON")))
		}
		rt.storage[key] = value
		rt.saveStorage()
		return goja.Undefined()
	})
	storageAPI.Set("remove", func(call goja.FunctionCall) goja.Value {
		delete(rt.storage, call.Argument(0).String())
		rt.saveStorage()
		return goja.Undefined()
	})
	api.Set("storage", storageAPI)

	api.Set("exec", func(call goja.FunctionCall) goja.Value {
		id, err := rt.startExec(call.Argument(0))
		if err != nil {
			panic(vm.NewGoError(err))
		}
		return vm.ToValue(id)
	})
	api.Set("cancel", func(call goja.FunctionCall) goja.Value {
		id := call.Argument(0).String()
		if job, ok := rt.jobs[id]; ok {
			job.cancel()
		}
		return goja.Undefined()
	})
	return vm.Set("nemorous", api)
}

func jsonSafe(v any) bool {
	if v == nil {
		return true
	}
	_, err := json.Marshal(v)
	return err == nil
}

func (rt *Runtime) startExec(arg goja.Value) (string, error) {
	if !rt.plugin.Manifest.HasPermission(PermissionProcessExec) {
		return "", fmt.Errorf("插件未声明 process.exec 权限")
	}
	obj := arg.ToObject(rt.vm)
	if obj == nil {
		return "", fmt.Errorf("exec: options object required")
	}
	get := func(name string) goja.Value {
		v := obj.Get(name)
		if v == nil || goja.IsUndefined(v) || goja.IsNull(v) {
			return nil
		}
		return v
	}
	command := ""
	if v := get("command"); v != nil {
		command = strings.TrimSpace(v.String())
	}
	if command == "" {
		return "", fmt.Errorf("exec: command is required")
	}
	var args []string
	if v := get("args"); v != nil {
		exported := v.Export()
		switch list := exported.(type) {
		case []any:
			if len(list) > 256 {
				return "", fmt.Errorf("exec: too many arguments")
			}
			for _, a := range list {
				s := fmt.Sprint(a)
				if len(s) > 4096 {
					return "", fmt.Errorf("exec: argument too long")
				}
				args = append(args, s)
			}
		case []string:
			args = append(args, list...)
		default:
			return "", fmt.Errorf("exec: args must be an array")
		}
	}
	cwd := rt.mgr.Workspace().Path
	if v := get("cwd"); v != nil && strings.TrimSpace(v.String()) != "" {
		cwd = strings.TrimSpace(v.String())
	}
	if cwd == "" {
		return "", fmt.Errorf("exec: no workspace loaded")
	}
	if !rt.pathAllowed(cwd) {
		return "", fmt.Errorf("exec: cwd is outside the workspace")
	}
	env := make(map[string]string)
	if v := get("env"); v != nil {
		if exported, ok := v.Export().(map[string]any); ok {
			for k, val := range exported {
				if len(env) > 64 {
					return "", fmt.Errorf("exec: too many env values")
				}
				env[k] = fmt.Sprint(val)
			}
		}
	}
	if len(rt.jobs) >= maxJobs {
		return "", fmt.Errorf("exec: 同时运行的任务过多")
	}
	rt.jobSeq++
	id := fmt.Sprintf("job-%d", rt.jobSeq)
	ctx, cancel := context.WithCancel(context.Background())
	job := &runtimeJob{id: id, cancel: cancel}
	if v := get("onOutput"); v != nil {
		job.onOutput = v
	}
	if v := get("onExit"); v != nil {
		job.onExit = v
	}
	rt.jobs[id] = job
	spec := jobSpec{Command: command, Args: args, Cwd: cwd, Env: env}
	finish, err := runJob(ctx, spec,
		func(line string) {
			rt.post(func() { rt.callJob(job.onOutput, line) })
		},
		func(code int, message string) {
			rt.post(func() {
				rt.callJob(job.onExit, code, message)
				delete(rt.jobs, id)
				rt.markDirty()
			})
		},
	)
	if err != nil {
		delete(rt.jobs, id)
		cancel()
		return "", err
	}
	job.cancel = func() {
		finish()
		cancel()
	}
	rt.dirty = true
	return id, nil
}

func (rt *Runtime) callJob(fn goja.Value, args ...any) {
	if fn == nil {
		return
	}
	callable, ok := goja.AssertFunction(fn)
	if !ok {
		return
	}
	values := make([]goja.Value, len(args))
	for i, a := range args {
		values[i] = rt.vm.ToValue(a)
	}
	if _, err := callable(goja.Undefined(), values...); err != nil {
		rt.report("callback", err)
	}
}

func (rt *Runtime) fsRead(p string) ([]byte, error) {
	if !rt.plugin.Manifest.HasPermission(PermissionWorkspaceRead) {
		return nil, fmt.Errorf("插件未声明 workspace.read 权限")
	}
	if err := rt.checkReadPath(p); err != nil {
		return nil, err
	}
	info, err := os.Stat(p)
	if err != nil {
		return nil, err
	}
	if info.Size() > maxFSFileSize {
		return nil, fmt.Errorf("文件超过 4 MB：%s", filepath.Base(p))
	}
	return os.ReadFile(p)
}

type fsEntry struct {
	Name string `json:"name"`
	Path string `json:"path"`
	Dir  bool   `json:"dir"`
	Size int64  `json:"size"`
}

func (rt *Runtime) fsList(p string) ([]fsEntry, error) {
	if !rt.plugin.Manifest.HasPermission(PermissionWorkspaceRead) {
		return nil, fmt.Errorf("插件未声明 workspace.read 权限")
	}
	if err := rt.checkReadPath(p); err != nil {
		return nil, err
	}
	entries, err := os.ReadDir(p)
	if err != nil {
		return nil, err
	}
	if len(entries) > maxFSEntries {
		return nil, fmt.Errorf("目录条目过多")
	}
	out := make([]fsEntry, 0, len(entries))
	for _, e := range entries {
		info, err := e.Info()
		if err != nil {
			continue
		}
		out = append(out, fsEntry{
			Name: e.Name(),
			Path: filepath.Join(p, e.Name()),
			Dir:  e.IsDir(),
			Size: info.Size(),
		})
	}
	return out, nil
}

func (rt *Runtime) fsExists(p string) (bool, error) {
	if !rt.plugin.Manifest.HasPermission(PermissionWorkspaceRead) {
		return false, fmt.Errorf("插件未声明 workspace.read 权限")
	}
	if err := rt.checkReadPath(p); err != nil {
		return false, err
	}
	_, err := os.Stat(p)
	if err == nil {
		return true, nil
	}
	if os.IsNotExist(err) {
		return false, nil
	}
	return false, err
}

func (rt *Runtime) fsResolve(base, rel string) (string, error) {
	if !rt.plugin.Manifest.HasPermission(PermissionWorkspaceRead) {
		return "", fmt.Errorf("插件未声明 workspace.read 权限")
	}
	if !rt.pathAllowed(base) {
		return "", fmt.Errorf("路径不在工作区内")
	}
	if filepath.IsAbs(rel) {
		return "", fmt.Errorf("相对路径不能是绝对路径")
	}
	p := filepath.Clean(filepath.Join(base, rel))
	if !rt.pathAllowed(p) {
		return "", fmt.Errorf("路径不在工作区内")
	}
	return p, nil
}

func (rt *Runtime) checkReadPath(p string) error {
	if !filepath.IsAbs(p) {
		return fmt.Errorf("需要绝对路径")
	}
	if !rt.pathAllowed(p) {
		return fmt.Errorf("路径不在工作区内")
	}
	return nil
}

func (rt *Runtime) pathAllowed(p string) bool {
	return rt.mgr.pathAllowed(p, rt.plugin.Dir)
}

// pathAllowed reports whether p is inside the workspace, one of its
// repositories or the plugin's own directory. Symlinks are resolved when
// they exist, so a workspace link cannot reach outside it.
func (m *Manager) pathAllowed(p, pluginDir string) bool {
	abs, err := filepath.Abs(p)
	if err != nil {
		return false
	}
	workspace := m.Workspace()
	roots := []string{workspace.Path, pluginDir}
	for _, repo := range workspace.Repos {
		roots = append(roots, repo.Path)
	}
	realAbs := abs
	if resolved, err := filepath.EvalSymlinks(abs); err == nil {
		realAbs = resolved
	}
	for _, root := range roots {
		if root == "" {
			continue
		}
		realRoot := root
		if resolved, err := filepath.EvalSymlinks(root); err == nil {
			realRoot = resolved
		}
		for _, candidate := range []string{abs, realAbs} {
			if within(realRoot, candidate) {
				return true
			}
		}
	}
	return false
}

func within(root, p string) bool {
	rel, err := filepath.Rel(root, p)
	if err != nil {
		return false
	}
	return rel == "." || (rel != ".." && !strings.HasPrefix(rel, ".."+string(filepath.Separator)) && !filepath.IsAbs(rel))
}
