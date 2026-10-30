package main

import (
	"context"
	"fmt"
	"math"
	"os"
	"path/filepath"
	"strings"
	"sync"
	"time"

	"github.com/egoist/mygo"
	"github.com/egoist/mygo/plugins/terminal"
	"github.com/egoist/mygo/ui"
	"github.com/egoist/mygo/ui/editor"
	"github.com/egoist/mygo/ui/markdown"
	"github.com/iwangjie/nemorous/internal/aimerge"
	"github.com/iwangjie/nemorous/internal/config"
	"github.com/iwangjie/nemorous/internal/conflict"
	"github.com/iwangjie/nemorous/internal/dap"
	"github.com/iwangjie/nemorous/internal/github"
	"github.com/iwangjie/nemorous/internal/gitops"
	"github.com/iwangjie/nemorous/internal/localhistory"
	"github.com/iwangjie/nemorous/internal/mani"
	"github.com/iwangjie/nemorous/internal/plugins"
)

// NavTab identifies the primary workspace view.
type NavTab string

const (
	TabChanges NavTab = "changes" // 提交
	TabCode    NavTab = "code"    // 文件
)

// PendingDiscard represents a file awaiting user confirmation before discard.
type PendingDiscard struct {
	RepoID    string
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

	blameEnabled  bool
	blameAll      bool
	blameLine     int
	blameLoading  bool
	blameLoaded   bool
	blameErr      string
	blameSeq      uint64
	blameLines    map[int]gitops.BlameLine
	blameShownKey string
	blameHiddenAt time.Time
	editorWidth   float32
	viewHandle    ui.Handle
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
	activeTab    NavTab
	showGitPanel bool
	gitEditorPct float32
	logEditorPct float32
	systemDark   bool // effective system appearance when cfg.Theme == "system"

	// Collapsible UI States (Progressive Disclosure)
	showDiffOptions       bool
	showCodeOptions       bool
	showMergeAdvanced     bool
	showMergeCheckDetails bool
	expandedCodeRepos     map[string]bool // "repoID" -> expanded in file tree
	expandedCodeDirs      map[string]bool // "repoID::dirRelPath" -> expanded in file tree
	codeRootCollapsed     bool            // workspace root row collapse state

	// Resizable Split Widths (in DIPs)
	changesSplit float32
	codeSplit    float32

	// Commit box height, dragged from the divider above it. Zero keeps the
	// content height; changesCommitAutoH remembers that height so the first
	// drag starts from it.
	changesCommitH     float32
	changesCommitAutoH float32

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
	commandCancel   context.CancelFunc
	commandID       uint64
	// refreshAfterCommand retries a refresh that was invalidated by a command,
	// so a stale pre-command snapshot is never published.
	refreshAfterCommand   bool
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
	selectedChangeKey        string
	selectedChangeFile       *gitops.ChangedFile
	checkedChanges           map[string]bool // ChangeFile.Key() -> selected for batch commit
	diffState                *editor.DiffState
	diffStagedOnly           bool
	diffEditable             bool
	diffModified             bool
	commitMessage            string
	commitAmend              bool
	lastBatchResults         []gitops.OpResult
	pendingClipboard         string
	changesFilterQuery       string
	changesFilterHandle      ui.Handle
	lastCheckedKey           string
	collapsedChangeRepos     map[string]bool
	collapsedChangeDirs      map[string]bool // "repoID::dirRelPath" -> collapsed
	collapsedStagedSection   bool
	collapsedUnstagedSection bool
	stagedSectionInit        bool

	// Multi-Project Branch Switch & Presets State
	branchInput         string
	branchCheckedRepos  map[string]bool    // repoID -> checked for multi-project branch switch
	branchPerRepoTarget map[string]*string // repoID -> target branch string pointer for per-repo input
	branchSwitchResults []gitops.OpResult
	presetNameInput     string // e.g. "AAA 需求"

	// Git Management State
	gitSubTab              string // "history" or "merge"
	gitFilterText          string
	gitFilterAuthor        string
	gitAuthorMe            bool
	gitAuthorSuggestions   []string
	gitAuthorsLoaded       bool
	gitAuthorsLoading      bool
	gitFilterPath          string
	gitPathCache           []string
	gitPathCacheKey        string
	gitFilterRepoID        string
	gitFilterBranch        string
	gitSinceDays           int
	gitFilterHandle        ui.Handle
	gitLogLoaded           bool
	gitLogSeq              uint64
	gitLogGroups           []gitRepoLog
	gitLogTargets          []gitops.RepoStatus
	gitUnifiedRows         []gitCommitRow
	gitUnifiedMaxLanes     int
	gitUnifiedLimit        int
	gitLogTotal            int
	gitGroupLoading        map[string]bool
	gitSelectedCommit      string
	gitAutoSelected        bool
	gitRevealCommit        string              // row to scroll into view once
	gitPinnedCommit        *gitops.CommitEntry // blame jump: keep the target visible outside the first log page
	gitDetail              *gitops.CommitDetail
	gitDetailLoading       bool
	gitDetailSeq           uint64
	gitDetailCollapsedDirs map[string]bool
	gitCollapsedRepos      map[string]bool
	gitRemoteOpen          map[string]bool
	gitLeftWidth           float32
	gitCenterPct           float32
	maniContentPct         float32
	gitDiffOpen            bool
	gitDiffLoading         bool
	gitDiffRepo            string
	gitDiffPath            string
	gitDiffOldPath         string
	gitDiffHash            string
	gitDiffErr             string
	gitDiffSeq             uint64
	gitCommitDiff          *editor.DiffState

	// Branches & AI Merge State
	mergeTargetBranch   string
	mergeTargets        []aimerge.MergeTarget
	mergeDetailPct      float32
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
	showManiPanel         bool
	maniRootCollapsed     bool
	maniTasksCollapsed    bool
	maniProjectsCollapsed bool
	maniParamsOpen        bool
	selectedManiTask      string
	maniLastTask          string
	maniLastScope         string
	maniParamBranch       string
	maniParamTarget       string
	maniTaskRunning       bool
	maniResults           []mani.TaskResult
	showManiResult        bool
	maniResultTitle       string
	maniResultOutput      string

	// Git advanced operations
	showGitOps       bool
	gitOpsTab        string
	gitOpsRepo       string // repoID
	gitOpsLoading    bool
	gitOpsErr        string
	gitOpsNotice     string
	gitOpsConfirm    string
	gitOpsStashes    []gitops.StashEntry
	gitOpsTags       []gitops.TagInfo
	gitOpsRemotes    []gitops.RemoteInfo
	gitOpsSubmodules []gitops.SubmoduleInfo
	gitOpsState      gitops.RepoOpState
	stashMessage     string
	stashUntracked   bool
	stashPreview     string
	stashPreviewRef  string
	gitOpsBranch     string
	gitOpsHash       string
	gitOpsMainline   string
	gitOpsTagName    string
	gitOpsTagMessage string
	gitOpsRemoteName string
	gitOpsRemoteURL  string
	gitOpsSubURL     string
	gitOpsSubPath    string

	// Conflict resolver
	showConflict         bool
	conflictRepoID       string
	conflictRepoPath     string
	conflictRepoName     string
	conflictManual       bool
	conflictFiles        []string
	conflictSelected     string
	conflictEditor       *editor.State
	conflictBaseView     *editor.State
	conflictOursView     *editor.State
	conflictTheirsView   *editor.State
	conflictHunkIdx      int
	conflictDrafts       map[string]*editor.State
	conflictHunkCount    map[string]int
	conflictBeforeAuto   string
	conflictOpLabel      string
	conflictBinary       bool
	conflictConfirmAbort bool
	conflictConfirmClose bool
	conflictDoc          *conflict.Document
	conflictBase         string
	conflictOurs         string
	conflictTheirs       string
	conflictErr          string
	conflictNotice       string
	conflictLoading      bool

	// Local history
	localHistoryStore     *localhistory.Store
	showLocalHistory      bool
	historyRepoID         string
	historyRepoPath       string
	historyRelPath        string
	historyEntries        []localhistory.Entry
	historySelected       string
	historyContent        string
	historyEditor         *editor.State
	historyDiff           *editor.DiffState
	historyDiffMode       bool
	historyLoading        bool
	historyErr            string
	historyNotice         string
	historyRestoreConfirm bool

	// GitHub
	showGitHub     bool
	githubRepo     string
	githubLoading  bool
	githubErr      string
	githubPRs      []github.PR
	githubNumber   int
	githubDetails  *github.Details
	githubComment  string
	githubState    string
	githubTitle    string
	githubBody     string
	githubBase     string
	githubDraft    bool
	githubCreating bool

	// Run / Debug / Test
	showRunPanel     bool
	runPanelPct      float32
	runConfigID      string
	runRunning       bool
	runOutput        []string
	runStatus        string
	runExit          int
	runCancel        context.CancelFunc
	runStartedAt     time.Time
	showRunConfig    bool
	runCfgDraft      config.RunConfig
	runCfgEnvText    string
	runCfgLaunchText string
	runCfgIsNew      bool
	runCfgErr        string
	debugClient      *dap.Client
	debugThreadID    int
	debugFrames      []dap.Frame
	debugVars        []dap.Variable
	debugScopes      []dap.Scope
	debugFrameID     int
	debugStopped     bool
	debugMsg         string
	breakpoints      map[string][]int // repoID::path -> lines
	debugEvents      chan dap.Event
	debugEventCancel context.CancelFunc
	runScroll        *ui.ScrollState
	runScrollLen     int

	// Change lists
	changeListFilter      string
	showChangeListModal   bool
	changeListNameInput   string
	changeListRenameFrom  string
	changeListRenameInput string
	changeListErr         string

	// Editor extras
	showStructure  bool
	structureItems []structureItem
	showTodos      bool
	todoQuery      string
	todos          []todoItem
	todosLoading   bool
	todoErr        string
	showBookmarks  bool
	bookmarks      []config.Bookmark
	bookmarkQuery  string

	// Session restore
	sessionRestored  bool
	restoringSession bool
	sessionTimer     *time.Timer

	// Plugin engine & marketplace
	pluginMgr             *plugins.Manager
	pluginPanel           string // active plugin panel ref, "maven:maven"
	pluginPanelPct        float32
	showPluginMarket      bool
	pluginMarketQuery     string
	pluginMarketTab       string // "market" or "installed"
	pluginMarketBusy      map[string]bool
	pluginMarketErr       string
	pluginMarketDraft     string
	pluginMarketConfirm   string
	pluginRegistryLoading bool
	pluginScroll          map[string]*ui.ScrollState
	pluginScrollText      map[string]string
	pluginIconCache       map[string]*ui.SVG

	// Signed GitHub auto-update state
	updateLoopCancel    context.CancelFunc
	updateCheckCancel   context.CancelFunc
	updateInstallCancel context.CancelFunc
	updateIdleTimer     *time.Timer
	updateMinutesDraft  float64
	updateChecking      bool
	updateLastCheck     time.Time
	updateLastErr       string
	updatePending       *mygo.Update
	updatePromptedFor   string
	updateShowDialog    bool
	updateInstalling    bool
	updateProgress      float64
	updateInstallErr    string
	updateDeferred      bool

	// Modals & Overlays
	showCmdPalette       bool
	cmdPaletteQuery      string
	showGlobalSearch     bool
	globalSearchQuery    string
	globalSearchHandle   ui.Handle
	globalSearchCase     bool
	globalSearchRegex    bool
	globalSearchGroups   []globalSearchGroup
	globalSearchTotal    int
	globalSearchRunning  bool
	globalSearchErr      string
	globalSearchPartial  bool
	globalSearchSeq      uint64
	globalSearchCancel   context.CancelFunc
	showSettings         bool
	showBranchModal      bool
	showBranchPicker     bool
	branchPickerQuery    string
	branchPickerRepoID   string
	branchPickerHandle   ui.Handle
	showOpLog            bool
	showLogPanel         bool
	logTab               string
	showTerminalPanel    bool
	terminalEditorPct    float32
	terminalTitle        string
	terminalErr          string
	terminalExited       bool
	terminalExitCode     int
	terminalGen          uint64
	terminal             *terminal.Terminal
	pendingDiscard       *PendingDiscard
	pendingCodeTabClose  *codeTab
	pendingCodeTabsClose []*codeTab
	pendingUndo          *PendingUndo
	fileHistory          *FileHistoryState
	notify               *AppNotification

	// Handles for focus & shortcuts
	rootHandle        ui.Handle
	cmdInputHandle    ui.Handle
	commitInputHandle ui.Handle
	codeFilterHandle  ui.Handle

	watcherCancel context.CancelFunc
	mu            sync.Mutex
	uiMu          sync.Mutex
	logMu         sync.Mutex
	commandLog    []appLogEntry
	runtimeLog    []appLogEntry
}

func NewApp(cfg config.Config) *App {
	cfg.Theme = normalizeTheme(cfg.Theme)
	agentName, agentBin := cfg.ResolvedAIAgent()
	a := &App{
		cfg:                    cfg,
		activeTab:              TabChanges,
		updateMinutesDraft:     float64(cfg.UpdateCheckMinutes),
		gitEditorPct:           62,
		logEditorPct:           62,
		terminalEditorPct:      62,
		gitSubTab:              "history",
		logTab:                 "command",
		gitCollapsedRepos:      make(map[string]bool),
		gitDetailCollapsedDirs: make(map[string]bool),
		gitGroupLoading:        make(map[string]bool),
		gitRemoteOpen:          make(map[string]bool),
		gitLeftWidth:           250,
		gitCenterPct:           62,
		gitUnifiedLimit:        80,
		maniContentPct:         72,
		mergeDetailPct:         66,
		statusByID:             make(map[string]gitops.RepoStatus),
		watchPending:           make(map[string]bool),
		checkedChanges:         make(map[string]bool),
		expandedCodeRepos:      make(map[string]bool),
		expandedCodeDirs:       make(map[string]bool),
		branchCheckedRepos:     make(map[string]bool),
		branchPerRepoTarget:    make(map[string]*string),
		mergeCheckedRepos:      make(map[string]bool),
		manualMerges:           make(map[string]ManualMerge),
		collapsedChangeRepos:   make(map[string]bool),
		collapsedChangeDirs:    make(map[string]bool),
		repoBranchTrees:        make(map[string]*branchNode),
		changeTreeCache:        make(map[string]*changeTreeNode),
		codeTreeCache:          make(map[string]*codeTreeDir),
		pluginMarketBusy:       make(map[string]bool),
		breakpoints:            make(map[string][]int),
		conflictDrafts:         make(map[string]*editor.State),
		conflictHunkCount:      make(map[string]int),
		changeListFilter:       "__all__",
		runPanelPct:            42,
		pluginScroll:           make(map[string]*ui.ScrollState),
		pluginScrollText:       make(map[string]string),
		pluginIconCache:        make(map[string]*ui.SVG),
		pluginPanelPct:         72,
		changesSplit:           320,
		codeSplit:              290,
		codeTabIdx:             -1,
		mergeTargetBranch:      cfg.DefaultTargetBranch,
		aiAgentName:            agentName,
		aiAgentBin:             agentBin,
		maniParamTarget:        cfg.DefaultTargetBranch,
	}
	if len(cfg.Breakpoints) > 0 {
		for key, lines := range cfg.Breakpoints {
			a.breakpoints[key] = append([]int(nil), lines...)
		}
	}
	a.logRuntimeDetail("INFO", "应用启动", strings.TrimSpace(cfg.WorkspacePath))
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
	switch a.cfg.Theme {
	case "darcula":
		return darculaPalette()
	case "dark":
		return darkPalette()
	case "system":
		if a.systemDark {
			return darkPalette()
		}
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
		return
	}
	// Headless tests have no window loop; serialize background callbacks so
	// concurrent repo loads cannot mutate UI state maps at the same time.
	a.uiMu.Lock()
	defer a.uiMu.Unlock()
	fn()
}

// startCommand marks a cancelable batch command and returns its context.
func (a *App) startCommand(label string) (context.Context, uint64) {
	if a.commandCancel != nil {
		a.commandCancel()
	}
	ctx, cancel := context.WithCancel(context.Background())
	a.commandCancel = cancel
	a.commandID++
	a.busyLabel = label
	a.logCommand("RUN", label, "")
	return ctx, a.commandID
}

// finishCommand clears the progress label when id is still the current command.
func (a *App) finishCommand(id uint64) bool {
	if a.commandID != id {
		return false
	}
	a.busyLabel = ""
	a.commandCancel = nil
	if a.refreshAfterCommand {
		a.refreshAfterCommand = false
		a.refreshInternal(true)
	}
	return true
}

// commandActive reports whether a batch command currently owns the repositories.
// Refreshes are held back so autostash's transient clean worktree never reaches
// the changes list; the command refreshes once when it completes.
func (a *App) commandActive() bool {
	return a.commandCancel != nil
}

// cancelCommand stops the running batch command and dismisses its progress.
func (a *App) cancelCommand() {
	if a.commandCancel != nil {
		a.commandCancel()
	}
	a.commandCancel = nil
	a.commandID++
	a.refreshAfterCommand = false
	a.busyLabel = ""
	a.logCommand("CANCEL", "已取消当前操作", "")
	a.showToast("warn", "已取消当前操作")
	a.RefreshAsync()
}

func (a *App) showToast(kind, msg string) {
	a.logRuntime(kind, msg)
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
	for _, row := range res {
		detail := fmt.Sprintf("%dms", row.Duration.Milliseconds())
		a.logCommand(row.Status, row.RepoName+" · "+row.Message, detail)
	}
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
	return a.showCmdPalette || a.showGlobalSearch || a.showSettings || a.showBranchModal || a.showBranchPicker || a.showOpLog ||
		a.pendingDiscard != nil || a.pendingUndo != nil || a.fileHistory != nil || a.pendingCodeTabClose != nil ||
		len(a.pendingCodeTabsClose) > 0 || a.showManiResult || a.updateShowDialog || a.showPluginMarket ||
		a.showGitOps || a.showConflict || a.showLocalHistory || a.showStructure || a.showTodos ||
		a.showBookmarks || a.showChangeListModal || a.showGitHub || a.showRunConfig
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

	a.gitAuthorsLoaded = false
	a.gitAuthorsLoading = false
	a.gitAuthorSuggestions = nil
	a.gitPathCacheKey = ""
	a.applySnapshot(repos, maniCfg, statuses, wsFiles)
	a.lastFullRefresh = time.Now()
	a.codeRootCollapsed = false
	a.mergeTargets = nil
	a.mergeScanAt = time.Time{}
	a.logRuntimeDetail("INFO", fmt.Sprintf("工作区加载完成：%d 个仓库", len(repos)), wsPath)
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
	if a.commandActive() {
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
	epoch := a.commandID
	go func() {
		ctx := context.Background()
		repos, maniCfg := mani.DiscoverRepos(wsPath, incRoot)
		statuses := gitops.InspectRepos(ctx, repos, conc)
		wsFiles := gitops.ListWorkspaceFiles(ctx, repos, 2500)
		a.runUI(func() {
			a.isRefreshing = false
			if a.commandActive() || a.commandID != epoch {
				a.watchPendingWorkspace = true
				a.refreshAfterCommand = true
				if !a.commandActive() {
					a.refreshAfterCommand = false
					a.refreshInternal(true)
				}
				return
			}
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
	if a.commandActive() {
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
	epoch := a.commandID
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
			if a.commandActive() || a.commandID != epoch {
				for _, repo := range targets {
					a.watchPending[repo.ID] = true
				}
				a.watchPendingFiles = a.watchPendingFiles || listFiles
				a.refreshAfterCommand = true
				if !a.commandActive() {
					a.refreshAfterCommand = false
					a.refreshInternal(true)
				}
				return
			}
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
	a.syncWindowTitle()
	a.maybeRestoreSession()
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
	a.logRuntimeDetail("INFO", "开始载入工作区", wsPath)
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
			a.logRuntimeDetail("INFO", fmt.Sprintf("工作区加载完成：%d 个仓库", len(repos)), wsPath)
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
	if a.commandActive() {
		return
	}
	// Silent refreshes only re-inspect repository status; file listings refresh
	// at most every 30 seconds.
	full := showBusy || a.lastFullRefresh.IsZero() || time.Since(a.lastFullRefresh) > 30*time.Second

	a.isRefreshing = true
	const refreshLabel = "正在刷新仓库状态..."
	if showBusy {
		a.busyLabel = refreshLabel
	}
	wsPath := a.cfg.WorkspacePath
	incRoot := a.cfg.IncludeRootRepo
	conc := a.cfg.Concurrency
	cachedFiles := a.workspaceFiles
	epoch := a.commandID

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
			if showBusy && a.busyLabel == refreshLabel {
				a.busyLabel = ""
			}
			if a.commandActive() || a.commandID != epoch {
				// A command may be autostashing; do not publish its transient
				// clean worktree. The command refreshes when it completes.
				a.refreshAfterCommand = true
				if !a.commandActive() {
					a.refreshAfterCommand = false
					a.refreshInternal(true)
				}
				return
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
	a.syncPluginsWorkspace(repos)
	a.maniCfg = maniCfg
	if !a.cfg.ManiEnabled || maniCfg == nil || len(maniCfg.Tasks) == 0 {
		a.showManiPanel = false
	}
	a.statuses = statuses
	for _, st := range statuses {
		if _, ok := a.gitCollapsedRepos[st.Repo.ID]; !ok {
			a.gitCollapsedRepos[st.Repo.ID] = true
		}
		if st.Err != nil {
			a.logRuntimeDetail("err", "仓库状态读取失败："+st.Repo.Name, st.Err.Error())
		}
	}
	a.workspaceFiles = wsFiles
	a.gitPathCacheKey = ""
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
	validKeys := make(map[string]bool, len(allValidKeys))
	for key := range allValidKeys {
		validKeys[key] = true
	}
	if a.cfg.PruneChangeLists(validKeys) {
		_ = a.cfg.Save()
	}
	bookmarkValid := make(map[string]bool, len(a.workspaceFiles))
	for _, f := range a.workspaceFiles {
		bookmarkValid[f.RepoID+"::"+f.RelPath] = true
	}
	for key := range allValidKeys {
		bookmarkValid[key] = true
	}
	if a.cfg.PruneBookmarks(bookmarkValid) {
		_ = a.cfg.Save()
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
	a.maybeRestoreSession()
}

func (a *App) selectChangedFile(cf gitops.ChangedFile) {
	a.gitDiffOpen = false
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
	a.recordLocalHistoryContent(a.selectedChangeFile.RepoID, a.selectedChangeFile.RepoPath, a.selectedChangeFile.Path, newContent, "保存")
	if a.cfg.FormatOnSave {
		if changed, ferr := a.formatFile(a.selectedChangeFile.RepoPath, a.selectedChangeFile.Path); ferr != nil {
			a.showToast("warn", "格式化失败: "+ferr.Error())
		} else if changed {
			a.selectChangedFile(*a.selectedChangeFile)
		}
	}
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
	a.gitDiffOpen = false
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
	tab := &codeTab{
		file: wf,
		state: &editor.State{
			Buffer:   buf,
			DiffBase: baseBuf,
			SoftWrap: a.cfg.EditorSoftWrap,
			Font:     ui.Font{Size: a.cfg.EditorFontSize},
			Bindings: []editor.Binding{{Mods: ui.Cmd, Key: ui.KeyG, Command: editor.GoToLine}},
		},
	}
	a.wireCodeBlame(tab)
	a.recordLocalHistory(wf.RepoID, wf.RepoPath, wf.RelPath, "打开")
	a.codeTabs = append(a.codeTabs, tab)
	a.codeTabIdx = len(a.codeTabs) - 1
	a.revealCodeFile(wf)
	a.scheduleSessionSave()
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

// requestCloseOtherCodeTabs closes every tab but keep, asking once when any
// of them has unsaved edits.
func (a *App) requestCloseOtherCodeTabs(keep *codeTab) {
	targets := make([]*codeTab, 0, len(a.codeTabs))
	for i, tab := range a.codeTabs {
		if tab == keep {
			a.codeTabIdx = i
			continue
		}
		targets = append(targets, tab)
	}
	a.requestCloseCodeTabs(targets)
}

// requestCloseAllCodeTabs closes every open code tab.
func (a *App) requestCloseAllCodeTabs() {
	a.requestCloseCodeTabs(append([]*codeTab(nil), a.codeTabs...))
}

func (a *App) requestCloseCodeTabs(tabs []*codeTab) {
	if len(tabs) == 0 {
		return
	}
	dirty := false
	for _, tab := range tabs {
		if tab.dirty() {
			dirty = true
			break
		}
	}
	if !dirty {
		a.closeCodeTabs(tabs)
		return
	}
	a.pendingCodeTabsClose = append([]*codeTab(nil), tabs...)
}

func (a *App) closeCodeTabs(tabs []*codeTab) {
	for _, tab := range tabs {
		a.closeCodeTab(tab)
	}
}

func (a *App) discardAndCloseCodeTabs(tabs []*codeTab) {
	a.pendingCodeTabsClose = nil
	a.closeCodeTabs(tabs)
}

func (a *App) saveAndCloseCodeTabs(tabs []*codeTab) {
	var failed []*codeTab
	for _, tab := range tabs {
		if tab.dirty() && !a.saveCodeTab(tab) {
			failed = append(failed, tab)
			continue
		}
		a.closeCodeTab(tab)
	}
	a.pendingCodeTabsClose = failed
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
	content := tab.state.Buffer.String()
	if err := gitops.SaveMicroEdit(tab.file.RepoPath, tab.file.RelPath, content); err != nil {
		a.showToast("err", "保存失败: "+err.Error())
		return false
	}
	a.recordLocalHistoryContent(tab.file.RepoID, tab.file.RepoPath, tab.file.RelPath, content, "保存")
	tab.state.Buffer.MarkSaved()
	if a.cfg.FormatOnSave {
		if changed, ferr := a.formatFile(tab.file.RepoPath, tab.file.RelPath); ferr != nil {
			a.showToast("warn", "格式化失败: "+ferr.Error())
		} else if changed {
			a.reloadCodeTab(tab)
			a.recordLocalHistory(tab.file.RepoID, tab.file.RepoPath, tab.file.RelPath, "格式化")
		}
	}
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
	repos := append([]mani.DiscoveredRepo(nil), a.repos...)
	conc := a.cfg.Concurrency
	ctx, cmdID := a.startCommand("正在获取远端更新 (Fetch)...")
	go func() {
		res := gitops.FetchAll(ctx, repos, conc)
		a.runUI(func() {
			if !a.finishCommand(cmdID) {
				return
			}
			a.setBatchResults(res)
			a.showToast("ok", fmt.Sprintf("已更新 %d 个仓库的远端分支信息", len(res)))
			a.RefreshAsync()
		})
	}()
}

// TriggerBatchPull runs git pull --ff-only across all clean repos.
func (a *App) TriggerBatchPull() {
	repos := append([]mani.DiscoveredRepo(nil), a.repos...)
	conc := a.cfg.Concurrency
	ctx, cmdID := a.startCommand("正在拉取更新...")
	go func() {
		res := gitops.PullAll(ctx, repos, conc)
		okCnt, skipCnt, failCnt := countOpResults(res)
		a.runUI(func() {
			if !a.finishCommand(cmdID) {
				return
			}
			a.setBatchResults(res)
			a.showToast("ok", fmt.Sprintf("拉取完成：%d 成功，%d 跳过，%d 失败", okCnt, skipCnt, failCnt))
			a.RefreshAsync()
		})
	}()
}

// TriggerSwitchToMain switches all clean repos back to their default branch and pulls.
func (a *App) TriggerSwitchToMain() {
	repos := append([]mani.DiscoveredRepo(nil), a.repos...)
	conc := a.cfg.Concurrency
	ctx, cmdID := a.startCommand("正在切回默认分支并拉取更新...")
	go func() {
		res := gitops.SwitchToMainAll(ctx, repos, conc)
		okCnt, skipCnt, failCnt := countOpResults(res)
		a.runUI(func() {
			if !a.finishCommand(cmdID) {
				return
			}
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
	repos := append([]mani.DiscoveredRepo(nil), a.repos...)
	conc := a.cfg.Concurrency
	ctx, cmdID := a.startCommand("正在切换分支至 " + targetBranch + "...")
	go func() {
		res := gitops.SwitchBranchAll(ctx, repos, targetBranch, createIfMissing, conc)
		okCnt, skipCnt, failCnt := countOpResults(res)
		a.runUI(func() {
			if !a.finishCommand(cmdID) {
				return
			}
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
			if a.fileInActiveChangeList(f) && a.checkedChanges[f.Key()] {
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
				a.collapsedStagedSection = false
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

	conc := a.cfg.Concurrency
	history := a.historyStore()
	ctx, cmdID := a.startCommand(fmt.Sprintf("正在提交 %d 个仓库...", len(targets)))
	go func() {
		res := gitops.BatchCommit(ctx, targets, msg, pushAfter, conc)
		if history != nil {
			for _, target := range targets {
				for _, file := range target.Files {
					_ = history.RecordFile(target.Repo.ID, target.Repo.AbsPath, file, "提交")
				}
			}
		}
		okCnt, _, failCnt := countOpResults(res)
		a.runUI(func() {
			if !a.finishCommand(cmdID) {
				return
			}
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
	conc := a.cfg.Concurrency
	ctx, cmdID := a.startCommand(fmt.Sprintf("正在推送 %d 个仓库...", len(aheadRepos)))
	go func() {
		res := gitops.BatchPush(ctx, aheadRepos, conc)
		okCnt, _, failCnt := countOpResults(res)
		a.runUI(func() {
			if !a.finishCommand(cmdID) {
				return
			}
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

// maniAvailable reports whether the Mani tool window should be shown.
func (a *App) maniAvailable() bool {
	return a.cfg.ManiEnabled && a.maniCfg != nil && len(a.maniCfg.Tasks) > 0
}

// maniExtraEnv carries the optional B and TARGET task parameters.
func (a *App) maniExtraEnv() map[string]string {
	extra := map[string]string{}
	if a.maniParamBranch != "" {
		extra["B"] = a.maniParamBranch
	}
	if a.maniParamTarget != "" {
		extra["TARGET"] = a.maniParamTarget
	}
	return extra
}

// TriggerManiTask runs the selected mani.yaml task across all projects in parallel.
func (a *App) TriggerManiTask(taskName string, extraEnv map[string]string) {
	if extraEnv == nil {
		extraEnv = a.maniExtraEnv()
	}
	a.runManiTask(taskName, extraEnv, nil, "")
}

// TriggerManiTaskForRepo runs a task only in one project directory.
func (a *App) TriggerManiTaskForRepo(taskName, repoID string) {
	var target mani.DiscoveredRepo
	found := false
	for _, r := range a.repos {
		if r.ID == repoID {
			target = r
			found = true
			break
		}
	}
	if !found {
		return
	}
	a.runManiTask(taskName, a.maniExtraEnv(), []mani.DiscoveredRepo{target}, target.Name)
}

func (a *App) runManiTask(taskName string, extraEnv map[string]string, targets []mani.DiscoveredRepo, scope string) {
	if a.maniCfg == nil || a.maniTaskRunning {
		return
	}
	spec, ok := a.maniCfg.Tasks[taskName]
	if !ok {
		return
	}
	a.selectedManiTask = taskName
	a.maniLastTask = taskName
	a.maniLastScope = scope
	a.maniTaskRunning = true
	if !a.showManiPanel {
		a.busyLabel = "正在执行脚本: " + taskName + "..."
	}
	wsDir := a.cfg.WorkspacePath
	repos := append([]mani.DiscoveredRepo(nil), a.repos...)
	conc := a.cfg.Concurrency

	go func() {
		ctx := context.Background()
		var res []mani.TaskResult
		if len(targets) > 0 {
			res = mani.RunTaskOn(ctx, spec, targets, extraEnv, conc)
		} else {
			res = mani.RunTask(ctx, wsDir, taskName, spec, repos, extraEnv, conc)
		}
		a.runUI(func() {
			a.maniTaskRunning = false
			a.busyLabel = ""
			a.maniResults = res
			if !a.showManiPanel {
				where := fmt.Sprintf("%d 个仓库", len(res))
				if scope != "" {
					where = scope
				}
				a.showToast("ok", fmt.Sprintf("脚本 %s 执行完成（%s）", taskName, where))
			}
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

// SetZoom updates the UI scaling factor and persists it.
func (a *App) SetZoom(z float32) {
	if z < 0.7 {
		z = 0.7
	}
	if z > 2.0 {
		z = 2.0
	}
	// Round to nearest 5% to prevent float rounding errors
	z = float32(math.Round(float64(z)*20)) / 20
	if a.cfg.Zoom == z {
		return
	}
	a.cfg.Zoom = z
	_ = a.cfg.Save()
	a.showToast("info", fmt.Sprintf("界面缩放：%.0f%%", z*100))
}

// ZoomIn increments the UI scaling factor by 10%.
func (a *App) ZoomIn() {
	a.SetZoom(a.cfg.Zoom + 0.1)
}

// ZoomOut decrements the UI scaling factor by 10%.
func (a *App) ZoomOut() {
	a.SetZoom(a.cfg.Zoom - 0.1)
}

// ZoomReset restores the UI scaling factor to 100%.
func (a *App) ZoomReset() {
	a.SetZoom(1.0)
}
