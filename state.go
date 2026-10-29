package main

import (
	"context"
	"fmt"
	"os"
	"path/filepath"
	"sort"
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

// ManualMerge holds the temporary worktree prepared for a hand-resolved merge.
type ManualMerge struct {
	RepoPath  string
	Path      string
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
	repoFilterID   string // "" means all repos, otherwise specific repo ID
	overviewFilter string // "all", "dirty", "ahead", "behind", "unaligned"
	selectedRepoID string // selected repo for detail inspection

	// Collapsible UI States (Progressive Disclosure)
	showCleanReposInChanges  bool
	showCommitOptions        bool
	showDiffOptions          bool
	showCodeOptions          bool
	showTopMoreMenu          bool
	showMergeAdvanced        bool
	showMergeCheckDetails    bool
	showRepoDetailDrawer     bool
	showStaleBranchesInModal bool
	collapsedCodeRepos       map[string]bool
	expandedCodeDirs         map[string]bool // "repoID::dirRelPath" -> expanded in file tree

	// Resizable Split Widths (in DIPs)
	changesSplit float32
	codeSplit    float32

	// Discovered Workspace Data
	repos           []mani.DiscoveredRepo
	maniCfg         *mani.Config
	statuses        []gitops.RepoStatus
	statusByID      map[string]gitops.RepoStatus
	workspaceFiles  []gitops.WorkspaceFile
	lastRefresh     time.Time
	lastFullRefresh time.Time
	isRefreshing    bool
	busyLabel       string

	// Derived caches, rebuilt on every snapshot so per-frame work stays small.
	recentBranches      []gitops.BranchInfo
	staleBranches       []gitops.BranchInfo
	workspaceBranchTree *branchNode
	workspaceStaleTree  *branchNode
	repoBranchTrees     map[string]*branchNode
	changeTreeCache     map[string]*changeTreeNode
	codeTreeCache       map[string]*codeTreeDir
	codeGroupsKey       string
	codeGroups          []codeRepoGroup

	// Changes & Diff View State
	selectedChangeKey     string
	selectedChangeFile    *gitops.ChangedFile
	checkedChanges        map[string]bool // ChangeFile.Key() -> selected for batch commit
	diffState             *editor.DiffState
	diffStagedOnly        bool
	diffEditable          bool
	diffModified          bool
	commitMessage         string
	commitScope           string // "checked" (勾选文件), "current" (当前仓库), "staged" (仅已暂存)
	commitAmend           bool
	generatedCommitRoot   string
	generatedCommitByRepo map[string]string
	generatedCommitKeys   string
	lastBatchResults      []gitops.OpResult
	pendingClipboard      string
	changesKindFilter     string // "", "A", "M", "D", "U"
	changesFilterQuery    string
	changesFilterHandle   ui.Handle
	lastCheckedKey        string
	showCommitPreview     bool
	collapsedChangeRepos  map[string]bool
	collapsedChangeDirs   map[string]bool // "repoID::dirRelPath" -> collapsed

	// Multi-Project Branch Switch & Presets State
	branchInput         string
	branchCheckedRepos  map[string]bool    // repoID -> checked for multi-project branch switch
	branchPerRepoTarget map[string]*string // repoID -> target branch string pointer for per-repo input
	branchSwitchResults []gitops.OpResult
	presetNameInput     string // e.g. "AAA 需求"

	// Branches & AI Merge State
	mergeTargetBranch       string
	mergeFetchFirst         bool
	mergePlans              []aimerge.RepoMergePlan
	mergeCheckedRepos       map[string]bool
	mergePresetInput        string
	mergeRunning            bool
	mergePreflightAttempted bool
	manualMerges            map[string]ManualMerge
	manualMergeRunning      bool
	selectedMergeRepoID     string
	mdOpts                  markdown.Options
	aiStreamRaw             strings.Builder
	aiVerifyReport          *aimerge.VerificationReport
	aiVerifyRepo            *mani.DiscoveredRepo
	aiAgentName             string
	aiAgentBin              string

	// Code Explorer State
	codeFilterQuery  string
	selectedCodeFile *gitops.WorkspaceFile
	codeEditorState  *editor.State
	codeMicroEdit    bool // false = 只读, true = 编辑
	codeDirty        bool

	// Mani Tasks State
	selectedManiTask string
	maniParamBranch  string
	maniParamTarget  string
	maniTaskRunning  bool
	maniResults      []mani.TaskResult

	// Modals & Overlays
	showCmdPalette  bool
	cmdPaletteQuery string
	showSettings    bool
	showBranchModal bool
	showOpLog       bool
	pendingDiscard  *PendingDiscard
	pendingUndo     *PendingUndo
	fileHistory     *FileHistoryState
	toastMessage    string
	toastKind       string // "ok", "warn", "err"
	toastExpires    time.Time

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
		checkedChanges:       make(map[string]bool),
		collapsedCodeRepos:   make(map[string]bool),
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
		commitScope:          "checked",
		mergeTargetBranch:    cfg.DefaultTargetBranch,
		mergeFetchFirst:      false,
		aiAgentName:          agentName,
		aiAgentBin:           agentBin,
		maniParamTarget:      cfg.DefaultTargetBranch,
	}
	initBuf := editor.NewBuffer("// 从左侧文件树选择文件，或按 ⌘K 搜索文件浏览代码\n")
	initBuf.SetLanguage(editor.LanguageFor("main.go"))
	a.codeEditorState = &editor.State{
		Buffer:   initBuf,
		ReadOnly: true,
		SoftWrap: cfg.EditorSoftWrap,
		Font:     ui.Font{Size: cfg.EditorFontSize},
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
	a.toastKind = kind
	a.toastMessage = msg
	a.toastExpires = time.Now().Add(4 * time.Second)
}

// overlayOpen reports whether a dialog owns the keyboard.
func (a *App) overlayOpen() bool {
	return a.showCmdPalette || a.showSettings || a.showBranchModal || a.showOpLog ||
		a.pendingDiscard != nil || a.pendingUndo != nil || a.fileHistory != nil
}

// commitAvailability explains why the commit buttons can or cannot run.
func (a *App) commitAvailability(checkedFiles, stagedTotal int) (bool, string) {
	switch a.commitScope {
	case "staged":
		if stagedTotal == 0 {
			return false, "暂存区为空：先暂存文件"
		}
	case "current":
		if a.selectedChangeFile == nil {
			return false, "先选择一个文件或仓库"
		}
	default:
		if checkedFiles == 0 {
			return false, "勾选文件后可提交"
		}
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

// triggerManualMerge prepares an isolated worktree with conflicts left in place.
func (a *App) triggerManualMerge(plan aimerge.RepoMergePlan) {
	if a.manualMergeRunning {
		return
	}
	if _, ok := a.manualMerges[plan.Repo.ID]; ok {
		a.showToast("warn", "已有临时合并工作区，请先清理再重试")
		return
	}
	a.manualMergeRunning = true
	a.busyLabel = "正在准备手动合并工作区..."
	go func() {
		path, conflicts, err := aimerge.PrepareManualMerge(context.Background(), plan)
		a.runUI(func() {
			a.manualMergeRunning = false
			a.busyLabel = ""
			if err != nil {
				a.showToast("err", err.Error())
				return
			}
			a.manualMerges[plan.Repo.ID] = ManualMerge{
				RepoPath:  plan.Repo.AbsPath,
				Path:      path,
				Conflicts: conflicts,
			}
			if len(conflicts) == 0 {
				a.showToast("ok", fmt.Sprintf("%s 已合并到临时工作区，可检查后再提交", plan.Repo.Name))
			} else {
				a.showToast("warn", fmt.Sprintf("%s 有 %d 个冲突，已保留在临时工作区", plan.Repo.Name, len(conflicts)))
			}
		})
	}()
}

// cleanupManualMerge removes a temporary manual merge worktree.
func (a *App) cleanupManualMerge(repoID string) {
	mm, ok := a.manualMerges[repoID]
	if !ok {
		return
	}
	delete(a.manualMerges, repoID)
	go func() {
		_ = aimerge.CleanupManualMerge(context.Background(), mm.RepoPath, mm.Path)
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
	a.mergePlans = nil
	a.mergePreflightAttempted = false
}

// LoadWorkspaceAsync discovers and inspects a workspace off the UI thread so the
// window paints immediately. Merge preflight is deferred to the merge view.
func (a *App) LoadWorkspaceAsync(wsPath string) {
	wsPath = filepath.Clean(wsPath)
	if wsPath == "" {
		return
	}
	a.cfg.AddRecentWorkspace(wsPath)
	a.busyLabel = "正在载入工作区..."
	a.isRefreshing = true
	a.mergePlans = nil
	a.mergePreflightAttempted = false
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
	// Silent refreshes only re-inspect repository status. Scanning every file and
	// running merge preflights on every poll would burn CPU for no visible gain.
	full := showBusy || a.lastFullRefresh.IsZero() || time.Since(a.lastFullRefresh) > 30*time.Second
	mergeVisible := full && a.activeTab == TabMerge

	a.isRefreshing = true
	if showBusy {
		a.busyLabel = "正在刷新仓库状态..."
	}
	wsPath := a.cfg.WorkspacePath
	incRoot := a.cfg.IncludeRootRepo
	conc := a.cfg.Concurrency
	targetBranch := a.mergeTargetBranch
	prot := append([]string(nil), a.cfg.ProtectedBranches...)
	cachedFiles := a.workspaceFiles

	go func() {
		ctx := context.Background()
		repos, maniCfg := mani.DiscoverRepos(wsPath, incRoot)
		statuses := gitops.InspectRepos(ctx, repos, conc)
		wsFiles := cachedFiles
		if full {
			wsFiles = gitops.ListWorkspaceFiles(ctx, repos, 2500)
		}
		var plans []aimerge.RepoMergePlan
		if mergeVisible {
			plans = aimerge.PreflightMergeAll(ctx, repos, targetBranch, false, prot, conc)
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
			if plans != nil {
				a.mergePlans = plans
			}
		})
	}()
}

func (a *App) applySnapshot(repos []mani.DiscoveredRepo, maniCfg *mani.Config, statuses []gitops.RepoStatus, wsFiles []gitops.WorkspaceFile) {
	a.repos = repos
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

	if a.selectedCodeFile == nil && len(wsFiles) > 0 {
		chosen := wsFiles[0]
		for _, wf := range wsFiles {
			base := filepath.Base(wf.RelPath)
			if base == "AGENTS.md" || base == "mani.yaml" || strings.HasSuffix(base, ".go") || strings.HasSuffix(base, ".java") {
				chosen = wf
				break
			}
		}
		a.selectCodeFile(chosen)
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

	if len(a.mergePlans) == 0 {
		a.mergePreflightAttempted = false
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

func (a *App) selectCodeFile(wf gitops.WorkspaceFile) {
	wfCopy := wf
	a.selectedCodeFile = &wfCopy
	a.codeDirty = false

	// Auto-expand ancestor directories in the file tree for wf
	dir := filepath.ToSlash(filepath.Dir(wf.RelPath))
	if dir != "." && dir != "" {
		parts := strings.Split(dir, "/")
		cur := ""
		for _, seg := range parts {
			if cur == "" {
				cur = seg
			} else {
				cur = cur + "/" + seg
			}
			a.expandedCodeDirs[wf.RepoID+"::"+cur] = true
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
	a.codeEditorState = &editor.State{
		Buffer:   buf,
		DiffBase: baseBuf,
		ReadOnly: !a.codeMicroEdit,
		SoftWrap: a.cfg.EditorSoftWrap,
		Font:     ui.Font{Size: a.cfg.EditorFontSize},
	}
}

func (a *App) saveCodeMicroEdit() {
	if a.selectedCodeFile == nil || a.codeEditorState == nil || a.codeEditorState.Buffer == nil {
		return
	}
	content := a.codeEditorState.Buffer.String()
	if err := gitops.SaveMicroEdit(a.selectedCodeFile.RepoPath, a.selectedCodeFile.RelPath, content); err != nil {
		a.showToast("err", "保存失败: "+err.Error())
		return
	}
	a.codeEditorState.Buffer.MarkSaved()
	a.codeDirty = false
	a.showToast("ok", fmt.Sprintf("已保存 %s/%s", a.selectedCodeFile.RepoName, a.selectedCodeFile.RelPath))
	a.RefreshAsync()
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

// BranchesByActivity returns the cached recent/stale branch partitions; the
// cache is rebuilt once per snapshot instead of once per frame.
func (a *App) BranchesByActivity() (recent []gitops.BranchInfo, stale []gitops.BranchInfo) {
	if a.recentBranches == nil && a.staleBranches == nil {
		return a.computeBranchesByActivity()
	}
	return a.recentBranches, a.staleBranches
}

// computeBranchesByActivity aggregates unique branches across repositories, sorted by most recently active first,
// and partitions them into recent (active within 7 days) vs stale (no changes in the last 7 days).
func (a *App) computeBranchesByActivity() (recent []gitops.BranchInfo, stale []gitops.BranchInfo) {
	byName := make(map[string]gitops.BranchInfo)
	activeCheckedOut := make(map[string]bool)

	for _, st := range a.statuses {
		if st.Branch != "" && !st.Detached {
			activeCheckedOut[st.Branch] = true
		}
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
	sort.Slice(all, func(i, j int) bool {
		if all[i].CommitUnix != all[j].CommitUnix {
			return all[i].CommitUnix > all[j].CommitUnix
		}
		return all[i].Name < all[j].Name
	})

	now := time.Now()
	for _, bi := range all {
		if bi.IsRecent(now) || activeCheckedOut[bi.Name] {
			recent = append(recent, bi)
		} else {
			stale = append(stale, bi)
		}
	}
	if len(recent) == 0 && len(stale) > 0 {
		n := min(len(stale), 5)
		recent = append(recent, stale[:n]...)
		stale = stale[n:]
	}
	return recent, stale
}

// rebuildBranchCaches refreshes every derived tree once per snapshot instead of
// rebuilding them during each frame.
func (a *App) rebuildBranchCaches() {
	a.recentBranches, a.staleBranches = a.computeBranchesByActivity()
	a.workspaceBranchTree = buildBranchTree(a.recentBranches)
	a.workspaceStaleTree = buildBranchTree(a.staleBranches)
	a.repoBranchTrees = make(map[string]*branchNode, len(a.statuses))
	for _, st := range a.statuses {
		a.repoBranchTrees[st.Repo.ID] = buildBranchTree(st.BranchInfos)
	}
	a.changeTreeCache = make(map[string]*changeTreeNode)
	a.codeTreeCache = make(map[string]*codeTreeDir)
	a.codeGroupsKey = "\x00"
	a.codeGroups = nil
}

// workspaceBranchTrees returns the cached hierarchy of recent and stale branches,
// falling back to a direct build before the first snapshot.
func (a *App) workspaceBranchTrees() (*branchNode, *branchNode) {
	recentTree := a.workspaceBranchTree
	if recentTree == nil {
		recentTree = buildBranchTree(a.recentBranches)
	}
	staleTree := a.workspaceStaleTree
	if staleTree == nil {
		staleTree = buildBranchTree(a.staleBranches)
	}
	return recentTree, staleTree
}

// filteredRepos returns the repositories targeted by the current repository filter.
func (a *App) filteredRepos() []mani.DiscoveredRepo {
	if a.repoFilterID == "" {
		return append([]mani.DiscoveredRepo(nil), a.repos...)
	}
	for _, st := range a.statuses {
		if st.Repo.ID == a.repoFilterID {
			return []mani.DiscoveredRepo{st.Repo}
		}
	}
	return nil
}

// TriggerStashAll stashes uncommitted changes (untracked included) in the visible repositories.
func (a *App) TriggerStashAll() {
	repos := a.filteredRepos()
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
			a.lastBatchResults = res
			a.showToast("ok", fmt.Sprintf("贮藏完成：%d 成功，%d 跳过，%d 失败", okCnt, skipCnt, failCnt))
			a.RefreshAsync()
		})
	}()
}

// TriggerStashPopAll restores the most recent stash in the visible repositories.
func (a *App) TriggerStashPopAll() {
	repos := a.filteredRepos()
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
			a.lastBatchResults = res
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
			a.lastBatchResults = res
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
	}

	recent, stale := a.BranchesByActivity()
	for _, bi := range recent {
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
	for _, bi := range stale {
		if !seen[bi.Name] {
			seen[bi.Name] = true
			out = append(out, bi)
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
			a.lastBatchResults = res
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
			a.lastBatchResults = res
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
			a.lastBatchResults = res
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
			a.lastBatchResults = res
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
			a.lastBatchResults = res
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
			a.lastBatchResults = res
			a.showBranchModal = false
			a.showToast("ok", fmt.Sprintf("已切换至 %s：%d 成功，%d 跳过，%d 失败", targetBranch, okCnt, skipCnt, failCnt))
			a.RefreshAsync()
		})
	}()
}

// commitFilesForScope returns the changed files covered by the active commit scope.
func (a *App) commitFilesForScope() []gitops.ChangedFile {
	var files []gitops.ChangedFile
	if a.commitScope == "staged" {
		for _, st := range a.statuses {
			if a.repoFilterID != "" && st.Repo.ID != a.repoFilterID {
				continue
			}
			for _, f := range st.Files {
				if f.Staged {
					files = append(files, f)
				}
			}
		}
		return files
	}
	if a.commitScope == "current" && a.selectedChangeFile != nil {
		for _, st := range a.statuses {
			if st.Repo.ID != a.selectedChangeFile.RepoID || len(st.Files) == 0 {
				continue
			}
			var picked []gitops.ChangedFile
			for _, f := range st.Files {
				if a.checkedChanges[f.Key()] {
					picked = append(picked, f)
				}
			}
			if len(picked) == 0 {
				picked = st.Files
			}
			return picked
		}
		return nil
	}
	for _, st := range a.statuses {
		if a.repoFilterID != "" && st.Repo.ID != a.repoFilterID {
			continue
		}
		for _, f := range st.Files {
			if a.checkedChanges[f.Key()] {
				files = append(files, f)
			}
		}
	}
	return files
}

// commitTargets groups the scoped files by repository. Each repository gets its
// own generated message unless the user replaced the generated text.
func (a *App) commitTargets() []gitops.BatchCommitTarget {
	files := a.commitFilesForScope()
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
	perRepo := a.generatedCommitByRepo
	if !a.perRepoMessagesActive() {
		perRepo = nil
	}
	targets := make([]gitops.BatchCommitTarget, 0, len(order))
	for _, repoID := range order {
		repo, ok := a.statusByID[repoID]
		if !ok {
			continue
		}
		target := gitops.BatchCommitTarget{Repo: repo.Repo, Message: perRepo[repoID], Amend: a.commitAmend}
		if a.commitScope != "staged" {
			seen := make(map[string]bool, len(byRepo[repoID]))
			for _, f := range byRepo[repoID] {
				if !seen[f.Path] {
					seen[f.Path] = true
					target.Files = append(target.Files, f.Path)
				}
			}
		}
		targets = append(targets, target)
	}
	return targets
}

// generateCommitMessage fills the commit input from the selected files:
// a summary title, per-kind counts and a compact file list per repository.
func (a *App) generateCommitMessage() {
	files := a.commitFilesForScope()
	if len(files) == 0 {
		if a.commitScope == "staged" {
			a.showToast("warn", "暂存区没有可提交的文件")
		} else {
			a.showToast("warn", "请先勾选要提交的文件")
		}
		return
	}
	set := buildCommitMessages(files)
	a.commitMessage = set.root
	a.generatedCommitRoot = set.root
	a.generatedCommitByRepo = set.byRepo
	a.generatedCommitKeys = commitSelectionSignature(files)
	a.showToast("ok", fmt.Sprintf("已根据 %d 个文件生成提交说明", len(files)))
}

// stageChangeFiles stages or unstages every given file, then refreshes statuses.
// commitSelectionSignature fingerprints the files a generated message describes.
func commitSelectionSignature(files []gitops.ChangedFile) string {
	parts := make([]string, 0, len(files))
	for _, f := range files {
		parts = append(parts, fmt.Sprintf("%s:%s:%d:%d", f.Key(), f.Kind, f.AddedLines, f.DeletedLines))
	}
	sort.Strings(parts)
	return strings.Join(parts, "|")
}

// perRepoMessagesActive reports whether the visible text still matches the
// generated per-repository messages for the current selection.
func (a *App) perRepoMessagesActive() bool {
	if a.generatedCommitRoot == "" || a.commitMessage != a.generatedCommitRoot || len(a.generatedCommitByRepo) == 0 {
		return false
	}
	return a.generatedCommitKeys == commitSelectionSignature(a.commitFilesForScope())
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

// mergePlanChecked reports whether a repository is included in the merge batch.
// Repositories without an explicit choice default to included.
func (a *App) mergePlanChecked(repoID string) bool {
	checked, ok := a.mergeCheckedRepos[repoID]
	if !ok {
		return true
	}
	return checked
}

// syncMergeSelection keeps the merge selection aligned with a fresh preflight:
// mergeable repositories are included by default, stale entries are dropped.
func (a *App) syncMergeSelection(plans []aimerge.RepoMergePlan) {
	keep := make(map[string]bool, len(plans))
	for _, plan := range plans {
		keep[plan.Repo.ID] = true
		mergeable := plan.Kind == aimerge.PlanClean || plan.Kind == aimerge.PlanConflict
		if !mergeable {
			// Nothing to merge: do not leave a stale selection behind.
			a.mergeCheckedRepos[plan.Repo.ID] = false
			continue
		}
		if _, ok := a.mergeCheckedRepos[plan.Repo.ID]; !ok {
			a.mergeCheckedRepos[plan.Repo.ID] = true
		}
	}
	for id := range a.mergeCheckedRepos {
		if !keep[id] {
			delete(a.mergeCheckedRepos, id)
		}
	}
}

// saveMergePreset stores the current target branch and checked repositories.
func (a *App) saveMergePreset() {
	name := strings.TrimSpace(a.mergePresetInput)
	if name == "" {
		a.showToast("warn", "请输入合并方案名称（如：合并到 test）")
		return
	}
	target := strings.TrimSpace(a.mergeTargetBranch)
	if target == "" {
		a.showToast("warn", "请先选择目标分支")
		return
	}
	var repoIDs []string
	for _, pl := range a.mergePlans {
		if a.mergePlanChecked(pl.Repo.ID) {
			repoIDs = append(repoIDs, pl.Repo.ID)
		}
	}
	if len(repoIDs) == 0 {
		a.showToast("warn", "请至少勾选一个要合并的仓库")
		return
	}
	a.cfg.SaveMergePreset(name, target, repoIDs)
	_ = a.cfg.Save()
	a.mergePresetInput = ""
	a.showToast("ok", fmt.Sprintf("已保存合并方案「%s」（%d 个仓库 → %s）", name, len(repoIDs), target))
}

// applyMergePreset checks the preset's repositories, sets its target branch and
// runs a fresh preflight so the user sees the outcome before merging.
func (a *App) applyMergePreset(preset config.MergePreset) {
	a.mergeTargetBranch = preset.TargetBranch
	for _, st := range a.statuses {
		a.mergeCheckedRepos[st.Repo.ID] = false
	}
	for _, id := range preset.RepoIDs {
		a.mergeCheckedRepos[id] = true
	}
	a.TriggerMergePreflight(false)
}

// TriggerReturnToDevelopmentBranches switches merged repositories back to the
// source branch recorded by the preflight (the branch the user was developing on).
func (a *App) TriggerReturnToDevelopmentBranches() {
	var targets []gitops.RepoBranchTarget
	for _, st := range a.statuses {
		if a.repoFilterID != "" && st.Repo.ID != a.repoFilterID {
			continue
		}
		target := ""
		for _, plan := range a.mergePlans {
			if plan.Repo.ID != st.Repo.ID {
				continue
			}
			if plan.SourceBranch != "" && plan.SourceBranch != plan.TargetBranch {
				target = plan.SourceBranch
			}
			break
		}
		if target == "" || target == st.Branch {
			continue
		}
		targets = append(targets, gitops.RepoBranchTarget{Repo: st.Repo, TargetBranch: target})
	}
	if len(targets) == 0 {
		if len(a.mergePlans) == 0 {
			a.showToast("warn", "先执行一次合并预演，工具才知道各仓库的开发分支")
		} else {
			a.showToast("ok", "所有仓库都已在开发分支上")
		}
		return
	}
	a.busyLabel = fmt.Sprintf("正在切回 %d 个仓库的开发分支...", len(targets))
	conc := a.cfg.Concurrency
	go func() {
		ctx := context.Background()
		res := gitops.SwitchBranchPlan(ctx, targets, false, conc)
		okCnt, skipCnt, failCnt := countOpResults(res)
		a.runUI(func() {
			a.busyLabel = ""
			a.lastBatchResults = res
			a.showToast("ok", fmt.Sprintf("已切回开发分支：%d 成功，%d 跳过，%d 失败", okCnt, skipCnt, failCnt))
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
		if a.commitScope == "staged" {
			a.showToast("warn", "暂存区没有可提交的文件")
		} else {
			a.showToast("warn", "请先勾选要提交的文件")
		}
		return
	}

	a.busyLabel = fmt.Sprintf("正在提交 %d 个仓库...", len(targets))
	conc := a.cfg.Concurrency
	go func() {
		ctx := context.Background()
		res := gitops.BatchCommit(ctx, targets, msg, pushAfter, conc)
		okCnt, _, failCnt := countOpResults(res)
		a.runUI(func() {
			a.lastBatchResults = res
			if failCnt == 0 {
				a.commitMessage = ""
				a.generatedCommitRoot = ""
				a.generatedCommitByRepo = nil
				a.generatedCommitKeys = ""
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
			a.lastBatchResults = res
			a.showToast("ok", fmt.Sprintf("推送完成：%d 成功，%d 失败", okCnt, failCnt))
			a.RefreshAsync()
		})
	}()
}

// TriggerMergePreflight runs non-destructive git merge-tree conflict checks across all repos.
func (a *App) TriggerMergePreflight(fetchFirst bool) {
	a.mergePreflightAttempted = true
	target := strings.TrimSpace(a.mergeTargetBranch)
	if target == "" {
		a.showToast("warn", "请输入目标分支（如 test 或 main）")
		return
	}
	a.mergeRunning = true
	a.busyLabel = fmt.Sprintf("正在检查合并到 %s 的冲突情况...", target)
	repos := append([]mani.DiscoveredRepo(nil), a.repos...)
	prot := append([]string(nil), a.cfg.ProtectedBranches...)
	conc := a.cfg.Concurrency

	go func() {
		ctx := context.Background()
		plans := aimerge.PreflightMergeAll(ctx, repos, target, fetchFirst, prot, conc)
		a.runUI(func() {
			a.mergeRunning = false
			a.busyLabel = ""
			a.mergePlans = plans
			a.syncMergeSelection(plans)
			cleanCnt, conflictCnt := 0, 0
			for _, p := range plans {
				if p.Kind == aimerge.PlanClean {
					cleanCnt++
				} else if p.Kind == aimerge.PlanConflict {
					conflictCnt++
				}
			}
			a.showToast("ok", fmt.Sprintf("检查完成：%d 个可直接合并，%d 个有冲突", cleanCnt, conflictCnt))
		})
	}()
}

// TriggerExecuteCleanMerges executes all PlanClean merges in isolated worktrees.
func (a *App) TriggerExecuteCleanMerges(pushRemote bool) {
	var cleanPlans []aimerge.RepoMergePlan
	for _, p := range a.mergePlans {
		if p.Kind == aimerge.PlanClean && a.mergePlanChecked(p.Repo.ID) {
			cleanPlans = append(cleanPlans, p)
		}
	}
	if len(cleanPlans) == 0 {
		a.showToast("warn", "当前没有勾选可直接合并的仓库")
		return
	}
	a.mergeRunning = true
	a.busyLabel = fmt.Sprintf("正在合并 %d 个无冲突仓库...", len(cleanPlans))

	go func() {
		ctx := context.Background()
		var res []gitops.OpResult
		for _, p := range cleanPlans {
			res = append(res, aimerge.ExecuteCleanMerge(ctx, p, pushRemote))
		}
		okCnt, _, failCnt := countOpResults(res)
		a.runUI(func() {
			a.mergeRunning = false
			a.busyLabel = ""
			a.lastBatchResults = res
			a.showToast("ok", fmt.Sprintf("合并完成：%d 成功，%d 失败", okCnt, failCnt))
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
