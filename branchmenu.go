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
	recent   int64
	children []*branchNode
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

	names := make([]string, 0, len(latest))
	for name := range latest {
		names = append(names, name)
	}
	sort.Slice(names, func(i, j int) bool {
		a, b := latest[names[i]], latest[names[j]]
		if a.CommitUnix != b.CommitUnix {
			return a.CommitUnix > b.CommitUnix
		}
		return a.Name < b.Name
	})

	root := &branchNode{}
	for _, name := range names {
		bi := latest[name]
		segs := nonEmptySegments(name)
		if len(segs) == 0 {
			continue
		}
		cur := root
		for i, seg := range segs {
			child := findBranchChild(cur, seg)
			if child == nil {
				child = &branchNode{name: seg}
				cur.children = append(cur.children, child)
			}
			if bi.CommitUnix > child.recent {
				child.recent = bi.CommitUnix
			}
			if i == len(segs)-1 {
				child.leaf = true
				child.full = name
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
		if a.recent != b.recent {
			return a.recent > b.recent
		}
		return a.name < b.name
	})
	for _, child := range n.children {
		sortBranchNode(child)
	}
}

// renderBranchMenu fills a menu with the hierarchy. A node that is a branch and
// also a namespace ("release" plus "release/2.0") gets both an item and a
// submenu, the submenu labeled with a trailing slash.
func renderBranchMenu(m *ui.Menu, n *branchNode, current string, onPick func(string)) {
	for _, child := range n.children {
		if child.leaf {
			label := child.name
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
				renderBranchMenu(sub, child, current, onPick)
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
