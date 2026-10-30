package main

import (
	"context"
	"fmt"
	"os"
	"path/filepath"
	"strings"
	"sync"
	"time"

	"github.com/egoist/mygo"
	"github.com/egoist/mygo/ui"
	"github.com/egoist/mygo/ui/editor"
	"github.com/egoist/mygo/ui/markdown"
	"github.com/iwangjie/nemorous/internal/aimerge"
	"github.com/iwangjie/nemorous/internal/config"
	"github.com/iwangjie/nemorous/internal/gitops"
	"github.com/iwangjie/nemorous/internal/mani"
)

// NavTab identifies the primary workspace view.
type NavTab string

const (
	TabChanges  NavTab = "changes"  // 变更
	TabCode     NavTab = "code"     // 文件
	TabOverview NavTab = "overview" // 仓库
	TabMerge    NavTab = "merge"    // 合并
	TabMani     NavTab = "mani"     // 脚本
)

// PendingDiscard represents a file awaiting user confirmation before discard.
type PendingDiscard struct {
	RepoPath  string
	RepoName  string
	RelPath   string
	Untracked bool
}

// PendingUndo represents a repository whose last commit may be undone.
type PendingUndo struct {
	RepoPath string
	RepoName string
}

// AppNotification is the bottom-right feedback card: running command progress,
// a batch result, or a transient toast.
type AppNotification struct {
	Kind      string // "ok", "warn", "err", "info"
	Message   string
	At        time.Time
	Expires   time.Time
	Clickable bool // batch result: opens the operation details
}

// codeTab is one open file in the code viewer, with its own editor buffer so
// switching tabs keeps unsaved edits and scroll positions intact.
type codeTab struct {
	file  gitops.WorkspaceFile
	state *editor.State
}

func (t *codeTab) dirty() bool {
	return t != nil && t.state != nil && t.state.Buffer != nil && t.state.Buffer.Modified()
}

// ManualMerge holds the temporary worktree prepared for a hand-resolved merge.
type ManualMerge struct {
	RepoPath  string
	Path      string
	Branch    string
	Conflicts []string
}

// FileHistoryState backs the per-file history dialog.
type FileHistoryState struct {
	RepoPath string
	RepoName string
	RelPath  string
	Entries  []gitops.CommitInfo
	Loaded   bool
}

// App holds the reactive application state for Nemorous.
// Strictly follows MyGo lifetime rules: never stores ui.Element or *ui.Context.
type App struct {
	win *mygo.Window
	cfg config.Config

	// Navigation & Filter
	activeTab      NavTab
	overviewFilter string // "all", "dirty", "ahead", "behind"
	selectedRepoID string // selected repo for detail inspection

	// Collapsible UI States (Progressive Disclosure)
	showDiffOptions       bool
	showCodeOptions       bool
	showTopMoreMenu       bool
	showMergeAdvanced     bool
	showMergeCheckDetails bool
	showRepoDetailDrawer  bool
	expandedCodeRepos     map[string]bool // "repoID" -> expanded in file tree
	expandedCodeDirs      map[string]bool // "repoID::dirRelPath" -> expanded in file tree

	// Resizable Split Widths (in DIPs)
	changesSplit float32
	codeSplit    float32

	// Discovered Workspace Data
	repos                 []mani.DiscoveredRepo
	maniCfg               *mani.Config
	statuses              []gitops.RepoStatus
	statusByID            map[string]gitops.RepoStatus
	workspaceFiles        []gitops.WorkspaceFile
	lastRefresh           time.Time
	lastFullRefresh       time.Time
	isRefreshing          bool
	busyLabel             string
	watchPending          map[string]bool
	watchPendingFiles     bool
	watchPendingWorkspace bool

	// Derived caches, rebuilt on every snapshot so per-frame work stays small.
	workspaceBranches   []gitops.BranchInfo
	workspaceBranchTree *branchNode
	repoBranchTrees     map[string]*branchNode
	changeTreeCache     map[string]*changeTreeNode
	codeTreeCache       map[string]*codeTreeDir
	codeGroupsKey       string
	codeGroups          []codeRepoGroup

	// Changes & Diff View State
	selectedChangeKey    string
	selectedChangeFile   *gitops.ChangedFile
	checkedChanges       map[string]bool // ChangeFile.Key() -> selected for batch commit
	diffState            *editor.DiffState
	diffStagedOnly       bool
	diffEditable         bool
	diffModified         bool
	commitMessage        string
	commitAmend          bool
	lastBatchResults     []gitops.OpResult
	pendingClipboard     string
	changesFilterQuery   string
	changesFilterHandle  ui.Handle
	lastCheckedKey       string
	collapsedChangeRepos map[string]bool
	collapsedChangeDirs  map[string]bool // "repoID::dirRelPath" -> collapsed

	// Multi-Project Branch Switch & Presets State
	branchInput         string
	branchCheckedRepos  map[string]bool    // repoID -> checked for multi-project branch switch
	branchPerRepoTarget map[string]*string // repoID -> target branch string pointer for per-repo input
	branchSwitchResults []gitops.OpResult
	presetNameInput     string // e.g. "AAA 需求"

	// Branches & AI Merge State
	mergeTargetBranch   string
	mergeTargets        []aimerge.MergeTarget
	mergeCheckedRepos   map[string]bool
	mergePresetInput    string
	mergeRunning        bool
	mergeScanLoading    bool
	mergeScanAt         time.Time
	manualMerges        map[string]ManualMerge
	manualMergeRunning  bool
	selectedMergeRepoID string
	mdOpts              markdown.Options
	aiStreamRaw         strings.Builder
	aiVerifyReport      *aimerge.VerificationReport
	aiVerifyRepo        *mani.DiscoveredRepo
	aiAgentName         string
	aiAgentBin          string

	// Code Explorer State
	codeFilterQuery string
	codeTabs        []*codeTab
	codeTabIdx      int

	// Mani Tasks State
	selectedManiTask string
	maniParamBranch  string
	maniParamTarget  string
	maniTaskRunning  bool
	maniResults      []mani.TaskResult

	// Modals & Overlays
	showCmdPalette      bool
	cmdPaletteQuery     string
	showSettings        bool
	showBranchModal     bool
	showBranchPicker    bool
	branchPickerQuery   string
	branchPickerRepoID  string
	branchPickerHandle  ui.Handle
	showOpLog           bool
	pendingDiscard      *PendingDiscard
	pendingCodeTabClose *codeTab
	pendingUndo         *PendingUndo
	fileHistory         *FileHistoryState
	notify              *AppNotification

	// Handles for focus & shortcuts
	rootHandle        ui.Handle
	cmdInputHandle    ui.Handle
	commitInputHandle ui.Handle
	codeFilterHandle  ui.Handle

	watcherCancel context.CancelFunc
	mu            sync.Mutex
}

func NewApp(cfg config.Config) *App {
	agentName, agentBin := cfg.ResolvedAIAgent()
	a := &App{
		cfg:                  cfg,
		activeTab:            TabChanges,
		overviewFilter:       "all",
		statusByID:           make(map[string]gitops.RepoStatus),
		watchPending:         make(map[string]bool),
		checkedChanges:       make(map[string]bool),
		expandedCodeRepos:    make(map[string]bool),
		expandedCodeDirs:     make(map[string]bool),
		branchCheckedRepos:   make(map[string]bool),
		branchPerRepoTarget:  make(map[string]*string),
		mergeCheckedRepos:    make(map[string]bool),
		manualMerges:         make(map[string]ManualMerge),
		collapsedChangeRepos: make(map[string]bool),
		collapsedChangeDirs:  make(map[string]bool),
		repoBranchTrees:      make(map[string]*branchNode),
		changeTreeCache:      make(map[string]*changeTreeNode),
		codeTreeCache:        make(map[string]*codeTreeDir),
		changesSplit:         320,
		codeSplit:            290,
		codeTabIdx:           -1,
		mergeTargetBranch:    cfg.DefaultTargetBranch,
		aiAgentName:          agentName,
		aiAgentBin:           agentBin,
		maniParamTarget:      cfg.DefaultTargetBranch,
	}
	a.diffState = &editor.DiffState{
		Old:           editor.NewBuffer(""),
		New:           editor.NewBuffer(""),
		Layout:        diffLayout(cfg.DiffSideBySide),
		Inline:        parseInlineMode(cfg.DiffInlineMode),
		Wrap:          cfg.EditorSoftWrap,
		Editable:      false,
		HunkActions:   true,
		LineSelection: true,
		Font:          ui.Font{Size: cfg.EditorFontSize},
	}
	return a
}

func (a *App) palette() Palette {
	if a.cfg.Theme == "dark" {
		return darkPalette()
	}
	return lightPalette()
}

func diffLayout(sideBySide bool) editor.DiffLayout {
	if sideBySide {
		return editor.DiffSplit
	}
	return editor.DiffUnified
}

func parseInlineMode(m string) editor.DiffInline {
	switch m {
	case "char":
		return editor.DiffInlineChar
	case "none":
		return editor.DiffInlineNone
	default:
		return editor.DiffInlineWordAlt
	}
}

func (a *App) runUI(fn func()) {
	if a.win != nil {
		a.win.Update(fn)
	} else {
		fn()
	}
}

func (a *App) showToast(kind, msg string) {
	now := time.Now()
	// A batch result is announced in the same UI update as its toast: fold the
	// toast into that card instead of replacing it with a plain notification.
	if a.notify != nil && a.notify.Clickable && now.Sub(a.notify.At) < 1500*time.Millisecond {
		a.notify.Kind = kind
		a.notify.Message = msg
		a.notify.Expires = now.Add(8 * time.Second)
		return
	}
	a.notify = &AppNotification{Kind: kind, Message: msg, At: now, Expires: now.Add(4 * time.Second)}
}

// setBatchResults publishes a finished batch to the notification card and keeps
// the full rows for the operation log.
func (a *App) setBatchResults(res []gitops.OpResult) {
	a.lastBatchResults = res
	now := time.Now()
	if len(res) == 0 {
		if a.notify != nil && a.notify.Clickable {
			a.notify = nil
		}
		return
	}
	okCnt, skipCnt, failCnt := countOpResults(res)
	kind := "ok"
	if failCnt > 0 {
		kind = "warn"
	}
	a.notify = &AppNotification{
		Kind:      kind,
		Message:   fmt.Sprintf("操作完成：%d 成功，%d 跳过，%d 失败", okCnt, skipCnt, failCnt),
		At:        now,
		Expires:   now.Add(8 * time.Second),
		Clickable: true,
	}
}

// overlayOpen reports whether a dialog owns the keyboard.
func (a *App) overlayOpen() bool {
	return a.showCmdPalette || a.showSettings || a.showBranchModal || a.showBranchPicker || a.showOpLog ||
		a.pendingDiscard != nil || a.pendingUndo != nil || a.fileHistory != nil || a.pendingCodeTabClose != nil
}

// commitAvailability explains why the commit buttons can or cannot run.
func (a *App) commitAvailability(checkedFiles int) (bool, string) {
	if checkedFiles == 0 {
		return false, "勾选文件后可提交"
	}
	return true, ""
}

// branchCoverage reports how many repositories contain a local branch.
func (a *App) branchCoverage(name string) string {
	if len(a.statuses) == 0 || name == "" {
		return ""
	}
	found := 0
	for _, st := range a.statuses {
		for _, bi := range st.BranchInfos {
			if !bi.IsRemote && bi.Name == name {
				found++
				break
			}
		}
	}
	return fmt.Sprintf("%d/%d", found, len(a.statuses))
}

// cleanupManualMerge removes a temporary manual merge worktree.
func (a *App) cleanupManualMerge(repoID string) {
	mm, ok := a.manualMerges[repoID]
	if !ok {
		return
	}
	delete(a.manualMerges, repoID)
	go func() {
		_ = aimerge.CleanupManualMerge(context.Background(), mm.RepoPath, mm.Path, mm.Branch)
	}()
	a.showToast("ok", "已清理临时合并工作区")
}

// openFileHistory loads and shows recent commits for one file.
func (a *App) openFileHistory(cf gitops.ChangedFile) {
	state := &FileHistoryState{
		RepoPath: cf.RepoPath,
		RepoName: cf.RepoName,
		RelPath:  cf.Path,
	}
	a.fileHistory = state
	go func() {
		entries := gitops.FileHistory(context.Background(), cf.RepoPath, cf.Path, 30)
		a.runUI(func() {
			if a.fileHistory == state {
				a.fileHistory.Entries = entries
				a.fileHistory.Loaded = true
			}
		})
	}()
}

// confirmUndoLastCommit soft-resets the pending repository one commit back.
func (a *App) confirmUndoLastCommit() {
	if a.pendingUndo == nil {
		return
	}
	pending := *a.pendingUndo
	a.pendingUndo = nil
	go func() {
		msg, err := gitops.UndoLastCommit(context.Background(), pending.RepoPath)
		a.runUI(func() {
			if err != nil {
				a.showToast("err", pending.RepoName+": "+err.Error())
				return
			}
			a.showToast("ok", pending.RepoName+": "+msg)
			a.RefreshAsync()
		})
	}()
}

// LoadWorkspaceSync synchronously discovers repos and inspects status.
func (a *App) LoadWorkspaceSync(wsPath string) {
	wsPath = filepath.Clean(wsPath)
	a.cfg.AddRecentWorkspace(wsPath)
	repos, maniCfg := mani.DiscoverRepos(wsPath, a.cfg.IncludeRootRepo)
	ctx := context.Background()
	statuses := gitops.InspectRepos(ctx, repos, a.cfg.Concurrency)
	wsFiles := gitops.ListWorkspaceFiles(ctx, repos, 2500)

	a.applySnapshot(repos, maniCfg, statuses, wsFiles)
	a.lastFullRefresh = time.Now()
	a.mergeTargets = nil
	a.mergeScanAt = time.Time{}
}

// reposSnapshot returns a copy of the discovered repositories for background work.
func (a *App) reposSnapshot() []mani.DiscoveredRepo {
	a.mu.Lock()
	defer a.mu.Unlock()
	return append([]mani.DiscoveredRepo(nil), a.repos...)
}

// RefreshWorkspaceAsync rediscovers repositories and reloads everything.
func (a *App) RefreshWorkspaceAsync() {
	if a.cfg.WorkspacePath == "" {
		return
	}
	if a.isRefreshing {
		a.watchPendingWorkspace = true
		return
	}
	a.isRefreshing = true
	wsPath := a.cfg.WorkspacePath
	incRoot := a.cfg.IncludeRootRepo
	conc := a.cfg.Concurrency
	go func() {
		ctx := context.Background()
		repos, maniCfg := mani.DiscoverRepos(wsPath, incRoot)
		statuses := gitops.InspectRepos(ctx, repos, conc)
		wsFiles := gitops.ListWorkspaceFiles(ctx, repos, 2500)
		a.runUI(func() {
			a.isRefreshing = false
			a.applySnapshot(repos, maniCfg, statuses, wsFiles)
			a.lastFullRefresh = time.Now()
		})
	}()
}

// RefreshReposAsync re-inspects only the repositories touched by a file event.
func (a *App) RefreshReposAsync(ids map[string]bool, listFiles bool) {
	if len(ids) == 0 {
		return
	}
	if a.isRefreshing {
		for id := range ids {
			a.watchPending[id] = true
		}
		a.watchPendingFiles = a.watchPendingFiles || listFiles
		return
	}
	repos := a.reposSnapshot()
	targets := make([]mani.DiscoveredRepo, 0, len(ids))
	for _, repo := range repos {
		if ids[repo.ID] {
			targets = append(targets, repo)
		}
	}
	if len(targets) == 0 {
		a.RefreshWorkspaceAsync()
		return
	}
	a.isRefreshing = true
	conc := a.cfg.Concurrency
	go func() {
		ctx := context.Background()
		statuses := gitops.InspectRepos(ctx, targets, conc)
		var files map[string][]gitops.WorkspaceFile
		if listFiles {
			files = make(map[string][]gitops.WorkspaceFile, len(targets))
			for _, repo := range targets {
				files[repo.ID] = gitops.ListWorkspaceFiles(ctx, []mani.DiscoveredRepo{repo}, 2500)
			}
		}
		a.runUI(func() {
			a.isRefreshing = false
			a.applyRepoUpdates(statuses, files)
			a.flushWatchQueue()
		})
	}()
}

// flushWatchQueue re-runs the refresh that arrived while one was in flight.
func (a *App) flushWatchQueue() {
	if a.watchPendingWorkspace {
		a.watchPendingWorkspace = false
		a.watchPending = make(map[string]bool)
		a.watchPendingFiles = false
		a.RefreshWorkspaceAsync()
		return
	}
	if len(a.watchPending) == 0 {
		return
	}
	ids := a.watchPending
	files := a.watchPendingFiles
	a.watchPending = make(map[string]bool)
	a.watchPendingFiles = false
	a.RefreshReposAsync(ids, files)
}

// applyRepoUpdates merges freshly inspected repositories into the current snapshot.
func (a *App) applyRepoUpdates(updates []gitops.RepoStatus, files map[string][]gitops.WorkspaceFile) {
	valid := make(map[string]gitops.ChangedFile)
	for _, st := range a.statuses {
		for _, f := range st.Files {
			valid[f.Key()] = f
		}
	}
	index := make(map[string]int, len(a.statuses))
	for i, st := range a.statuses {
		index[st.Repo.ID] = i
	}
	for _, st := range updates {
		if i, ok := index[st.Repo.ID]; ok {
			a.statuses[i] = st
		} else {
			index[st.Repo.ID] = len(a.statuses)
			a.statuses = append(a.statuses, st)
		}
		a.statusByID[st.Repo.ID] = st
		prefix := st.Repo.ID + "::"
		for key := range valid {
			if strings.HasPrefix(key, prefix) {
				delete(valid, key)
			}
		}
		for _, f := range st.Files {
			valid[f.Key()] = f
			if _, ok := a.checkedChanges[f.Key()]; !ok {
				a.checkedChanges[f.Key()] = true
			}
		}
		for key := range a.checkedChanges {
			if strings.HasPrefix(key, prefix) {
				if _, ok := valid[key]; !ok {
					delete(a.checkedChanges, key)
				}
			}
		}
	}
	if len(files) > 0 {
		next := make([]gitops.WorkspaceFile, 0, len(a.workspaceFiles))
		for _, wf := range a.workspaceFiles {
			if _, replaced := files[wf.RepoID]; !replaced {
				next = append(next, wf)
			}
		}
		for _, repoFiles := range files {
			next = append(next, repoFiles...)
		}
		a.workspaceFiles = next
	}
	if a.selectedChangeKey != "" {
		if cf, ok := valid[a.selectedChangeKey]; ok {
			cfCopy := cf
			a.selectedChangeFile = &cfCopy
			isEditing := a.diffEditable || (a.diffState != nil && a.diffState.New != nil && a.diffState.New.Modified())
			if !isEditing && a.diffState != nil {
				oldTxt, newTxt, _, err := gitops.GetFileDiff(context.Background(), cfCopy.RepoPath, cfCopy.Path, a.diffStagedOnly)
				if err == nil {
					curOld, curNew := "", ""
					if a.diffState.Old != nil {
						curOld = a.diffState.Old.String()
					}
					if a.diffState.New != nil {
						curNew = a.diffState.New.String()
					}
					if oldTxt != curOld || newTxt != curNew {
						a.selectChangedFile(cfCopy)
					}
				}
			}
		} else {
			a.selectedChangeKey = ""
			a.selectedChangeFile = nil
		}
	}
	if a.selectedChangeFile == nil {
		for _, st := range updates {
			if len(st.Files) > 0 {
				a.selectChangedFile(st.Files[0])
				break
			}
		}
	}
	a.lastRefresh = time.Now()
	a.rebuildBranchCaches()
}

// LoadWorkspaceAsync discovers and inspects a workspace off the UI thread so the
// window paints immediately.
func (a *App) LoadWorkspaceAsync(wsPath string) {
	wsPath = filepath.Clean(wsPath)
	if wsPath == "" {
		return
	}
	a.cfg.AddRecentWorkspace(wsPath)
	a.busyLabel = "正在载入工作区..."
	a.isRefreshing = true
	a.mergeTargets = nil
	a.mergeScanAt = time.Time{}
	incRoot := a.cfg.IncludeRootRepo
	conc := a.cfg.Concurrency
	go func() {
		ctx := context.Background()
		repos, maniCfg := mani.DiscoverRepos(wsPath, incRoot)
		statuses := gitops.InspectRepos(ctx, repos, conc)
		wsFiles := gitops.ListWorkspaceFiles(ctx, repos, 2500)
		a.runUI(func() {
			a.isRefreshing = false
			a.busyLabel = ""
			a.applySnapshot(repos, maniCfg, statuses, wsFiles)
			a.lastFullRefresh = time.Now()
			a.flushWatchQueue()
		})
	}()
}

// RefreshAsync refreshes all repository statuses with a visible status label.
func (a *App) RefreshAsync() {
	a.refreshInternal(true)
}

// RefreshSilentAsync refreshes all repository statuses silently in real time without status-bar flicker.
func (a *App) RefreshSilentAsync() {
	a.refreshInternal(false)
}

func (a *App) refreshInternal(showBusy bool) {
	if a.isRefreshing || a.cfg.WorkspacePath == "" {
		return
	}
	// Silent refreshes only re-inspect repository status; file listings refresh
	// at most every 30 seconds.
	full := showBusy || a.lastFullRefresh.IsZero() || time.Since(a.lastFullRefresh) > 30*time.Second

	a.isRefreshing = true
	if showBusy {
		a.busyLabel = "正在刷新仓库状态..."
	}
	wsPath := a.cfg.WorkspacePath
	incRoot := a.cfg.IncludeRootRepo
	conc := a.cfg.Concurrency
	cachedFiles := a.workspaceFiles

	go func() {
		ctx := context.Background()
		repos, maniCfg := mani.DiscoverRepos(wsPath, incRoot)
		statuses := gitops.InspectRepos(ctx, repos, conc)
		wsFiles := cachedFiles
		if full {
			wsFiles = gitops.ListWorkspaceFiles(ctx, repos, 2500)
		}
		a.runUI(func() {
			a.isRefreshing = false
			if showBusy {
				a.busyLabel = ""
			}
			if full {
				a.lastFullRefresh = time.Now()
			}
			a.applySnapshot(repos, maniCfg, statuses, wsFiles)
			a.flushWatchQueue()
		})
	}()
}

func (a *App) applySnapshot(repos []mani.DiscoveredRepo, maniCfg *mani.Config, statuses []gitops.RepoStatus, wsFiles []gitops.WorkspaceFile) {
	a.mu.Lock()
	a.repos = repos
	a.mu.Unlock()
	a.maniCfg = maniCfg
	a.statuses = statuses
	a.workspaceFiles = wsFiles
	a.lastRefresh = time.Now()

	byID := make(map[string]gitops.RepoStatus, len(statuses))
	allValidKeys := make(map[string]gitops.ChangedFile)
	var firstChanged *gitops.ChangedFile

	for _, st := range statuses {
		byID[st.Repo.ID] = st
		if _, ok := a.branchCheckedRepos[st.Repo.ID]; !ok {
			a.branchCheckedRepos[st.Repo.ID] = true
		}
		if _, ok := a.branchPerRepoTarget[st.Repo.ID]; !ok {
			b := st.Branch
			a.branchPerRepoTarget[st.Repo.ID] = &b
		}
		for i := range st.Files {
			cf := st.Files[i]
			k := cf.Key()
			allValidKeys[k] = cf
			if firstChanged == nil {
				cfCopy := cf
				firstChanged = &cfCopy
			}
			if _, exists := a.checkedChanges[k]; !exists {
				a.checkedChanges[k] = true
			}
		}
	}
	for k := range a.checkedChanges {
		if _, ok := allValidKeys[k]; !ok {
			delete(a.checkedChanges, k)
		}
	}
	a.statusByID = byID

	if a.selectedRepoID == "" && len(repos) > 0 {
		a.selectedRepoID = repos[0].ID
	}

	// Keep selected changed file and its DiffView buffer live in real time!
	if a.selectedChangeKey != "" {
		if cf, ok := allValidKeys[a.selectedChangeKey]; ok {
			cfCopy := cf
			a.selectedChangeFile = &cfCopy
			// Auto-refresh DiffView if user is not currently editing unsaved changes
			isEditingUnsaved := a.diffEditable || (a.diffState != nil && a.diffState.New != nil && a.diffState.New.Modified())
			if !isEditingUnsaved && a.diffState != nil {
				oldTxt, newTxt, _, err := gitops.GetFileDiff(context.Background(), cfCopy.RepoPath, cfCopy.Path, a.diffStagedOnly)
				if err == nil {
					curOld := ""
					curNew := ""
					if a.diffState.Old != nil {
						curOld = a.diffState.Old.String()
					}
					if a.diffState.New != nil {
						curNew = a.diffState.New.String()
					}
					if oldTxt != curOld || newTxt != curNew {
						a.selectChangedFile(cfCopy)
					}
				}
			}
		} else {
			a.selectedChangeKey = ""
			a.selectedChangeFile = nil
		}
	}
	if a.selectedChangeFile == nil && firstChanged != nil {
		a.selectChangedFile(*firstChanged)
	}

	if a.selectedManiTask == "" && maniCfg != nil && len(maniCfg.Tasks) > 0 {
		if _, ok := maniCfg.Tasks["status"]; ok {
			a.selectedManiTask = "status"
		} else {
			for k := range maniCfg.Tasks {
				a.selectedManiTask = k
				break
			}
		}
	}

	a.rebuildBranchCaches()
}

func (a *App) selectChangedFile(cf gitops.ChangedFile) {
	a.selectedChangeKey = cf.Key()
	cfCopy := cf
	a.selectedChangeFile = &cfCopy
	a.diffModified = false
	if a.diffStagedOnly && !cf.Staged {
		a.diffStagedOnly = false
	}

	ctx := context.Background()
	oldTxt, newTxt, _, err := gitops.GetFileDiff(ctx, cf.RepoPath, cf.Path, a.diffStagedOnly)
	if err != nil {
		oldTxt = ""
		newTxt = "// 无法读取文件差异: " + err.Error()
	}
	lang := editor.LanguageFor(cf.Path)
	oldBuf := editor.NewBuffer(oldTxt)
	newBuf := editor.NewBuffer(newTxt)
	oldBuf.SetLanguage(lang)
	newBuf.SetLanguage(lang)
	a.diffState = &editor.DiffState{
		Old:           oldBuf,
		New:           newBuf,
		Layout:        diffLayout(a.cfg.DiffSideBySide),
		Inline:        parseInlineMode(a.cfg.DiffInlineMode),
		Wrap:          a.cfg.EditorSoftWrap,
		Editable:      a.diffEditable,
		HunkActions:   true,
		LineSelection: true,
		Font:          ui.Font{Size: a.cfg.EditorFontSize},
	}
}

func (a *App) saveDiffMicroEdit() {
	if a.selectedChangeFile == nil || a.diffState == nil || a.diffState.New == nil {
		return
	}
	newContent := a.diffState.New.String()
	err := gitops.SaveMicroEdit(a.selectedChangeFile.RepoPath, a.selectedChangeFile.Path, newContent)
	if err != nil {
		a.showToast("err", "保存失败: "+err.Error())
		return
	}
	a.diffState.New.MarkSaved()
	a.diffModified = false
	a.showToast("ok", fmt.Sprintf("已保存 %s/%s", a.selectedChangeFile.RepoName, a.selectedChangeFile.Path))
	a.RefreshAsync()
}

// activeCodeTab returns the open tab shown in the editor, nil when none is open.
func (a *App) activeCodeTab() *codeTab {
	if a.codeTabIdx < 0 || a.codeTabIdx >= len(a.codeTabs) {
		return nil
	}
	return a.codeTabs[a.codeTabIdx]
}

// activeCodeFile returns the file of the active tab.
func (a *App) activeCodeFile() *gitops.WorkspaceFile {
	if tab := a.activeCodeTab(); tab != nil {
		return &tab.file
	}
	return nil
}

// activeCodeEditor returns the editor state of the active tab.
func (a *App) activeCodeEditor() *editor.State {
	if tab := a.activeCodeTab(); tab != nil {
		return tab.state
	}
	return nil
}

// normalizeWorkspaceFile fills the repository metadata that callers coming from
// the changes view do not carry.
func (a *App) normalizeWorkspaceFile(wf gitops.WorkspaceFile) gitops.WorkspaceFile {
	if st, ok := a.statusByID[wf.RepoID]; ok {
		if wf.RepoPath == "" {
			wf.RepoPath = st.Repo.AbsPath
		}
		if wf.Monogram == "" {
			wf.Monogram = st.Repo.Monogram
			wf.ColorIdx = st.Repo.ColorIndex
		}
	}
	if wf.AbsPath == "" && wf.RepoPath != "" {
		wf.AbsPath = filepath.Join(wf.RepoPath, wf.RelPath)
	}
	return wf
}

// revealCodeFile expands the repository and every ancestor directory of wf.
func (a *App) revealCodeFile(wf gitops.WorkspaceFile) {
	a.expandedCodeRepos[wf.RepoID] = true
	dir := filepath.ToSlash(filepath.Dir(wf.RelPath))
	if dir == "." || dir == "" {
		return
	}
	cur := ""
	for _, seg := range strings.Split(dir, "/") {
		if cur == "" {
			cur = seg
		} else {
			cur += "/" + seg
		}
		a.expandedCodeDirs[wf.RepoID+"::"+cur] = true
	}
}

// selectCodeFile opens wf in a tab, or activates its existing tab without
// reloading, so unsaved edits survive switching.
func (a *App) selectCodeFile(wf gitops.WorkspaceFile) {
	wf = a.normalizeWorkspaceFile(wf)
	for i, tab := range a.codeTabs {
		if tab.file.RepoID == wf.RepoID && tab.file.RelPath == wf.RelPath {
			a.codeTabIdx = i
			a.revealCodeFile(wf)
			return
		}
	}

	ctx := context.Background()
	curTxt, headTxt, err := gitops.ReadRepoFile(ctx, wf.RepoPath, wf.RelPath)
	if err != nil {
		curTxt = "// 无法读取文件: " + err.Error()
		headTxt = ""
	}
	if len(curTxt) > 512*1024 {
		curTxt = curTxt[:512*1024] + "\n\n// [大文件已截断至前 512KB]"
	}

	lang := editor.LanguageFor(wf.RelPath)
	buf := editor.NewBuffer(curTxt)
	buf.SetLanguage(lang)
	var baseBuf *editor.Buffer
	if headTxt != "" {
		baseBuf = editor.NewBuffer(headTxt)
		baseBuf.SetLanguage(lang)
	}
	a.codeTabs = append(a.codeTabs, &codeTab{
		file: wf,
		state: &editor.State{
			Buffer:   buf,
			DiffBase: baseBuf,
			SoftWrap: a.cfg.EditorSoftWrap,
			Font:     ui.Font{Size: a.cfg.EditorFontSize},
			Bindings: []editor.Binding{{Mods: ui.Cmd, Key: ui.KeyG, Command: editor.GoToLine}},
		},
	})
	a.codeTabIdx = len(a.codeTabs) - 1
	a.revealCodeFile(wf)
}

// activateCodeTab switches the editor to tab i.
func (a *App) activateCodeTab(i int) {
	if i < 0 || i >= len(a.codeTabs) {
		return
	}
	a.codeTabIdx = i
}

// requestCloseCodeTab closes a clean tab at once and asks before discarding
// unsaved edits.
func (a *App) requestCloseCodeTab(tab *codeTab) {
	if tab == nil {
		return
	}
	if tab.dirty() {
		a.pendingCodeTabClose = tab
		return
	}
	a.closeCodeTab(tab)
}

// closeCodeTab removes a tab and keeps the neighbour active.
func (a *App) closeCodeTab(tab *codeTab) {
	for i, t := range a.codeTabs {
		if t != tab {
			continue
		}
		a.codeTabs = append(a.codeTabs[:i], a.codeTabs[i+1:]...)
		switch {
		case len(a.codeTabs) == 0:
			a.codeTabIdx = -1
		case a.codeTabIdx > i:
			a.codeTabIdx--
		case a.codeTabIdx == i:
			a.codeTabIdx = min(i, len(a.codeTabs)-1)
		}
		return
	}
}

func (a *App) saveCodeMicroEdit() {
	a.saveCodeTab(a.activeCodeTab())
}

// saveCodeTab writes one tab's buffer back to disk.
func (a *App) saveCodeTab(tab *codeTab) bool {
	if tab == nil || tab.state == nil || tab.state.Buffer == nil {
		return false
	}
	if err := gitops.SaveMicroEdit(tab.file.RepoPath, tab.file.RelPath, tab.state.Buffer.String()); err != nil {
		a.showToast("err", "保存失败: "+err.Error())
		return false
	}
	tab.state.Buffer.MarkSaved()
	a.showToast("ok", fmt.Sprintf("已保存 %s/%s", tab.file.RepoName, tab.file.RelPath))
	a.RefreshAsync()
	return true
}

// DominantBranch computes the most common active branch across child repositories.
func (a *App) DominantBranch() (branch string, alignedCount int, totalCount int) {
	counts := make(map[string]int)
	for _, st := range a.statuses {
		if st.Repo.IsRoot && len(a.statuses) > 1 {
			continue
		}
		totalCount++
		if st.Branch != "" {
			counts[st.Branch]++
		}
	}
	bestBranch := "main"
	bestCnt := 0
	for b, c := range counts {
		if c > bestCnt || (c == bestCnt && b < bestBranch) {
			bestBranch = b
			bestCnt = c
		}
	}
	return bestBranch, bestCnt, totalCount
}

// SummaryCounts returns aggregate workspace metrics.
func (a *App) SummaryCounts() (dirtyRepos, totalChangedFiles, aheadRepos, behindRepos, conflictRepos int) {
	for _, st := range a.statuses {
		if len(st.Files) > 0 {
			dirtyRepos++
			totalChangedFiles += len(st.Files)
		}
		if st.Ahead > 0 {
			aheadRepos++
		}
		if st.Behind > 0 {
			behindRepos++
		}
		if st.ConflictCount > 0 {
			conflictRepos++
		}
	}
	return
}

// WorkspaceBranches returns every local branch aggregated across repositories,
// ordered the way every picker shows them: my recent commits, recent commits by
// others, then the rest. The list is cached once per snapshot.
func (a *App) WorkspaceBranches() []gitops.BranchInfo {
	if a.workspaceBranches == nil {
		return a.computeWorkspaceBranches()
	}
	return a.workspaceBranches
}

// computeWorkspaceBranches deduplicates branch names across repositories and
// keeps the newest tip for each name, carrying that tip's authorship.
func (a *App) computeWorkspaceBranches() []gitops.BranchInfo {
	byName := make(map[string]gitops.BranchInfo)
	for _, st := range a.statuses {
		for _, bi := range st.BranchInfos {
			if bi.IsRemote {
				continue
			}
			existing, ok := byName[bi.Name]
			if !ok || bi.CommitUnix > existing.CommitUnix {
				byName[bi.Name] = bi
			}
		}
	}
	all := make([]gitops.BranchInfo, 0, len(byName))
	for _, bi := range byName {
		all = append(all, bi)
	}
	sortBranchesForPicker(all)
	return all
}

// rebuildBranchCaches refreshes every derived tree once per snapshot instead of
// rebuilding them during each frame.
func (a *App) rebuildBranchCaches() {
	a.workspaceBranches = a.computeWorkspaceBranches()
	a.workspaceBranchTree = buildBranchTree(a.workspaceBranches)
	a.repoBranchTrees = make(map[string]*branchNode, len(a.statuses))
	for _, st := range a.statuses {
		a.repoBranchTrees[st.Repo.ID] = buildBranchTree(st.BranchInfos)
	}
	a.changeTreeCache = make(map[string]*changeTreeNode)
	a.codeTreeCache = make(map[string]*codeTreeDir)
	a.codeGroupsKey = "\x00"
	a.codeGroups = nil
}

// branchTree returns the cached workspace branch hierarchy, falling back to a
// direct build before the first snapshot.
func (a *App) branchTree() *branchNode {
	if a.workspaceBranchTree == nil {
		return buildBranchTree(a.WorkspaceBranches())
	}
	return a.workspaceBranchTree
}

// TriggerStashAll stashes uncommitted changes (untracked included) in every repository.
func (a *App) TriggerStashAll() {
	repos := a.reposSnapshot()
	if len(repos) == 0 {
		a.showToast("warn", "没有可贮藏的仓库")
		return
	}
	a.busyLabel = fmt.Sprintf("正在贮藏 %d 个仓库的改动...", len(repos))
	message := "nemorous: 贮藏于 " + time.Now().Format("01-02 15:04")
	conc := a.cfg.Concurrency
	go func() {
		res := gitops.BatchStash(context.Background(), repos, message, conc)
		okCnt, skipCnt, failCnt := countOpResults(res)
		a.runUI(func() {
			a.busyLabel = ""
			a.setBatchResults(res)
			a.showToast("ok", fmt.Sprintf("贮藏完成：%d 成功，%d 跳过，%d 失败", okCnt, skipCnt, failCnt))
			a.RefreshAsync()
		})
	}()
}

// TriggerStashPopAll restores the most recent stash in every repository.
func (a *App) TriggerStashPopAll() {
	repos := a.reposSnapshot()
	if len(repos) == 0 {
		a.showToast("warn", "没有可恢复贮藏的仓库")
		return
	}
	a.busyLabel = fmt.Sprintf("正在恢复 %d 个仓库的贮藏...", len(repos))
	conc := a.cfg.Concurrency
	go func() {
		res := gitops.BatchStashPop(context.Background(), repos, conc)
		okCnt, skipCnt, failCnt := countOpResults(res)
		a.runUI(func() {
			a.busyLabel = ""
			a.setBatchResults(res)
			a.showToast("ok", fmt.Sprintf("恢复贮藏完成：%d 成功，%d 跳过，%d 失败", okCnt, skipCnt, failCnt))
			a.RefreshAsync()
		})
	}()
}

// stashRepo stashes or restores one repository's changes.
func (a *App) stashRepo(repo mani.DiscoveredRepo, pop bool) {
	go func() {
		ctx := context.Background()
		var res []gitops.OpResult
		if pop {
			res = gitops.BatchStashPop(ctx, []mani.DiscoveredRepo{repo}, 1)
		} else {
			res = gitops.BatchStash(ctx, []mani.DiscoveredRepo{repo}, "nemorous: 贮藏于 "+time.Now().Format("01-02 15:04"), 1)
		}
		a.runUI(func() {
			a.setBatchResults(res)
			a.RefreshAsync()
		})
	}()
}

// BranchCandidatesForRepo returns deduplicated BranchInfo candidates for fuzzy autocomplete.
// When repoID != "", that repository's own branches come first (sorted by recent commit activity),
// followed by branches from other repositories or saved presets.
func (a *App) BranchCandidatesForRepo(repoID string) []gitops.BranchInfo {
	seen := make(map[string]bool)
	var out []gitops.BranchInfo

	if repoID != "" {
		if st, ok := a.statusByID[repoID]; ok {
			for _, bi := range st.BranchInfos {
				if bi.IsRemote || seen[bi.Name] {
					continue
				}
				seen[bi.Name] = true
				out = append(out, bi)
			}
		}
		sortBranchesForPicker(out)
	}

	for _, bi := range a.WorkspaceBranches() {
		if !seen[bi.Name] {
			seen[bi.Name] = true
			out = append(out, bi)
		}
	}
	for _, preset := range a.cfg.BranchPresets {
		for _, br := range preset.RepoBranches {
			br = strings.TrimSpace(br)
			if br != "" && !seen[br] {
				seen[br] = true
				out = append(out, gitops.BranchInfo{Name: br, RelTime: "分支组合"})
			}
		}
	}
	return out
}

func (a *App) openBranchSwitchModal() {
	a.branchInput = ""
	a.branchSwitchResults = nil
	for _, st := range a.statuses {
		if _, ok := a.branchCheckedRepos[st.Repo.ID]; !ok {
			a.branchCheckedRepos[st.Repo.ID] = true
		}
		if ptr, ok := a.branchPerRepoTarget[st.Repo.ID]; !ok || ptr == nil {
			b := st.Branch
			a.branchPerRepoTarget[st.Repo.ID] = &b
		} else if *ptr == "" {
			*ptr = st.Branch
		}
	}
	a.showBranchModal = true
}

// SetUnifiedBranchForChecked sets targetBranch on all currently checked repositories in the branch switch modal.
func (a *App) SetUnifiedBranchForChecked(targetBranch string) {
	a.branchInput = targetBranch
	for _, st := range a.statuses {
		if a.branchCheckedRepos[st.Repo.ID] {
			if ptr, ok := a.branchPerRepoTarget[st.Repo.ID]; ok && ptr != nil {
				*ptr = targetBranch
			} else {
				b := targetBranch
				a.branchPerRepoTarget[st.Repo.ID] = &b
			}
		}
	}
}

// SaveCurrentSelectionAsPreset saves the checked repositories and their target branches as a named preset.
func (a *App) SaveCurrentSelectionAsPreset() {
	name := strings.TrimSpace(a.presetNameInput)
	if name == "" {
		a.showToast("warn", "请输入分支组合名称（如：AAA 需求）")
		return
	}
	mapping := make(map[string]string)
	for _, st := range a.statuses {
		if !a.branchCheckedRepos[st.Repo.ID] {
			continue
		}
		target := strings.TrimSpace(a.branchInput)
		if ptr, ok := a.branchPerRepoTarget[st.Repo.ID]; ok && ptr != nil && strings.TrimSpace(*ptr) != "" {
			target = strings.TrimSpace(*ptr)
		}
		if target != "" {
			mapping[st.Repo.ID] = target
		}
	}
	if len(mapping) == 0 {
		a.showToast("warn", "请至少勾选一个仓库并填写目标分支")
		return
	}
	a.cfg.SaveBranchPreset(name, mapping)
	_ = a.cfg.Save()
	a.presetNameInput = ""
	a.showToast("ok", fmt.Sprintf("已保存分支组合「%s」（%d 个仓库）", name, len(mapping)))
}

// ApplyBranchPreset loads a saved branch preset and optionally executes the switch immediately.
func (a *App) ApplyBranchPreset(preset config.BranchPreset, executeNow bool) {
	for _, st := range a.statuses {
		if target, ok := preset.RepoBranches[st.Repo.ID]; ok && target != "" {
			a.branchCheckedRepos[st.Repo.ID] = true
			if ptr, exists := a.branchPerRepoTarget[st.Repo.ID]; exists && ptr != nil {
				*ptr = target
			} else {
				tCopy := target
				a.branchPerRepoTarget[st.Repo.ID] = &tCopy
			}
			a.branchInput = target
		} else {
			a.branchCheckedRepos[st.Repo.ID] = false
		}
	}
	if executeNow {
		a.TriggerCheckedBranchSwitch()
	}
}

// TriggerCheckedBranchSwitch switches all checked repositories in the modal to their respective target branches.
func (a *App) TriggerCheckedBranchSwitch() {
	a.triggerCheckedBranchSwitch(false)
}

// TriggerCheckedBranchCreate switches the checked repositories, creating the
// target branch from the current commit where it does not exist yet.
func (a *App) TriggerCheckedBranchCreate() {
	a.triggerCheckedBranchSwitch(true)
}

func (a *App) triggerCheckedBranchSwitch(createMissing bool) {
	var targets []gitops.RepoBranchTarget
	for _, st := range a.statuses {
		if !a.branchCheckedRepos[st.Repo.ID] {
			continue
		}
		target := strings.TrimSpace(a.branchInput)
		if ptr, ok := a.branchPerRepoTarget[st.Repo.ID]; ok && ptr != nil && strings.TrimSpace(*ptr) != "" {
			target = strings.TrimSpace(*ptr)
		}
		if target != "" {
			targets = append(targets, gitops.RepoBranchTarget{
				Repo:         st.Repo,
				TargetBranch: target,
			})
		}
	}
	if len(targets) == 0 {
		a.showToast("warn", "请先勾选要切换分支的仓库并输入分支名")
		return
	}

	a.busyLabel = fmt.Sprintf("正在切换 %d 个仓库的分支...", len(targets))
	a.branchSwitchResults = nil
	conc := a.cfg.Concurrency
	go func() {
		ctx := context.Background()
		res := gitops.SwitchBranchPlan(ctx, targets, createMissing, conc)
		okCnt, skipCnt, failCnt := countOpResults(res)
		a.runUI(func() {
			a.setBatchResults(res)
			a.branchSwitchResults = res
			a.busyLabel = ""
			a.showToast("ok", fmt.Sprintf("分支切换完成：%d 成功，%d 跳过，%d 失败", okCnt, skipCnt, failCnt))
			a.RefreshAsync()
		})
	}()
}

// TriggerBatchFetch runs git fetch across all repos.
func (a *App) TriggerBatchFetch() {
	a.busyLabel = "正在获取远端更新 (Fetch)..."
	repos := append([]mani.DiscoveredRepo(nil), a.repos...)
	conc := a.cfg.Concurrency
	go func() {
		ctx := context.Background()
		res := gitops.FetchAll(ctx, repos, conc)
		a.runUI(func() {
			a.setBatchResults(res)
			a.showToast("ok", fmt.Sprintf("已更新 %d 个仓库的远端分支信息", len(res)))
			a.RefreshAsync()
		})
	}()
}

// TriggerBatchPull runs git pull --ff-only across all clean repos.
func (a *App) TriggerBatchPull() {
	a.busyLabel = "正在拉取更新..."
	repos := append([]mani.DiscoveredRepo(nil), a.repos...)
	conc := a.cfg.Concurrency
	go func() {
		ctx := context.Background()
		res := gitops.PullAll(ctx, repos, conc)
		okCnt, skipCnt, failCnt := countOpResults(res)
		a.runUI(func() {
			a.setBatchResults(res)
			a.showToast("ok", fmt.Sprintf("拉取完成：%d 成功，%d 跳过，%d 失败", okCnt, skipCnt, failCnt))
			a.RefreshAsync()
		})
	}()
}

// TriggerSwitchToMain switches all clean repos back to their default branch and pulls.
func (a *App) TriggerSwitchToMain() {
	a.busyLabel = "正在切回默认分支并拉取更新..."
	repos := append([]mani.DiscoveredRepo(nil), a.repos...)
	conc := a.cfg.Concurrency
	go func() {
		ctx := context.Background()
		res := gitops.SwitchToMainAll(ctx, repos, conc)
		okCnt, skipCnt, failCnt := countOpResults(res)
		a.runUI(func() {
			a.setBatchResults(res)
			a.showToast("ok", fmt.Sprintf("已切回默认分支：%d 成功，%d 跳过，%d 失败", okCnt, skipCnt, failCnt))
			a.RefreshAsync()
		})
	}()
}

// switchRepoBranch switches one repository to targetBranch off the UI thread.
func (a *App) switchRepoBranch(repo mani.DiscoveredRepo, targetBranch string) {
	go func() {
		res := gitops.SwitchBranchAll(context.Background(), []mani.DiscoveredRepo{repo}, targetBranch, false, 1)
		a.runUI(func() {
			a.setBatchResults(res)
			a.RefreshAsync()
		})
	}()
}

// TriggerBatchSwitch switches all repos to targetBranch.
func (a *App) TriggerBatchSwitch(targetBranch string, createIfMissing bool) {
	targetBranch = strings.TrimSpace(targetBranch)
	if targetBranch == "" {
		a.showToast("warn", "请输入分支名称")
		return
	}
	a.busyLabel = "正在切换分支至 " + targetBranch + "..."
	repos := append([]mani.DiscoveredRepo(nil), a.repos...)
	conc := a.cfg.Concurrency
	go func() {
		ctx := context.Background()
		res := gitops.SwitchBranchAll(ctx, repos, targetBranch, createIfMissing, conc)
		okCnt, skipCnt, failCnt := countOpResults(res)
		a.runUI(func() {
			a.setBatchResults(res)
			a.showBranchModal = false
			a.showToast("ok", fmt.Sprintf("已切换至 %s：%d 成功，%d 跳过，%d 失败", targetBranch, okCnt, skipCnt, failCnt))
			a.RefreshAsync()
		})
	}()
}

// commitFiles returns the changed files the user checked.
func (a *App) commitFiles() []gitops.ChangedFile {
	var files []gitops.ChangedFile
	for _, st := range a.statuses {
		for _, f := range st.Files {
			if a.checkedChanges[f.Key()] {
				files = append(files, f)
			}
		}
	}
	return files
}

// commitTargets groups the checked files by repository. Every repository gets
// the commit message typed in the commit box.
func (a *App) commitTargets() []gitops.BatchCommitTarget {
	files := a.commitFiles()
	if len(files) == 0 {
		return nil
	}
	order := make([]string, 0, len(a.statuses))
	byRepo := make(map[string][]gitops.ChangedFile, len(a.statuses))
	for _, f := range files {
		if _, ok := byRepo[f.RepoID]; !ok {
			order = append(order, f.RepoID)
		}
		byRepo[f.RepoID] = append(byRepo[f.RepoID], f)
	}
	targets := make([]gitops.BatchCommitTarget, 0, len(order))
	for _, repoID := range order {
		repo, ok := a.statusByID[repoID]
		if !ok {
			continue
		}
		target := gitops.BatchCommitTarget{Repo: repo.Repo, Amend: a.commitAmend}
		seen := make(map[string]bool, len(byRepo[repoID]))
		for _, f := range byRepo[repoID] {
			if !seen[f.Path] {
				seen[f.Path] = true
				target.Files = append(target.Files, f.Path)
			}
		}
		targets = append(targets, target)
	}
	return targets
}

func (a *App) stageChangeFiles(files []gitops.ChangedFile, stage bool) {
	if len(files) == 0 {
		return
	}
	copied := append([]gitops.ChangedFile(nil), files...)
	go func() {
		ctx := context.Background()
		seen := make(map[string]bool, len(copied))
		for _, f := range copied {
			key := f.RepoPath + "\x00" + f.Path
			if seen[key] {
				continue
			}
			seen[key] = true
			if stage {
				_ = gitops.StageFile(ctx, f.RepoPath, f.Path)
			} else {
				_ = gitops.UnstageFile(ctx, f.RepoPath, f.Path)
			}
		}
		a.runUI(func() {
			if stage {
				a.showToast("ok", fmt.Sprintf("已暂存 %d 个文件", len(seen)))
			} else {
				a.showToast("ok", fmt.Sprintf("已取消暂存 %d 个文件", len(seen)))
			}
			a.RefreshAsync()
		})
	}()
}

// TriggerCommit executes either a unified multi-repo commit or single-repo commit, optionally followed by push.
func (a *App) TriggerCommit(pushAfter bool) {
	msg := strings.TrimSpace(a.commitMessage)
	if msg == "" {
		a.showToast("warn", "请填写提交说明")
		return
	}

	targets := a.commitTargets()
	if len(targets) == 0 {
		a.showToast("warn", "请先勾选要提交的文件")
		return
	}

	a.busyLabel = fmt.Sprintf("正在提交 %d 个仓库...", len(targets))
	conc := a.cfg.Concurrency
	go func() {
		ctx := context.Background()
		res := gitops.BatchCommit(ctx, targets, msg, pushAfter, conc)
		okCnt, _, failCnt := countOpResults(res)
		a.runUI(func() {
			a.setBatchResults(res)
			if failCnt == 0 {
				a.commitMessage = ""
				a.commitAmend = false
				a.showToast("ok", fmt.Sprintf("已提交 %d 个仓库", okCnt))
			} else {
				a.showToast("warn", fmt.Sprintf("提交完成：%d 成功，%d 失败", okCnt, failCnt))
			}
			a.RefreshAsync()
		})
	}()
}

// TriggerPushAllAhead pushes all repositories that have Ahead > 0.
func (a *App) TriggerPushAllAhead() {
	var aheadRepos []mani.DiscoveredRepo
	for _, st := range a.statuses {
		if st.Ahead > 0 {
			aheadRepos = append(aheadRepos, st.Repo)
		}
	}
	if len(aheadRepos) == 0 {
		a.showToast("ok", "所有仓库均已推送，无需推送")
		return
	}
	a.busyLabel = fmt.Sprintf("正在推送 %d 个仓库...", len(aheadRepos))
	conc := a.cfg.Concurrency
	go func() {
		ctx := context.Background()
		res := gitops.BatchPush(ctx, aheadRepos, conc)
		okCnt, _, failCnt := countOpResults(res)
		a.runUI(func() {
			a.setBatchResults(res)
			a.showToast("ok", fmt.Sprintf("推送完成：%d 成功，%d 失败", okCnt, failCnt))
			a.RefreshAsync()
		})
	}()
}

// TriggerAIMergeConflict launches the AI Agent conflict resolver and verification for one conflicted plan.
func (a *App) TriggerAIMergeConflict(plan aimerge.RepoMergePlan) {
	if a.mergeRunning {
		return
	}
	a.mergeRunning = true
	a.selectedMergeRepoID = plan.Repo.ID
	a.aiVerifyReport = nil
	repoCopy := plan.Repo
	a.aiVerifyRepo = &repoCopy
	a.aiStreamRaw.Reset()
	a.aiStreamRaw.WriteString(fmt.Sprintf("### 正在使用 %s 解决冲突：%s (`%s` → `%s`)\n", a.aiAgentName, plan.Repo.Name, plan.SourceBranch, plan.TargetBranch))

	wsDir := a.cfg.WorkspacePath
	agentName := a.aiAgentName
	agentBin := a.aiAgentBin

	go func() {
		ctx := context.Background()
		report, err := aimerge.ResolveConflictWithAI(ctx, wsDir, plan, agentName, agentBin, func(line string) {
			a.runUI(func() {
				a.aiStreamRaw.WriteString(line + "\n")
			})
		})
		a.runUI(func() {
			a.mergeRunning = false
			a.busyLabel = ""
			if err != nil {
				a.aiStreamRaw.WriteString("\n> **错误**: " + err.Error() + "\n")
				a.showToast("err", "AI 解决冲突失败: "+err.Error())
				return
			}
			a.aiVerifyReport = report
			if report.Passed {
				a.showToast("ok", fmt.Sprintf("%s 冲突已解决并通过安全检查，请确认后推送", plan.Repo.Name))
			} else {
				a.showToast("warn", fmt.Sprintf("合并检查未通过: %s", report.FailureReason))
			}
			a.RefreshAsync()
		})
	}()
}

// TriggerManiTask runs the selected mani.yaml task across projects in parallel.
func (a *App) TriggerManiTask(taskName string, extraEnv map[string]string) {
	if a.maniCfg == nil || a.maniTaskRunning {
		return
	}
	spec, ok := a.maniCfg.Tasks[taskName]
	if !ok {
		return
	}
	a.selectedManiTask = taskName
	a.maniTaskRunning = true
	a.busyLabel = "正在执行脚本: " + taskName + "..."
	wsDir := a.cfg.WorkspacePath
	repos := append([]mani.DiscoveredRepo(nil), a.repos...)
	conc := a.cfg.Concurrency

	go func() {
		ctx := context.Background()
		res := mani.RunTask(ctx, wsDir, taskName, spec, repos, extraEnv, conc)
		a.runUI(func() {
			a.maniTaskRunning = false
			a.busyLabel = ""
			a.maniResults = res
			a.showToast("ok", fmt.Sprintf("脚本 %s 执行完成（%d 个仓库）", taskName, len(res)))
			a.RefreshAsync()
		})
	}()
}

func countOpResults(res []gitops.OpResult) (okCnt, skipCnt, failCnt int) {
	for _, r := range res {
		switch r.Status {
		case "OK":
			okCnt++
		case "SKIP":
			skipCnt++
		default:
			failCnt++
		}
	}
	return
}

// displayPath shortens a path under the home directory to use "~".
func displayPath(p string) string {
	home, err := os.UserHomeDir()
	if err != nil || home == "" {
		return p
	}
	if p == home {
		return "~"
	}
	if strings.HasPrefix(p, home+string(filepath.Separator)) {
		return "~" + p[len(home):]
	}
	return p
}
