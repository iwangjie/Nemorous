package main

import (
	"fmt"
	"sort"
	"strings"
	"time"

	"github.com/egoist/mygo/ui"
	"github.com/iwangjie/nemorous/internal/gitops"
)

// branchNode is one level of a branch name hierarchy: "feature/aaa/login"
// becomes feature → aaa → login. Siblings keep the most recently committed
// branch first, and every group carries the newest commit time of its subtree,
// so active work always floats to the top of the dropdown.
type branchNode struct {
	name     string
	full     string // full branch name when this node is a branch itself
	leaf     bool
	mine     bool // the branch itself was last committed by the local user
	tier     int  // best picker tier in this subtree: 0 mine, 1 others recent, 2 rest
	recent   int64
	children []*branchNode
}

// branchPickerTier is the order every branch selector uses: my recent
// branches first, then branches others committed to recently, then the rest.
func branchPickerTier(bi gitops.BranchInfo, now time.Time) int {
	if !bi.IsRecent(now) {
		return 2
	}
	if bi.Mine {
		return 0
	}
	return 1
}

// sortBranchesForPicker sorts a flat candidate list by picker tier, then by
// the newest commit in each tier.
func sortBranchesForPicker(branches []gitops.BranchInfo) {
	now := time.Now()
	sort.SliceStable(branches, func(i, j int) bool {
		ti, tj := branchPickerTier(branches[i], now), branchPickerTier(branches[j], now)
		if ti != tj {
			return ti < tj
		}
		if branches[i].CommitUnix != branches[j].CommitUnix {
			return branches[i].CommitUnix > branches[j].CommitUnix
		}
		return branches[i].Name < branches[j].Name
	})
}

// buildBranchTree deduplicates branch names and arranges them by "/" segments.
// Remote-tracking branches are skipped: a menu picks a local branch to check out.
func buildBranchTree(infos []gitops.BranchInfo) *branchNode {
	latest := make(map[string]gitops.BranchInfo)
	for _, bi := range infos {
		name := strings.TrimSpace(bi.Name)
		if bi.IsRemote || name == "" || name == "HEAD" {
			continue
		}
		if old, ok := latest[name]; !ok || bi.CommitUnix > old.CommitUnix {
			latest[name] = bi
		}
	}

	ordered := make([]gitops.BranchInfo, 0, len(latest))
	for _, bi := range latest {
		ordered = append(ordered, bi)
	}
	sortBranchesForPicker(ordered)

	now := time.Now()
	root := &branchNode{tier: 3}
	for _, bi := range ordered {
		name := bi.Name
		segs := nonEmptySegments(name)
		if len(segs) == 0 {
			continue
		}
		tier := branchPickerTier(bi, now)
		cur := root
		for i, seg := range segs {
			child := findBranchChild(cur, seg)
			if child == nil {
				child = &branchNode{name: seg, tier: 3}
				cur.children = append(cur.children, child)
			}
			if tier < child.tier {
				child.tier = tier
			}
			if bi.CommitUnix > child.recent {
				child.recent = bi.CommitUnix
			}
			if i == len(segs)-1 {
				child.leaf = true
				child.full = name
				child.mine = bi.Mine
			}
			cur = child
		}
	}
	sortBranchNode(root)
	return root
}

func nonEmptySegments(name string) []string {
	parts := strings.Split(name, "/")
	out := make([]string, 0, len(parts))
	for _, part := range parts {
		if part != "" {
			out = append(out, part)
		}
	}
	return out
}

func findBranchChild(n *branchNode, name string) *branchNode {
	for _, child := range n.children {
		if child.name == name {
			return child
		}
	}
	return nil
}

func sortBranchNode(n *branchNode) {
	sort.Slice(n.children, func(i, j int) bool {
		a, b := n.children[i], n.children[j]
		if a.tier != b.tier {
			return a.tier < b.tier
		}
		if a.recent != b.recent {
			return a.recent > b.recent
		}
		return a.name < b.name
	})
	for _, child := range n.children {
		sortBranchNode(child)
	}
}

// closeBranchPicker resets and hides the branch popover.
func (a *App) closeBranchPicker() {
	a.showBranchPicker = false
	a.branchPickerQuery = ""
	a.branchPickerRepoID = ""
}

// repoBranchInfos returns one local BranchInfo per branch name for a repository.
func (a *App) repoBranchInfos(repoID string) []gitops.BranchInfo {
	st, ok := a.statusByID[repoID]
	if !ok {
		return nil
	}
	seen := make(map[string]bool)
	out := make([]gitops.BranchInfo, 0, len(st.BranchInfos))
	for _, bi := range st.BranchInfos {
		if bi.IsRemote || seen[bi.Name] {
			continue
		}
		seen[bi.Name] = true
		out = append(out, bi)
	}
	sortBranchesForPicker(out)
	return out
}

// pickBranch switches the workspace (or one repository) to name and closes the
// picker.
func (a *App) pickBranch(name string) {
	name = strings.TrimSpace(name)
	if name == "" {
		return
	}
	repoID := a.branchPickerRepoID
	a.closeBranchPicker()
	if repoID != "" {
		if st, ok := a.statusByID[repoID]; ok {
			a.switchRepoBranch(st.Repo, name)
		}
		return
	}
	a.TriggerBatchSwitch(name, false)
}

func (a *App) firstBranchMatch(query string) string {
	query = strings.TrimSpace(query)
	if query == "" {
		return ""
	}
	branches := a.WorkspaceBranches()
	if a.branchPickerRepoID != "" {
		branches = a.repoBranchInfos(a.branchPickerRepoID)
	}
	matches := fuzzyMatchBranches(branches, query, 1)
	if len(matches) == 0 {
		return ""
	}
	return matches[0].Name
}

// buildBranchPicker opens an IDEA-style branch popover below anchor: search,
// quick actions, repositories and recent branches, without the flat branch wall.
func (a *App) buildBranchPicker(c *ui.Context, p Palette, anchor ui.Element, current string) {
	shadow := ui.RGBA(0, 0, 0, 0.16)
	if p.IsDark {
		shadow = ui.RGBA(0, 0, 0, 0.44)
	}
	ui.PopoverBase(c, anchor, &a.showBranchPicker, func(panel ui.Element) {
		panel.Width(420).Padding(8).Gap(6).Radius(10).Background(p.Surface).Border(1, p.Border).Shadow(0, 8, 24, 0, shadow)
		panel.Children(func() {
			query := strings.TrimSpace(a.branchPickerQuery)
			ui.Row(c).Gap(6).AlignItems(ui.Center).Children(func() {
				input := ui.SearchField(c.Key("branch-picker-q"), &a.branchPickerQuery).Bind(&a.branchPickerHandle).
					Placeholder("搜索分支、操作、仓库…").AutoFocus().Grow(1)
				if input.Submitted() {
					a.pickBranch(a.firstBranchMatch(query))
				}
				fetch := ui.Box(c).Size(28, 28).Radius(6).Center().Shrink(0).Cursor(ui.CursorPointer).
					Background(p.Surface2).Tooltip("获取远端分支 (Fetch)")
				if fetch.Hovered() {
					fetch.Background(p.SurfaceHover)
				}
				if fetch.Clicked() {
					a.TriggerBatchFetch()
				}
				fetch.Children(func() {
					drawIcon(c, icoFetch, 14, p.TextMuted)
				})
			})

			ui.Scroll(c).MaxHeight(500).Gap(1).Children(func() {
				switch {
				case query != "":
					a.renderBranchPickerSearch(c, p, query, current)
				case a.branchPickerRepoID != "":
					a.renderBranchPickerRepo(c, p, current)
				default:
					a.renderBranchPickerHome(c, p, current)
				}
			})
		})
	})
	if !a.showBranchPicker {
		a.branchPickerRepoID = ""
		a.branchPickerQuery = ""
	}
}

func (a *App) branchPickerHeading(c *ui.Context, p Palette, label string, count int) {
	ui.Row(c).Padding(6, 8, 2, 8).Gap(6).AlignItems(ui.Center).Children(func() {
		ui.Text(c, label).FontSize(10.5).FontWeight(weightSemibold).TextColor(p.TextMuted)
		if count > 0 {
			ui.Text(c, fmt.Sprintf("%d", count)).FontSize(10).TextColor(p.TextMuted)
		}
	})
}

func (a *App) branchPickerRow(c *ui.Context, p Palette, key, name, age string, current, mine, withMe bool) {
	row := ui.Row(c.Key("bp-"+key+"-"+name)).Height(30).Padding(0, 8).Gap(8).Radius(5).Cursor(ui.CursorPointer).AlignItems(ui.Center).Tooltip(name)
	if row.Hovered() {
		row.Background(p.RailBg)
	}
	if row.Clicked() {
		a.pickBranch(name)
	}
	row.Children(func() {
		check := ui.Box(c).Size(12, 12).Shrink(0)
		if current {
			check.Children(func() {
				drawIcon(c, icoCheck, 12, p.Accent)
			})
		}
		ui.Text(c, name).FontSize(12).FontWeight(weightMedium).TextColor(p.TextPrimary).SingleLine().Grow(1)
		if withMe && mine {
			badgeBox(c, "me", p.Accent, p.AccentSoft, ui.Color{}, 9.5, true)
		}
		if age != "" {
			ui.Text(c, age).FontSize(10.5).TextColor(p.TextMuted).SingleLine()
		}
	})
}

func (a *App) branchPickerAction(c *ui.Context, p Palette, key, label string, icon *ui.SVG, shortcut string, onClick func()) {
	row := ui.Row(c.Key("bpa-"+key)).Height(30).Padding(0, 8).Gap(8).Radius(5).Cursor(ui.CursorPointer).AlignItems(ui.Center)
	if row.Hovered() {
		row.Background(p.RailBg)
	}
	if row.Clicked() {
		a.closeBranchPicker()
		onClick()
	}
	row.Children(func() {
		drawIcon(c, icon, 13, p.TextMuted)
		ui.Text(c, label).FontSize(12).TextColor(p.TextPrimary).Grow(1).SingleLine()
		if shortcut != "" {
			ui.Text(c, shortcut).FontSize(10.5).TextColor(p.TextMuted).SingleLine()
		}
	})
}

func (a *App) branchPickerHomeActions(c *ui.Context, p Palette, query string) int {
	actions := []struct {
		key, label, shortcut string
		icon                 *ui.SVG
		run                  func()
	}{
		{"pull", "拉取全部仓库", "", icoPull, a.TriggerBatchPull},
		{"push", "推送待推送仓库", "", icoPush, a.TriggerPushAllAhead},
		{"switch", "切换或新建分支…", "⌘B", icoBranch, a.openBranchSwitchModal},
		{"refresh", "刷新仓库状态", "⌘R", icoRefresh, a.RefreshAsync},
		{"default", "全部切回默认分支", "", icoHome, a.TriggerSwitchToMain},
	}
	shown := 0
	for _, act := range actions {
		if query != "" && !strings.Contains(strings.ToLower(act.label), strings.ToLower(query)) {
			continue
		}
		shown++
		actCopy := act
		a.branchPickerAction(c, p, actCopy.key, actCopy.label, actCopy.icon, actCopy.shortcut, actCopy.run)
	}
	return shown
}

func (a *App) renderBranchPickerHome(c *ui.Context, p Palette, current string) {
	if a.branchPickerHomeActions(c, p, "") > 0 {
		ui.Divider(c)
	}

	a.branchPickerHeading(c, p, "仓库", len(a.statuses))
	for _, st := range a.statuses {
		stCopy := st
		row := ui.Row(c.Key("bpr-"+stCopy.Repo.ID)).Height(30).Padding(0, 8).Gap(8).Radius(5).Cursor(ui.CursorPointer).AlignItems(ui.Center)
		if row.Hovered() {
			row.Background(p.RailBg)
		}
		if row.Clicked() {
			a.branchPickerRepoID = stCopy.Repo.ID
			a.branchPickerQuery = ""
		}
		row.Children(func() {
			badge := repoBadgeColors(stCopy.Repo.ColorIndex, p.IsDark)
			ui.Box(c).Size(8, 8).Radius(4).Background(badge.Text).Shrink(0)
			ui.Text(c, stCopy.Repo.Name).FontSize(12).TextColor(p.TextPrimary).Grow(1).SingleLine()
			ui.Text(c, stCopy.Branch).FontSize(11).TextColor(p.TextMuted).SingleLine()
			drawIcon(c, icoChevronRight, 10, p.TextMuted)
		})
	}

	now := time.Now()
	var mine, others []gitops.BranchInfo
	for _, bi := range a.WorkspaceBranches() {
		if !bi.IsRecent(now) {
			continue
		}
		if bi.Mine {
			if len(mine) < 5 {
				mine = append(mine, bi)
			}
		} else if len(others) < 5 {
			others = append(others, bi)
		}
	}
	if len(mine) > 0 || len(others) > 0 {
		ui.Divider(c)
	}
	if len(mine) > 0 {
		a.branchPickerHeading(c, p, "我的最近提交", 0)
		for _, bi := range mine {
			a.branchPickerRow(c, p, "mine", bi.Name, branchAge(bi.CommitUnix), current == bi.Name, true, false)
		}
	}
	if len(others) > 0 {
		a.branchPickerHeading(c, p, "其他人的最近提交", 0)
		for _, bi := range others {
			a.branchPickerRow(c, p, "other", bi.Name, branchAge(bi.CommitUnix), current == bi.Name, false, false)
		}
	}

	ui.Divider(c)
	a.branchPickerAction(c, p, "all", "全部分支与分支组合…", icoBranch, "", a.openBranchSwitchModal)
}

func (a *App) renderBranchPickerRepo(c *ui.Context, p Palette, current string) {
	repoID := a.branchPickerRepoID
	st, ok := a.statusByID[repoID]
	if !ok {
		a.branchPickerRepoID = ""
		a.renderBranchPickerHome(c, p, current)
		return
	}
	checkBranch := st.Branch
	if checkBranch == "" {
		checkBranch = current
	}

	back := ui.Row(c.Key("bpr-back")).Height(28).Padding(0, 8).Gap(6).Radius(5).Cursor(ui.CursorPointer).AlignItems(ui.Center)
	if back.Hovered() {
		back.Background(p.RailBg)
	}
	if back.Clicked() {
		a.branchPickerRepoID = ""
		a.branchPickerQuery = ""
	}
	back.Children(func() {
		ui.Text(c, "← 全部仓库").FontSize(11).TextColor(p.TextMuted).SingleLine()
	})

	badge := repoBadgeColors(st.Repo.ColorIndex, p.IsDark)
	ui.Row(c).Padding(4, 8).Gap(8).AlignItems(ui.Center).Children(func() {
		ui.Box(c).Size(8, 8).Radius(4).Background(badge.Text).Shrink(0)
		ui.Text(c, st.Repo.Name).FontSize(12.5).FontWeight(weightSemibold).TextColor(p.TextPrimary).Grow(1)
		ui.Text(c, st.Branch).FontSize(11).TextColor(p.TextMuted)
	})

	branches := a.repoBranchInfos(repoID)
	a.branchPickerHeading(c, p, "本地分支", len(branches))
	if len(branches) == 0 {
		ui.Text(c, "没有本地分支").FontSize(11.5).TextColor(p.TextMuted).Padding(8)
		return
	}
	for _, bi := range branches {
		a.branchPickerRow(c, p, "repo", bi.Name, branchAge(bi.CommitUnix), checkBranch == bi.Name, bi.Mine, true)
	}
}

func (a *App) renderBranchPickerSearch(c *ui.Context, p Palette, query, current string) {
	key := strings.ToLower(query)
	shownAny := false

	if n := a.branchPickerHomeActions(c, p, query); n > 0 {
		shownAny = true
	}

	var repos []gitops.RepoStatus
	for _, st := range a.statuses {
		if strings.Contains(strings.ToLower(st.Repo.Name), key) {
			repos = append(repos, st)
		}
	}
	if len(repos) > 0 {
		if shownAny {
			ui.Divider(c)
		}
		shownAny = true
		a.branchPickerHeading(c, p, "仓库", len(repos))
		for _, st := range repos {
			stCopy := st
			row := ui.Row(c.Key("bpsr-"+stCopy.Repo.ID)).Height(30).Padding(0, 8).Gap(8).Radius(5).Cursor(ui.CursorPointer).AlignItems(ui.Center)
			if row.Hovered() {
				row.Background(p.RailBg)
			}
			if row.Clicked() {
				a.branchPickerRepoID = stCopy.Repo.ID
				a.branchPickerQuery = ""
			}
			row.Children(func() {
				badge := repoBadgeColors(stCopy.Repo.ColorIndex, p.IsDark)
				ui.Box(c).Size(8, 8).Radius(4).Background(badge.Text).Shrink(0)
				ui.Text(c, stCopy.Repo.Name).FontSize(12).TextColor(p.TextPrimary).Grow(1).SingleLine()
				ui.Text(c, stCopy.Branch).FontSize(11).TextColor(p.TextMuted).SingleLine()
				drawIcon(c, icoChevronRight, 10, p.TextMuted)
			})
		}
	}

	branches := a.WorkspaceBranches()
	checkBranch := current
	if a.branchPickerRepoID != "" {
		branches = a.repoBranchInfos(a.branchPickerRepoID)
		if st, ok := a.statusByID[a.branchPickerRepoID]; ok {
			checkBranch = st.Branch
		}
	}
	matches := fuzzyMatchBranches(branches, query, 20)
	if len(matches) > 0 {
		if shownAny {
			ui.Divider(c)
		}
		shownAny = true
		a.branchPickerHeading(c, p, "分支", len(matches))
		for _, bi := range matches {
			a.branchPickerRow(c, p, "search", bi.Name, branchAge(bi.CommitUnix), checkBranch == bi.Name, bi.Mine, true)
		}
	}

	if !shownAny {
		ui.Text(c, "没有匹配的分支、操作或仓库").FontSize(11.5).TextColor(p.TextMuted).Padding(10)
	}
}

// renderBranchMenu fills a menu with the hierarchy. A node that is a branch and
// also a namespace ("release" plus "release/2.0") gets both an item and a
// submenu, the submenu labeled with a trailing slash.
func renderBranchMenu(m *ui.Menu, n *branchNode, current string, coverage func(string) string, onPick func(string)) {
	for _, child := range n.children {
		if child.leaf {
			label := child.name
			if child.mine {
				label += "  ·  me"
			}
			if coverage != nil {
				if cov := coverage(child.full); cov != "" {
					label += "  ·  " + cov
				}
			}
			if age := branchAge(child.recent); age != "" {
				label += "  ·  " + age
			}
			item := m.Item(label)
			item.Checked(current == child.full)
			if item.Chosen() {
				onPick(child.full)
			}
		}
		if len(child.children) > 0 {
			m.Submenu(child.name+"/", func(sub *ui.Menu) {
				renderBranchMenu(sub, child, current, coverage, onPick)
			})
		}
	}
}

// branchAge renders a compact Chinese relative time for a branch's last commit.
func branchAge(unix int64) string {
	if unix <= 0 {
		return ""
	}
	age := time.Since(time.Unix(unix, 0))
	switch {
	case age < time.Minute:
		return "刚刚"
	case age < time.Hour:
		return fmt.Sprintf("%d 分钟前", int(age.Minutes()))
	case age < 24*time.Hour:
		return fmt.Sprintf("%d 小时前", int(age.Hours()))
	case age < 30*24*time.Hour:
		return fmt.Sprintf("%d 天前", int(age.Hours()/24))
	case age < 365*24*time.Hour:
		return fmt.Sprintf("%d 个月前", int(age.Hours()/24/30))
	default:
		return fmt.Sprintf("%d 年前", int(age.Hours()/24/365))
	}
}

// branchMenuPill is a pill that opens a branch hierarchy on click instead of a
// flat wall of branch buttons.
func branchMenuPill(c *ui.Context, p Palette, label string, active bool, build func(m *ui.Menu)) {
	bg := p.Surface2
	fg := p.TextPrimary
	if active {
		bg = p.AccentSoft
		fg = p.Accent
	}
	el := ui.Row(c).Padding(5, 10).Gap(5).Radius(6).Background(bg).Cursor(ui.CursorPointer).AlignItems(ui.Center).Shrink(0)
	if el.Hovered() {
		el.Background(p.SurfaceHover)
	}
	el.PopoverMenuButton(build)
	el.Children(func() {
		drawIcon(c, icoBranch, 12, p.Accent)
		ui.Text(c, label).FontSize(12).FontWeight(weightMedium).TextColor(fg).SingleLine()
		drawIcon(c, icoChevronDown, 11, p.TextMuted)
	})
}
