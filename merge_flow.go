package main

import (
	"context"
	"fmt"
	"strings"
	"sync"
	"time"

	"github.com/iwangjie/nemorous/internal/aimerge"
	"github.com/iwangjie/nemorous/internal/config"
	"github.com/iwangjie/nemorous/internal/gitops"
	"github.com/iwangjie/nemorous/internal/mani"
)

// mergeRepoChecked reports whether a repository is included in the merge batch.
func (a *App) mergeRepoChecked(repoID string) bool {
	checked, ok := a.mergeCheckedRepos[repoID]
	if !ok {
		return true
	}
	return checked
}

// syncMergeSelection keeps the merge selection aligned with a fresh scan.
func (a *App) syncMergeSelection(targets []aimerge.MergeTarget) {
	keep := make(map[string]bool, len(targets))
	for _, target := range targets {
		keep[target.Repo.ID] = true
		_, hasManual := a.manualMerges[target.Repo.ID]
		mergeable := target.TargetExists && !target.Detached &&
			target.SourceBranch != target.TargetBranch && target.AheadCommits > 0 && !hasManual
		if !mergeable {
			a.mergeCheckedRepos[target.Repo.ID] = false
			continue
		}
		if _, ok := a.mergeCheckedRepos[target.Repo.ID]; !ok {
			a.mergeCheckedRepos[target.Repo.ID] = true
		}
	}
	for id := range a.mergeCheckedRepos {
		if !keep[id] {
			delete(a.mergeCheckedRepos, id)
		}
	}
}

// TriggerMergeScan reads each repository's merge direction without simulating a merge.
func (a *App) TriggerMergeScan() {
	if a.mergeScanLoading {
		return
	}
	branch := strings.TrimSpace(a.mergeTargetBranch)
	if branch == "" {
		a.showToast("warn", "请输入目标分支（如 test 或 main）")
		return
	}
	repos := a.reposSnapshot()
	if len(repos) == 0 {
		return
	}
	a.mergeScanLoading = true
	a.mergeScanAt = time.Now()
	conc := a.cfg.Concurrency
	go func() {
		ctx := context.Background()
		results := make([]aimerge.MergeTarget, len(repos))
		sem := make(chan struct{}, max(conc, 1))
		var wg sync.WaitGroup
		for i, repo := range repos {
			wg.Add(1)
			go func(idx int, r mani.DiscoveredRepo) {
				defer wg.Done()
				sem <- struct{}{}
				defer func() { <-sem }()
				results[idx] = aimerge.InspectMergeTarget(ctx, r, branch)
			}(i, repo)
		}
		wg.Wait()
		a.runUI(func() {
			a.mergeScanLoading = false
			a.mergeTargets = results
			a.syncMergeSelection(results)
			if a.selectedMergeRepoID == "" && len(results) > 0 {
				a.selectedMergeRepoID = results[0].Repo.ID
			}
		})
	}()
}

// mergeTargetByID returns the current scan entry for a repository.
func (a *App) mergeTargetByID(repoID string) *aimerge.MergeTarget {
	for i := range a.mergeTargets {
		if a.mergeTargets[i].Repo.ID == repoID {
			return &a.mergeTargets[i]
		}
	}
	return nil
}

// TriggerExecuteMerges runs every checked merge directly, in isolated worktrees.
func (a *App) TriggerExecuteMerges(pushRemote bool) {
	var targets []aimerge.MergeTarget
	for _, target := range a.mergeTargets {
		if a.mergeRepoChecked(target.Repo.ID) {
			targets = append(targets, target)
		}
	}
	a.executeMergeTargets(targets, pushRemote)
}

// TriggerMergeOne runs a single repository's merge.
func (a *App) TriggerMergeOne(target aimerge.MergeTarget, pushRemote bool) {
	a.executeMergeTargets([]aimerge.MergeTarget{target}, pushRemote)
}

func (a *App) executeMergeTargets(targets []aimerge.MergeTarget, pushRemote bool) {
	if len(targets) == 0 {
		a.showToast("warn", "请先勾选要合并的仓库")
		return
	}
	if a.mergeRunning {
		return
	}
	a.mergeRunning = true
	a.busyLabel = fmt.Sprintf("正在合并 %d 个仓库...", len(targets))
	go func() {
		ctx := context.Background()
		var results []gitops.OpResult
		conflicts := make(map[string]aimerge.MergeOutcome)
		for _, target := range targets {
			if _, exists := a.manualMerges[target.Repo.ID]; exists {
				results = append(results, gitops.OpResult{
					RepoID:   target.Repo.ID,
					RepoName: target.Repo.Name,
					Status:   "SKIP",
					Message:  "已有临时冲突工作区，请先处理或清理",
				})
				continue
			}
			outcome := aimerge.MergeIntoBranch(ctx, target, pushRemote)
			results = append(results, outcome.Result)
			if outcome.Result.Status == "CONFLICT" {
				conflicts[target.Repo.ID] = outcome
			}
		}
		a.runUI(func() {
			a.mergeRunning = false
			a.busyLabel = ""
			a.setBatchResults(results)
			for _, target := range targets {
				outcome, ok := conflicts[target.Repo.ID]
				if !ok {
					continue
				}
				a.manualMerges[target.Repo.ID] = ManualMerge{
					RepoPath:  target.Repo.AbsPath,
					Path:      outcome.Worktree,
					Branch:    outcome.Branch,
					Conflicts: outcome.Conflicts,
				}
				a.mergeCheckedRepos[target.Repo.ID] = false
			}
			okCnt, skipCnt, failCnt, conflictCnt := mergeResultCounts(results)
			kind := "ok"
			if failCnt > 0 || conflictCnt > 0 {
				kind = "warn"
			}
			a.showToast(kind, fmt.Sprintf("合并完成：%d 成功，%d 冲突，%d 跳过，%d 失败", okCnt, conflictCnt, skipCnt, failCnt))
			a.RefreshAsync()
			a.TriggerMergeScan()
		})
	}()
}

func mergeResultCounts(results []gitops.OpResult) (okCnt, skipCnt, failCnt, conflictCnt int) {
	for _, res := range results {
		switch res.Status {
		case "OK":
			okCnt++
		case "SKIP":
			skipCnt++
		case "CONFLICT":
			conflictCnt++
		default:
			failCnt++
		}
	}
	return
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
	for _, row := range a.mergeTargets {
		if a.mergeRepoChecked(row.Repo.ID) {
			repoIDs = append(repoIDs, row.Repo.ID)
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

// applyMergePreset checks the preset's repositories, sets its target and rescans.
func (a *App) applyMergePreset(preset config.MergePreset) {
	a.mergeTargetBranch = preset.TargetBranch
	for _, st := range a.statuses {
		a.mergeCheckedRepos[st.Repo.ID] = false
	}
	for _, id := range preset.RepoIDs {
		a.mergeCheckedRepos[id] = true
	}
	a.TriggerMergeScan()
}

// planForTarget adapts a scan row for the AI conflict resolver.
func (a *App) planForTarget(target aimerge.MergeTarget) aimerge.RepoMergePlan {
	return aimerge.RepoMergePlan{
		Repo:         target.Repo,
		SourceBranch: target.SourceBranch,
		TargetBranch: target.TargetBranch,
		TargetRef:    target.TargetRef,
	}
}

// finishManualMerge completes a hand-resolved merge and updates the target branch.
func (a *App) finishManualMerge(repoID string, pushRemote bool) {
	mm, ok := a.manualMerges[repoID]
	if !ok {
		return
	}
	target := a.mergeTargetByID(repoID)
	if target == nil {
		a.showToast("warn", "找不到该仓库的合并方向，先刷新状态")
		return
	}
	branch := target.TargetBranch
	go func() {
		res, err := aimerge.FinishManualMerge(context.Background(), mm.RepoPath, mm.Path, mm.Branch, branch, pushRemote)
		a.runUI(func() {
			if err != nil {
				a.showToast("err", err.Error())
				return
			}
			delete(a.manualMerges, repoID)
			a.setBatchResults([]gitops.OpResult{res})
			a.showToast("ok", res.Message)
			a.RefreshAsync()
			a.TriggerMergeScan()
		})
	}()
}

// TriggerReturnToDevelopmentBranches switches merged repositories back to the
// source branch recorded by the merge scan.
func (a *App) TriggerReturnToDevelopmentBranches() {
	var targets []gitops.RepoBranchTarget
	for _, st := range a.statuses {
		target := ""
		for _, row := range a.mergeTargets {
			if row.Repo.ID != st.Repo.ID {
				continue
			}
			if row.SourceBranch != "" && row.SourceBranch != row.TargetBranch {
				target = row.SourceBranch
			}
			break
		}
		if target == "" || target == st.Branch {
			continue
		}
		targets = append(targets, gitops.RepoBranchTarget{Repo: st.Repo, TargetBranch: target})
	}
	if len(targets) == 0 {
		if len(a.mergeTargets) == 0 {
			a.showToast("warn", "先打开合并视图读取各仓库的合并方向")
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
			a.setBatchResults(res)
			a.showToast("ok", fmt.Sprintf("已切回开发分支：%d 成功，%d 跳过，%d 失败", okCnt, skipCnt, failCnt))
			a.RefreshAsync()
		})
	}()
}

// mergeableTarget reports whether a scan row has commits to merge.
func mergeableTarget(target aimerge.MergeTarget) bool {
	return target.TargetExists && !target.Detached &&
		target.SourceBranch != target.TargetBranch && target.AheadCommits > 0
}

// triggerAIForTarget hands a repository to the AI conflict resolver.
func (a *App) triggerAIForTarget(target aimerge.MergeTarget) {
	if _, ok := a.manualMerges[target.Repo.ID]; ok {
		a.showToast("warn", "先清理临时冲突工作区，再交给 AI 合并")
		return
	}
	if target.Dirty {
		a.showToast("warn", "先提交或贮藏改动：AI 合并需要干净的工作区")
		return
	}
	if !mergeableTarget(target) {
		a.showToast("warn", "当前没有可合并的提交")
		return
	}
	a.TriggerAIMergeConflict(a.planForTarget(target))
}
