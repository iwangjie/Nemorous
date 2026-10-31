package main

import (
	"context"
	"fmt"
	"path/filepath"
	"strings"
	"time"

	"github.com/egoist/mygo"
	"github.com/egoist/mygo/ui"
	"github.com/iwangjie/nemorous/internal/credentials"
	"github.com/iwangjie/nemorous/internal/gitops"
)

func (a *App) openWorkspaceManager() {
	a.showWorkspaceManager = true
	if a.workspaceTab == "" {
		a.workspaceTab = "repos"
	}
	a.wsErr, a.wsNotice, a.wsConfirm = "", "", ""
	a.wsAddMode, a.wsAddCred, a.wsGenKey = "", false, false
	a.wsCredToken = ""
	a.ensureWorkspaceRepo()
	a.loadWorkspaceTab()
}

func (a *App) ensureWorkspaceRepo() {
	if a.wsRepo != "" {
		for i := range a.statuses {
			if a.statuses[i].Repo.ID == a.wsRepo {
				return
			}
		}
	}
	if len(a.statuses) > 0 {
		a.wsRepo = a.statuses[0].Repo.ID
	}
}

func (a *App) wsRepoStatus() *gitops.RepoStatus {
	for i := range a.statuses {
		if a.statuses[i].Repo.ID == a.wsRepo {
			return &a.statuses[i]
		}
	}
	return nil
}

func (a *App) loadWorkspaceTab() {
	a.wsErr = ""
	switch a.workspaceTab {
	case "credentials":
		go func() {
			ctx, cancel := context.WithTimeout(context.Background(), 15*time.Second)
			defer cancel()
			helpers := credentials.CredentialHelpers(ctx)
			keys, _ := credentials.ListSSHKeys(ctx)
			agent, _ := credentials.AgentKeys(ctx)
			a.runUI(func() {
				a.wsCredHelpers = helpers
				a.wsSSHKeys = keys
				a.wsAgentKeys = agent
			})
		}()
	case "storage", "worktrees", "signing":
		st := a.wsRepoStatus()
		if st == nil {
			return
		}
		repoPath := st.Repo.AbsPath
		tab := a.workspaceTab
		go func() {
			ctx, cancel := context.WithTimeout(context.Background(), 30*time.Second)
			defer cancel()
			var lfs gitops.LFSState
			var sparse gitops.SparseState
			var worktrees []gitops.Worktree
			var signing gitops.SigningConfig
			switch tab {
			case "storage":
				lfs = gitops.InspectLFS(ctx, repoPath)
				sparse = gitops.InspectSparse(ctx, repoPath)
			case "worktrees":
				worktrees, _ = gitops.ListWorktrees(ctx, repoPath)
			case "signing":
				signing = gitops.InspectSigning(ctx, repoPath)
			}
			a.runUI(func() {
				switch tab {
				case "storage":
					a.wsLFS = lfs
					a.wsSparse = sparse
					if a.wsSparseInput == "" {
						a.wsSparseInput = strings.Join(sparse.Patterns, "\n")
					}
				case "worktrees":
					a.wsWorktrees = worktrees
				case "signing":
					a.wsSigning = signing
					a.wsSigningEnabled = signing.Enabled
					a.wsSigningFormat = signing.Format
					a.wsSigningKey = signing.Key
				}
			})
		}()
	}
}

// wsRun runs one workspace operation and refreshes the relevant state.
func (a *App) wsRun(fn func(ctx context.Context) (string, error), refresh bool) {
	if a.wsBusy {
		return
	}
	a.wsBusy = true
	a.wsErr, a.wsNotice = "", ""
	go func() {
		ctx, cancel := context.WithTimeout(context.Background(), 20*time.Minute)
		defer cancel()
		msg, err := fn(ctx)
		a.runUI(func() {
			a.wsBusy = false
			if err != nil {
				a.wsErr = err.Error()
				if strings.TrimSpace(msg) != "" {
					a.wsErr = strings.TrimSpace(msg) + "\n" + a.wsErr
				}
				return
			}
			a.wsNotice = msg
			if refresh {
				a.RefreshWorkspaceAsync()
			}
			a.loadWorkspaceTab()
		})
	}()
}

func (a *App) buildWorkspaceModal(c *ui.Context, p Palette) {
	ui.DialogBase(c, &a.showWorkspaceManager, func(backdrop, panel ui.Element) {
		backdrop.Background(ui.RGBA(20, 22, 30, 0.40))
		w, h := float32(990), float32(700)
		if a.win != nil {
			if cw, ch := a.win.ContentSize(); cw > 0 && ch > 0 {
				w = min(w, float32(cw)-60)
				h = min(h, float32(ch)-48)
			}
		}
		panel.Width(w).Height(h).Padding(14).Gap(8).Radius(12).Background(p.Surface).Border(1, p.Border)
		st := a.wsRepoStatus()
		sub := filepath.Base(a.cfg.WorkspacePath)
		if st != nil {
			sub += " · " + st.Repo.Name
		}
		modalHeader(c, p, icoFolder, "工作区", sub, func() {
			toolButton(c, p, "ws-settings", icoSettings, "设置", false, true, func() { a.showSettings = true })
			if a.wsBusy {
				ui.Spinner(c).Size(13, 13).Label("执行中").TextColor(p.TextMuted)
			} else {
				toolButton(c, p, "ws-refresh", icoRefresh, "刷新", false, true, func() {
					a.loadWorkspaceTab()
					a.RefreshWorkspaceAsync()
				})
			}
		}, func() { a.showWorkspaceManager = false })

		ui.Row(c).Grow(1).FillWidth().Gap(10).Children(func() {
			buildModalNav(c, p, 166, []modalNavItem{
				{ID: "repos", Label: "仓库", Group: "工作区", Icon: icoFolder},
				{ID: "credentials", Label: "凭据", Group: "配置", Icon: icoKey},
				{ID: "storage", Label: "LFS / Sparse", Group: "配置", Icon: icoArchive},
				{ID: "worktrees", Label: "Worktree", Group: "配置", Icon: icoBox},
				{ID: "signing", Label: "提交签名", Group: "配置", Icon: icoShield},
			}, a.workspaceTab, func(id string) {
				a.workspaceTab = id
				a.wsConfirm = ""
				a.wsAddMode = ""
				a.loadWorkspaceTab()
			})

			ui.Column(c).Grow(1).FillHeight().Gap(6).Children(func() {
				if a.wsErr != "" {
					ui.Row(c).FillWidth().Padding(5, 7).Gap(6).Radius(4).Background(p.DangerSoft).Children(func() {
						drawIcon(c, icoAlert, 12, p.Danger)
						ui.Text(c, a.wsErr).FontSize(10).TextColor(p.Danger).MaxLines(5)
					})
				}
				if a.wsNotice != "" {
					ui.Row(c).FillWidth().Padding(5, 7).Gap(6).Radius(4).Background(p.GreenSoft).Children(func() {
						drawIcon(c, icoCheck, 12, p.Green)
						ui.Text(c, a.wsNotice).FontSize(10).TextColor(p.Green).MaxLines(4)
					})
				}

				ui.Scroll(c).Grow(1).FillWidth().Children(func() {
					ui.Column(c).FillWidth().Gap(8).Children(func() {
						switch a.workspaceTab {
						case "credentials":
							a.buildWorkspaceCredentials(c, p)
						case "storage":
							a.buildWorkspaceStorage(c, p)
						case "worktrees":
							a.buildWorkspaceWorktrees(c, p)
						case "signing":
							a.buildWorkspaceSigning(c, p)
						default:
							a.buildWorkspaceRepos(c, p)
						}
					})
				})
			})
		})
	})
}

func (a *App) workspaceRepoPicker(c *ui.Context, p Palette) {
	menuIconPill(c, p, "ws-repo-picker", icoFolder, func() string {
		if st := a.wsRepoStatus(); st != nil {
			return st.Repo.Name
		}
		return "选择仓库"
	}(), true, func(m *ui.Menu) {
		for i := range a.statuses {
			st := a.statuses[i]
			id := st.Repo.ID
			if m.Item(st.Repo.Name).Radio(a.wsRepo == id).Chosen() {
				a.wsRepo = id
				a.wsWorktreePath = ""
				a.wsConfirm = ""
				a.loadWorkspaceTab()
			}
		}
	})
}

func (a *App) buildWorkspaceRepos(c *ui.Context, p Palette) {
	ui.Row(c).FillWidth().Gap(6).AlignItems(ui.Center).Children(func() {
		drawIcon(c, icoFolder, 13, p.TextMuted)
		ui.Text(c, a.cfg.WorkspacePath).Font("monospace").FontSize(10).TextColor(p.TextSecondary).Grow(1).MaxLines(1)
		pill(c, p, "更换…", false, func() {
			go func() {
				paths, err := mygo.Dialog.Open(mygo.OpenDialogOptions{Parent: a.win, Title: "选择多仓库工作区目录", Directory: true})
				if err == nil && len(paths) > 0 {
					a.runUI(func() {
						a.LoadWorkspaceAsync(paths[0])
						a.showWorkspaceManager = false
					})
				}
			}()
		})
		menuIconPill(c, p, "ws-add-repo", icoPlus, "添加仓库", false, func(m *ui.Menu) {
			if m.Item("克隆仓库…").Chosen() {
				a.wsAddMode = "clone"
				a.wsErr = ""
			}
			if m.Item("初始化仓库…").Chosen() {
				a.wsAddMode = "init"
				a.wsErr = ""
			}
			if m.Item("导入已有仓库…").Chosen() {
				a.wsAddMode = "import"
				a.wsErr = ""
			}
		})
	})
	if a.wsAddMode != "" {
		a.buildWorkspaceAddForm(c, p)
	}
	ui.Text(c, fmt.Sprintf("%d 个仓库 · 隐藏只从视图移除，不删除文件", len(a.repos))).FontSize(9.5).TextColor(p.TextMuted)
	for _, repo := range a.repos {
		repo := repo
		kc := c.Key("ws-repo-" + repo.ID)
		row := ui.Row(kc).FillWidth().Padding(6, 4).Gap(8).AlignItems(ui.Center).Radius(4)
		if row.Hovered() {
			row.Background(p.SurfaceHover)
		}
		row.Children(func() {
			drawIcon(kc, icoBox, 12, p.TextMuted)
			ui.Text(kc, repo.Name).FontSize(11.5).FontWeight(weightMedium).TextColor(p.TextPrimary).Width(180).MaxLines(1)
			if repo.IsRoot {
				badgeBox(kc, "根仓库", p.AccentSoftText, p.AccentSoft, ui.Color{}, 9.5, false)
			}
			ui.Text(kc, repo.RelPath).Font("monospace").FontSize(9.5).TextColor(p.TextMuted).Grow(1).MaxLines(1)
			if a.wsConfirm == "hide:"+repo.RelPath {
				ui.Text(kc, "隐藏？").FontSize(9.5).TextColor(p.Amber)
				toolButton(kc, p, "ws-hide-yes-"+repo.ID, icoCheck, "确认隐藏", false, true, func() {
					a.wsConfirm = ""
					a.cfg.HideRepo(repo.RelPath)
					_ = a.cfg.Save()
					a.wsNotice = "已隐藏 " + repo.Name
					a.RefreshWorkspaceAsync()
				})
				toolButton(kc, p, "ws-hide-no-"+repo.ID, icoClose, "取消", false, true, func() { a.wsConfirm = "" })
			} else {
				menuIconAction(kc, p, "ws-more-"+repo.ID, icoMore, "更多", false, func(m *ui.Menu) {
					if m.Item("复制相对路径").Chosen() {
						a.pendingClipboard = repo.RelPath
					}
					if !repo.IsRoot {
						if m.Item("从视图隐藏").Chosen() {
							a.wsConfirm = "hide:" + repo.RelPath
						}
					}
				})
			}
		})
		ui.Box(c).Height(1).FillWidth().Background(p.BorderSubtle)
	}
	if len(a.repos) == 0 {
		emptyState(c, p, icoFolder, "没有仓库", "点击「添加仓库」克隆、初始化或导入")
	}
	if len(a.cfg.HiddenRepos) > 0 {
		kc := c.Key("ws-hidden-toggle")
		row := ui.Row(kc).FillWidth().Padding(5, 4).Gap(5).Radius(4).AlignItems(ui.Center).Cursor(ui.CursorPointer)
		if row.Hovered() {
			row.Background(p.SurfaceHover)
		}
		if row.Clicked() {
			a.wsShowHidden = !a.wsShowHidden
		}
		row.Children(func() {
			chev := icoChevronRight
			if a.wsShowHidden {
				chev = icoChevronDown
			}
			drawIcon(kc, chev, 10, p.TextMuted)
			ui.Text(kc, fmt.Sprintf("已隐藏 %d 个仓库", len(a.cfg.HiddenRepos))).FontSize(10.5).TextColor(p.TextMuted).Grow(1)
		})
		if a.wsShowHidden {
			for _, hidden := range a.cfg.HiddenRepos {
				hidden := hidden
				hkc := c.Key("ws-hidden-" + hidden)
				ui.Row(hkc).FillWidth().Padding(5, 3).Gap(6).Padding(0, 16, 0, 22).AlignItems(ui.Center).Children(func() {
					drawIcon(hkc, icoMinus, 12, p.TextMuted)
					ui.Text(hkc, hidden).Font("monospace").FontSize(10).TextColor(p.TextMuted).Grow(1)
					toolButton(hkc, p, "ws-show-"+hidden, icoCheck, "恢复显示", false, true, func() {
						a.cfg.ShowRepo(hidden)
						_ = a.cfg.Save()
						a.wsNotice = "已恢复 " + hidden
						a.RefreshWorkspaceAsync()
					})
				})
			}
		}
	}
}

// buildWorkspaceAddForm is the single compact form for clone, init or import;
// only the fields of the chosen action are shown.
func (a *App) buildWorkspaceAddForm(c *ui.Context, p Palette) {
	ui.Column(c).FillWidth().Padding(10).Gap(6).Radius(6).Background(p.RailBg).Border(1, p.BorderSubtle).Children(func() {
		title, hint := "克隆仓库", "HTTPS 凭据交给系统 helper；私有仓库可先在「凭据」中保存 Token。"
		icon := icoCloud
		switch a.wsAddMode {
		case "init":
			title, hint, icon = "初始化仓库", "创建 main 分支的空仓库，并登记到 mani.yaml。", icoPlus
		case "import":
			title, hint, icon = "导入已有仓库", "选择工作区内的 Git 仓库目录。", icoFolder
		}
		ui.Row(c).FillWidth().Gap(6).AlignItems(ui.Center).Children(func() {
			drawIcon(c, icon, 13, p.Accent)
			ui.Text(c, title).FontSize(12).FontWeight(weightSemibold).TextColor(p.TextPrimary).Grow(1)
			toolButton(c, p, "ws-add-close", icoClose, "取消", false, true, func() {
				a.wsAddMode = ""
				a.wsErr = ""
			})
		})
		ui.Text(c, hint).FontSize(9.5).TextColor(p.TextMuted)
		switch a.wsAddMode {
		case "init":
			ui.Row(c).Gap(6).AlignItems(ui.Center).Children(func() {
				ui.TextInput(c.Key("ws-init-name"), &a.wsInitName).Placeholder("仓库目录名").Grow(1)
				iconPill(c, p, icoPlus, "初始化", true, func() {
					name := strings.TrimSpace(a.wsInitName)
					if name == "" {
						a.wsErr = "目录名不能为空"
						return
					}
					workspace := a.cfg.WorkspacePath
					a.wsAddMode = ""
					a.wsRun(func(ctx context.Context) (string, error) {
						dest, err := gitops.InitRepo(ctx, workspace, name)
						if err != nil {
							return "", err
						}
						return "已初始化 " + dest, nil
					}, true)
				})
			})
		case "import":
			ui.Row(c).Gap(6).AlignItems(ui.Center).Children(func() {
				ui.TextInput(c.Key("ws-import-path"), &a.wsImportPath).Placeholder("工作区内仓库路径").Grow(1)
				pill(c, p, "选择…", false, func() {
					go func() {
						paths, err := mygo.Dialog.Open(mygo.OpenDialogOptions{Parent: a.win, Title: "选择已有 Git 仓库", Directory: true})
						if err == nil && len(paths) > 0 {
							a.runUI(func() { a.wsImportPath = paths[0] })
						}
					}()
				})
				iconPill(c, p, icoPlus, "导入", true, func() {
					path := a.wsImportPath
					workspace := a.cfg.WorkspacePath
					a.wsAddMode = ""
					a.wsRun(func(ctx context.Context) (string, error) {
						name, rel, err := gitops.ImportRepo(ctx, workspace, path)
						if err != nil {
							return "", err
						}
						return "已导入 " + name + " (" + rel + ")", nil
					}, true)
				})
			})
		default:
			ui.Row(c).Gap(6).AlignItems(ui.Center).Children(func() {
				ui.TextInput(c.Key("ws-clone-url"), &a.wsCloneURL).Placeholder("https://github.com/owner/repo.git 或 git@…").Grow(1)
				ui.TextInput(c.Key("ws-clone-name"), &a.wsCloneName).Placeholder("目录名（可选）").Width(170)
				iconPill(c, p, icoCloud, "克隆", true, func() {
					url, name := a.wsCloneURL, a.wsCloneName
					if strings.TrimSpace(url) == "" {
						a.wsErr = "仓库地址不能为空"
						return
					}
					workspace := a.cfg.WorkspacePath
					a.wsAddMode = ""
					a.wsRun(func(ctx context.Context) (string, error) {
						dest, err := gitops.CloneRepo(ctx, workspace, url, name)
						if err != nil {
							return "", err
						}
						return "已克隆到 " + dest, nil
					}, true)
				})
			})
			ui.Row(c).Gap(6).AlignItems(ui.Center).Children(func() {
				ui.Text(c, "私有仓库需要 HTTPS Token 或 SSH Key。").FontSize(9).TextColor(p.TextMuted).Grow(1)
				miniBtn(c, p, "配置凭据…", func() {
					a.wsAddMode = ""
					a.workspaceTab = "credentials"
					a.loadWorkspaceTab()
				})
			})
		}
	})
}

func (a *App) buildWorkspaceCredentials(c *ui.Context, p Palette) {
	ui.Row(c).FillWidth().AlignItems(ui.Center).Children(func() {
		ui.Text(c, "HTTPS 凭据").FontSize(12).FontWeight(weightSemibold).TextColor(p.TextPrimary).Grow(1)
		miniBtn(c, p, "添加 / 更新…", func() { a.wsAddCred = !a.wsAddCred })
	})
	ui.Text(c, "令牌不会写入 Nemorous 配置；通过系统 Git credential helper 保存。").FontSize(9.5).TextColor(p.TextMuted)
	if len(a.wsCredHelpers) == 0 {
		ui.Text(c, "未配置 credential.helper，令牌可能只保存在内存 helper 中。macOS 可运行 git config --global credential.helper osxkeychain。").FontSize(9.5).TextColor(p.Amber)
	} else {
		ui.Text(c, "helper: "+strings.Join(a.wsCredHelpers, ", ")).Font("monospace").FontSize(9.5).TextColor(p.TextMuted).MaxLines(1)
	}
	if a.wsAddCred {
		ui.Column(c).FillWidth().Padding(8).Gap(6).Radius(6).Background(p.RailBg).Border(1, p.BorderSubtle).Children(func() {
			ui.Row(c).Gap(6).AlignItems(ui.Center).Children(func() {
				ui.TextInput(c.Key("ws-cred-host"), &a.wsCredHost).Placeholder("github.com").Width(190)
				ui.TextInput(c.Key("ws-cred-user"), &a.wsCredUser).Placeholder("用户名（可选）").Width(170)
				ui.TextInput(c.Key("ws-cred-token"), &a.wsCredToken).Placeholder("Personal Access Token").Grow(1).Password()
			})
			ui.Row(c).Gap(6).AlignItems(ui.Center).Children(func() {
				iconPill(c, p, icoCheck, "保存", true, func() {
					host, user, token := a.wsCredHost, a.wsCredUser, a.wsCredToken
					if strings.TrimSpace(host) == "" || strings.TrimSpace(token) == "" {
						a.wsErr = "主机和令牌不能为空"
						return
					}
					a.wsRun(func(ctx context.Context) (string, error) {
						if err := credentials.SaveHTTPS(ctx, host, user, token); err != nil {
							return "", err
						}
						return "已保存 HTTPS 凭据 " + credentials.HTTPSHost(host), nil
					}, false)
					a.wsCredToken = ""
					a.wsAddCred = false
				})
				miniBtn(c, p, "删除此主机凭据", func() {
					host, user := a.wsCredHost, a.wsCredUser
					a.wsRun(func(ctx context.Context) (string, error) {
						if err := credentials.EraseHTTPS(ctx, host, user); err != nil {
							return "", err
						}
						return "已删除凭据 " + credentials.HTTPSHost(host), nil
					}, false)
					a.wsAddCred = false
				})
				miniBtn(c, p, "取消", func() { a.wsAddCred = false })
			})
		})
	}
	ui.Box(c).Height(1).FillWidth().Background(p.Border).MarginY(4)
	ui.Row(c).FillWidth().AlignItems(ui.Center).Children(func() {
		ui.Text(c, "SSH Key").FontSize(12).FontWeight(weightSemibold).TextColor(p.TextPrimary).Grow(1)
		miniBtn(c, p, "生成密钥…", func() { a.wsGenKey = !a.wsGenKey })
	})
	ui.Text(c, fmt.Sprintf("~/.ssh 中 %d 个公钥；ssh-agent 中 %d 个身份。", len(a.wsSSHKeys), len(a.wsAgentKeys))).FontSize(9.5).TextColor(p.TextMuted)
	for _, key := range a.wsSSHKeys {
		key := key
		kc := c.Key("ws-ssh-" + key.Path)
		ui.Row(kc).FillWidth().Padding(5, 3).Gap(6).AlignItems(ui.Center).Children(func() {
			drawIcon(kc, icoBox, 12, p.TextMuted)
			ui.Text(kc, filepath.Base(key.Path)).Font("monospace").FontSize(10.5).TextColor(p.TextPrimary).Width(150)
			ui.Text(kc, key.Fingerprint).Font("monospace").FontSize(9).TextColor(p.TextMuted).Grow(1).MaxLines(1)
			toolButton(kc, p, "ws-ssh-signing-"+key.Path, icoEdit, "设为当前仓库 SSH 签名密钥", false, true, func() {
				a.workspaceTab = "signing"
				a.wsSigningEnabled = true
				a.wsSigningFormat = "ssh"
				a.wsSigningKey = key.PublicPath
				a.loadWorkspaceTab()
			})
		})
	}
	if a.wsGenKey {
		ui.Column(c).FillWidth().Padding(8).Gap(6).Radius(6).Background(p.RailBg).Border(1, p.BorderSubtle).Children(func() {
			ui.Row(c).Gap(6).AlignItems(ui.Center).Children(func() {
				ui.TextInput(c.Key("ws-key-name"), &a.wsKeyName).Placeholder("密钥名，如 id_ed25519_work").Width(220)
				ui.TextInput(c.Key("ws-key-comment"), &a.wsKeyComment).Placeholder("注释 / 邮箱").Grow(1)
				ui.TextInput(c.Key("ws-key-pass"), &a.wsKeyPassphrase).Placeholder("Passphrase（可选）").Width(170).Password()
			})
			ui.Row(c).Gap(6).AlignItems(ui.Center).Children(func() {
				iconPill(c, p, icoPlus, "生成 ed25519", true, func() {
					name, comment, pass := a.wsKeyName, a.wsKeyComment, a.wsKeyPassphrase
					a.wsRun(func(ctx context.Context) (string, error) {
						pub, err := credentials.GenerateSSHKey(ctx, name, comment, pass)
						if err != nil {
							return "", err
						}
						return "已生成公钥：\n" + pub, nil
					}, false)
					a.wsKeyPassphrase = ""
					a.wsGenKey = false
				})
				miniBtn(c, p, "取消", func() { a.wsGenKey = false })
			})
		})
	}
	ui.Row(c).Gap(6).AlignItems(ui.Center).Margin(4, 0, 0, 0).Children(func() {
		ui.TextInput(c.Key("ws-ssh-host"), &a.wsSSHHost).Placeholder("github.com").Width(200)
		miniBtn(c, p, "测试 SSH", func() {
			host := a.wsSSHHost
			a.wsRun(func(ctx context.Context) (string, error) {
				out, err := credentials.TestSSH(ctx, host)
				if err != nil && strings.TrimSpace(out) == "" {
					return "", err
				}
				return out, nil
			}, false)
		})
		if a.wsSSHOut != "" {
			ui.Text(c, a.wsSSHOut).FontSize(9.5).TextColor(p.TextMuted).MaxLines(2).Grow(1)
		}
	})
}

func (a *App) buildWorkspaceStorage(c *ui.Context, p Palette) {
	a.workspaceRepoPicker(c, p)
	if a.wsRepoStatus() == nil {
		emptyState(c, p, icoFolder, "没有仓库", "先加载工作区")
		return
	}
	repoPath := a.wsRepoStatus().Repo.AbsPath
	ui.Text(c, "Git LFS").FontSize(12).FontWeight(weightSemibold).TextColor(p.TextPrimary)
	if !a.wsLFS.Installed {
		ui.Text(c, "未检测到 git-lfs，请先安装 Git LFS。").FontSize(9.5).TextColor(p.Amber)
	} else {
		ui.Text(c, fmt.Sprintf("%s · %s · %d 个模式", a.wsLFS.Version, map[bool]string{true: "已启用", false: "未启用"}[a.wsLFS.Enabled], len(a.wsLFS.Patterns))).FontSize(9.5).TextColor(p.TextMuted)
		if len(a.wsLFS.Patterns) > 0 {
			ui.Text(c, strings.Join(a.wsLFS.Patterns, "  ")).Font("monospace").FontSize(9).TextColor(p.TextSecondary).MaxLines(3)
		}
	}
	ui.Row(c).Gap(6).AlignItems(ui.Center).Children(func() {
		iconPill(c, p, icoCheck, "启用 LFS", true, func() {
			a.wsRun(func(ctx context.Context) (string, error) {
				return gitops.LFSInstall(ctx, repoPath)
			}, false)
		})
		iconPill(c, p, icoPull, "拉取对象", false, func() {
			a.wsRun(func(ctx context.Context) (string, error) { return gitops.LFSPull(ctx, repoPath) }, false)
		})
		ui.TextInput(c.Key("ws-lfs-pattern"), &a.wsLFSPattern).Placeholder("*.psd").Width(180)
		iconAction(c, p, "ws-lfs-track", icoPlus, "追踪模式", false, func() {
			pattern := a.wsLFSPattern
			a.wsRun(func(ctx context.Context) (string, error) {
				return gitops.LFSTrack(ctx, repoPath, pattern)
			}, false)
		})
	})
	ui.Box(c).Height(1).FillWidth().Background(p.Border).MarginY(6)
	ui.Text(c, "Sparse Checkout").FontSize(12).FontWeight(weightSemibold).TextColor(p.TextPrimary)
	ui.Text(c, "逗号分隔的 no-cone 模式；启用后会改变工作区可见文件。").FontSize(9.5).TextColor(p.TextMuted)
	ui.TextInput(c.Key("ws-sparse"), &a.wsSparseInput).Placeholder("src/, docs/").Grow(1)
	ui.Row(c).Gap(6).AlignItems(ui.Center).Children(func() {
		iconPill(c, p, icoCheck, "应用 Sparse", true, func() {
			patterns := splitPatterns(a.wsSparseInput)
			a.wsRun(func(ctx context.Context) (string, error) {
				return gitops.SetSparse(ctx, repoPath, patterns)
			}, false)
		})
		iconPill(c, p, icoClose, "关闭 Sparse", false, func() {
			a.wsRun(func(ctx context.Context) (string, error) {
				return gitops.DisableSparse(ctx, repoPath)
			}, false)
		})
	})
}

func splitPatterns(raw string) []string {
	fields := strings.FieldsFunc(raw, func(r rune) bool { return r == '\n' || r == ',' })
	out := make([]string, 0, len(fields))
	for _, f := range fields {
		if f = strings.TrimSpace(f); f != "" {
			out = append(out, f)
		}
	}
	return out
}

func (a *App) buildWorkspaceWorktrees(c *ui.Context, p Palette) {
	a.workspaceRepoPicker(c, p)
	if a.wsRepoStatus() == nil {
		emptyState(c, p, icoFolder, "没有仓库", "")
		return
	}
	repoPath := a.wsRepoStatus().Repo.AbsPath
	ui.Row(c).Gap(6).AlignItems(ui.Center).Children(func() {
		ui.TextInput(c.Key("ws-wt-path"), &a.wsWorktreePath).Placeholder("Worktree 路径").Grow(1)
		ui.TextInput(c.Key("ws-wt-branch"), &a.wsWorktreeBranch).Placeholder("分支（可选，默认 detached）").Width(220)
		iconPill(c, p, icoPlus, "添加", true, func() {
			path, branch := a.wsWorktreePath, a.wsWorktreeBranch
			a.wsRun(func(ctx context.Context) (string, error) {
				return gitops.AddWorktree(ctx, repoPath, path, branch)
			}, false)
			a.wsWorktreePath = ""
		})
		toolButton(c, p, "ws-wt-prune", icoRefresh, "清理失效记录", false, true, func() {
			a.wsRun(func(ctx context.Context) (string, error) {
				return gitops.PruneWorktrees(ctx, repoPath)
			}, false)
		})
	})
	for _, wt := range a.wsWorktrees {
		wt := wt
		kc := c.Key("ws-wt-" + wt.Path)
		row := ui.Row(kc).FillWidth().Padding(6, 4).Gap(8).AlignItems(ui.Center).Radius(4)
		if row.Hovered() {
			row.Background(p.SurfaceHover)
		}
		row.Children(func() {
			drawIcon(kc, icoBox, 12, p.TextMuted)
			ui.Text(kc, wt.Path).Font("monospace").FontSize(10).TextColor(p.TextPrimary).Grow(1).MaxLines(1)
			branch := wt.Branch
			if branch == "" {
				branch = "detached"
			}
			badgeBox(kc, branch, p.TextMuted, p.Surface2, p.Border, 9, false)
			if wt.Prunable != "" {
				badgeBox(kc, "stale", p.Amber, p.AmberSoft, ui.Color{}, 9, false)
			}
			if a.wsConfirm == "wt:"+wt.Path {
				toolButton(kc, p, "ws-wt-yes-"+wt.Path, icoCheck, "确认移除", false, true, func() {
					a.wsConfirm = ""
					path := wt.Path
					a.wsRun(func(ctx context.Context) (string, error) {
						return gitops.RemoveWorktree(ctx, repoPath, path, false)
					}, false)
				})
				toolButton(kc, p, "ws-wt-no-"+wt.Path, icoClose, "取消", false, true, func() { a.wsConfirm = "" })
			} else {
				toolButton(kc, p, "ws-wt-del-"+wt.Path, icoTrash, "移除", false, true, func() { a.wsConfirm = "wt:" + wt.Path })
			}
		})
		ui.Box(c).Height(1).FillWidth().Background(p.BorderSubtle)
	}
	if len(a.wsWorktrees) == 0 {
		emptyState(c, p, icoBox, "没有 Worktree", "为同一仓库并行检出多个分支")
	}
}

func (a *App) buildWorkspaceSigning(c *ui.Context, p Palette) {
	a.workspaceRepoPicker(c, p)
	if a.wsRepoStatus() == nil {
		emptyState(c, p, icoFolder, "没有仓库", "")
		return
	}
	ui.Text(c, "提交签名").FontSize(12).FontWeight(weightSemibold).TextColor(p.TextPrimary)
	ui.Text(c, "仅写入当前仓库的本地 Git 配置；启用前请确认密钥可用。").FontSize(9.5).TextColor(p.TextMuted)
	ui.Row(c).Gap(10).AlignItems(ui.Center).Children(func() {
		ui.Checkbox(c, &a.wsSigningEnabled, "启用 commit.gpgsign")
		for _, format := range []struct{ id, label string }{{"openpgp", "GPG"}, {"ssh", "SSH"}} {
			format := format
			active := a.wsSigningFormat == format.id
			el := ui.Row(c.Key("ws-sign-fmt-"+format.id)).Padding(3, 9).Radius(4).Cursor(ui.CursorPointer).Background(p.Surface2)
			if active {
				el.Background(p.AccentSoft)
			}
			if el.Clicked() {
				a.wsSigningFormat = format.id
			}
			el.Children(func() {
				col := p.TextSecondary
				if active {
					col = p.AccentSoftText
				}
				ui.Text(c, format.label).FontSize(10.5).TextColor(col)
			})
		}
	})
	repoPath := a.wsRepoStatus().Repo.AbsPath
	ui.Row(c).Gap(6).AlignItems(ui.Center).Children(func() {
		ui.TextInput(c.Key("ws-sign-key"), &a.wsSigningKey).Placeholder("GPG key ID 或 SSH 公钥路径").Grow(1)
		iconPill(c, p, icoCheck, "保存", true, func() {
			enabled, format, key := a.wsSigningEnabled, a.wsSigningFormat, a.wsSigningKey
			a.wsRun(func(ctx context.Context) (string, error) {
				return gitops.SetSigning(ctx, repoPath, enabled, format, key)
			}, false)
		})
	})
	if !a.wsSigning.Enabled {
		ui.Text(c, "当前仓库未启用提交签名。").FontSize(9.5).TextColor(p.TextMuted)
	}
}
