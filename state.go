package main

import (
	"context"
	"fmt"
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
	showCleanReposInChanges   bool
	showCommitOptions         bool
	showDiffOptions           bool
	showCodeOptions           bool
	showTopMoreMenu           bool
	showMergeAdvanced         bool
	showMergeCheckDetails     bool
	showRepoDetailDrawer      bool
	showStaleBranchesInModal  bool
	showStaleBranchesInDrawer bool
	collapsedCodeRepos        map[string]bool
	expandedCodeDirs          map[string]bool // "repoID::dirRelPath" -> expanded in file tree

	// Resizable Split Widths (in DIPs)
	changesSplit float32
	codeSplit    float32

	// Discovered Workspace Data
	repos          []mani.DiscoveredRepo
	maniCfg        *mani.Config
	statuses       []gitops.RepoStatus
	statusByID     map[string]gitops.RepoStatus
	workspaceFiles []gitops.WorkspaceFile
	lastRefresh    time.Time
	isRefreshing   bool
	busyLabel      string

	// Changes & Diff View State
	selectedChangeKey  string
	selectedChangeFile *gitops.ChangedFile
	checkedChanges     map[string]bool // ChangeFile.Key() -> selected for batch commit
	diffState          *editor.DiffState
	diffStagedOnly     bool
	diffEditable       bool
	diffModified       bool
	commitMessage      string
	commitScope        string // "checked" (提交所有勾选), "current" (仅提交当前仓库)
	lastBatchResults   []gitops.OpResult

	// Multi-Project Branch Switch & Presets State
	branchInput         string
	branchCreateMissing bool
	branchCheckedRepos  map[string]bool    // repoID -> checked for multi-project branch switch
	branchPerRepoTarget map[string]*string // repoID -> target branch string pointer for per-repo input
	presetNameInput     string             // e.g. "AAA 需求"

	// Branches & AI Merge State
	mergeTargetBranch   string
	mergeFetchFirst     bool
	mergePlans          []aimerge.RepoMergePlan
	mergeRunning        bool
	selectedMergeRepoID string
	mdOpts              markdown.Options
	aiStreamRaw         strings.Builder
	aiVerifyReport      *aimerge.VerificationReport
	aiVerifyRepo        *mani.DiscoveredRepo
	aiAgentName         string
	aiAgentBin          string

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
	pendingDiscard  *PendingDiscard
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
		cfg:                 cfg,
		activeTab:           TabChanges,
		overviewFilter:      "all",
		statusByID:          make(map[string]gitops.RepoStatus),
		checkedChanges:      make(map[string]bool),
		collapsedCodeRepos:  make(map[string]bool),
		expandedCodeDirs:    make(map[string]bool),
		branchCheckedRepos:  make(map[string]bool),
		branchPerRepoTarget: make(map[string]*string),
		changesSplit:        320,
		codeSplit:           290,
		commitScope:         "checked",
		mergeTargetBranch:   cfg.DefaultTargetBranch,
		mergeFetchFirst:     false,
		aiAgentName:         agentName,
		aiAgentBin:          agentBin,
		maniParamTarget:     cfg.DefaultTargetBranch,
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

// LoadWorkspaceSync synchronously discovers repos and inspects status.
func (a *App) LoadWorkspaceSync(wsPath string) {
	wsPath = filepath.Clean(wsPath)
	a.cfg.AddRecentWorkspace(wsPath)
	repos, maniCfg := mani.DiscoverRepos(wsPath, a.cfg.IncludeRootRepo)
	ctx := context.Background()
	statuses := gitops.InspectRepos(ctx, repos, a.cfg.Concurrency)
	wsFiles := gitops.ListWorkspaceFiles(ctx, repos, 2500)

	a.applySnapshot(repos, maniCfg, statuses, wsFiles)
	plans := aimerge.PreflightMergeAll(ctx, repos, a.mergeTargetBranch, false, a.cfg.ProtectedBranches, a.cfg.Concurrency)
	a.mergePlans = plans
	if len(plans) > 0 && a.selectedMergeRepoID == "" {
		a.selectedMergeRepoID = plans[0].Repo.ID
	}
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
	a.isRefreshing = true
	if showBusy {
		a.busyLabel = "正在刷新仓库状态..."
	}
	wsPath := a.cfg.WorkspacePath
	incRoot := a.cfg.IncludeRootRepo
	conc := a.cfg.Concurrency
	targetBranch := a.mergeTargetBranch
	prot := append([]string(nil), a.cfg.ProtectedBranches...)

	go func() {
		ctx := context.Background()
		repos, maniCfg := mani.DiscoverRepos(wsPath, incRoot)
		statuses := gitops.InspectRepos(ctx, repos, conc)
		wsFiles := gitops.ListWorkspaceFiles(ctx, repos, 2500)
		plans := aimerge.PreflightMergeAll(ctx, repos, targetBranch, false, prot, conc)

		a.runUI(func() {
			a.isRefreshing = false
			if showBusy {
				a.busyLabel = ""
			}
			a.applySnapshot(repos, maniCfg, statuses, wsFiles)
			a.mergePlans = plans
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
}

func (a *App) selectChangedFile(cf gitops.ChangedFile) {
	a.selectedChangeKey = cf.Key()
	cfCopy := cf
	a.selectedChangeFile = &cfCopy
	a.diffModified = false

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

// BranchesByActivity aggregates unique branches across repositories, sorted by most recently active first,
// and partitions them into recent (active within 7 days) vs stale (no changes in the last 7 days).
func (a *App) BranchesByActivity() (recent []gitops.BranchInfo, stale []gitops.BranchInfo) {
	byName := make(map[string]gitops.BranchInfo)
	activeCheckedOut := make(map[string]bool)

	for _, st := range a.statuses {
		if st.Branch != "" && !st.Detached {
			activeCheckedOut[st.Branch] = true
		}
		for _, bi := range st.BranchInfos {
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

// RepoBranchesByActivity partitions a single repository's local branches into recent (<=7 days) and stale (>7 days),
// ordered by most recently active first.
func (a *App) RepoBranchesByActivity(st gitops.RepoStatus) (recent []gitops.BranchInfo, stale []gitops.BranchInfo) {
	now := time.Now()
	seen := make(map[string]bool)
	for _, bi := range st.BranchInfos {
		if bi.IsRemote || seen[bi.Name] {
			continue
		}
		seen[bi.Name] = true
		if bi.IsRecent(now) || bi.Name == st.Branch || bi.Name == st.DefaultBranch {
			recent = append(recent, bi)
		} else {
			stale = append(stale, bi)
		}
	}
	return recent, stale
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
				if !seen[bi.Name] {
					seen[bi.Name] = true
					out = append(out, bi)
				}
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
	createMissing := a.branchCreateMissing
	conc := a.cfg.Concurrency
	go func() {
		ctx := context.Background()
		res := gitops.SwitchBranchPlan(ctx, targets, createMissing, conc)
		okCnt, skipCnt, failCnt := countOpResults(res)
		a.runUI(func() {
			a.lastBatchResults = res
			a.showBranchModal = false
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

// TriggerCommit executes either a unified multi-repo commit or single-repo commit, optionally followed by push.
func (a *App) TriggerCommit(pushAfter bool) {
	msg := strings.TrimSpace(a.commitMessage)
	if msg == "" {
		a.showToast("warn", "请填写提交说明")
		return
	}

	var targets []gitops.BatchCommitTarget
	if a.commitScope == "current" && a.selectedChangeFile != nil {
		for _, st := range a.statuses {
			if st.Repo.ID == a.selectedChangeFile.RepoID && len(st.Files) > 0 {
				var files []string
				for _, f := range st.Files {
					if a.checkedChanges[f.Key()] {
						files = append(files, f.Path)
					}
				}
				if len(files) == 0 {
					for _, f := range st.Files {
						files = append(files, f.Path)
					}
				}
				targets = append(targets, gitops.BatchCommitTarget{Repo: st.Repo, Files: files})
				break
			}
		}
	} else {
		for _, st := range a.statuses {
			if a.repoFilterID != "" && st.Repo.ID != a.repoFilterID {
				continue
			}
			var files []string
			for _, f := range st.Files {
				if a.checkedChanges[f.Key()] {
					files = append(files, f.Path)
				}
			}
			if len(files) > 0 {
				targets = append(targets, gitops.BatchCommitTarget{Repo: st.Repo, Files: files})
			}
		}
	}

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
			a.lastBatchResults = res
			if failCnt == 0 {
				a.commitMessage = ""
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
		if p.Kind == aimerge.PlanClean {
			cleanPlans = append(cleanPlans, p)
		}
	}
	if len(cleanPlans) == 0 {
		a.showToast("warn", "当前没有可直接合并的仓库")
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
