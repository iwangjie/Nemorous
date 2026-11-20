package main

import (
	"os"
	"path/filepath"
	"strconv"

	"github.com/egoist/mygo/ui"
	"github.com/iwangjie/nemorous/internal/gitops"
)

const copyPathMenu = "复制路径／引用"

func changedWorkspaceFile(cf gitops.ChangedFile) gitops.WorkspaceFile {
	return gitops.WorkspaceFile{RepoID: cf.RepoID, RepoName: cf.RepoName, RepoPath: cf.RepoPath, RelPath: cf.Path}
}

func (a *App) activeReviewFile() (gitops.WorkspaceFile, int) {
	switch a.editorTab {
	case "file":
		if tab := a.activeCodeTab(); tab != nil {
			return tab.file, a.activeCodeEditor().CursorPos().Line + 1
		}
	case "changes":
		if a.selectedChangeFile != nil && a.diffState != nil {
			return changedWorkspaceFile(*a.selectedChangeFile), sourcePosition(a.diffState).Line + 1
		}
	case "commit":
		return a.activeCommitDiffLocation()
	}
	return gitops.WorkspaceFile{}, 0
}

func (a *App) activeCommitDiffLocation() (gitops.WorkspaceFile, int) {
	wf := a.gitDiffWorkspaceFile()
	diff := a.gitCommitDiff
	if wf.RelPath == "" || diff == nil {
		return wf, 0
	}
	return a.commitDiffSideFile(diff.ActiveSide()), diff.CursorPos().Line + 1
}

func (a *App) gitDiffWorkspaceFile() gitops.WorkspaceFile {
	if detail := a.currentCommitDiffDetail(); detail != nil && a.gitDiffPath != "" {
		for _, repo := range a.repos {
			if repo.AbsPath == detail.RepoPath {
				return gitops.WorkspaceFile{RepoID: repo.ID, RepoName: repo.Name, RepoPath: repo.AbsPath, RelPath: a.gitDiffPath, IsRoot: repo.IsRoot}
			}
		}
		return gitops.WorkspaceFile{RepoName: a.gitDiffRepo, RepoPath: detail.RepoPath, RelPath: a.gitDiffPath}
	}
	return gitops.WorkspaceFile{}
}

func (a *App) relativeWorkspacePath(abs string) string {
	if a.cfg.WorkspacePath == "" || abs == "" {
		return ""
	}
	rel, err := filepath.Rel(a.cfg.WorkspacePath, abs)
	if err != nil || !filepath.IsLocal(rel) {
		return ""
	}
	return filepath.ToSlash(rel)
}

func (a *App) copyReference(wf gitops.WorkspaceFile, line int) {
	wf = a.normalizeWorkspaceFile(wf)
	name := a.relativeWorkspacePath(wf.AbsPath)
	if name == "" {
		name = wf.AbsPath
	}
	if name != "" && line > 0 {
		a.pendingClipboard = name + ":" + strconv.Itoa(line)
	}
}

// The standard Copy action follows keyboard focus. A tree row owns it
// while selected, leaving editor and text field copy to their own handlers.
func (a *App) bindTreeCopy(row ui.Element, wf gitops.WorkspaceFile, copyDisplayPath bool) {
	row.Focusable()
	if !row.Focused() {
		return
	}
	if row.Shortcut(ui.Cmd, ui.KeyC) {
		a.pendingClipboard = a.treeCopyText(wf, copyDisplayPath)
	}
	if a.shortcutOn(row, keyCopyAbsolute) {
		a.pendingClipboard = a.normalizeWorkspaceFile(wf).AbsPath
	}
}

func (a *App) treeCopyText(wf gitops.WorkspaceFile, copyDisplayPath bool) string {
	wf = a.normalizeWorkspaceFile(wf)
	if wf.AbsPath == "" {
		return ""
	}
	if copyDisplayPath {
		return displayPath(wf.AbsPath)
	}
	return filepath.Base(wf.AbsPath)
}

func (a *App) fileLocationMenu(m *ui.Menu, wf gitops.WorkspaceFile, directory bool, line int) {
	a.treeLocationMenu(m, wf, directory, line, false)
}

func (a *App) treeLocationMenu(m *ui.Menu, wf gitops.WorkspaceFile, directory bool, line int, copyDisplayPath bool) {
	wf = a.normalizeWorkspaceFile(wf)
	if wf.AbsPath == "" {
		return
	}
	abs := wf.AbsPath
	if m.Item("复制").Chosen() {
		a.pendingClipboard = a.treeCopyText(wf, copyDisplayPath)
	}
	m.Submenu(copyPathMenu, func(sm *ui.Menu) {
		nameLabel := "文件名"
		if directory {
			nameLabel = "目录名"
		}
		if sm.Item(nameLabel).Chosen() {
			a.pendingClipboard = filepath.Base(abs)
		}
		if sm.Item("绝对路径").Chosen() {
			a.pendingClipboard = abs
		}
		if wf.RelPath != "" && sm.Item("相对仓库路径").Chosen() {
			a.pendingClipboard = filepath.ToSlash(wf.RelPath)
		}
		if rel := a.relativeWorkspacePath(abs); rel != "" && sm.Item("相对工作区路径").Chosen() {
			a.pendingClipboard = rel
		}
		if !directory && sm.Item("所在目录").Chosen() {
			a.pendingClipboard = filepath.Dir(abs)
		}
		if line > 0 && sm.Item("文件与行号").Chosen() {
			a.copyReference(wf, line)
		}
	})
	if !directory && wf.RepoID != "" && m.Item("在文件树中定位").Chosen() {
		a.locateFileInTree(wf)
	}
	if m.Item("在文件管理器中显示").Chosen() {
		for target := abs; ; target = filepath.Dir(target) {
			if _, err := os.Stat(target); err == nil {
				if err := revealPath(target); err != nil {
					a.showToast("err", "无法打开文件管理器："+err.Error())
				}
				break
			}
			if target == filepath.Dir(target) {
				break
			}
		}
	}
}

func (a *App) locateFileInTree(wf gitops.WorkspaceFile) {
	if a.selectFileInTree(wf, true) {
		a.activeTab = TabCode
	}
}

func (a *App) selectFileInTree(wf gitops.WorkspaceFile, reportMissing bool) bool {
	wf = a.normalizeWorkspaceFile(wf)
	if _, ok := a.workspacePreviewFileByKey(wf.RepoID, wf.RelPath); !ok {
		st, inWorkspace := a.statusByID[wf.RepoID]
		if !inWorkspace || wf.RepoPath != st.Repo.AbsPath || !filepath.IsLocal(wf.RelPath) || filepath.Join(wf.RepoPath, wf.RelPath) != wf.AbsPath {
			if reportMissing {
				a.showToast("info", "文件不在当前工作区文件树中")
			}
			return false
		}
		if info, err := os.Lstat(wf.AbsPath); err != nil || info.IsDir() {
			if reportMissing {
				a.showToast("info", "当前工作区中没有此文件")
			}
			return false
		}
		wf.IsRoot = st.Repo.IsRoot
		a.codeExtraFile = []gitops.WorkspaceFile{wf}
		a.codeGroupsKey = ""
		clear(a.codeTreeCache)
	}
	a.codeFilterQuery = ""
	a.revealCodeFile(wf)
	a.codeLocatedKey = wf.RepoID + "::" + wf.RelPath
	a.codeRevealKey = a.codeLocatedKey
	return true
}
