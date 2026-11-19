package main

import (
	"context"
	"errors"
	"fmt"
	"os/exec"
	"path/filepath"
	"regexp"
	"strings"
	"time"

	"github.com/egoist/mygo/nemorouseditor"
	"github.com/egoist/mygo/ui"
	"github.com/iwangjie/nemorous/internal/gitops"
)

type replaceAllMatch struct {
	path    string
	version gitops.ReviewVersion
}

type replaceAllPlan struct {
	query, replacement string
	matches            []replaceAllMatch
	total              int
	focus              bool
}

func (a *App) buildReplaceInFilesActions(c *ui.Context, p Palette) {
	hit := a.globalSearchSelected
	ready := hit != nil && !a.globalSearchRunning && !a.globalReplaceRunning && a.globalReplacePlan == nil
	if hit != nil {
		ui.Text(c, fmt.Sprintf("%s:%d", hit.RelPath, hit.Line)).FontSize(11).TextColor(p.TextMuted).SingleLine()
		ui.Text(c, hit.lineText).Font("SF Mono, Menlo, monospace").FontSize(11).SingleLine().TextColor(p.TextSecondary)
		if re, err := globalSearchRegexp(a.globalSearchQuery, a.globalSearchOptions()); err == nil {
			preview, _, _ := replaceSearchText(hit.lineText, re, a.globalSearchReplacement, a.globalSearchRegex, &globalSearchHit{
				Line: 1, Column: hit.Column, lineText: hit.lineText, matchEnd: hit.matchEnd,
			})
			ui.Text(c, preview).Font("SF Mono, Menlo, monospace").FontSize(11).SingleLine().TextColor(p.TextPrimary)
		}
	}
	ui.Row(c).Gap(6).AlignItems(ui.Center).Children(func() {
		ui.Spacer(c).Grow(1)
		miniBtn(c.Key("replace-one"), p, "替换", func() {
			if ready {
				a.replaceSearchResults(false)
			}
		})
		miniBtn(c.Key("replace-all"), p, "全部替换", func() {
			if ready {
				a.prepareReplaceAll()
			}
		})
	})
	if ready && c.Shortcut(ui.Shift|ui.Alt, ui.KeyEnter) {
		a.prepareReplaceAll()
	} else if ready && c.Shortcut(ui.Shift, ui.KeyEnter) {
		a.replaceSearchResults(false)
	}
}

func (a *App) buildReplaceAllConfirmation(c *ui.Context, p Palette) {
	plan := a.globalReplacePlan
	if plan == nil || !a.showGlobalSearch {
		return
	}
	open := true
	ui.DialogBase(c.Key("replace-all-confirmation"), &open, func(backdrop, panel ui.Element) {
		backdrop.Background(ui.RGBA(20, 22, 30, 0.32))
		panel.Width(410).Padding(14).Gap(12).Radius(10).Background(p.Surface).Border(1, p.Border).Focusable()
		if plan.focus {
			panel.Focus()
			plan.focus = false
		}
		if panel.Shortcut(0, ui.KeyEnter) {
			a.replaceSearchResults(true)
		}
		ui.Text(c, "全部替换").FontSize(14).FontWeight(weightSemibold).TextColor(p.TextPrimary)
		ui.Text(c, fmt.Sprintf("将 %d 个文件中的 %d 处 %q 替换为 %q？", len(plan.matches), plan.total, plan.query, plan.replacement)).
			FontSize(11.5).TextColor(p.TextSecondary)
		ui.Row(c).Gap(8).AlignItems(ui.Center).Children(func() {
			ui.Spacer(c)
			miniBtn(c.Key("cancel"), p, "取消", func() { open = false })
			pill(c.Key("confirm"), p, "替换", true, func() { a.replaceSearchResults(true) })
		})
	})
	if !open {
		a.globalReplacePlan = nil
	}
}

func (a *App) prepareReplaceAll() {
	opts := a.globalSearchOptions()
	mask, err := parseSearchFileMask(opts.Mask)
	if err != nil {
		a.globalSearchErr = err.Error()
		return
	}
	query := a.globalSearchQuery
	re, err := globalSearchRegexp(query, opts)
	if err != nil {
		a.globalSearchErr = err.Error()
		return
	}
	plan := &replaceAllPlan{query: query, replacement: a.globalSearchReplacement, focus: true}
	root := filepath.Clean(a.cfg.WorkspacePath)
	scope := root
	if a.globalSearchDirectory != "" {
		scope = a.globalSearchDirectory
	}
	fallback := make([]string, 0, len(a.workspaceFiles))
	for _, file := range a.workspaceFiles {
		if rel, err := filepath.Rel(scope, file.AbsPath); err == nil && filepath.IsLocal(rel) && rel != "." {
			fallback = append(fallback, file.AbsPath)
		}
	}
	seq := a.globalSearchSeq
	a.globalReplaceRunning = true
	go func() {
		ctx, cancel := context.WithTimeout(context.Background(), time.Minute)
		defer cancel()
		seen := map[string]bool{}
		for _, path := range replaceSearchFiles(ctx, scope, fallback) {
			if ctx.Err() != nil {
				break
			}
			path = filepath.Clean(path)
			rel, err := filepath.Rel(root, path)
			if err != nil || rel == "." || !filepath.IsLocal(rel) || seen[path] || !mask.matches(path) {
				continue
			}
			seen[path] = true
			text, err := gitops.ReadReviewWorktree(root, rel)
			if err != nil {
				continue
			}
			_, count, _ := replaceSearchText(text, re, plan.replacement, opts.Regex, nil)
			if count > 0 {
				plan.matches = append(plan.matches, replaceAllMatch{path, gitops.ReviewTextVersion(text)})
				plan.total += count
			}
		}
		a.runUI(func() {
			a.globalReplaceRunning = false
			if seq != a.globalSearchSeq || !a.showGlobalSearch || a.globalSearchReplacement != plan.replacement {
				return
			}
			if ctx.Err() != nil {
				a.globalSearchErr = ctx.Err().Error()
			} else if plan.total > 0 {
				a.globalReplacePlan = plan
			} else {
				a.scheduleGlobalSearch()
			}
		})
	}()
}

type replacedSearchFile struct {
	path          string
	before, after string
}

func (a *App) replaceSearchResults(all bool) {
	if a.globalReplaceRunning || a.globalSearchSelected == nil || a.globalSearchQuery == "" {
		return
	}
	opts := a.globalSearchOptions()
	mask, err := parseSearchFileMask(opts.Mask)
	if err != nil {
		a.globalSearchErr = err.Error()
		return
	}
	re, err := globalSearchRegexp(a.globalSearchQuery, opts)
	if err != nil {
		a.globalSearchErr = err.Error()
		return
	}
	replacement := a.globalSearchReplacement
	hit := *a.globalSearchSelected
	root := filepath.Clean(a.cfg.WorkspacePath)
	plan := a.globalReplacePlan
	a.globalReplacePlan = nil
	if all {
		if plan == nil || plan.replacement != replacement || plan.query != a.globalSearchQuery {
			return
		}
	}
	dirty := map[string]bool{}
	openTabs := map[string]bool{}
	for _, tab := range a.codeTabs {
		if tab.revision == "" {
			openTabs[filepath.Clean(tab.file.AbsPath)] = true
		}
		if tab.dirty() {
			dirty[filepath.Clean(tab.file.AbsPath)] = true
		}
	}
	if a.workingDiffDirty() && a.selectedChangeFile != nil {
		file := a.normalizeWorkspaceFile(changedWorkspaceFile(*a.selectedChangeFile))
		dirty[filepath.Clean(file.AbsPath)] = true
	}
	gen := a.globalSearchSeq
	store := a.historyStore()
	type repoLocation struct{ root, id string }
	var repos []repoLocation
	for _, status := range a.statuses {
		repos = append(repos, repoLocation{filepath.Clean(status.Repo.AbsPath), status.Repo.ID})
	}
	a.globalReplaceRunning = true
	go func() {
		ctx, cancel := context.WithTimeout(context.Background(), time.Minute)
		defer cancel()
		targets := []replaceAllMatch{{path: hit.AbsPath}}
		one := &hit
		if all {
			one = nil
			targets = plan.matches
		}
		var applied []replacedSearchFile
		total := 0
		var runErr error
		for _, target := range targets {
			if ctx.Err() != nil {
				runErr = ctx.Err()
				break
			}
			name := filepath.Clean(target.path)
			if !mask.matches(name) {
				continue
			}
			rel, err := filepath.Rel(root, name)
			if err != nil || !filepath.IsLocal(rel) || rel == "." {
				continue
			}
			before, err := gitops.ReadReviewWorktree(root, rel)
			if err != nil {
				runErr = errors.Join(runErr, err)
				continue
			}
			version := gitops.ReviewTextVersion(before)
			if all && version != target.version {
				runErr = errors.Join(runErr, fmt.Errorf("%s：%w", rel, gitops.ErrReviewFileChanged))
				continue
			}
			after, count, err := replaceSearchText(before, re, replacement, opts.Regex, one)
			if err == nil && count > 0 && after != before {
				if dirty[name] {
					err = fmt.Errorf("%s 有未保存的改动", rel)
				} else {
					err = gitops.SaveReviewText(root, rel, after, version)
				}
				if err == nil {
					total += count
					if openTabs[name] {
						applied = append(applied, replacedSearchFile{name, before, after})
					}
					if store != nil {
						historyPath, repoID, best := rel, "__workspace__", ""
						for _, repo := range repos {
							if len(repo.root) <= len(best) || !strings.HasPrefix(name, repo.root+string(filepath.Separator)) {
								continue
							}
							best, historyPath, repoID = repo.root, strings.TrimPrefix(name, repo.root+string(filepath.Separator)), repo.id
						}
						_ = store.Record(repoID, filepath.ToSlash(historyPath), before, "在文件中替换")
					}
				}
			}
			runErr = errors.Join(runErr, err)
		}
		a.runUI(func() {
			a.globalReplaceRunning = false
			for _, changed := range applied {
				a.applySearchReplacementToOpenTab(changed)
			}
			if total > 0 {
				a.showToast("ok", fmt.Sprintf("已替换 %d 处", total))
				a.RefreshAsync()
			}
			if gen == a.globalSearchSeq && a.showGlobalSearch {
				if all && runErr == nil {
					a.closeGlobalSearch()
				} else {
					a.scheduleGlobalSearch()
				}
				if runErr != nil {
					a.globalSearchErr = runErr.Error()
				}
			} else if runErr != nil {
				a.showToast("err", runErr.Error())
			}
		})
	}()
}

func (a *App) applySearchReplacementToOpenTab(changed replacedSearchFile) {
	for _, tab := range a.codeTabs {
		if tab.revision != "" || filepath.Clean(tab.file.AbsPath) != changed.path || tab.dirty() || tab.state.ReadOnly ||
			tab.diskVersion != gitops.ReviewTextVersion(changed.before) {
			continue
		}
		buffer := tab.state.Buffer
		selection := tab.state.Selection()
		buffer.Edit(editor.Edit{Start: 0, End: buffer.Snapshot().Len(), Text: changed.after})
		buffer.MarkSaved()
		tab.state.SetSelections(selection)
		tab.reloadSeq++
		tab.diskVersion = gitops.ReviewTextVersion(changed.after)
	}
}

func replaceSearchFiles(ctx context.Context, root string, fallback []string) []string {
	cmd := exec.CommandContext(ctx, "rg", "--files", "--null", "--hidden", "--glob=!**/.git/**", ".")
	cmd.Dir = root
	if data, err := cmd.Output(); err == nil {
		files := make([]string, 0, len(fallback))
		for name := range strings.SplitSeq(string(data), "\x00") {
			if name != "" {
				files = append(files, filepath.Join(root, name))
			}
		}
		return files
	}
	return fallback
}

func replaceSearchText(text string, re *regexp.Regexp, replacement string, expand bool, selected *globalSearchHit) (string, int, error) {
	var out strings.Builder
	count := 0
	for index, raw := range strings.Split(text, "\n") {
		if index != 0 {
			out.WriteByte('\n')
		}
		line := strings.TrimSuffix(raw, "\r")
		if selected != nil && selected.Line != index+1 {
			out.WriteString(raw)
			continue
		}
		if selected != nil && selected.lineText != line {
			return "", 0, gitops.ErrReviewFileChanged
		}
		end := 0
		for _, match := range re.FindAllStringSubmatchIndex(line, -1) {
			start, next := match[0], match[1]
			if selected != nil && (selected.Column-1 != start || selected.matchEnd != next) {
				continue
			}
			out.WriteString(line[end:start])
			if expand {
				out.Write(re.ExpandString(nil, replacement, line, match))
			} else {
				out.WriteString(replacement)
			}
			end = next
			count++
		}
		out.WriteString(raw[end:])
	}
	if selected != nil && count != 1 {
		return "", 0, gitops.ErrReviewFileChanged
	}
	return out.String(), count, nil
}
