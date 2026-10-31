<div align="center">

<a href="https://github.com/iwangjie/Nemorous"><img src="assets/icon.png" width="104" alt="Nemorous"></a>

# Nemorous

### 多仓 Git，不再来回切窗口

面向「一次改动跨多个仓库」的原生、键盘优先 Git 工作台。

[![Release](https://img.shields.io/github/v/release/iwangjie/Nemorous?label=release&color=10B981)](https://github.com/iwangjie/Nemorous/releases/latest)
[![CI](https://img.shields.io/github/actions/workflow/status/iwangjie/Nemorous/ci.yml?branch=main&label=CI&color=10B981)](https://github.com/iwangjie/Nemorous/actions/workflows/ci.yml)
[![License](https://img.shields.io/badge/license-MIT-6366F1)](LICENSE)
[![Go](https://img.shields.io/badge/Go-1.27%2B-00ADD8?logo=go&logoColor=white)](go.mod)

![macOS](https://img.shields.io/badge/macOS-Metal-000000?logo=apple&logoColor=white)
![Windows](https://img.shields.io/badge/Windows-Direct3D_11-0078D4?logo=windows&logoColor=white)
![Linux](https://img.shields.io/badge/Linux-OpenGL-FCC624?logo=linux&logoColor=black)
![Architecture](https://img.shields.io/badge/Arch-arm64_%7C_amd64-059669)

<br>

**[下载](https://github.com/iwangjie/Nemorous/releases/latest)** &nbsp;·&nbsp;
**[功能](#功能)** &nbsp;·&nbsp;
**[截图](#截图)** &nbsp;·&nbsp;
**[安装](#安装)** &nbsp;·&nbsp;
**[快捷键](#快捷键)** &nbsp;·&nbsp;
**[构建](#从源码构建)** &nbsp;·&nbsp;
[English](README.md) · **简体中文**

</div>

<img src="docs/screenshots/demo_changes.png" width="100%" alt="变更树与 Diff">

## 为什么用 Nemorous

现代产品很少只有一个仓库。一个需求可能同时涉及 API、计费服务、共享库和网关。为了对齐分支、暂存改动、查看 Diff 和提交，在多个 IDE 窗口之间来回切换既慢又容易出错。

Nemorous 把整个工作区放在一个界面里：

- **一张分支矩阵**：所有仓库的分支、ahead/behind 和脏状态一目了然，支持分支预设。
- **一棵变更树**：已暂存 / 未暂存分组，文件、变更块和行级暂存。
- **一条提交流程**：可选 AI 生成提交说明，支持多仓批量提交与推送。
- **一套历史工具**：交互式 Rebase、Reflog、Worktree、显式合并策略。
- **一个代码工作区**：可选 LSP 诊断、引用、快速修复和格式化。
- **一层集成能力**：GitHub、GitLab、Jenkins、Kubernetes、数据库等通过插件接入。

基于 Go 与 [MyGo Native UI](https://mygo.egoist.dev/) 构建：无 Electron、无 WebView、无 JavaScript UI 运行时，`CGO=0`。

## 功能

### 多仓协同
- 分支矩阵、ahead/behind、脏状态与一键切换。
- 分支预设与按仓库指定目标分支。
- 安全 Pull：自动 stash/pop、仅允许 fast-forward。
- Fetch / Pull / Commit / Push 批量执行，逐仓展示结果。

### 变更与提交
- IDEA 风格已暂存 / 未暂存树，三态勾选与键盘导航。
- 双栏 / 单栏 Diff、语法高亮、微编辑与本地历史。
- Diff 工具栏直接暂存/取消暂存变更块，按行暂存新增、删除和替换。
- 可选 AI 生成提交说明，基于勾选文件的真实 Patch。
- Amend、Revert、Cherry-pick、变更列表与提交信息历史。

### 历史与恢复
- 跨仓 Commit Forest。
- 交互式 Rebase：pick、squash、reword、drop、调整顺序。
- Reflog 浏览与 soft / mixed / hard / keep Reset（二次确认）。
- 合并策略：default、`--no-ff`、`--ff-only`、squash。
- `--force-with-lease` 安全强推。
- 任意两个 ref 的双点 / 三点对比。
- Worktree 隔离合并与 AI 冲突解决、机器校验。

### 代码智能
LSP **默认关闭**，避免大型仓库在未启用时承担语言服务器的内存与 CPU 开销。

启用后，已安装的语言服务可提供：
- 补全、悬停、跳转定义
- 引用查找与快速修复
- 实时诊断与 Problems 面板
- 文档格式化
- Inlay Hints
- 符号重命名与工作区编辑

内置识别 Go、TypeScript/JavaScript、Python、Rust、C/C++、Java；可在配置的 `lsp_servers` 中覆盖命令。

### 工作区与安全
- 应用内克隆、初始化、导入和隐藏仓库。
- HTTPS 凭据交给系统 Git credential helper；macOS 默认写入钥匙串。
- SSH key 列表、ed25519 生成、连接测试与 SSH 提交签名。
- 按仓库配置 GPG 或 SSH 提交签名。
- Git LFS、Sparse Checkout、Worktree 管理。
- 密钥与 Token 不写入 Nemorous 配置。

### 集成
内置 GitHub PR / CI；官方插件覆盖 GitLab、Maven、Java、Markdown、CSV、SSH、Code Stats、Jenkins、Kubernetes、Database、Bookmarks。

## 截图

<table>
  <tr>
    <td width="50%"><img src="docs/screenshots/demo_git.png" alt="IDEA 风格 Git 操作"></td>
    <td width="50%"><img src="docs/screenshots/demo_code.png" alt="代码浏览"></td>
  </tr>
  <tr>
    <td width="50%"><img src="docs/screenshots/demo_workspace.png" alt="工作区管理"></td>
    <td width="50%"><img src="docs/screenshots/demo_branches.png" alt="分支矩阵"></td>
  </tr>
</table>

## 安装

从 **[GitHub Releases](https://github.com/iwangjie/Nemorous/releases/latest)** 下载：

| 平台 | 安装包 |
|---|---|
| macOS（Apple Silicon / Intel） | `.dmg` |
| Windows（arm64 / amd64） | `.exe` 安装器或 `.tar.gz` |
| Linux（arm64 / amd64） | `.deb` 或 `.tar.gz` |

更新包带签名，可在后台安装；无需单独运行时。

## 快速开始

1. 打开 Nemorous，选择工作区目录。
2. 点击工作区名称，在「添加仓库」中克隆、初始化或导入仓库。
3. 私有仓库先在「凭据」中添加 HTTPS Token 或 SSH Key。
4. 勾选变更，编写或 AI 生成提交说明，然后提交；「提交并推送」一次完成。
5. `⌘3` / `Ctrl+3` 查看提交历史，`⌘B` / `Ctrl+B` 打开分支矩阵。

`mani.yaml` 可选；存在时会用于项目名称、描述与 Mani 任务。

## 快捷键

`Cmd` 在 Windows / Linux 上对应 `Ctrl`，`Alt` 在 macOS 上对应 `Option`。

| 快捷键 | 操作 |
|---|---|
| `⌘K` | 聚焦提交说明 / 打开提交视图 |
| `⌘⇧K` | 推送所有有未推送提交的仓库 |
| `⌘T` | 更新全部仓库（Fetch + fast-forward Pull） |
| `⌘↩` | 提交勾选文件 |
| `⌘⇧↩` | 提交并推送 |
| `⌘1` / `⌘2` / `⌘3` / `⌘4` | 变更 / 文件 / Git / Mani |
| `⌘B` | 分支矩阵 |
| `⌘⇧P` / `⌘P` | 命令面板 / 文件检索 |
| `⌘⇧F` | 全局搜索 |
| `⌘R` | 刷新工作区 |
| `⌘S` | 保存编辑器或 Diff 微编辑 |
| `Space` | 切换当前文件勾选 |
| `S` / `U` | 暂存 / 取消暂存当前文件 |
| `⌥F7` | 查找引用（LSP） |
| `⌥↩` | 快速修复 / Code Action（LSP） |
| `⌘⌥L` | 格式化当前文件 |
| `⌘⌥Z` | 还原当前变更 |

文件、仓库分组和提交行也提供右键菜单。

## 可选 AI

Nemorous 不内置模型，只调用本机安装的 CLI：

```json
{
  "ai_agent": "auto"
}
```

支持 `omp`、`pi`、`claude`、`codex`。AI 仅在明确动作（生成提交说明、解决冲突）时调用，不会后台请求。

## 可选 LSP

在 **设置 → 可选代码智能 (LSP)** 中启用。只有打开匹配文件时才会启动对应语言服务。可以在配置中覆盖命令：

```json
{
  "lsp_enabled": true,
  "lsp_servers": {
    "go": "/opt/homebrew/bin/gopls",
    "typescript": "typescript-language-server --stdio"
  }
}
```

## 从源码构建

要求：Go 1.27+、Git。

```bash
git clone https://github.com/iwangjie/Nemorous.git
cd Nemorous
go test ./...
go vet ./...
go tool mygo vet .
go tool mygo build
```

打包：

```bash
go tool mygo build -platform darwin/arm64,linux/amd64,windows/amd64
```

## 工程说明

- 原生 MyGo UI，无浏览器内核。
- `CGO_ENABLED=0`，跨平台单文件分发。
- Git 操作走 Git CLI，兼容 credential helper 与现有 Git 配置。
- AI 与 LSP 请求带超时和结果上限。
- CI 在 macOS、Linux、Windows 上执行格式化检查、Vet、单元测试与无头 UI 测试；Linux 额外执行后端 Race 测试。
- 每个 Release 提供校验和与签名更新清单。

## 安全

请通过 [GitHub Security Advisories](https://github.com/iwangjie/Nemorous/security/advisories/new) 私下报告漏洞，不要开公开 Issue。范围与处理方式见 [SECURITY.md](SECURITY.md)。

## 贡献

欢迎 Issue 与 Pull Request。开发环境、测试要求和代码风格见 [CONTRIBUTING.md](CONTRIBUTING.md)。

## License

MIT，详见 [LICENSE](LICENSE)。
